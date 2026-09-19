//! Which prover a run drives.

use crate::load::{LoadError, Loaded};
use ply_eval::host::{HostBinding, HostRuntime};
use ply_eval::{DEFAULT_MAX_CALLS, Machine, Seed, Value};
use ply_prove::concurrency::{self, BodyRun, LawSearch, ValueDomain};
use ply_prove::domain::{self, Finite};
use ply_prove::property::{self, GenStream, Judge, Outcome, TypeWorld, judge_case, run_property};
use ply_prove::prove::claims::{Clause, Code, Definition, Law};
use ply_prove::prove::{self, Blocker, Claims, Decision, Goal, Limits, Proof};
use ply_prove::{
    Binding, Certificate, Counterexample, Discharge, Evidence, Gap, Obligation, ObligationKind,
    ProvePlan, Rule, Vacuity, VacuityKind,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_store::Store;
use ply_ty::{CheckOutput, DefInfo, Front, LawBinder, LawInfo, Literal, SpecKind};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// The discharger this build drives, its claims kept in `store`.
pub fn of<'a>(
    loaded: &'a Loaded,
    hosting: Option<Hosting<'a>>,
    backend: Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>,
    store: &mut Store,
) -> Result<Box<dyn ply_test::obligation::Discharger + 'a>, LoadError> {
    let prover = Prover::over(loaded, Some(store))?;
    let prover = match hosting {
        Some(hosting) => prover.with_hosting(hosting),
        None => prover,
    };
    Ok(Box::new(prover.with_backend(backend)))
}

fn claims_of(loaded: &Loaded, store: Option<&mut Store>) -> Result<Claims, LoadError> {
    crate::driver::claims(loaded, store).map_err(|why| {
        loaded.refused(
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the front end could not lower this program's claims: {why}"),
            )
            .primary(Span::DUMMY, "nothing was proved, so nothing is claimed")
            .note("this is Ply's fault: the compiler's own front end is what failed here"),
        )
    })
}

/// Where an obligation's claim is written, found once per run.
enum Claim<'s> {
    Ensures {
        owner: &'s DefInfo,
        def: &'s Definition,
        clause: &'s Clause,
        /// Its place among the owner's `ensures` clauses, which names its root.
        index: usize,
    },
    Law {
        info: &'s LawInfo,
        /// Its place among the module's laws, which names its roots.
        ordinal: usize,
        law: &'s Law,
    },
}

impl<'s> Claim<'s> {
    /// The propositions that narrow the domain: an owner's `requires` clauses, or a law's `where`.
    fn guards(&self) -> Vec<&'s Clause> {
        match self {
            Claim::Ensures { def, .. } => def
                .spec
                .iter()
                .filter(|(kind, _)| *kind == SpecKind::Requires)
                .map(|(_, clause)| clause)
                .collect(),
            Claim::Law { law, .. } => law.guard.iter().collect(),
        }
    }

    fn body(&self) -> &'s Code {
        match self {
            Claim::Ensures { clause, .. } => &clause.code,
            Claim::Law { law, .. } => &law.body,
        }
    }

    /// Where a vacuity points.
    fn guard_span(&self, fallback: Span) -> Span {
        self.guards().first().map_or(fallback, |g| g.span)
    }
}

pub struct Prover<'a> {
    check: &'a CheckOutput,
    front: &'a Front,
    world: TypeWorld,
    /// Built once; `machine()` runs per obligation.
    ctx: prove::Context<'a>,
    laws: HashMap<Symbol, (usize, &'a LawInfo)>,
    /// What a `law/host` is discharged against.
    hosting: Option<Hosting<'a>>,
    /// A compiled unit holding the laws' and clauses' roots, where those propositions are entered.
    backend: Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>,
}

/// The binding and the reactor a `law/host` runs against.
pub struct Hosting<'a> {
    pub binding: Arc<HostBinding>,
    pub runtime: Option<&'a (dyn Fn() -> Rc<dyn HostRuntime> + Sync)>,
}

impl<'a> Prover<'a> {
    pub fn new(loaded: &'a Loaded) -> Result<Prover<'a>, LoadError> {
        Prover::over(loaded, None)
    }

    fn over(loaded: &'a Loaded, store: Option<&mut Store>) -> Result<Prover<'a>, LoadError> {
        let check = &loaded.check;
        let mut laws = HashMap::new();
        let mut ordinals: HashMap<&Symbol, usize> = HashMap::new();
        for law in &check.laws {
            let ordinal = ordinals.entry(law.module.as_symbol()).or_default();
            laws.insert(law.key.clone(), (*ordinal, law));
            *ordinal += 1;
        }
        Ok(Prover {
            check,
            front: &loaded.front,
            world: TypeWorld::new(check.ctors.values()),
            ctx: prove::Context::new(claims_of(loaded, store)?, check),
            laws,
            hosting: None,
            backend: None,
        })
    }

    pub fn with_backend(
        mut self,
        backend: Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>,
    ) -> Prover<'a> {
        self.backend = backend;
        self
    }

    /// The unit, attached once per thread: obligations are discharged on pool threads.
    fn compiled(&self) -> Option<Rc<dyn ply_eval::Compiled>> {
        thread_local! {
            static ATTACHED: RefCell<Vec<(usize, Rc<dyn ply_eval::Compiled>)>> =
                const { RefCell::new(Vec::new()) };
        }
        let (provider, spec) = self.backend.as_ref()?;
        let key = std::ptr::from_ref(*provider).cast::<()>() as usize;
        ATTACHED.with(|attached| {
            if let Some((_, c)) = attached.borrow().iter().find(|(k, _)| *k == key) {
                return Some(Rc::clone(c));
            }
            let c = provider.attach(spec);
            attached.borrow_mut().push((key, Rc::clone(&c)));
            Some(c)
        })
    }

    /// A proposition's body root: its program-wide name in the unit.
    fn body_root(&self, claim: &Claim<'_>) -> Symbol {
        match claim {
            Claim::Ensures { owner, index, .. } => owner.module.qualify(
                &ply_codegen::clause_root_name(&owner.simple_name, "ensures", *index),
            ),
            Claim::Law { info, ordinal, .. } => info
                .module
                .qualify(&ply_codegen::law_root_name(*ordinal, "body")),
        }
    }

    /// Each guard's compiled root, in [`Claim::guards`] order, which `source.rs` numbers alike.
    fn guard_roots(&self, claim: &Claim<'_>) -> Vec<Symbol> {
        match claim {
            Claim::Ensures { owner, .. } => (0..claim.guards().len())
                .map(|k| {
                    owner.module.qualify(&ply_codegen::clause_root_name(
                        &owner.simple_name,
                        "requires",
                        k,
                    ))
                })
                .collect(),
            Claim::Law { info, ordinal, .. } => claim
                .guards()
                .iter()
                .map(|_| {
                    info.module
                        .qualify(&ply_codegen::law_root_name(*ordinal, "guard"))
                })
                .collect(),
        }
    }

    /// Bind the host, so that a `law/host` is attempted rather than reported as a gap.
    pub fn with_hosting(mut self, hosting: Hosting<'a>) -> Prover<'a> {
        self.hosting = Some(hosting);
        self
    }

    fn claim(&self, obligation: &Obligation) -> Option<Claim<'_>> {
        let claims = self.ctx.claims();
        match obligation.kind {
            ObligationKind::Ensures { index } => {
                let def = claims.defs.get(&obligation.owner)?;
                let (_, clause) = def
                    .spec
                    .iter()
                    .filter(|(kind, _)| *kind == SpecKind::Ensures)
                    .nth(index)?;
                Some(Claim::Ensures {
                    owner: self.check.defs.get(&obligation.owner)?,
                    def,
                    clause,
                    index,
                })
            }
            ObligationKind::Law => {
                let &(ordinal, info) = self.laws.get(&obligation.owner)?;
                Some(Claim::Law {
                    info,
                    ordinal,
                    law: claims.laws.get(&obligation.owner)?,
                })
            }
        }
    }

    fn machine(&self) -> Machine<'a> {
        let mut machine = Machine::new(self.front).with_max_calls(DEFAULT_MAX_CALLS);
        // An owner is called through the machine to produce `result`, so the machine must hold the
        // tier its propositions are entered on, or that call declines with no body.
        if let Some((provider, spec)) = self.backend.as_ref() {
            machine.set_compiled(provider.attach(spec));
        }
        machine
    }

    /// The machine a `law/host`'s body runs on: the run's binding and a reactor for this thread.
    fn host_machine(&self, hosting: &Hosting<'a>) -> Machine<'a> {
        let mut machine = self.machine();
        machine.set_host_binding(Arc::clone(&hosting.binding));
        if let Some(factory) = hosting.runtime {
            machine.set_host_runtime(factory());
        }
        machine
    }

    fn decide(
        &self,
        obligation: &Obligation,
        claim: &Claim<'_>,
        plan: &ProvePlan,
    ) -> (Decision, Vec<Blocker>) {
        let guards: Vec<&Code> = claim.guards().into_iter().map(|g| &g.code).collect();
        let result = match claim {
            Claim::Ensures { def, .. } => obligation.result_binder().map(|_| &def.body),
            Claim::Law { .. } => None,
        };
        let goal = Goal {
            binders: &obligation.binders,
            guards: &guards,
            result,
            body: claim.body(),
        };
        let limits = Limits {
            steps: plan.prove_budget,
            ..Limits::default()
        };
        prove::decide_and_diagnose(&self.ctx, &goal, &limits)
    }

    fn attempt_static(
        &self,
        obligation: &Obligation,
        claim: &Claim<'_>,
        plan: &ProvePlan,
    ) -> Static {
        match self.decide(obligation, claim, plan).0 {
            Decision::GuardUnsatisfiable { .. } => Static::Vacuous,
            Decision::Proved(proof) => match proof.certify(false) {
                Some(certificate) => Static::Proved(certificate),
                None => Static::NeedsWitness(proof),
            },
            Decision::Unknown { .. } => Static::Inconclusive,
        }
    }

    /// What the static tier alone answered, and where the obligation left the fragment on the way.
    pub fn reach(&self, obligation: &Obligation, plan: &ProvePlan) -> Option<Reach> {
        if obligation.is_concurrency_law() {
            return None;
        }
        let claim = self.claim(obligation)?;
        let (decision, blockers) = self.decide(obligation, &claim, plan);
        Some(Reach { decision, blockers })
    }
}

/// What the static tier answered, and the fragment boundaries it crossed.
pub struct Reach {
    pub decision: Decision,
    pub blockers: Vec<Blocker>,
}

/// What the static tier had to say, before anything ran.
enum Static {
    Proved(Certificate),
    /// A decided body over a domain the prover could not show inhabited.
    NeedsWitness(Proof),
    Vacuous,
    Inconclusive,
}

impl ply_test::obligation::Discharger for Prover<'_> {
    fn discharge(&self, obligation: &Obligation, plan: &ProvePlan) -> Discharge {
        self.discharge_with(obligation, plan)
    }
}

impl<'a> Prover<'a> {
    /// One obligation, at the strongest tier this build can demonstrate.
    pub fn discharge_with(&self, obligation: &Obligation, plan: &ProvePlan) -> Discharge {
        let Some(claim) = self.claim(obligation) else {
            return Discharge::Unattempted(Gap::UnhandledEffect(obligation.footprint.clone()));
        };

        if obligation.is_concurrency_law() {
            return self.search_interleavings(obligation, &claim, plan);
        }

        if obligation.host {
            return self.discharge_host(obligation, &claim, plan);
        }

        let witness = match self.attempt_static(obligation, &claim, plan) {
            Static::Proved(certificate) => return Discharge::Held(Evidence::Proof(certificate)),
            Static::Vacuous => {
                return Discharge::Vacuous(Vacuity {
                    guard: claim.guard_span(obligation.span),
                    kind: VacuityKind::ProvedUnsatisfiable,
                });
            }
            Static::NeedsWitness(proof) => Some(proof),
            Static::Inconclusive => None,
        };

        // Checking an `ensures` calls the definition, which needs handlers nothing supplies.
        if let Some(footprint) = self.unhandled(obligation) {
            return Discharge::Unattempted(Gap::UnhandledEffect(footprint));
        }

        let mut cases = match self.cases(obligation, &claim) {
            Ok(cases) => cases,
            Err(gap) => return Discharge::Unattempted(gap),
        };

        if let Some(finite) = domain::finite(obligation.generated(), &self.world) {
            return self.enumerate(obligation, &claim, &finite, &mut cases, witness);
        }

        let discharge = run_property(
            obligation.key,
            obligation.generated(),
            &self.world,
            plan,
            claim.guard_span(obligation.span),
            &mut cases,
        );
        match discharge {
            // Keeping no sample means the generator missed the guard, not that it admits nothing.
            Discharge::Vacuous(Vacuity {
                kind: VacuityKind::NoCaseKept { generated },
                ..
            }) => match self.witness(obligation, &claim, &mut cases) {
                Some(values) => match witness.and_then(|proof| proof.certify(true)) {
                    Some(certificate) => Discharge::Held(Evidence::Proof(certificate)),
                    None => Discharge::Unattempted(Gap::GuardNotSampled {
                        generated,
                        witness: bindings_of(obligation.generated(), &values),
                    }),
                },
                None => discharge,
            },
            other => upgrade(other, witness),
        }
    }

    /// A `law/host`, discharged by running it.
    fn discharge_host(
        &self,
        obligation: &Obligation,
        claim: &Claim<'_>,
        plan: &ProvePlan,
    ) -> Discharge {
        let Some(hosting) = &self.hosting else {
            return Discharge::Unattempted(Gap::ReachesHost(obligation.footprint.clone()));
        };
        let mut cases = match self.cases(obligation, claim) {
            Ok(cases) => cases,
            Err(gap) => return Discharge::Unattempted(gap),
        };
        cases.machine = self.host_machine(hosting);
        run_property(
            obligation.key,
            obligation.generated(),
            &self.world,
            plan,
            claim.guard_span(obligation.span),
            &mut cases,
        )
    }

    /// Binder values the guard admits, tried at points named by the guard's own literals.
    fn witness(
        &self,
        obligation: &Obligation,
        claim: &Claim<'_>,
        cases: &mut Cases<'a>,
    ) -> Option<Vec<Value>> {
        let literals = self.literals(claim);
        let mut stream = GenStream::new(0, obligation.key);
        let mut columns: Vec<Vec<Value>> = Vec::with_capacity(cases.binders.len());
        let mut points = 1usize;
        for binder in &cases.binders {
            let column = match self.candidates(&binder.ty, &literals) {
                Some(column) => column,
                // A shape the guard's literals cannot name: a list, record, ADT or function.
                None => vec![property::generate(&binder.ty, &self.world, &mut stream, 0).ok()?],
            };
            points = points.checked_mul(column.len())?;
            if points > WITNESS_POINTS {
                return None;
            }
            columns.push(column);
        }

        for index in 0..points {
            let mut values = Vec::with_capacity(columns.len());
            let mut rest = index;
            for column in &columns {
                values.push(column[rest % column.len()].clone());
                rest /= column.len();
            }
            // A point the guard raises at is not admitted; the property tier reports the raise.
            if cases.guard(&values).unwrap_or(false) {
                return Some(values);
            }
        }
        None
    }

    fn literals(&self, claim: &Claim<'_>) -> Literals {
        let written = match claim {
            Claim::Ensures { owner, .. } => self
                .front
                .defs_written
                .get(&owner.name)
                .map(|w| w.requires_literals.as_slice()),
            Claim::Law { info, .. } => self.front.law_literals.get(info.index).map(Vec::as_slice),
        };
        Literals::of(written.unwrap_or_default())
    }

    /// The values one binder is tried at, smallest and most literal first.
    fn candidates(&self, ty: &ply_ty::Type, literals: &Literals) -> Option<Vec<Value>> {
        let ply_ty::Type::Con(name, args) = ty else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        let mut out: Vec<Value> = match name.as_str() {
            "Bool" => vec![Value::Bool(false), Value::Bool(true)],
            "Unit" => vec![Value::Unit],
            "String" => {
                let mut out = vec![Value::str(String::new())];
                out.extend(literals.strings.iter().map(|s| Value::str(s.clone())));
                out
            }
            "Bytes" => {
                let mut out = vec![Value::bytes([])];
                out.extend(literals.bytes.iter().map(Value::bytes));
                out
            }
            "Int" => {
                // Each literal and its neighbours: `x > 1000000` is satisfied by `1000001`.
                let mut out = vec![0i64, 1, -1];
                for &k in &literals.ints {
                    for candidate in [k, k.saturating_add(1), k.saturating_sub(1)] {
                        if !out.contains(&candidate) {
                            out.push(candidate);
                        }
                    }
                }
                out.into_iter().map(Value::Int).collect()
            }
            _ => return None,
        };
        out.truncate(WITNESS_PER_BINDER);
        Some(out)
    }

    /// The sampled tier alone, with the static tier and the enumeration skipped.
    pub fn resample(&self, obligation: &Obligation, plan: &ProvePlan) -> Discharge {
        let Some(claim) = self.claim(obligation) else {
            return Discharge::Unattempted(Gap::UnhandledEffect(obligation.footprint.clone()));
        };
        if obligation.is_concurrency_law() {
            return self.search_interleavings(obligation, &claim, plan);
        }
        if let Some(footprint) = self.unhandled(obligation) {
            return Discharge::Unattempted(Gap::UnhandledEffect(footprint));
        }
        let mut cases = match self.cases(obligation, &claim) {
            Ok(cases) => cases,
            Err(gap) => return Discharge::Unattempted(gap),
        };
        run_property(
            obligation.key,
            obligation.generated(),
            &self.world,
            plan,
            claim.guard_span(obligation.span),
            &mut cases,
        )
    }

    /// The owner's footprint, when it is one no obligation can supply handlers for.
    fn unhandled(&self, obligation: &Obligation) -> Option<ply_ty::Footprint> {
        let ObligationKind::Ensures { .. } = obligation.kind else {
            return None;
        };
        let footprint = &self.check.defs.get(&obligation.owner)?.footprint;
        (!footprint.is_empty()).then(|| footprint.clone())
    }

    fn cases(&self, obligation: &Obligation, claim: &Claim<'_>) -> Result<Cases<'a>, Gap> {
        let call = match claim {
            Claim::Ensures { .. } => Some(obligation.owner.clone()),
            Claim::Law { .. } => None,
        };
        let result = obligation.result_binder().map(|b| b.name.clone());
        for binder in obligation.generated() {
            if property::generatable(&binder.ty, &self.world).is_err() {
                return Err(Gap::Ungeneratable {
                    param: binder.name.clone(),
                    ty: binder.ty.clone(),
                });
            }
        }
        Ok(Cases {
            machine: self.machine(),
            compiled: self.compiled(),
            guard_roots: self.guard_roots(claim),
            body_root: self.body_root(claim),
            binders: obligation.generated().to_vec(),
            span: obligation.span,
            call,
            result,
        })
    }

    fn enumerate(
        &self,
        obligation: &Obligation,
        claim: &Claim<'_>,
        finite: &Finite,
        cases: &mut Cases<'a>,
        witness: Option<Proof>,
    ) -> Discharge {
        let mut kept = 0u64;
        for point in 0..finite.points {
            // A domain that cannot produce its own point has not been covered.
            let Some(values) = finite.point(&self.world, point) else {
                return Discharge::Unattempted(Gap::Ungeneratable {
                    param: obligation.generated()[0].name.clone(),
                    ty: obligation.generated()[0].ty.clone(),
                });
            };
            match judge_case(cases, &values) {
                Outcome::Rejected => {}
                Outcome::Held => kept += 1,
                Outcome::Failed => {
                    // No shrinking: the enumeration order is fixed.
                    let bindings = bindings_of(obligation.generated(), &values);
                    return Discharge::Refuted(Counterexample {
                        original: bindings.clone(),
                        bindings,
                        shrinks: 0,
                        root: 0,
                        case: u32::try_from(point).unwrap_or(u32::MAX),
                        race: None,
                        sim_seed: None,
                    });
                }
                Outcome::Raised(diagnostic) => {
                    return Discharge::Unattempted(Gap::Raised {
                        bindings: bindings_of(obligation.generated(), &values),
                        diagnostic,
                    });
                }
            }
        }

        if kept == 0 {
            // A finite domain enumerated with nothing kept decides the guard unsatisfiable.
            return Discharge::Vacuous(Vacuity {
                guard: claim.guard_span(obligation.span),
                kind: VacuityKind::ProvedUnsatisfiable,
            });
        }

        // A kept point witnesses the domain, so the static argument can now be certified.
        if let Some(proof) = witness
            && let Some(certificate) = proof.certify(true)
        {
            return Discharge::Held(Evidence::Proof(certificate));
        }

        let rule = if obligation.generated().is_empty() {
            Rule::GroundEvaluation
        } else {
            Rule::ExhaustiveEnumeration {
                domain: finite.name(),
                points: finite.points,
            }
        };
        Discharge::Held(Evidence::Proof(Certificate {
            rules: vec![rule],
            steps: u32::try_from(finite.points).unwrap_or(u32::MAX),
            guard_satisfiable: true,
            sorts: Vec::new(),
        }))
    }

    /// A law whose body reaches a `simulate` region, discharged by searching interleavings.
    fn search_interleavings(
        &self,
        obligation: &Obligation,
        claim: &Claim<'_>,
        plan: &ProvePlan,
    ) -> Discharge {
        let mut cases = match self.cases(obligation, claim) {
            Ok(cases) => cases,
            Err(gap) => return Discharge::Unattempted(gap),
        };

        let (points, domain) = match self.law_domain(obligation, &mut cases, plan) {
            Ok(kept) => kept,
            Err(gap) => return Discharge::Unattempted(gap),
        };

        let mut search = Search {
            compiled: self.compiled(),
            body_root: cases.body_root.clone(),
            binders: obligation.generated().to_vec(),
            points,
            steps: plan.sim.steps,
            span: obligation.span,
        };
        concurrency::discharge(obligation, &plan.sim, &domain, &mut search).discharge
    }

    /// The points a concurrency law is searched at, and what claim covering them supports.
    fn law_domain(
        &self,
        obligation: &Obligation,
        cases: &mut Cases<'a>,
        plan: &ProvePlan,
    ) -> Result<(Vec<Vec<Value>>, ValueDomain), Gap> {
        let binders = obligation.generated();
        let mut kept: Vec<Vec<Value>> = Vec::new();

        if let Some(finite) = domain::finite(binders, &self.world) {
            for point in 0..finite.points {
                let Some(values) = finite.point(&self.world, point) else {
                    continue;
                };
                if self.admits(cases, &values)? {
                    kept.push(values);
                }
            }
            let domain = ValueDomain::Enumerated {
                domain: finite.name(),
                points: finite.points,
                kept: kept.len() as u64,
            };
            return Ok((kept, domain));
        }

        let plan = plan.clone().normalized();
        let types: Vec<ply_ty::Type> = binders.iter().map(|b| b.ty.clone()).collect();
        let mut generated = 0u32;
        for &root in &plan.roots {
            let mut stream = GenStream::new(root, obligation.key);
            for case in 0..plan.cases {
                let mut values = Vec::with_capacity(binders.len());
                for binder in binders {
                    match property::generate(&binder.ty, &self.world, &mut stream, case) {
                        Ok(value) => values.push(value),
                        Err(_) => {
                            return Err(Gap::Ungeneratable {
                                param: binder.name.clone(),
                                ty: binder.ty.clone(),
                            });
                        }
                    }
                }
                generated = generated.saturating_add(1);
                if self.admits(cases, &values)? {
                    kept.push(values);
                }
            }
        }
        let domain = ValueDomain::Sampled {
            generated,
            kept: u32::try_from(kept.len()).unwrap_or(u32::MAX),
            rejected: generated.saturating_sub(u32::try_from(kept.len()).unwrap_or(u32::MAX)),
            instantiations: property::instantiations(&types),
        };
        Ok((kept, domain))
    }

    fn admits(&self, cases: &mut Cases<'a>, values: &[Value]) -> Result<bool, Gap> {
        cases.guard(values).map_err(|diagnostic| Gap::Raised {
            bindings: bindings_of(&cases.binders, values),
            diagnostic,
        })
    }
}

/// The most guard evaluations one witness search spends.
const WITNESS_POINTS: usize = 4096;

/// The most candidate values one binder contributes.
const WITNESS_PER_BINDER: usize = 12;

/// The literals a guard is written in terms of, which is where its domain is.
#[derive(Default)]
struct Literals {
    ints: Vec<i64>,
    strings: Vec<String>,
    bytes: Vec<Vec<u8>>,
}

impl Literals {
    fn of(written: &[Literal]) -> Literals {
        let mut out = Literals::default();
        for literal in written {
            match literal {
                Literal::Int(k) => out.ints.push(*k),
                Literal::Str(s) => out.strings.push(s.clone()),
                Literal::Bytes(b) => out.bytes.push(b.clone()),
            }
        }
        out
    }
}

/// Certifies a static argument the prover could not vouch for, once a run kept a case.
fn upgrade(discharge: Discharge, witness: Option<Proof>) -> Discharge {
    let Some(proof) = witness else {
        return discharge;
    };
    let Discharge::Held(Evidence::Cases(report)) = &discharge else {
        return discharge;
    };
    if report.kept == 0 {
        return discharge;
    }
    match proof.certify(true) {
        Some(certificate) => Discharge::Held(Evidence::Proof(certificate)),
        None => discharge,
    }
}

fn bindings_of(binders: &[LawBinder], values: &[Value]) -> Vec<Binding> {
    binders
        .iter()
        .zip(values)
        .map(|(binder, value)| Binding {
            name: binder.name.clone(),
            ty: binder.ty.clone(),
            rendered: value.render(),
        })
        .collect()
}

/// How a tuple of binder values is judged: guard first, always.
struct Cases<'a> {
    machine: Machine<'a>,
    compiled: Option<Rc<dyn ply_eval::Compiled>>,
    guard_roots: Vec<Symbol>,
    body_root: Symbol,
    binders: Vec<LawBinder>,
    span: Span,
    /// The definition an `ensures` is attached to, called to produce `result`.
    call: Option<Symbol>,
    result: Option<Symbol>,
}

impl Cases<'_> {
    /// The proposition entered on the tier, the only evaluator a proposition has.
    fn on_tier(&self, root: &Symbol, args: &[Value]) -> Result<Value, Diagnostic> {
        let entered = match &self.compiled {
            Some(compiled) => compiled.enter_whole(root, args, DEFAULT_MAX_CALLS),
            None => ply_eval::Entered::Declined,
        };
        match entered {
            ply_eval::Entered::Answered(value) => Ok(value),
            ply_eval::Entered::Raised(d) => Err(d),
            ply_eval::Entered::Declined => Err(ply_eval::err_not_compiled(root, self.span)),
        }
    }

    fn boolean(&self, value: Value) -> Result<bool, Diagnostic> {
        match value {
            Value::Bool(b) => Ok(b),
            other => Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("a spec expression came to `{other}` rather than to a Boolean"),
            )
            .primary(self.span, "a spec states a proposition, so its type is `Bool`")
            .note("the type checker rejects a non-`Bool` clause with E0201, so reaching this is a defect in Ply")),
        }
    }
}

impl Judge for Cases<'_> {
    fn guard(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        for root in &self.guard_roots {
            let value = self.on_tier(root, values)?;
            if !self.boolean(value)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        // A law's binders, or an owner's parameters then `result`: the order `source.rs` expects.
        let mut args = values.to_vec();
        if let (Some(name), Some(_)) = (&self.call, &self.result) {
            let returned = self
                .machine
                .call(name.as_str(), values.to_vec(), self.span)?;
            args.push(returned);
        }
        let value = self.on_tier(&self.body_root, &args)?;
        self.boolean(value)
    }
}

/// One law body, run at a point of its value domain under a seed the interleaving search chooses.
struct Search {
    compiled: Option<Rc<dyn ply_eval::Compiled>>,
    body_root: Symbol,
    binders: Vec<LawBinder>,
    /// The points the guard kept, in order.
    points: Vec<Vec<Value>>,
    steps: u32,
    span: Span,
}

impl LawSearch for Search {
    fn run(&mut self, point: u64, seed: &Seed) -> BodyRun {
        let values = self.points.get(point as usize).cloned().unwrap_or_default();
        let declined = || ply_eval::err_not_compiled(&self.body_root, self.span);
        let (value, record) = match &self.compiled {
            Some(compiled) => {
                compiled.set_seed(seed.clone(), self.steps);
                match compiled.enter_whole(&self.body_root, &values, DEFAULT_MAX_CALLS) {
                    ply_eval::Entered::Answered(value) => (Ok(value), compiled.simulated()),
                    ply_eval::Entered::Raised(raised) => (Err(raised), compiled.simulated()),
                    ply_eval::Entered::Declined => (Err(declined()), None),
                }
            }
            None => (Err(declined()), None),
        };
        concurrency::body_run(record.as_ref(), value, self.span)
    }

    fn bindings(&self, point: u64) -> Vec<Binding> {
        match self.points.get(point as usize) {
            Some(values) => bindings_of(&self.binders, values),
            None => Vec::new(),
        }
    }
}

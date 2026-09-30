//! Which prover a run drives.

use crate::load::{LoadError, Loaded};
use ply_eval::host::{HostBinding, HostRuntime};
use ply_eval::{
    CheckOutput, DEFAULT_MAX_CALLS, DefInfo, Diagnostic, Front, LawInfo, Literal, Machine, Seed,
    Span, SpecKind, Symbol, Value, codes,
};
use ply_prove::concurrency::{self, BodyRun, LawSearch, ValueDomain};
use ply_prove::domain::Finite;
use ply_prove::property::{
    self, GenStream, Judge, Outcome, bindings, judge_case, run_property, ungeneratable,
};
use ply_prove::prove::claims::{Clause, Code, Definition, Law};
use ply_prove::prove::{self, Blocker, Claims, Decision, Goal, Limits, Proof};
use ply_prove::{
    Binder, Binding, Certificate, Counterexample, Discharge, Evidence, Gap, Obligation,
    ObligationKind, Points, ProvePlan, Rule, Sort, Strategy, Unsettled, Vacuity, VacuityKind,
    World,
};
use ply_store::Store;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// The discharger this build drives over the program's `world`, its claims kept in `store`. One
/// built once serves a whole run's discharges and re-runs alike.
pub fn prover<'a>(
    loaded: &'a Loaded,
    world: &'a World,
    hosting: Option<Hosting>,
    backend: &'static dyn ply_eval::Provider,
    store: &mut Store,
) -> Result<Prover<'a>, LoadError> {
    let prover = Prover::over(loaded, world, backend, Some(store))?;
    Ok(match hosting {
        Some(hosting) => prover.with_hosting(hosting),
        None => prover,
    })
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
    world: &'a World,
    /// Built once; `machine()` runs per obligation.
    ctx: prove::Context<'a>,
    laws: HashMap<Symbol, (usize, &'a LawInfo)>,
    /// What a `law/host` is discharged against.
    hosting: Option<Hosting>,
    /// A compiled unit holding the laws' and clauses' roots, where those propositions are entered.
    backend: &'static dyn ply_eval::Provider,
}

/// The binding and the reactor a `law/host` runs against. The factory is owned rather than
/// borrowed, so a prover holding a hosting borrows the program and nothing else, and a caller can
/// keep one prover across many steps.
pub struct Hosting {
    pub binding: Arc<HostBinding>,
    pub runtime: Option<Arc<dyn Fn() -> Rc<dyn HostRuntime> + Sync + Send>>,
}

impl<'a> Prover<'a> {
    /// `backend` is the unit built from `loaded`, laws' and clauses' roots included.
    pub fn new(
        loaded: &'a Loaded,
        world: &'a World,
        backend: &'static dyn ply_eval::Provider,
    ) -> Result<Prover<'a>, LoadError> {
        Prover::over(loaded, world, backend, None)
    }

    fn over(
        loaded: &'a Loaded,
        world: &'a World,
        backend: &'static dyn ply_eval::Provider,
        store: Option<&mut Store>,
    ) -> Result<Prover<'a>, LoadError> {
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
            world,
            ctx: prove::Context::new(claims_of(loaded, store)?, world),
            laws,
            hosting: None,
            backend,
        })
    }

    /// The unit, attached once per thread: obligations are discharged on pool threads.
    fn compiled(&self) -> Rc<dyn ply_eval::Compiled> {
        thread_local! {
            static ATTACHED: RefCell<Vec<(usize, Rc<dyn ply_eval::Compiled>)>> =
                const { RefCell::new(Vec::new()) };
        }
        let key = std::ptr::from_ref(self.backend).cast::<()>() as usize;
        ATTACHED.with(|attached| {
            if let Some((_, c)) = attached.borrow().iter().find(|(k, _)| *k == key) {
                return Rc::clone(c);
            }
            let c = self.backend.attach();
            attached.borrow_mut().push((key, Rc::clone(&c)));
            c
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
    pub fn with_hosting(mut self, hosting: Hosting) -> Prover<'a> {
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

    /// What an owner is called through to produce `result`: the tier its propositions are entered
    /// on, attached afresh.
    fn machine(&self) -> Result<Machine<'a>, Gap> {
        Machine::new(self.front, self.backend.attach())
            .map(|machine| machine.with_max_calls(DEFAULT_MAX_CALLS))
            .map_err(|refused| Gap::Raised {
                bindings: Vec::new(),
                diagnostic: Box::new(refused),
                root: 0,
                case: 0,
            })
    }

    /// The machine a `law/host`'s body runs on: the run's binding and a reactor for this thread.
    fn host_machine(&self, hosting: &Hosting) -> Result<Machine<'a>, Gap> {
        let mut machine = self.machine()?;
        machine.set_host_binding(Arc::clone(&hosting.binding));
        if let Some(factory) = &hosting.runtime {
            machine.set_host_runtime(factory());
        }
        Ok(machine)
    }

    fn decide(
        &self,
        obligation: &Obligation,
        claim: &Claim<'_>,
        plan: &ProvePlan,
    ) -> (Decision, Vec<Blocker>) {
        let guards: Vec<&Code> = claim.guards().into_iter().map(|g| &g.code).collect();
        let result = match claim {
            Claim::Ensures { def, .. } => obligation.result.as_ref().map(|_| &def.body),
            Claim::Law { .. } => None,
        };
        let binders = obligation.all_binders();
        let goal = Goal {
            binders: &binders,
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
            Decision::Proved(proof) => match proof.certify(false, &obligation.variables) {
                Some(certificate) => Static::Proved(certificate),
                None => Static::NeedsWitness(proof),
            },
            Decision::Unknown { .. } => Static::Inconclusive,
        }
    }

    /// What the static tier alone answered, and where the obligation left the fragment on the way:
    /// nothing for a claim whose strategy never asks it.
    pub fn reach(&self, obligation: &Obligation, plan: &ProvePlan) -> Option<Reach> {
        if let Strategy::Interleave(_) = obligation.strategy {
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

/// One point of one obligation's guard, as `claims.ply` reads it: what a search over cases is
/// made of, one case at a time. `Undrawn` is the obligation's own gap — the points are not drawn,
/// or not drawn one at a time — rather than anything about this draw.
#[derive(Debug)]
pub enum Point {
    Kept(Vec<Binding>),
    /// The guard admitted the point and the body does not hold there: this point falsifies the
    /// claim, which is what a shrinker starts from.
    Falsified(Vec<Binding>),
    Rejected,
    /// No point was drawn, and the gap says why.
    Undrawn(Gap),
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
    /// The program as the prover reads it.
    pub fn world(&self) -> &World {
        self.world
    }

    /// Judge one tuple the way a discharge would: the same guard, the same body, the same case
    /// machinery. Which tuples to ask about is the program's, so the walk that makes a
    /// counterexample small drives this rather than running here.
    pub fn judge_at(&self, obligation: &Obligation, plan: &ProvePlan, values: &[Value]) -> Outcome {
        let Some(claim) = self.claim(obligation) else {
            return Outcome::Rejected;
        };
        match self.cases(obligation, &claim, plan) {
            Ok(mut cases) => judge_case(&mut cases, values),
            Err(Gap::Raised { diagnostic, .. }) => Outcome::Raised(*diagnostic),
            Err(_) => Outcome::Rejected,
        }
    }

    /// One obligation, discharged the way its strategy says, at the strongest tier this build can
    /// demonstrate.
    pub fn discharge_with(&self, obligation: &Obligation, plan: &ProvePlan) -> Discharge {
        let Some(claim) = self.claim(obligation) else {
            return Discharge::Unattempted(Gap::UnhandledEffect(obligation.footprint.clone()));
        };
        match &obligation.strategy {
            Strategy::Interleave(points) => {
                self.search_interleavings(obligation, &claim, plan, points)
            }
            Strategy::Hosted => self.discharge_host(obligation, &claim, plan),
            Strategy::Static(unsettled) => {
                self.discharge_static(obligation, &claim, plan, unsettled)
            }
        }
    }

    /// The static prover first; what it did not settle is a gap, or a run over the claim's points.
    fn discharge_static(
        &self,
        obligation: &Obligation,
        claim: &Claim<'_>,
        plan: &ProvePlan,
        unsettled: &Unsettled,
    ) -> Discharge {
        let witness = match self.attempt_static(obligation, claim, plan) {
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
        let points = match unsettled {
            Unsettled::Unhandled(row) => {
                return Discharge::Unattempted(Gap::UnhandledEffect(Some(row.clone())));
            }
            Unsettled::Run(points) => points,
        };
        let mut cases = match self.cases(obligation, claim, plan) {
            Ok(cases) => cases,
            Err(gap) => return Discharge::Unattempted(gap),
        };
        match points {
            Points::Every(finite) => self.enumerate(obligation, claim, finite, &mut cases, witness),
            Points::Drawn => self.sample(obligation, claim, plan, &mut cases, witness),
        }
    }

    /// The property search, with the static argument certified once a kept case witnesses its
    /// domain.
    fn sample(
        &self,
        obligation: &Obligation,
        claim: &Claim<'_>,
        plan: &ProvePlan,
        cases: &mut Cases<'a>,
        witness: Option<Proof>,
    ) -> Discharge {
        let discharge = run_property(
            obligation.key,
            &obligation.binders,
            &obligation.variables,
            self.world,
            plan,
            claim.guard_span(obligation.span),
            cases,
        );
        match discharge {
            // Keeping no sample means the generator missed the guard, not that it admits nothing.
            Discharge::Vacuous(Vacuity {
                kind: VacuityKind::NoCaseKept { generated },
                ..
            }) => match self.witness(obligation, claim, cases) {
                Some(values) => {
                    match witness.and_then(|proof| proof.certify(true, &obligation.variables)) {
                        Some(certificate) => Discharge::Held(Evidence::Proof(certificate)),
                        None => Discharge::Unattempted(Gap::GuardNotSampled {
                            generated,
                            witness: bindings(&obligation.binders, &values),
                        }),
                    }
                }
                None => discharge,
            },
            other => upgrade(other, witness, &obligation.variables),
        }
    }

    /// One point of one obligation's guard, at a root and case the caller chose, or the gap that
    /// stops the obligation being run a point at a time at all. This is what a search over cases
    /// is made of, one case at a time: a caller that wants to shrink a refutation, or cover a
    /// finite domain, drives the draws rather than asking for the whole search.
    pub fn point_at(
        &self,
        obligation: &Obligation,
        root: u64,
        case: u32,
        plan: &ProvePlan,
    ) -> Point {
        let Some(claim) = self.claim(obligation) else {
            return Point::Undrawn(Gap::UnhandledEffect(obligation.footprint.clone()));
        };
        if let Strategy::Interleave(_) = obligation.strategy {
            return Point::Undrawn(Gap::NotDrawn);
        }
        let mut cases = match self.cases(obligation, &claim, plan) {
            Ok(cases) => cases,
            Err(gap) => return Point::Undrawn(gap),
        };
        match &obligation.strategy {
            Strategy::Hosted => {
                let Some(hosting) = &self.hosting else {
                    return Point::Undrawn(Gap::ReachesHost(obligation.footprint.clone()));
                };
                cases.machine = match self.host_machine(hosting) {
                    Ok(machine) => machine,
                    Err(gap) => return Point::Undrawn(gap),
                };
            }
            Strategy::Static(Unsettled::Unhandled(row)) => {
                return Point::Undrawn(Gap::UnhandledEffect(Some(row.clone())));
            }
            Strategy::Static(Unsettled::Run(_)) | Strategy::Interleave(_) => {}
        }

        // The draw is the generator's, from a stream the caller seeds: the same point a whole
        // run would have reached at this root and case.
        let mut stream = GenStream::new(root, obligation.key);
        let mut values = Vec::with_capacity(obligation.binders.len());
        for binder in &obligation.binders {
            match property::generate(&binder.sort, self.world, &mut stream, case) {
                Ok(value) => values.push(value),
                Err(_) => return Point::Undrawn(ungeneratable(binder)),
            }
        }
        let bindings = bindings(&obligation.binders, &values);
        match judge_case(&mut cases, &values) {
            Outcome::Held => Point::Kept(bindings),
            Outcome::Failed => Point::Falsified(bindings),
            Outcome::Rejected => Point::Rejected,
            // A raise at one point is the obligation's own gap, in the same words a whole run
            // reports it in: nothing was refuted and nothing was established.
            // A point drawn for a replay has no root or case to go back to: it was named, not
            // drawn, so there is nothing a walk could regenerate.
            Outcome::Raised(diagnostic) => Point::Undrawn(Gap::Raised {
                bindings,
                diagnostic: Box::new(diagnostic),
                root: 0,
                case: 0,
            }),
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
        let mut cases = match self.cases(obligation, claim, plan) {
            Ok(cases) => cases,
            Err(gap) => return Discharge::Unattempted(gap),
        };
        cases.machine = match self.host_machine(hosting) {
            Ok(machine) => machine,
            Err(gap) => return Discharge::Unattempted(gap),
        };
        run_property(
            obligation.key,
            &obligation.binders,
            &obligation.variables,
            self.world,
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
            let column = match self.candidates(&binder.sort, &literals) {
                Some(column) => column,
                // A shape the guard's literals cannot name: a list, record, ADT or function.
                None => vec![property::generate(&binder.sort, self.world, &mut stream, 0).ok()?],
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
    fn candidates(&self, sort: &Sort, literals: &Literals) -> Option<Vec<Value>> {
        let Sort::Con(name, args) = sort else {
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

    fn cases(
        &self,
        obligation: &Obligation,
        claim: &Claim<'_>,
        plan: &ProvePlan,
    ) -> Result<Cases<'a>, Gap> {
        let call = match claim {
            Claim::Ensures { .. } => Some(obligation.owner.clone()),
            Claim::Law { .. } => None,
        };
        let result = obligation.result.as_ref().map(|b| b.name.clone());
        if let Some(binder) = obligation
            .binders
            .iter()
            .find(|b| property::generatable(&b.sort, self.world).is_err())
        {
            return Err(ungeneratable(binder));
        }
        Ok(Cases {
            machine: self.machine()?,
            compiled: self.compiled(),
            guard_roots: self.guard_roots(claim),
            body_root: self.body_root(claim),
            binders: obligation.binders.clone(),
            span: obligation.span,
            call,
            result,
            step_budget: plan.step_budget,
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
            let Some(values) = finite.point(point) else {
                return Discharge::Unattempted(ungeneratable(&obligation.binders[0]));
            };
            match judge_case(cases, &values) {
                Outcome::Rejected => {}
                Outcome::Held => kept += 1,
                Outcome::Failed => {
                    // No shrinking: the enumeration order is fixed.
                    let bindings = bindings(&obligation.binders, &values);
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
                        bindings: bindings(&obligation.binders, &values),
                        diagnostic: Box::new(diagnostic),
                        // The domain's order is the walk's: the index is the case.
                        root: 0,
                        case: u32::try_from(point).unwrap_or(u32::MAX),
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
            && let Some(certificate) = proof.certify(true, &obligation.variables)
        {
            return Discharge::Held(Evidence::Proof(certificate));
        }

        let rule = if obligation.binders.is_empty() {
            Rule::GroundEvaluation
        } else {
            Rule::ExhaustiveEnumeration {
                domain: finite.name.clone(),
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
        points: &Points,
    ) -> Discharge {
        let mut cases = match self.cases(obligation, claim, plan) {
            Ok(cases) => cases,
            Err(gap) => return Discharge::Unattempted(gap),
        };

        let (points, domain) = match self.law_domain(obligation, &mut cases, plan, points) {
            Ok(kept) => kept,
            Err(gap) => return Discharge::Unattempted(gap),
        };

        let mut search = Search {
            compiled: self.compiled(),
            body_root: cases.body_root.clone(),
            binders: obligation.binders.clone(),
            points,
            steps: plan.sim.steps,
            step_budget: plan.step_budget,
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
        points: &Points,
    ) -> Result<(Vec<Vec<Value>>, ValueDomain), Gap> {
        let binders = &obligation.binders;
        let mut kept: Vec<Vec<Value>> = Vec::new();

        if let Points::Every(finite) = points {
            for point in 0..finite.points {
                let Some(values) = finite.point(point) else {
                    continue;
                };
                if self.admits(cases, &values)? {
                    kept.push(values);
                }
            }
            let domain = ValueDomain::Enumerated {
                domain: finite.name.clone(),
                points: finite.points,
                kept: kept.len() as u64,
            };
            return Ok((kept, domain));
        }

        let plan = plan.clone().normalized();
        let mut generated = 0u32;
        for &root in &plan.roots {
            let mut stream = GenStream::new(root, obligation.key);
            for case in 0..plan.cases {
                let mut values = Vec::with_capacity(binders.len());
                for binder in binders {
                    match property::generate(&binder.sort, self.world, &mut stream, case) {
                        Ok(value) => values.push(value),
                        Err(_) => return Err(ungeneratable(binder)),
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
            instantiations: property::instantiations(binders, &obligation.variables),
        };
        Ok((kept, domain))
    }

    fn admits(&self, cases: &mut Cases<'a>, values: &[Value]) -> Result<bool, Gap> {
        cases.guard(values).map_err(|diagnostic| Gap::Raised {
            bindings: bindings(&cases.binders, values),
            diagnostic: Box::new(diagnostic),
            // A guard that raised while values were handed in: nothing here knows the draw.
            root: 0,
            case: 0,
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
fn upgrade(discharge: Discharge, witness: Option<Proof>, variables: &[Symbol]) -> Discharge {
    let Some(proof) = witness else {
        return discharge;
    };
    let Discharge::Held(Evidence::Cases(report)) = &discharge else {
        return discharge;
    };
    if report.kept == 0 {
        return discharge;
    }
    match proof.certify(true, variables) {
        Some(certificate) => Discharge::Held(Evidence::Proof(certificate)),
        None => discharge,
    }
}

/// How a tuple of binder values is judged: guard first, always.
struct Cases<'a> {
    machine: Machine<'a>,
    compiled: Rc<dyn ply_eval::Compiled>,
    guard_roots: Vec<Symbol>,
    body_root: Symbol,
    binders: Vec<Binder>,
    span: Span,
    /// The definition an `ensures` is attached to, called to produce `result`.
    call: Option<Symbol>,
    result: Option<Symbol>,
    step_budget: i64,
}

impl Cases<'_> {
    /// The proposition entered on the tier, the only evaluator a proposition has.
    fn on_tier(&self, root: &Symbol, args: &[Value]) -> Result<Value, Diagnostic> {
        let entered = ply_codegen::rt::with_step_budget(self.step_budget, || {
            self.compiled.enter_whole(root, args, DEFAULT_MAX_CALLS)
        });
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
    compiled: Rc<dyn ply_eval::Compiled>,
    body_root: Symbol,
    binders: Vec<Binder>,
    /// The points the guard kept, in order.
    points: Vec<Vec<Value>>,
    /// A `simulate` region's own budget: scheduling steps, not calls.
    steps: u32,
    /// Calls one evaluation of the body may make.
    step_budget: i64,
    span: Span,
}

impl LawSearch for Search {
    fn run(&mut self, point: u64, seed: &Seed) -> BodyRun {
        let values = self.points.get(point as usize).cloned().unwrap_or_default();
        let compiled = &self.compiled;
        compiled.set_seed(seed.clone(), self.steps);
        let entered = ply_codegen::rt::with_step_budget(self.step_budget, || {
            compiled.enter_whole(&self.body_root, &values, DEFAULT_MAX_CALLS)
        });
        let (value, record) = match entered {
            ply_eval::Entered::Answered(value) => (Ok(value), compiled.simulated()),
            ply_eval::Entered::Raised(raised) => (Err(raised), compiled.simulated()),
            ply_eval::Entered::Declined => (
                Err(ply_eval::err_not_compiled(&self.body_root, self.span)),
                None,
            ),
        };
        concurrency::body_run(record.as_ref(), value, self.span)
    }

    fn bindings(&self, point: u64) -> Vec<Binding> {
        match self.points.get(point as usize) {
            Some(values) => bindings(&self.binders, values),
            None => Vec::new(),
        }
    }
}

//! Which prover a run drives.

use ply_core::{CheckOutput, LawBinder};
use ply_eval::host::{HostBinding, HostRuntime};
use ply_eval::{DEFAULT_MAX_CALLS, Machine, Seed, Value};
use ply_prove::concurrency::{self, BodyRun, LawSearch, ValueDomain};
use ply_prove::domain::{self, Finite};
use ply_prove::property::{self, GenStream, Judge, Outcome, TypeWorld, judge_case, run_property};
use ply_prove::prove::{self, Blocker, Decision, Goal, Limits, Proof};
use ply_prove::{
    Binding, Certificate, Counterexample, Discharge, Evidence, Gap, Obligation, ObligationKind,
    ProvePlan, Rule, Vacuity, VacuityKind,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Expr, ExprKind, FnDef, Item, LawDef, Program, SpecKind};
use ply_syntax::resolve::Resolved;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// The discharger this build drives, and what the reader has to be told about it.
pub fn of<'a>(
    program: &'a Program,
    resolved: &'a Resolved,
    check: &'a CheckOutput,
    complete: bool,
    obligations: usize,
    hosting: Option<Hosting<'a>>,
) -> (
    Box<dyn ply_test::obligation::Discharger + 'a>,
    Option<Diagnostic>,
) {
    // Every clause and every law body is an AST this run has to hold: a claim is discharged by
    // reasoning about the expression that states it, and there is no cached form of one.
    if !complete {
        return (
            Box::new(ply_test::obligation::Undecided),
            (obligations > 0).then(incomplete),
        );
    }
    let prover = Prover::new(program, resolved, check);
    let prover = match hosting {
        Some(hosting) => prover.with_hosting(hosting),
        None => prover,
    };
    (Box::new(prover), None)
}

fn incomplete() -> Diagnostic {
    Diagnostic::warning(
        codes::OBLIGATION_NOT_DISCHARGED,
        "not every module was parsed, so no obligation was attempted",
    )
    .note("every obligation is reported `unattempted`: no tier is claimed for any of them")
    .note("run again with `--no-incremental`")
}

/// Where an obligation's claim is written, found once per run.
enum Claim<'a> {
    Ensures {
        module: usize,
        owner: Symbol,
        def: &'a FnDef,
        clause: &'a Expr,
    },
    Law {
        module: usize,
        def: &'a LawDef,
    },
}

impl<'a> Claim<'a> {
    fn module(&self) -> usize {
        match self {
            Claim::Ensures { module, .. } | Claim::Law { module, .. } => *module,
        }
    }

    /// The propositions that narrow the domain: an owner's `requires` clauses, or a law's `where`.
    fn guards(&self) -> Vec<&'a Expr> {
        match self {
            Claim::Ensures { def, .. } => def
                .spec
                .iter()
                .filter(|c| c.kind == SpecKind::Requires)
                .map(|c| &c.expr)
                .collect(),
            Claim::Law { def, .. } => def.guard.iter().collect(),
        }
    }

    fn body(&self) -> &'a Expr {
        match self {
            Claim::Ensures { clause, .. } => clause,
            Claim::Law { def, .. } => &def.body,
        }
    }

    /// Where a vacuity points.
    fn guard_span(&self, fallback: Span) -> Span {
        self.guards().first().map_or(fallback, |g| g.span)
    }
}

pub struct Prover<'a> {
    program: &'a Program,
    resolved: &'a Resolved,
    check: &'a CheckOutput,
    world: TypeWorld,
    /// Built once.
    ctx: prove::Context<'a>,
    defs: HashMap<Symbol, (usize, &'a FnDef)>,
    laws: HashMap<Symbol, (usize, &'a LawDef)>,
    /// What a `law/host` is discharged against.
    hosting: Option<Hosting<'a>>,
    /// This program's region kinds, for the reason `ctx` is built once: the analysis behind them is
    /// whole-program, and `machine()` is called per obligation.
    region_kinds: ply_eval::region_kind::Kinds,
}

/// The binding and the reactor a `law/host` runs against.
pub struct Hosting<'a> {
    pub binding: Arc<HostBinding>,
    pub runtime: Option<&'a (dyn Fn() -> Rc<dyn HostRuntime> + Sync)>,
}

impl<'a> Prover<'a> {
    pub fn new(program: &'a Program, resolved: &'a Resolved, check: &'a CheckOutput) -> Prover<'a> {
        let mut defs = HashMap::new();
        let mut laws = HashMap::new();
        for (index, module) in program.modules.iter().enumerate() {
            for item in &module.items {
                match item {
                    Item::Fn(def) => {
                        defs.insert(module.name.qualify(&def.name.name), (index, &**def));
                    }
                    Item::Law(def) => {
                        laws.insert(
                            module.name.qualify(&Symbol::new(&def.name)),
                            (index, &**def),
                        );
                    }
                    _ => {}
                }
            }
        }
        Prover {
            program,
            resolved,
            check,
            world: TypeWorld::new(check.ctors.values()),
            ctx: prove::Context::new(program, resolved, check),
            defs,
            laws,
            hosting: None,
            region_kinds: ply_eval::region_kind::Kinds::default(),
        }
    }

    /// Bind the host, so that a `law/host` is attempted rather than reported as a gap.
    pub fn with_hosting(mut self, hosting: Hosting<'a>) -> Prover<'a> {
        self.hosting = Some(hosting);
        self
    }

    fn claim(&self, obligation: &Obligation) -> Option<Claim<'a>> {
        match obligation.kind {
            ObligationKind::Ensures { index } => {
                let &(module, def) = self.defs.get(&obligation.owner)?;
                let clause = def
                    .spec
                    .iter()
                    .filter(|c| c.kind == SpecKind::Ensures)
                    .nth(index)?;
                Some(Claim::Ensures {
                    module,
                    owner: obligation.owner.clone(),
                    def,
                    clause: &clause.expr,
                })
            }
            ObligationKind::Law => {
                let &(module, def) = self.laws.get(&obligation.owner)?;
                Some(Claim::Law { module, def })
            }
        }
    }

    fn machine(&self) -> Machine<'a> {
        let mut machine =
            Machine::new(self.program, self.resolved, self.check).with_max_calls(DEFAULT_MAX_CALLS);
        machine.share_region_kinds(ply_eval::region_kind::Kinds::clone(&self.region_kinds));
        machine
    }

    /// The machine a `law/host`'s body runs on: the run's binding, and a reactor minted for this
    /// thread.
    fn host_machine(&self, hosting: &Hosting<'a>) -> Machine<'a> {
        let mut machine = self.machine();
        machine.set_host_binding(Arc::clone(&hosting.binding));
        if let Some(factory) = hosting.runtime {
            machine.set_host_runtime(factory());
        }
        machine
    }

    fn attempt_static(
        &self,
        obligation: &Obligation,
        claim: &Claim<'a>,
        plan: &ProvePlan,
    ) -> Static {
        let guards = claim.guards();
        let result = match claim {
            Claim::Ensures { def, .. } => obligation
                .result_binder()
                .map(|binder| (binder.name.clone(), &def.body)),
            Claim::Law { .. } => None,
        };
        let goal = Goal {
            module: claim.module(),
            binders: &obligation.binders,
            guards: &guards,
            result,
            body: claim.body(),
        };
        let limits = Limits {
            steps: plan.prove_budget,
            ..Limits::default()
        };
        match prove::decide(&self.ctx, &goal, &limits) {
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
        let guards = claim.guards();
        let result = match &claim {
            Claim::Ensures { def, .. } => obligation
                .result_binder()
                .map(|binder| (binder.name.clone(), &def.body)),
            Claim::Law { .. } => None,
        };
        let goal = Goal {
            module: claim.module(),
            binders: &obligation.binders,
            guards: &guards,
            result,
            body: claim.body(),
        };
        let limits = Limits {
            steps: plan.prove_budget,
            ..Limits::default()
        };
        let (decision, blockers) = prove::decide_and_diagnose(&self.ctx, &goal, &limits);
        Some(Reach { decision, blockers })
    }
}

/// What the static tier answered for one obligation, and the fragment boundaries it crossed getting
/// there.
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

        // Checking an `ensures` at either running tier means *calling* the definition, and a
        // definition that performs needs a handler nothing supplies.
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
            // A sampled run that kept nothing has **not** established that the guard admits
            // nothing: it has established that the generator drew nothing the guard wanted.
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
        claim: &Claim<'a>,
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

    /// A tuple of binder values the guard admits, found by evaluating the guard at points the
    /// guard's own literals name.
    fn witness(
        &self,
        obligation: &Obligation,
        claim: &Claim<'a>,
        cases: &mut Cases<'a>,
    ) -> Option<Vec<Value>> {
        let mut literals = Literals::default();
        for guard in claim.guards() {
            literals.collect(guard);
        }
        let mut stream = GenStream::new(0, obligation.key);
        let mut columns: Vec<Vec<Value>> = Vec::with_capacity(cases.binders.len());
        let mut points = 1usize;
        for binder in &cases.binders {
            let column = match self.candidates(&binder.ty, &literals) {
                Some(column) => column,
                // A shape whose candidates the guard's literals do not name — a list, a record, an
                // ADT, a function.
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
            // A point the guard raises at is not a point it admits, and a raise here is the
            // property tier's business rather than this one's.
            if cases.guard(&values).unwrap_or(false) {
                return Some(values);
            }
        }
        None
    }

    /// The values one binder is tried at, smallest and most literal first.
    fn candidates(&self, ty: &ply_core::Type, literals: &Literals) -> Option<Vec<Value>> {
        let ply_core::Type::Con(name, args) = ty else {
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
                // A bound and the two integers beside it: `x > 1000000` is satisfied by `1000001`
                // and by nothing the literal itself names.
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
    fn unhandled(&self, obligation: &Obligation) -> Option<ply_core::Footprint> {
        let ObligationKind::Ensures { .. } = obligation.kind else {
            return None;
        };
        let footprint = &self.check.defs.get(&obligation.owner)?.footprint;
        (!footprint.is_empty()).then(|| footprint.clone())
    }

    fn cases(&self, obligation: &Obligation, claim: &Claim<'a>) -> Result<Cases<'a>, Gap> {
        let call = match claim {
            Claim::Ensures { owner, .. } => Some(owner.clone()),
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
            module: claim.module(),
            binders: obligation.generated().to_vec(),
            guards: claim.guards(),
            body: claim.body(),
            span: obligation.span,
            call,
            result,
        })
    }

    fn enumerate(
        &self,
        obligation: &Obligation,
        claim: &Claim<'a>,
        finite: &Finite,
        cases: &mut Cases<'a>,
        witness: Option<Proof>,
    ) -> Discharge {
        let mut kept = 0u64;
        for point in 0..finite.points {
            // A domain that cannot produce one of its own points is not one this tier may claim to
            // have covered, and reporting anything but a gap for it would be claiming coverage
            // nothing established.
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
                    // No shrinking: the point is already a member of a domain enumerated in a fixed
                    // order, so it is the same value on every run and there is nothing smaller to
                    // find.
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
            // Enumerating a finite domain and keeping nothing *decides* the guard unsatisfiable —
            // §5.1(f) applied to the guard rather than to the body.
            return Discharge::Vacuous(Vacuity {
                guard: claim.guard_span(obligation.span),
                kind: VacuityKind::ProvedUnsatisfiable,
            });
        }

        // A kept point witnesses the domain, so a static argument the prover could not vouch for is
        // now vouched for — by a value that actually ran.
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

    /// A law whose body reaches a `simulate` region: discharged by execution, and the only place in
    /// this milestone a `proved` does not come from a static argument.
    fn search_interleavings(
        &self,
        obligation: &Obligation,
        claim: &Claim<'a>,
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
            prover: self,
            module: claim.module(),
            body: claim.body(),
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
        let types: Vec<ply_core::Type> = binders.iter().map(|b| b.ty.clone()).collect();
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
    fn collect(&mut self, expr: &Expr) {
        let mut stack = vec![expr];
        while let Some(e) = stack.pop() {
            match &e.kind {
                ExprKind::Lit(ply_syntax::ast::Lit::Int(k)) => {
                    if !self.ints.contains(k) {
                        self.ints.push(*k);
                    }
                }
                ExprKind::Lit(ply_syntax::ast::Lit::Str(s)) => {
                    if !self.strings.contains(s) {
                        self.strings.push(s.clone());
                    }
                }
                ExprKind::Lit(ply_syntax::ast::Lit::Bytes(b)) => {
                    if !self.bytes.contains(b) {
                        self.bytes.push(b.clone());
                    }
                }
                ExprKind::Lit(_) | ExprKind::Var(_) => {}
                ExprKind::Binary { lhs, rhs, .. } => {
                    stack.push(lhs);
                    stack.push(rhs);
                }
                ExprKind::Unary { op, operand } => {
                    // `-1000000` is a negation of a literal in the AST and a bound in the guard, so
                    // the value the search wants is the negated one.
                    if let (
                        ply_syntax::ast::UnOp::Neg,
                        ExprKind::Lit(ply_syntax::ast::Lit::Int(k)),
                    ) = (op, &operand.kind)
                    {
                        let negated = k.saturating_neg();
                        if !self.ints.contains(&negated) {
                            self.ints.push(negated);
                        }
                    }
                    stack.push(operand);
                }
                ExprKind::App { func, args, .. } => {
                    stack.push(func);
                    stack.extend(args);
                }
                ExprKind::If {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    stack.push(cond);
                    stack.push(then_branch);
                    stack.push(else_branch);
                }
                ExprKind::Lambda { body, .. } => stack.push(body),
                ExprKind::Match { scrutinee, arms } => {
                    stack.push(scrutinee);
                    for arm in arms {
                        stack.extend(arm.guard.iter());
                        stack.push(&arm.body);
                    }
                }
                ExprKind::Block { stmts, tail } => {
                    for stmt in stmts {
                        match stmt {
                            ply_syntax::ast::Stmt::Let { value, .. } => stack.push(value),
                            ply_syntax::ast::Stmt::Expr(e) => stack.push(e),
                        }
                    }
                    stack.extend(tail.as_deref());
                }
                ExprKind::Record { fields } => stack.extend(fields.iter().map(|(_, v)| v)),
                ExprKind::RecordUpdate { base, fields } => {
                    stack.push(base);
                    stack.extend(fields.iter().map(|(_, v)| v));
                }
                ExprKind::Field { base, .. } => stack.push(base),
                ExprKind::Try { operand } => stack.push(operand),
                ExprKind::List { items } => stack.extend(items),
                ExprKind::Perform { args, .. } => stack.extend(args),
                ExprKind::Handle { body, .. } => stack.push(body),
                ExprKind::WithCell { init, body, .. } => {
                    stack.push(init);
                    stack.push(body);
                }
                ExprKind::WithRegion { body, .. } | ExprKind::Simulate { body } => stack.push(body),
            }
        }
    }
}

/// A static argument the prover made but could not vouch for the domain of, upgraded by a run that
/// kept a case.
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
    module: usize,
    binders: Vec<LawBinder>,
    guards: Vec<&'a Expr>,
    body: &'a Expr,
    span: Span,
    /// The definition an `ensures` is attached to, called to produce `result`.
    call: Option<Symbol>,
    result: Option<Symbol>,
}

impl Cases<'_> {
    fn scope(&self, values: &[Value]) -> Vec<(Symbol, Value)> {
        self.binders
            .iter()
            .zip(values)
            .map(|(binder, value)| (binder.name.clone(), value.clone()))
            .collect()
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
        let scope = self.scope(values);
        for guard in &self.guards {
            let value = self.machine.eval_expr_in(guard, self.module, &scope)?;
            if !self.boolean(value)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        let mut scope = self.scope(values);
        if let (Some(name), Some(result)) = (&self.call, &self.result) {
            let returned = self
                .machine
                .call(name.as_str(), values.to_vec(), self.span)?;
            scope.push((result.clone(), returned));
        }
        let value = self.machine.eval_expr_in(self.body, self.module, &scope)?;
        self.boolean(value)
    }
}

/// One law body, run at a point of its value domain under a seed the interleaving search chooses.
struct Search<'a, 'p> {
    prover: &'p Prover<'a>,
    module: usize,
    body: &'a Expr,
    binders: Vec<LawBinder>,
    /// The points the guard kept, in order.
    points: Vec<Vec<Value>>,
    steps: u32,
    span: Span,
}

impl LawSearch for Search<'_, '_> {
    fn run(&mut self, point: u64, seed: &Seed) -> BodyRun {
        let values = self.points.get(point as usize).cloned().unwrap_or_default();
        let scope: Vec<(Symbol, Value)> = self
            .binders
            .iter()
            .zip(&values)
            .map(|(binder, value)| (binder.name.clone(), value.clone()))
            .collect();
        let mut machine = self.prover.machine();
        machine.set_seed(seed.clone(), self.steps);
        let value = machine.eval_expr_in(self.body, self.module, &scope);
        concurrency::body_run(&machine, value, self.span)
    }

    fn bindings(&self, point: u64) -> Vec<Binding> {
        match self.points.get(point as usize) {
            Some(values) => bindings_of(&self.binders, values),
            None => Vec::new(),
        }
    }
}

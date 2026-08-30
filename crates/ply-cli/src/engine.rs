//! Which prover a run drives.
//!
//! One file, because it is the only place the two halves of `ply prove` meet:
//! `ply-test` owns what a cached discharge means and `ply-prove` owns what a
//! fresh one is worth, and neither may quietly acquire the other's
//! responsibility. `ply-prove` does not depend on `ply-test` and never will, so
//! the [`Discharger`] that ties them together lives here, in the one crate that
//! depends on both.
//!
//! **The order every obligation is attempted in is the whole honesty of this
//! module**, and it is ADR 0007's:
//!
//! 1. the **static prover**, because a proof needs to run nothing and is valid
//!    under every plan. It answers `proved`, `vacuous`, or nothing at all —
//!    never a refutation, because a model over uninterpreted symbols need not
//!    correspond to any Ply value;
//! 2. **exhaustive enumeration** of a finite value domain, which is a proof for
//!    the same reason the prover's answer is: the domain was covered rather
//!    than sampled. A ground claim is the degenerate case with one point;
//! 3. the **property tier**, which is where refutation happens, with a value
//!    that actually ran;
//! 4. a **directed witness search**, reached only where the sampled run kept no
//!    case. It answers one question — does the guard admit anything? — by
//!    evaluating it at the points the guard's own literals name, and it is what
//!    separates "the search missed the domain" from "there is no domain".
//!
//! An inconclusive step falls through to the next one. Nothing here can report
//! `proved` without a [`Certificate`], and nothing constructs one except a
//! decided proof or a covered domain.

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

/// The discharger this build drives, and what the reader has to be told about
/// it.
///
/// `obligations` is how many claims are in front of it. A project with none is
/// told nothing: there is no gap to disclose when nothing was going to be
/// attempted, and a warning on every run of an unspecified project is noise
/// that teaches a reader to skip warnings.
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
    // Every clause and every law body is an AST this run has to hold: a claim
    // is discharged by reasoning about the expression that states it, and there
    // is no cached form of one. A partial parse would silently answer
    // `unattempted` for everything it did not read, which reports a gap where
    // there is a claim.
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

    /// The propositions that narrow the domain: an owner's `requires` clauses,
    /// or a law's `where`. A `requires` is never an obligation of its own — it
    /// is a filter on the domain of the `ensures` clauses beside it.
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

    /// Where a vacuity points. The first guard if there is one, else the claim
    /// itself — a domain nothing narrowed is empty because its binders' types
    /// are.
    fn guard_span(&self, fallback: Span) -> Span {
        self.guards().first().map_or(fallback, |g| g.span)
    }
}

pub struct Prover<'a> {
    program: &'a Program,
    resolved: &'a Resolved,
    check: &'a CheckOutput,
    world: TypeWorld,
    /// Built once. Its construction walks every definition's body and runs
    /// Tarjan over the call graph, so building one per obligation makes a run
    /// cost the program size times the obligation count — which is what it did
    /// until this was hoisted.
    ctx: prove::Context<'a>,
    defs: HashMap<Symbol, (usize, &'a FnDef)>,
    laws: HashMap<Symbol, (usize, &'a LawDef)>,
    /// What a `law/host` is discharged against. `None` in a hermetic run, which
    /// makes every one of them `Gap::ReachesHost` rather than green.
    ///
    /// A **factory** for the runtime rather than a handle, for the reason
    /// `ply test` uses one: obligations are discharged on a rayon pool, a
    /// reactor handle belongs to the one thread its machine runs on, and each of
    /// those machines needs an identity of its own so that two host laws in
    /// flight are two scope stacks.
    hosting: Option<Hosting<'a>>,
    /// This program's region kinds, for the reason `ctx` is built once: the
    /// analysis behind them is whole-program, and `machine()` is called per
    /// obligation.
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

    /// Bind the host, so that a `law/host` is attempted rather than reported as
    /// a gap.
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

    /// The machine a `law/host`'s body runs on: the run's binding, and a reactor
    /// minted for this thread.
    fn host_machine(&self, hosting: &Hosting<'a>) -> Machine<'a> {
        let mut machine = self.machine();
        machine.set_host_binding(Arc::clone(&hosting.binding));
        if let Some(factory) = hosting.runtime {
            machine.set_host_runtime(factory());
        }
        machine
    }

    /// Step 1. Answers `proved`, `vacuous`, or "carry on".
    ///
    /// A [`Proof`] whose guard the prover could not show inhabited is **not** a
    /// certificate yet: `guard ⟹ body` over an empty domain is valid and says
    /// nothing. It is carried forward so that a run which keeps a case can
    /// witness the domain the prover could not, which is the one place a
    /// sampled run upgrades a static argument rather than replacing it.
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

    /// What the static tier alone answered, and where the obligation left the
    /// fragment on the way.
    ///
    /// A measurement of the prover's reach, and nothing else: the answer here
    /// is the one [`Prover::discharge_with`] already acts on, and a [`Blocker`]
    /// never reaches a discharge. `None` for a concurrency law, which is
    /// decided by execution rather than by this fragment, and for an obligation
    /// whose claim is not in the program.
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

/// What the static tier answered for one obligation, and the fragment
/// boundaries it crossed getting there.
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
    ///
    /// Inherent as well as a [`ply_test::obligation::Discharger`] method so that
    /// an audit can drive it without the trait in scope — the trait exists to
    /// keep the cache rule and the fragment in different crates, not to hide
    /// this.
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

        // Checking an `ensures` at either running tier means *calling* the
        // definition, and a definition that performs needs a handler nothing
        // supplies. Inventing one would be inventing a behaviour and then
        // testing against it, so this is a reported gap rather than a claim.
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
            // A sampled run that kept nothing has **not** established that the
            // guard admits nothing: it has established that the generator drew
            // nothing the guard wanted. Those are different claims, and only the
            // first is `E0420`. So the domain is looked for directly before that
            // error is reported, and a value found here settles the same
            // question a kept case would have.
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
    ///
    /// **The static tier and the finite enumeration are both skipped, and that is
    /// structural rather than a convention.** Either would be a claim about every
    /// value of the domain, and a claim about every value is exactly what a law
    /// whose body reaches the world cannot make: the world is not a function of
    /// the arguments. `property` is the ceiling and the tier says so.
    ///
    /// Under a hermetic run there is nothing to run it against, and the answer is
    /// a gap naming the flag rather than a green tick.
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

    /// A tuple of binder values the guard admits, found by evaluating the guard
    /// at points the guard's own literals name.
    ///
    /// The generator draws from the whole of a type, so a guard admitting nine
    /// integers a million away from zero is one that two hundred draws will
    /// never satisfy. That is a fact about the search, not about the program,
    /// and two decisions must not confuse the two: [`Proof::certify`] needs
    /// *something* to vouch for the domain of an argument it has already made,
    /// and [`Discharge::Vacuous`] is the claim that there is no domain at all.
    ///
    /// It is a witness and never a refutation: finding nothing here says only
    /// that this search found nothing. A value it does find was evaluated, so
    /// what it establishes is established by a run and not by an assumption —
    /// the same standard a kept case meets.
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
                // A shape whose candidates the guard's literals do not name —
                // a list, a record, an ADT, a function. One drawn value keeps
                // the binder out of the way of the ones that *are* named, which
                // is the whole point: what a sampled run misses is a narrow
                // numeric window, not a list.
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
            // A point the guard raises at is not a point it admits, and a raise
            // here is the property tier's business rather than this one's.
            if cases.guard(&values).unwrap_or(false) {
                return Some(values);
            }
        }
        None
    }

    /// The values one binder is tried at, smallest and most literal first.
    ///
    /// `None` for a type whose candidates this search does not enumerate — a
    /// record, an ADT, a function. Those are the shapes a *sampled* run reaches
    /// perfectly well; what it cannot reach is a narrow numeric window, which is
    /// exactly what the guard's own literals name.
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
                // A bound and the two integers beside it: `x > 1000000` is
                // satisfied by `1000001` and by nothing the literal itself
                // names.
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
    ///
    /// This is the seam ADR 0007 §11's **differential tier audit** needs, and it
    /// exists for the same reason `--engine both` does: a claim that two
    /// mechanisms agree is only worth what the comparison costs. Every
    /// obligation a run reports `proved` is re-run here at a widened plan, and a
    /// refutation is a **defect in Ply** rather than in the program — a wrong
    /// `proved` is the one failure this milestone cannot ship, and the only way
    /// to catch it is to go and look.
    ///
    /// A concurrency law is not resampled: its proof comes from an exhaustive
    /// interleaving search rather than from a static argument, and sampling
    /// schedules is not an independent check on having covered them all.
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

    /// The owner's footprint, when it is one no obligation can supply handlers
    /// for. A law's own row is `{}` or `{sim.read}` and neither needs one.
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

    /// Step 2. Every point of a finite domain, which is a **proof** when they
    /// all hold: the domain was covered, not sampled.
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
            // A domain that cannot produce one of its own points is not one
            // this tier may claim to have covered, and reporting anything but a
            // gap for it would be claiming coverage nothing established.
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
                    // No shrinking: the point is already a member of a domain
                    // enumerated in a fixed order, so it is the same value on
                    // every run and there is nothing smaller to find.
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
            // Enumerating a finite domain and keeping nothing *decides* the
            // guard unsatisfiable — §5.1(f) applied to the guard rather than to
            // the body.
            return Discharge::Vacuous(Vacuity {
                guard: claim.guard_span(obligation.span),
                kind: VacuityKind::ProvedUnsatisfiable,
            });
        }

        // A kept point witnesses the domain, so a static argument the prover
        // could not vouch for is now vouched for — by a value that actually ran.
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

    /// A law whose body reaches a `simulate` region: discharged by execution,
    /// and the only place in this milestone a `proved` does not come from a
    /// static argument.
    ///
    /// The value domain is decided **here** and handed to
    /// [`concurrency::discharge`] as a [`ValueDomain`], because that is ADR 0007
    /// §6's condition 5 and it is the one an implementer drops: an exhaustive
    /// interleaving search over sampled values proves something about those
    /// values and nothing about the law.
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

    /// The points a concurrency law is searched at, and what claim covering
    /// them supports.
    ///
    /// A guard is exactly pure, so it is applied here — before any interleaving
    /// runs — rather than inside the search: which values the law speaks about
    /// is not a question a schedule can answer.
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

/// The most guard evaluations one witness search spends. A bound on the work,
/// not on the correctness: the search only ever answers "here is a value" or
/// "this search found none", and the second is what it already said.
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
                    // `-1000000` is a negation of a literal in the AST and a
                    // bound in the guard, so the value the search wants is the
                    // negated one.
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
                ExprKind::App { func, args } => {
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

/// A static argument the prover made but could not vouch for the domain of,
/// upgraded by a run that kept a case.
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
///
/// For an `ensures` this calls the definition to get `result`, which is why
/// `result` is not something the generator draws. For a law it evaluates the
/// body directly.
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

/// One law body, run at a point of its value domain under a seed the
/// interleaving search chooses.
///
/// A fresh machine per interleaving, exactly as `ply test` re-runs a whole test
/// per interleaving: restoring the arena as of region entry is a snapshot
/// capability the language does not have.
struct Search<'a, 'p> {
    prover: &'p Prover<'a>,
    module: usize,
    body: &'a Expr,
    binders: Vec<LawBinder>,
    /// The points the guard kept, in order. A point index names one of these.
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

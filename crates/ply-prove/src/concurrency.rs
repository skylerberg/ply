//! Concurrency laws: the bridge from an obligation to M7's interleaving search.
//!
//! A concurrency law claims something holds under **every** interleaving, and
//! M7 already answers whether its footprint-guided search covered every one.
//! This module is the reading of that answer, and it is the only place in M8
//! where a [`Tier::Proved`] comes from running a program rather than from
//! reasoning about one — which is what makes the exhaustiveness condition
//! load-bearing rather than decorative.
//!
//! Two coverage claims, and a proof needs both:
//!
//! - **schedules.** [`Exploration::exhaustive`] means the frontier emptied, so
//!   every interleaving at scheduler-visible granularity ran (ADR 0006 §3.3 is
//!   what "scheduler-visible" costs, and it is a disclosed limit rather than a
//!   hole).
//! - **values.** A law over `n: Int` ranges over 2⁶⁴ of them, and an exhaustive
//!   search over sampled values proves something about those values and nothing
//!   about the law. [`ValueDomain::Enumerated`] is the only domain a proof may
//!   be reported over.
//!
//! [`crate::interleaving_proves`] states the five conditions. This module adds
//! the sixth that ADR 0007 §6 leaves implicit because ADR 0006 states it
//! elsewhere: **a search that reached no `simulate` region is exhaustive over
//! nothing.** `explore` is handed a run with no steps, empties its frontier
//! immediately, and reports `exhaustive: true` over a program it never
//! scheduled. `ply-test` plugs that hole in its driver rather than in
//! `Exploration`, so a second consumer of the flag has to plug it again —
//! [`body_run`] is where this one does, and `observed` is private for exactly
//! that reason: outside this crate the only way to obtain a `BodyRun` that
//! claims a region ran is to hand over a [`Machine`] that recorded one.
//!
//! `docs/adr/0007-specs.md` §6 is the specification; `docs/adr/0006-deterministic-simulation.md`
//! §3.3, §3.6 and §6.4 are what `exhaustive` means.

use crate::{
    Binding, CaseReport, Certificate, Counterexample, Discharge, Evidence, Gap, Obligation, Rule,
    Vacuity, VacuityKind,
};
use ply_core::Type;
use ply_eval::{Exploration, Interleaving, Machine, Plan, Seed, Value, Verdict, explore};
use ply_span::{Diagnostic, Span, Symbol, codes};

/// How much of a law's value domain the points it was run at cover.
///
/// This is ADR 0007 §6's condition 5, as a type rather than as a `bool` a caller
/// can pass the wrong way round. Only [`ValueDomain::Enumerated`] admits a
/// proof, and it means what §5.1(f) means: every binder's type is finite, the
/// product of their cardinalities is at most [`crate::ENUMERATION_BOUND`], and
/// every point of that product was visited.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ValueDomain {
    /// The law has no binders, or every point of a finite domain was visited.
    Enumerated {
        /// The domain, rendered, for [`Rule::ExhaustiveEnumeration`]. `unit` for
        /// a ground law, whose domain is the single empty tuple.
        domain: Symbol,
        /// Points of the whole domain, guard-rejected ones included.
        points: u64,
        /// Points the guard kept, and therefore points the body ran at.
        kept: u64,
    },
    /// The points were drawn. Whatever the interleaving search reports, the
    /// value claim is a sample, so the obligation is `property` at best.
    Sampled {
        generated: u32,
        kept: u32,
        rejected: u32,
        /// Type variables monomorphised to generate, e.g. `a := Int`.
        instantiations: Vec<(Symbol, Type)>,
    },
}

impl ValueDomain {
    /// The ground domain: one point, the empty tuple, and no way to miss any of
    /// it.
    pub fn ground() -> ValueDomain {
        ValueDomain::Enumerated {
            domain: "unit".into(),
            points: 1,
            kept: 1,
        }
    }

    /// ADR 0007 §6 condition 5. False for every sampled domain, whatever its
    /// case count: two hundred draws from `Int` is not `Int`.
    pub fn covers_every_value(&self) -> bool {
        matches!(self, ValueDomain::Enumerated { .. })
    }

    /// Points the guard kept, which is how many times the body is run.
    pub fn kept(&self) -> u64 {
        match *self {
            ValueDomain::Enumerated { kept, .. } => kept,
            ValueDomain::Sampled { kept, .. } => u64::from(kept),
        }
    }

    fn rejected(&self) -> u64 {
        match *self {
            ValueDomain::Enumerated { points, kept, .. } => points.saturating_sub(kept),
            ValueDomain::Sampled { rejected, .. } => u64::from(rejected),
        }
    }

    fn instantiations(&self) -> Vec<(Symbol, Type)> {
        match self {
            ValueDomain::Enumerated { .. } => Vec::new(),
            ValueDomain::Sampled { instantiations, .. } => instantiations.clone(),
        }
    }

    fn vacuity(&self) -> VacuityKind {
        match *self {
            // Enumerating a finite domain and keeping nothing *decides* the
            // guard unsatisfiable, which is §5.1(f) applied to the guard rather
            // than to the body.
            ValueDomain::Enumerated { .. } => VacuityKind::ProvedUnsatisfiable,
            ValueDomain::Sampled { generated, .. } => VacuityKind::NoCaseKept { generated },
        }
    }
}

/// One evaluation of a law body, at one point of the value domain, under one
/// seed.
///
/// `observed` is private and there is one public constructor, [`body_run`],
/// which takes the [`Machine`] that ran the body. That is the whole defence
/// against this module's worst available defect: an exhaustive search over a
/// body that entered no `simulate` region reports `exhaustive: true` having
/// scheduled nothing, and a caller that could write `observed: true` by hand
/// could hand a proof to a law nobody searched.
#[derive(Clone, Debug)]
pub struct BodyRun {
    interleaving: Interleaving,
    observed: bool,
    raised: bool,
}

impl BodyRun {
    /// Whether the machine recorded a `simulate` region for this run.
    pub fn observed(&self) -> bool {
        self.observed
    }

    /// Whether the body raised instead of coming to a Boolean. A law that raises
    /// is not false, so this is a [`Gap`] rather than a refutation.
    pub fn raised(&self) -> bool {
        self.raised
    }

    pub fn interleaving(&self) -> &Interleaving {
        &self.interleaving
    }
}

/// The one way to build a [`BodyRun`]: from the machine that just ran the body.
///
/// `value` is what the body came to. A law body is a proposition, so `false` is
/// the failure the search is hunting for and a raise is not a failure of the law
/// at all — the two are separated here, on the value, rather than recovered
/// later from a diagnostic code.
pub fn body_run(machine: &Machine<'_>, value: Result<Value, Diagnostic>, span: Span) -> BodyRun {
    let (outcome, raised) = match value {
        Ok(Value::Bool(true)) => (Ok(()), false),
        Ok(Value::Bool(false)) => (Err(body_was_false(span)), false),
        Ok(other) => (Err(body_was_not_boolean(&other, span)), true),
        Err(diagnostic) => (Err(diagnostic), true),
    };
    let record = machine.simulated();
    BodyRun {
        interleaving: match record {
            Some(record) => record.interleaving(&outcome),
            // The verdict is still the run's own: a body that reached no region
            // must report nothing about interleavings and must not turn a false
            // law true on the way past.
            None => match outcome {
                Ok(()) => Interleaving::passed(Vec::new()),
                Err(diagnostic) => Interleaving::failed(Vec::new(), diagnostic),
            },
        },
        observed: record.is_some(),
        raised,
    }
}

fn body_was_false(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::OBLIGATION_REFUTED,
        "this law does not hold under every interleaving",
    )
    .primary(span, "evaluated to `false` in this interleaving")
}

fn body_was_not_boolean(value: &Value, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a law body came to `{value}` rather than to a Boolean"),
    )
    .primary(span, "a law is a proposition, so its body is `Bool`")
    .note("the type checker rejects a non-`Bool` law body with E0201, so reaching this is a defect in Ply")
}

/// Running a law body at a point of its value domain, under a seed the search
/// chooses.
///
/// The generator, the finite-domain enumerator and the evaluator all live
/// outside this module; a point is an index into whatever the caller drew, so
/// nothing here knows how a value was made. What this module owns is the
/// decision the runs add up to.
pub trait LawSearch {
    /// Evaluate the law body with the binders bound to point `point`, under
    /// `seed`.
    ///
    /// Must be a pure function of the definition set, `point` and `seed`:
    /// [`explore`] re-runs prefixes and reports [`codes::SIMULATION_DIVERGENCE`]
    /// when it catches a driver that is not.
    fn run(&mut self, point: u64, seed: &Seed) -> BodyRun;

    /// The bindings at `point`, as a counterexample renders them. Empty for a
    /// ground law.
    fn bindings(&self, point: u64) -> Vec<Binding>;
}

/// What discharging a concurrency law did, beside the discharge itself.
///
/// Reported rather than derived by a consumer: `interleavings` is the number the
/// certificate names and the number a summary line prints, and `observed` is the
/// answer to "was there a search here at all", which a zero cannot distinguish.
#[derive(Clone, Debug)]
pub struct Searched {
    pub discharge: Discharge,
    /// Interleavings that actually entered a `simulate` region, summed over the
    /// points the guard kept.
    pub interleavings: u32,
    /// Body evaluations, whether or not they reached a region.
    pub evaluations: u32,
    /// Every point's frontier emptied within its budget.
    pub exhaustive: bool,
    /// Some point's search spent its budget, so it proved nothing about the
    /// interleavings it did not reach.
    pub exhausted: bool,
    /// Every run entered a `simulate` region. A `false` here is why an
    /// `exhaustive` search is still not a proof.
    pub observed: bool,
    /// Points the body ran at.
    pub points: u64,
}

impl Searched {
    /// `12 interleavings · exhaustive`, or why not. `None` when nothing ran.
    pub fn line(&self) -> Option<String> {
        if self.evaluations == 0 {
            return None;
        }
        if !self.observed {
            return Some(format!(
                "{} evaluation{} · no `simulate` region reached",
                self.evaluations,
                plural(self.evaluations)
            ));
        }
        let mut line = format!(
            "{} interleaving{}",
            self.interleavings,
            plural(self.interleavings)
        );
        if self.points > 1 {
            line.push_str(&format!(" over {} points", self.points));
        }
        if self.exhaustive {
            line.push_str(" · exhaustive");
        }
        if self.exhausted {
            line.push_str(" · budget spent");
        }
        Some(line)
    }
}

fn plural(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Discharge a concurrency law by searching every point of its value domain.
///
/// The tier is not chosen here; it is [`Evidence::tier`]'s, and the only way out
/// of this function with a [`crate::Tier::Proved`] is a [`Certificate`] every
/// one of ADR 0007 §6's conditions signed off on — plus the sixth this module's
/// header names.
pub fn discharge(
    obligation: &Obligation,
    plan: &Plan,
    domain: &ValueDomain,
    search: &mut dyn LawSearch,
) -> Searched {
    let plan = plan.clone().normalized();
    let points = domain.kept();
    let mut totals = Totals::new(points);

    for point in 0..points {
        let (explored, entered, observed, failing) = {
            let mut driver = Driver {
                search: &mut *search,
                point,
                observed: true,
                entered: 0,
                failures: Vec::new(),
            };
            let explored = explore(&plan, &mut driver);
            let failing = explored
                .exploration
                .failure
                .as_ref()
                .map(|seed| driver.classify(seed));
            (explored, driver.entered, driver.observed, failing)
        };

        totals.absorb(&explored.exploration, entered, observed);

        if let Some(seed) = explored.exploration.failure.clone() {
            let diagnostic = explored.diagnostic.clone().unwrap_or_else(|| {
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    "an interleaving search reported a failing seed with no diagnostic",
                )
                .primary(obligation.span, "this law's search")
            });
            // A raise is not a refutation: a law that divides by zero says
            // nothing about whether it is true. `check_recording` and
            // `check_replay` failures land here too — the driver never saw them,
            // so `Raised` is what they are, and E0415 is carried out whole
            // rather than relabelled as a counterexample.
            return totals.finish(if failing.unwrap_or(Failing::Raised) == Failing::Raised {
                Discharge::Unattempted(Gap::Raised {
                    bindings: search.bindings(point),
                    diagnostic,
                })
            } else {
                Discharge::Refuted(Counterexample {
                    bindings: search.bindings(point),
                    original: search.bindings(point),
                    // Neither half is shrunk. The schedule is ADR 0006 §6.5:
                    // truncating a choice path changes what the suffix means.
                    // The values are un-shrunk because a candidate has to be
                    // re-refuted by a fresh interleaving search, and a point
                    // index cannot name a value the caller did not draw — the
                    // seam for that is a `LawSearch` that admits arbitrary
                    // values, and it is not built.
                    shrinks: 0,
                    root: seed.root,
                    case: u32::try_from(point).unwrap_or(u32::MAX),
                    race: explored.exploration.race.clone(),
                    sim_seed: Some(seed),
                })
            });
        }
    }

    let discharge = totals.held(obligation, domain, &plan);
    totals.finish(discharge)
}

/// Aggregated over every point of the value domain, because a law is a claim
/// about all of them: one point whose search spent its budget is a law whose
/// search spent its budget.
struct Totals {
    points: u64,
    interleavings: u32,
    evaluations: u32,
    exhaustive: bool,
    exhausted: bool,
    observed: bool,
    /// Carried rather than inferred from the early return a failure takes:
    /// [`Totals::proves`] must be false on a failing search whatever the shape
    /// of the loop above it later becomes.
    failure: Option<Seed>,
}

impl Totals {
    fn new(points: u64) -> Totals {
        Totals {
            points,
            interleavings: 0,
            evaluations: 0,
            // Vacuously true over no points, and `proves` requires an
            // interleaving to have run before it reads this.
            exhaustive: true,
            exhausted: false,
            observed: true,
            failure: None,
        }
    }

    fn absorb(&mut self, exploration: &Exploration, entered: u32, observed: bool) {
        self.evaluations = self.evaluations.saturating_add(exploration.explored);
        self.interleavings = self.interleavings.saturating_add(entered);
        self.exhaustive &= exploration.exhaustive;
        self.exhausted |= exploration.exhausted;
        self.observed &= observed;
        self.failure = self.failure.take().or_else(|| exploration.failure.clone());
    }

    /// The conditions, all of them, in one place.
    fn proves(&self, plan: &Plan, domain: &ValueDomain) -> bool {
        let exploration = Exploration {
            exhaustive: self.exhaustive,
            exhausted: self.exhausted,
            failure: self.failure.clone(),
            ..Exploration::default()
        };
        // The five of ADR 0007 §6, and then the sixth: a search that entered no
        // region emptied a frontier it never filled.
        crate::interleaving_proves(plan, &exploration, domain.covers_every_value())
            && self.observed
            && self.interleavings > 0
    }

    fn held(&self, obligation: &Obligation, domain: &ValueDomain, plan: &Plan) -> Discharge {
        if domain.kept() == 0 {
            return Discharge::Vacuous(Vacuity {
                guard: obligation.span,
                kind: domain.vacuity(),
            });
        }
        if self.proves(plan, domain) {
            return Discharge::Held(Evidence::Proof(self.certificate(obligation, domain)));
        }
        Discharge::Held(Evidence::Cases(CaseReport {
            // Interleavings rather than value points, because what a concurrency
            // law samples is schedules: a ground law whose search spent its
            // budget ran 256 cases, and calling that one case would report
            // `example` for the strongest sampled evidence in the language.
            // `rejected` is the odd one out and counts value points, which is
            // the only thing a guard can reject.
            generated: self.evaluations,
            kept: self.evaluations,
            rejected: u32::try_from(domain.rejected()).unwrap_or(u32::MAX),
            roots: plan.roots.clone(),
            instantiations: domain.instantiations(),
        }))
    }

    fn certificate(&self, obligation: &Obligation, domain: &ValueDomain) -> Certificate {
        let mut rules = Vec::new();
        // Both coverage claims are named, so an audit can check condition 5
        // against the certificate rather than re-deriving it from the law.
        //
        // The condition is "the law has binders and every point of their domain
        // ran", **not** "more than one point ran". A law over `Unit` has one
        // point and no way to miss any of it, and a certificate that stayed
        // silent about covering it made ADR 0007 §11's audit reject a proof that
        // was correct — a false red where the whole file is about false greens,
        // and still a defect.
        if let ValueDomain::Enumerated {
            domain: name,
            points,
            ..
        } = domain
            && !obligation.generated().is_empty()
        {
            rules.push(Rule::ExhaustiveEnumeration {
                domain: name.clone(),
                points: *points,
            });
        }
        rules.push(Rule::ExhaustiveInterleaving {
            interleavings: self.interleavings,
        });
        Certificate {
            rules,
            steps: self.interleavings,
            // A point was kept, so the guard admits a value. An empty domain
            // took the `Vacuous` path above and never reaches here.
            guard_satisfiable: true,
            // A proof about one program, not about an uninterpreted sort.
            sorts: Vec::new(),
        }
    }

    fn finish(&self, discharge: Discharge) -> Searched {
        Searched {
            discharge,
            interleavings: self.interleavings,
            evaluations: self.evaluations,
            exhaustive: self.exhaustive,
            exhausted: self.exhausted,
            observed: self.observed,
            points: self.points,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Failing {
    /// The body evaluated to `false`.
    Refuted,
    /// The body raised, or the search caught its own driver diverging.
    Raised,
}

struct Driver<'a> {
    search: &'a mut dyn LawSearch,
    point: u64,
    observed: bool,
    /// Runs that reached a `simulate` region.
    entered: u32,
    /// Which of the two things happened at each seed that ended badly. Recorded
    /// rather than read back off the diagnostic's code, because a user program
    /// can raise a diagnostic and only this driver knows whether the body came
    /// to `false`.
    failures: Vec<(Seed, Failing)>,
}

impl Driver<'_> {
    fn classify(&self, seed: &Seed) -> Failing {
        self.failures
            .iter()
            .find(|(s, _)| s == seed)
            .map_or(Failing::Raised, |(_, failing)| *failing)
    }
}

impl ply_eval::Simulation for Driver<'_> {
    fn run(&mut self, seed: &Seed) -> Interleaving {
        let run = self.search.run(self.point, seed);
        if run.observed {
            self.entered = self.entered.saturating_add(1);
        } else {
            self.observed = false;
        }
        if matches!(run.interleaving.verdict, Verdict::Failed(_)) {
            self.failures.push((
                seed.clone(),
                if run.raised {
                    Failing::Raised
                } else {
                    Failing::Refuted
                },
            ));
        }
        run.interleaving
    }
}

/// The command that replays exactly this failure.
///
/// M7's artifact with one word changed: an obligation is not a test, so the
/// command is `ply prove`, and the seed means what it means in `ply test`.
pub fn replay_command(seed: &Seed, law: &str) -> String {
    format!("ply prove --seed {seed} --filter \"{law}\"")
}

/// The failure artifact for a refuted concurrency law: the search's own
/// diagnostic, with the seed, the race and the replay command attached.
///
/// The same three lines `ply test` prints for a race, because a concurrency law
/// failure *is* a race — the assertion it flipped is the law rather than a
/// `assert_eq` inside a test, and nothing else about reproducing it differs.
pub fn refutation(law: &str, counterexample: &Counterexample, found: Diagnostic) -> Diagnostic {
    let Some(seed) = &counterexample.sim_seed else {
        return found;
    };
    let mut diagnostic = found.note(format!("seed: {seed}"));
    if let Some(race) = &counterexample.race {
        diagnostic = diagnostic
            .note(format!("race: {}", race_site(&race.left)))
            .note(format!("      {}", race_site(&race.right)));
    }
    if !counterexample.bindings.is_empty() {
        let bindings: Vec<String> = counterexample
            .bindings
            .iter()
            .map(|b| format!("{} = {}", b.name, b.rendered))
            .collect();
        diagnostic = diagnostic.note(format!("at {}", bindings.join(", ")));
        diagnostic = diagnostic.note(
            "the seed replays the interleaving; the bindings are redrawn from the same prove plan, \
             so replay under the flags this run used",
        );
    }
    diagnostic.note(format!("replay: {}", replay_command(seed, law)))
}

fn race_site(site: &ply_eval::RaceSite) -> String {
    let definition = site
        .definition
        .as_ref()
        .map(|d| d.to_string())
        .unwrap_or_else(|| "-".to_string());
    format!("{}  {definition}   {}", site.task, site.access)
}

/// What ADR 0007 §11's concurrency-law condition test asserts, as a function so
/// that the assertion is one call rather than a re-derivation that can drift
/// from the thing it audits.
///
/// A certificate naming [`Rule::ExhaustiveInterleaving`] is an execution-derived
/// proof, and the two things that make one honest are checkable from the
/// certificate and the obligation alone: the guard admitted something, and a law
/// with binders also carries the enumeration that covered them.
pub fn audit_interleaving_proof(
    obligation: &Obligation,
    certificate: &Certificate,
) -> Result<(), String> {
    let interleavings = certificate.rules.iter().find_map(|rule| match rule {
        Rule::ExhaustiveInterleaving { interleavings } => Some(*interleavings),
        _ => None,
    });
    let Some(interleavings) = interleavings else {
        return Ok(());
    };
    if !certificate.guard_satisfiable {
        return Err(format!(
            "`{}` is proved over a guard nothing was shown to satisfy",
            obligation.owner
        ));
    }
    if interleavings == 0 {
        return Err(format!(
            "`{}` is proved by an exhaustive search that ran no interleaving",
            obligation.owner
        ));
    }
    if !obligation.binders.is_empty()
        && !certificate
            .rules
            .iter()
            .any(|rule| matches!(rule, Rule::ExhaustiveEnumeration { .. }))
    {
        return Err(format!(
            "`{}` has {} binder(s) and is proved by an exhaustive interleaving search that \
             covered no value domain",
            obligation.owner,
            obligation.binders.len()
        ));
    }
    if !certificate.sorts.is_empty() {
        return Err(format!(
            "`{}` is a proof about one program, so it has no uninterpreted sorts",
            obligation.owner
        ));
    }
    Ok(())
}

#[cfg(test)]
impl BodyRun {
    /// The constructor `body_run` is deliberately the only public one, so a
    /// model search inside this crate needs its own. Nothing outside it can
    /// claim a region ran without a machine that recorded one.
    fn model(interleaving: Interleaving, observed: bool, raised: bool) -> BodyRun {
        BodyRun {
            interleaving,
            observed,
            raised,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frame, ObligationKind, Tier};
    use ply_core::{Footprint, LawBinder};
    use ply_eval::explore::Step;
    use ply_eval::{Access, CellId, Domain, SimId, SimMode, StepFootprint, Stream, TaskId};
    use ply_hash::DefHash;
    use ply_syntax::ast::Mode;

    /// `tasks` tasks, each reading a shared counter and writing it back — the
    /// lost update, which is the shape every concurrency law worth writing is
    /// about. Every step touches one cell, so no pair commutes and the search
    /// enumerates every interleaving that respects per-task order.
    ///
    /// A model rather than the machine for the same reason `ply_eval::explore`
    /// is tested against one: what is under test here is the decision the runs
    /// add up to, and a model scheduler is the only way to fix the runs.
    struct Model {
        tasks: usize,
        /// What the law claims of the counter when every task has finished.
        claims: fn(i64) -> bool,
        /// Points whose runs reach no `simulate` region.
        unobserved: bool,
        /// Points whose runs raise instead of coming to a Boolean.
        raises: bool,
        /// Every choice sequence this model was asked for, in order.
        traces: Vec<Vec<u16>>,
    }

    impl Model {
        fn new(tasks: usize, claims: fn(i64) -> bool) -> Model {
            Model {
                tasks,
                claims,
                unobserved: false,
                raises: false,
                traces: Vec::new(),
            }
        }

        fn interleave(&mut self, seed: &Seed) -> Interleaving {
            let mut sched = Stream::new(seed.root, Domain::Sched);
            let mut pc = vec![0usize; self.tasks];
            let mut register = vec![0i64; self.tasks];
            let mut counter = 0i64;
            let mut steps = Vec::new();
            let mut taken = Vec::new();

            loop {
                let enabled: Vec<TaskId> = (0..self.tasks)
                    .filter(|&t| pc[t] < 2)
                    .map(|t| TaskId(t as u32))
                    .collect();
                if enabled.is_empty() {
                    break;
                }
                let choice = match seed.choice(steps.len()) {
                    Some(fixed) => usize::from(fixed).min(enabled.len() - 1),
                    None => sched.below(enabled.len() as u64).unwrap_or(0) as usize,
                };
                let task = enabled[choice];
                let t = task.0 as usize;
                let mode = if pc[t] == 0 {
                    register[t] = counter;
                    Mode::Read
                } else {
                    counter = register[t] + 1;
                    Mode::Write
                };
                pc[t] += 1;
                taken.push(choice as u16);
                steps.push(Step {
                    region: SimId(0),
                    task,
                    enabled,
                    choice: choice as u16,
                    accesses: StepFootprint::from_accesses([Access::Cell {
                        id: CellId(0),
                        mode,
                    }]),
                    definition: Some(Symbol::new("transfer")),
                    span: Span::DUMMY,
                    // No synchronization at all, so no pair is ordered and every
                    // dependent pair is a candidate.
                    stamp: Vec::new(),
                });
            }

            self.traces.push(taken);
            if (self.claims)(counter) {
                Interleaving::passed(steps)
            } else {
                Interleaving::failed(steps, body_was_false(Span::DUMMY))
            }
        }
    }

    impl LawSearch for Model {
        fn run(&mut self, _point: u64, seed: &Seed) -> BodyRun {
            if self.raises {
                return BodyRun::model(
                    Interleaving::failed(
                        Vec::new(),
                        Diagnostic::error(codes::RUNTIME_ERROR, "divided by zero"),
                    ),
                    false,
                    true,
                );
            }
            if self.unobserved {
                // What a body that never reaches a `simulate` region hands the
                // search: a run with no steps and the body's own verdict.
                return BodyRun::model(Interleaving::passed(Vec::new()), false, false);
            }
            BodyRun::model(self.interleave(seed), true, false)
        }

        fn bindings(&self, point: u64) -> Vec<Binding> {
            vec![Binding {
                name: Symbol::new("n"),
                ty: Type::int(),
                rendered: point.to_string(),
            }]
        }
    }

    fn law(binders: usize) -> Obligation {
        Obligation {
            key: DefHash([3; 32]),
            owner: Symbol::new("bank.transfers conserve value"),
            kind: ObligationKind::Law,
            span: Span::DUMMY,
            frame: Frame::Pure,
            binders: (0..binders)
                .map(|i| LawBinder {
                    name: Symbol::new(format!("n{i}")),
                    ty: Type::int(),
                    span: Span::DUMMY,
                })
                .collect(),
            guarded: false,
            host: false,
            footprint: Footprint::from_atoms([ply_core::EffectAtom::new(
                "sim",
                ply_core::Resource::Singleton,
                Mode::Read,
            )]),
        }
    }

    fn dpor(budget: u32) -> Plan {
        Plan {
            budget,
            ..Plan::default()
        }
    }

    fn certificate(searched: &Searched) -> &Certificate {
        match &searched.discharge {
            Discharge::Held(Evidence::Proof(c)) => c,
            other => panic!("expected a proof, got {other:?}"),
        }
    }

    // ------------------------------------------------------- the four cases

    /// A law that holds under **every** interleaving, discharged as a proof.
    /// The one place in M8 a `proved` comes from execution.
    #[test]
    fn a_law_that_holds_under_every_interleaving_is_proved() {
        let obligation = law(0);
        let mut model = Model::new(2, |counter| counter <= 2);
        let searched = discharge(&obligation, &dpor(64), &ValueDomain::ground(), &mut model);

        assert_eq!(searched.discharge.tier(), Some(Tier::Proved));
        assert!(searched.exhaustive);
        assert!(!searched.exhausted);
        assert!(searched.observed);
        assert!(searched.interleavings > 1, "the search found real choices");

        let certificate = certificate(&searched);
        assert_eq!(
            certificate.rules,
            vec![Rule::ExhaustiveInterleaving {
                interleavings: searched.interleavings
            }],
            "a ground law's certificate names the interleaving search and nothing else"
        );
        assert!(certificate.guard_satisfiable);
        assert!(certificate.sorts.is_empty());
        assert_eq!(audit_interleaving_proof(&obligation, certificate), Ok(()));
    }

    /// A law that holds under *some* interleavings: a failure carrying the seed
    /// that reproduces it.
    #[test]
    fn a_law_that_holds_only_sometimes_is_refuted_with_a_seed() {
        let obligation = law(0);
        // The lost update: two tasks that each read the counter and write it
        // back reach 1 in the interleavings where the reads precede both writes.
        let mut model = Model::new(2, |counter| counter == 2);
        let searched = discharge(&obligation, &dpor(64), &ValueDomain::ground(), &mut model);

        let Discharge::Refuted(counterexample) = &searched.discharge else {
            panic!("expected a refutation, got {:?}", searched.discharge);
        };
        assert_eq!(searched.discharge.tier(), None);
        let seed = counterexample
            .sim_seed
            .as_ref()
            .expect("a concurrency failure names the interleaving that produced it");
        assert!(
            !seed.is_root(),
            "the failing interleaving is a path, not just a root: {seed}"
        );
        assert!(
            counterexample.race.is_some(),
            "the search observed the flip, so it names the pair of steps"
        );
        assert!(
            !searched.exhaustive,
            "a search that stopped at a failure covered nothing"
        );
    }

    /// A search that spends its budget proved nothing about the interleavings it
    /// did not reach, so it reports the sampled tier and says how many it ran.
    #[test]
    fn a_search_that_spends_its_budget_is_property_and_says_how_many() {
        let obligation = law(0);
        let mut model = Model::new(3, |counter| counter <= 3);
        let searched = discharge(&obligation, &dpor(30), &ValueDomain::ground(), &mut model);

        assert_eq!(searched.discharge.tier(), Some(Tier::Property));
        assert!(searched.exhausted);
        assert!(!searched.exhaustive);
        let Discharge::Held(Evidence::Cases(report)) = &searched.discharge else {
            panic!("expected sampled evidence, got {:?}", searched.discharge);
        };
        assert_eq!(report.kept, 30, "the count is the interleavings it ran");
        assert_eq!(report.kept, searched.evaluations);
        assert!(searched.line().unwrap().contains("budget spent"));
    }

    /// A spent budget is a claim about the plan that spent it, so it may never
    /// be read back under a wider one.
    #[test]
    fn a_spent_budget_is_not_written_under_the_bare_key() {
        let obligation = law(0);
        let mut model = Model::new(3, |counter| counter <= 3);
        let searched = discharge(&obligation, &dpor(30), &ValueDomain::ground(), &mut model);
        let plan = crate::ProvePlan::default();
        assert_ne!(
            crate::key::result_key(obligation.key, searched.discharge.tier(), &plan),
            obligation.key
        );
    }

    /// The artifact is M7's: `--seed` replays the interleaving exactly, and the
    /// replay refutes the same law with the same seed and the same trace.
    #[test]
    fn a_reported_failure_replays_exactly() {
        let obligation = law(0);
        let mut search = Model::new(2, |counter| counter == 2);
        let found = discharge(&obligation, &dpor(64), &ValueDomain::ground(), &mut search);
        let Discharge::Refuted(counterexample) = &found.discharge else {
            panic!("expected a refutation");
        };
        let seed = counterexample.sim_seed.clone().unwrap();
        let failing_trace = search.traces.last().cloned().unwrap();

        let mut replay = Model::new(2, |counter| counter == 2);
        let again = discharge(
            &obligation,
            &Plan::once(seed.clone()),
            &ValueDomain::ground(),
            &mut replay,
        );
        let Discharge::Refuted(replayed) = &again.discharge else {
            panic!(
                "the replay must reproduce the refutation, got {:?}",
                again.discharge
            );
        };
        assert_eq!(replayed.sim_seed.as_ref(), Some(&seed));
        assert_eq!(
            replay.traces,
            vec![failing_trace],
            "byte-for-byte the same interleaving"
        );
        // `once` observes no flip, so it invents no race — the seed is the
        // exact half and the pair is the half the search happened to see.
        assert!(replayed.race.is_none());
        assert_eq!(again.discharge.tier(), None);
    }

    // ------------------------------------------------ condition 5, and the sixth

    /// ADR 0007 §6's condition 5, and the required test that goes with it: the
    /// same law with a binder is `property` however exhaustive the schedules
    /// were. `exhaustive: true` is a claim about schedules; `n: Int` is 2⁶⁴
    /// values.
    #[test]
    fn an_exhaustive_search_over_sampled_values_is_never_proved() {
        let obligation = law(1);
        let mut model = Model::new(2, |counter| counter <= 2);
        let searched = discharge(
            &obligation,
            &dpor(64),
            &ValueDomain::Sampled {
                generated: 200,
                kept: 200,
                rejected: 0,
                instantiations: Vec::new(),
            },
            &mut model,
        );
        assert!(searched.exhaustive, "every point's frontier emptied");
        assert_eq!(
            searched.discharge.tier(),
            Some(Tier::Property),
            "exhaustive over schedules says nothing about the values that were sampled"
        );
    }

    /// The same law over a domain that *was* covered is proved, and its
    /// certificate names both coverage claims so an audit can check condition 5
    /// without re-deriving it.
    #[test]
    fn an_enumerated_value_domain_proves_and_names_its_enumeration() {
        let obligation = law(1);
        let mut model = Model::new(2, |counter| counter <= 2);
        let searched = discharge(
            &obligation,
            &dpor(64),
            &ValueDomain::Enumerated {
                domain: Symbol::new("Bool"),
                points: 2,
                kept: 2,
            },
            &mut model,
        );
        let certificate = certificate(&searched);
        assert!(certificate.rules.contains(&Rule::ExhaustiveEnumeration {
            domain: Symbol::new("Bool"),
            points: 2,
        }));
        assert!(
            certificate
                .rules
                .iter()
                .any(|r| matches!(r, Rule::ExhaustiveInterleaving { .. }))
        );
        assert_eq!(audit_interleaving_proof(&obligation, certificate), Ok(()));
        assert_eq!(searched.points, 2);
    }

    /// The audit catches a certificate that claims an exhaustive search over a
    /// law whose values nobody covered. This is the shape of the defect ADR 0007
    /// §6 calls the milestone's worst available one.
    #[test]
    fn the_audit_rejects_an_interleaving_proof_that_covered_no_value_domain() {
        let forged = Certificate {
            rules: vec![Rule::ExhaustiveInterleaving { interleavings: 12 }],
            steps: 12,
            guard_satisfiable: true,
            sorts: Vec::new(),
        };
        assert!(audit_interleaving_proof(&law(1), &forged).is_err());
        assert_eq!(audit_interleaving_proof(&law(0), &forged), Ok(()));

        let empty = Certificate {
            rules: vec![Rule::ExhaustiveInterleaving { interleavings: 0 }],
            ..forged.clone()
        };
        assert!(audit_interleaving_proof(&law(0), &empty).is_err());

        let unguarded = Certificate {
            guard_satisfiable: false,
            ..forged
        };
        assert!(audit_interleaving_proof(&law(0), &unguarded).is_err());
    }

    /// **The sixth condition.** A body that reaches no `simulate` region hands
    /// the search a run with no steps; the frontier empties on the first
    /// interleaving and `Exploration::exhaustive` is `true` over a program
    /// nothing scheduled. The five conditions of ADR 0007 §6 all pass on it.
    #[test]
    fn a_search_that_reached_no_region_is_exhaustive_over_nothing() {
        let plan = dpor(64);
        let mut nothing = Model::new(2, |_| true);
        nothing.unobserved = true;

        // What `Exploration` alone says, which is the overclaim.
        let mut probe = Driver {
            search: &mut nothing,
            point: 0,
            observed: true,
            entered: 0,
            failures: Vec::new(),
        };
        let explored = explore(&plan, &mut probe);
        assert!(
            explored.exploration.exhaustive,
            "this is the flag M8 is invited to read as a proof"
        );
        assert!(crate::interleaving_proves(
            &plan,
            &explored.exploration,
            true
        ));

        // What this module reports, having asked whether a region ran.
        let searched = discharge(&law(0), &plan, &ValueDomain::ground(), &mut nothing);
        assert!(!searched.observed);
        assert_eq!(searched.interleavings, 0);
        assert_ne!(
            searched.discharge.tier(),
            Some(Tier::Proved),
            "a search that scheduled nothing proves nothing"
        );
        assert!(searched.line().unwrap().contains("no `simulate` region"));
    }

    // ------------------------------------------------------- the other outcomes

    /// Under `once` and `random` there is no frontier to empty, so there is
    /// nothing exhaustive to claim whatever the run reports.
    #[test]
    fn a_sampled_plan_never_proves() {
        for plan in [Plan::random(4), Plan::once(Seed::root(7))] {
            assert_ne!(plan.mode, SimMode::Dpor);
            let mut model = Model::new(2, |counter| counter <= 2);
            let searched = discharge(&law(0), &plan, &ValueDomain::ground(), &mut model);
            assert_ne!(searched.discharge.tier(), Some(Tier::Proved));
            assert!(searched.discharge.holds());
        }
    }

    /// A law that raises is not a law that is false, so a raise is a gap and the
    /// raising input is reported rather than presented as a counterexample.
    #[test]
    fn a_body_that_raises_is_a_gap_and_not_a_refutation() {
        let mut model = Model::new(2, |_| true);
        model.raises = true;
        let searched = discharge(&law(1), &dpor(64), &ValueDomain::ground(), &mut model);
        let Discharge::Unattempted(Gap::Raised {
            bindings,
            diagnostic,
        }) = &searched.discharge
        else {
            panic!("expected a gap, got {:?}", searched.discharge);
        };
        assert_eq!(diagnostic.code, codes::RUNTIME_ERROR);
        assert_eq!(bindings.len(), 1);
    }

    /// A guard that admits nothing makes the obligation trivially valid and
    /// therefore silent. Reporting it `proved` would turn a typo into a proof of
    /// everything.
    #[test]
    fn a_domain_the_guard_emptied_is_vacuous_and_not_proved() {
        let mut model = Model::new(2, |_| true);
        let enumerated = discharge(
            &law(1),
            &dpor(64),
            &ValueDomain::Enumerated {
                domain: Symbol::new("Bool"),
                points: 2,
                kept: 0,
            },
            &mut model,
        );
        assert!(matches!(
            &enumerated.discharge,
            Discharge::Vacuous(Vacuity {
                kind: VacuityKind::ProvedUnsatisfiable,
                ..
            })
        ));
        assert_eq!(enumerated.discharge.tier(), None);

        let sampled = discharge(
            &law(1),
            &dpor(64),
            &ValueDomain::Sampled {
                generated: 200,
                kept: 0,
                rejected: 200,
                instantiations: Vec::new(),
            },
            &mut model,
        );
        assert!(matches!(
            &sampled.discharge,
            Discharge::Vacuous(Vacuity {
                kind: VacuityKind::NoCaseKept { generated: 200 },
                ..
            })
        ));
    }

    /// One point of a law's domain whose search spent its budget is a law whose
    /// search spent its budget: a claim about every value is only as strong as
    /// its weakest point.
    #[test]
    fn one_unexhausted_point_costs_the_whole_law_its_proof() {
        struct Mixed {
            inner: Model,
            /// The point whose search is given no room.
            starved: u64,
        }

        impl LawSearch for Mixed {
            fn run(&mut self, point: u64, seed: &Seed) -> BodyRun {
                if point == self.starved && seed.path.len() > 1 {
                    // A run the search cannot branch past, standing in for a
                    // point whose space is larger than the budget.
                    return BodyRun::model(Interleaving::passed(Vec::new()), true, false);
                }
                self.inner.run(point, seed)
            }

            fn bindings(&self, point: u64) -> Vec<Binding> {
                self.inner.bindings(point)
            }
        }

        let mut mixed = Mixed {
            inner: Model::new(3, |counter| counter <= 3),
            starved: 1,
        };
        let searched = discharge(
            &law(1),
            &dpor(20),
            &ValueDomain::Enumerated {
                domain: Symbol::new("Bool"),
                points: 2,
                kept: 2,
            },
            &mut mixed,
        );
        assert!(searched.exhausted);
        assert!(!searched.exhaustive);
        assert_ne!(searched.discharge.tier(), Some(Tier::Proved));
    }

    /// Two runs over one law produce one artifact — the same tier, the same
    /// counts, the same seed. A search that varied would make today's report
    /// undiffable against yesterday's.
    #[test]
    fn two_runs_over_one_law_agree() {
        let run = || {
            let mut model = Model::new(3, |counter| counter == 3);
            let searched = discharge(&law(0), &dpor(64), &ValueDomain::ground(), &mut model);
            let seed = match &searched.discharge {
                Discharge::Refuted(c) => c.sim_seed.clone(),
                other => panic!("expected a refutation, got {other:?}"),
            };
            (seed, searched.interleavings, model.traces)
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn the_replay_command_is_the_command() {
        assert_eq!(
            replay_command(&Seed::at(0, vec![1, 0, 3]), "transfers conserve value"),
            "ply prove --seed 0:1.0.3 --filter \"transfers conserve value\""
        );
    }

    /// The failure artifact carries what M7's carries: the seed, the race sites,
    /// and the command that replays it.
    #[test]
    fn a_refutation_reports_the_seed_the_race_and_the_replay() {
        let obligation = law(1);
        let mut model = Model::new(2, |counter| counter == 2);
        let searched = discharge(
            &obligation,
            &dpor(64),
            &ValueDomain::Sampled {
                generated: 4,
                kept: 4,
                rejected: 0,
                instantiations: Vec::new(),
            },
            &mut model,
        );
        let Discharge::Refuted(counterexample) = &searched.discharge else {
            panic!("expected a refutation");
        };
        let rendered = refutation(
            "transfers conserve value",
            counterexample,
            body_was_false(Span::DUMMY),
        );
        let notes = rendered.notes.join("\n");
        assert!(notes.contains("seed: "));
        assert!(notes.contains("race: "));
        assert!(notes.contains("replay: ply prove --seed "));
        assert!(notes.contains("n = 0"));
    }

    /// A concurrency law is discharged by execution, and `is_concurrency_law` is
    /// what routes it here. A law whose row is empty is not one.
    #[test]
    fn only_a_law_carrying_sim_read_is_routed_to_a_search() {
        assert!(law(0).is_concurrency_law());
        let pure = Obligation {
            footprint: Footprint::empty(),
            ..law(0)
        };
        assert!(!pure.is_concurrency_law());
    }

    // ------------------------------------------------------------ end to end

    /// The bridge, against the real machine rather than a model: a ground
    /// concurrency law whose body the evaluator runs, whose region the scheduler
    /// records, and whose search this module reads.
    ///
    /// Ground only. A law with binders needs the body evaluated under an
    /// environment binding them, and `ply-eval` exposes no entry point that
    /// takes one — `Machine::eval_expr_for_test` binds `Env::empty()` and
    /// `Machine::drive` is private. That is the gap between this path and a
    /// wired `ply prove`, and it is named here rather than worked around.
    mod machine {
        use super::*;
        use ply_core::CheckOutput;
        use ply_span::SourceId;
        use ply_syntax::ast::{Expr, Item, Program};
        use ply_syntax::resolve::Resolved;

        struct Compiled {
            program: Program,
            resolved: Resolved,
            check: CheckOutput,
        }

        fn compile(src: &str) -> Compiled {
            let module = ply_syntax::parse(SourceId(0), src).expect("the fixture must parse");
            let program = Program::single(module);
            let resolved = ply_syntax::resolve(&program)
                .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
            let check = ply_core::check_program(&program, &resolved)
                .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
            Compiled {
                program,
                resolved,
                check,
            }
        }

        fn law_body(program: &Program) -> (&Expr, Span) {
            program.modules[0]
                .items
                .iter()
                .find_map(|item| match item {
                    Item::Law(law) => Some((&law.body, law.span)),
                    _ => None,
                })
                .expect("the fixture declares a law")
        }

        struct Ground<'a> {
            compiled: &'a Compiled,
            body: &'a Expr,
            span: Span,
            steps: u32,
        }

        impl LawSearch for Ground<'_> {
            fn run(&mut self, _point: u64, seed: &Seed) -> BodyRun {
                // A fresh machine per interleaving, so each one starts from the
                // same world: replay is whole-body, exactly as `ply-test`
                // replays a whole test.
                let mut machine = Machine::new(
                    &self.compiled.program,
                    &self.compiled.resolved,
                    &self.compiled.check,
                );
                machine.set_seed(seed.clone(), self.steps);
                let value = machine.eval_expr_for_test(self.body);
                body_run(&machine, value, self.span)
            }

            fn bindings(&self, _point: u64) -> Vec<Binding> {
                Vec::new()
            }
        }

        #[test]
        fn a_ground_concurrency_law_is_proved_by_the_machines_own_search() {
            let compiled = compile(
                "law \"spawned tasks all finish\" {\n\
                 \x20 simulate {\n\
                 \x20   let a = task.spawn(|| 1);\n\
                 \x20   let b = task.spawn(|| 2);\n\
                 \x20   task.join(a) + task.join(b) == 3\n\
                 \x20 }\n\
                 }\n",
            );
            let (body, span) = law_body(&compiled.program);
            let plan = Plan::default();
            let mut search = Ground {
                compiled: &compiled,
                body,
                span,
                steps: plan.steps,
            };
            let searched = discharge(&law(0), &plan, &ValueDomain::ground(), &mut search);
            assert!(
                searched.observed,
                "the body entered a `simulate` region, so there was a search to read"
            );
            assert_eq!(searched.discharge.tier(), Some(Tier::Proved));
            assert!(searched.interleavings >= 1);
            assert_eq!(
                audit_interleaving_proof(&law(0), certificate(&searched)),
                Ok(())
            );
        }

        /// The same shape with a false body: the machine's search finds it and
        /// the failure names the seed that replays it.
        #[test]
        fn a_false_ground_concurrency_law_is_refuted_with_a_replayable_seed() {
            let compiled = compile(
                "law \"spawned tasks all finish\" {\n\
                 \x20 simulate {\n\
                 \x20   let a = task.spawn(|| 1);\n\
                 \x20   task.join(a) == 2\n\
                 \x20 }\n\
                 }\n",
            );
            let (body, span) = law_body(&compiled.program);
            let plan = Plan::default();
            let mut search = Ground {
                compiled: &compiled,
                body,
                span,
                steps: plan.steps,
            };
            let searched = discharge(&law(0), &plan, &ValueDomain::ground(), &mut search);
            let Discharge::Refuted(counterexample) = &searched.discharge else {
                panic!("expected a refutation, got {:?}", searched.discharge);
            };
            assert!(counterexample.sim_seed.is_some());
        }

        /// A body that reaches no `simulate` region: the machine records nothing,
        /// `body_run` says so, and the law is held on the evidence it has rather
        /// than proved on evidence it does not.
        #[test]
        fn a_law_body_with_no_region_is_never_proved_by_the_machine() {
            let compiled = compile("law \"ground truth\" {\n \x201 + 1 == 2\n}\n");
            let (body, span) = law_body(&compiled.program);
            let plan = Plan::default();
            let mut search = Ground {
                compiled: &compiled,
                body,
                span,
                steps: plan.steps,
            };
            let searched = discharge(&law(0), &plan, &ValueDomain::ground(), &mut search);
            assert!(!searched.observed);
            assert!(searched.discharge.holds());
            assert_ne!(searched.discharge.tier(), Some(Tier::Proved));
        }
    }
}

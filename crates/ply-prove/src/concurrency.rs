//! Concurrency laws: the bridge from an obligation to M7's interleaving search.

use crate::{
    Binding, CaseReport, Certificate, Counterexample, Discharge, Evidence, Gap, Obligation, Rule,
    Vacuity, VacuityKind,
};
use ply_core::Type;
use ply_eval::{Exploration, Interleaving, Machine, Plan, Seed, Value, Verdict, explore};
use ply_span::{Diagnostic, Span, Symbol, codes};

/// How much of a law's value domain the points it was run at cover.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ValueDomain {
    /// The law has no binders, or every point of a finite domain was visited.
    Enumerated {
        /// The domain, rendered, for [`Rule::ExhaustiveEnumeration`].
        domain: Symbol,
        /// Points of the whole domain, guard-rejected ones included.
        points: u64,
        /// Points the guard kept, and therefore points the body ran at.
        kept: u64,
    },
    /// The points were drawn.
    Sampled {
        generated: u32,
        kept: u32,
        rejected: u32,
        /// Type variables monomorphised to generate, e.g. `a := Int`.
        instantiations: Vec<(Symbol, Type)>,
    },
}

impl ValueDomain {
    /// The ground domain: one point, the empty tuple, and no way to miss any of it.
    pub fn ground() -> ValueDomain {
        ValueDomain::Enumerated {
            domain: "unit".into(),
            points: 1,
            kept: 1,
        }
    }

    /// The concurrency-law conditions condition 5.
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
            // Enumerating a finite domain and keeping nothing *decides* the guard unsatisfiable,
            // which is exhaustive enumeration applied to the guard rather than to the body.
            ValueDomain::Enumerated { .. } => VacuityKind::ProvedUnsatisfiable,
            ValueDomain::Sampled { generated, .. } => VacuityKind::NoCaseKept { generated },
        }
    }
}

/// One evaluation of a law body, at one point of the value domain, under one seed.
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

    /// Whether the body raised instead of coming to a Boolean.
    pub fn raised(&self) -> bool {
        self.raised
    }

    pub fn interleaving(&self) -> &Interleaving {
        &self.interleaving
    }

    /// A run assembled by hand rather than read off a machine, for a search that models one.
    pub fn model(interleaving: Interleaving, observed: bool, raised: bool) -> BodyRun {
        BodyRun {
            interleaving,
            observed,
            raised,
        }
    }
}

/// The one way to build a [`BodyRun`]: from the machine that just ran the body.
pub fn body_run(machine: &Machine<'_>, value: Result<Value, Diagnostic>, span: Span) -> BodyRun {
    body_run_recorded(machine.simulated(), value, span)
}

/// The same over what a compiled unit recorded of the run.
pub fn body_run_recorded(
    record: Option<&ply_eval::region::Record>,
    value: Result<Value, Diagnostic>,
    span: Span,
) -> BodyRun {
    let (outcome, raised) = match value {
        Ok(Value::Bool(true)) => (Ok(()), false),
        Ok(Value::Bool(false)) => (Err(body_was_false(span)), false),
        Ok(other) => (Err(body_was_not_boolean(&other, span)), true),
        Err(diagnostic) => (Err(diagnostic), true),
    };
    let observed = record.is_some();
    BodyRun {
        interleaving: match record {
            Some(record) => record.interleaving(&outcome),
            // The verdict is still the run's own: a body that reached no region must report nothing
            // about interleavings and must not turn a false law true on the way past.
            None => match outcome {
                Ok(()) => Interleaving::passed(Vec::new()),
                Err(diagnostic) => Interleaving::failed(Vec::new(), diagnostic),
            },
        },
        observed,
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

/// Running a law body at a point of its value domain, under a seed the search chooses.
pub trait LawSearch {
    /// Evaluate the law body with the binders bound to point `point`, under `seed`.
    fn run(&mut self, point: u64, seed: &Seed) -> BodyRun;

    /// The bindings at `point`, as a counterexample renders them.
    fn bindings(&self, point: u64) -> Vec<Binding>;
}

/// What discharging a concurrency law did, beside the discharge itself.
#[derive(Clone, Debug)]
pub struct Searched {
    pub discharge: Discharge,
    /// Interleavings that actually entered a `simulate` region, summed over the points the guard
    /// kept.
    pub interleavings: u32,
    /// Body evaluations, whether or not they reached a region.
    pub evaluations: u32,
    /// Every point's frontier emptied within its budget.
    pub exhaustive: bool,
    /// Some point's search spent its budget, so it proved nothing about the interleavings it did
    /// not reach.
    pub exhausted: bool,
    /// Every run entered a `simulate` region.
    pub observed: bool,
    /// Points the body ran at.
    pub points: u64,
}

impl Searched {
    /// `12 interleavings · exhaustive`, or why not.
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
            // A raise is not a refutation: a law that divides by zero says nothing about whether it
            // is true.
            return totals.finish(if failing.unwrap_or(Failing::Raised) == Failing::Raised {
                Discharge::Unattempted(Gap::Raised {
                    bindings: search.bindings(point),
                    diagnostic,
                })
            } else {
                Discharge::Refuted(Counterexample {
                    bindings: search.bindings(point),
                    original: search.bindings(point),
                    // Neither half is shrunk.
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

/// Aggregated over every point of the value domain, because a law is a claim about all of them: one
/// point whose search spent its budget is a law whose search spent its budget.
struct Totals {
    points: u64,
    interleavings: u32,
    evaluations: u32,
    exhaustive: bool,
    exhausted: bool,
    observed: bool,
    /// Carried rather than inferred from the early return a failure takes: [`Totals::proves`] must
    /// be false on a failing search whatever the shape of the loop above it later becomes.
    failure: Option<Seed>,
}

impl Totals {
    fn new(points: u64) -> Totals {
        Totals {
            points,
            interleavings: 0,
            evaluations: 0,
            // Vacuously true over no points, and `proves` requires an interleaving to have run
            // before it reads this.
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
        // The five of the concurrency-law conditions, and then the sixth: a search that entered no region emptied a
        // frontier it never filled.
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
            // Interleavings rather than value points, because what a concurrency law samples is
            // schedules: a ground law whose search spent its budget ran 256 cases, and calling that
            // one case would report `example` for the strongest sampled evidence in the language.
            generated: self.evaluations,
            kept: self.evaluations,
            rejected: u32::try_from(domain.rejected()).unwrap_or(u32::MAX),
            roots: plan.roots.clone(),
            instantiations: domain.instantiations(),
        }))
    }

    fn certificate(&self, obligation: &Obligation, domain: &ValueDomain) -> Certificate {
        let mut rules = Vec::new();
        // Both coverage claims are named, so an audit can check condition 5 against the certificate
        // rather than re-deriving it from the law.
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
            // A point was kept, so the guard admits a value.
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
    /// Which of the two things happened at each seed that ended badly.
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
pub fn replay_command(seed: &Seed, law: &str) -> String {
    format!("ply prove --seed {seed} --filter \"{law}\"")
}

/// The failure artifact for a refuted concurrency law: the search's own diagnostic, with the seed,
/// the race and the replay command attached.
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

/// What the differential tier audit's concurrency-law condition test asserts, as a function so that the assertion
/// is one call rather than a re-derivation that can drift from the thing it audits.
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

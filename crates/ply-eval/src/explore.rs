//! Footprint-guided interleaving exploration.
//!
//! Dynamic partial-order reduction in the backtrack-set formulation, with
//! [`StepFootprint::conflicts_with`] — resource-granular, exact, and the same
//! predicate that decides which tests may run concurrently — substituted for the
//! alias analysis the literature has to approximate. Two steps that do not
//! conflict commute, so exploring both of their orders is provably redundant;
//! everything in this module is bookkeeping over that one fact.
//!
//! The scheduler is not here. This module consumes a recorded run — steps, their
//! access sets, and the enabled set at every scheduling point — and decides
//! which seed to run next, so a caller supplies [`Simulation`] and gets back an
//! [`Exploration`]. That split is what lets the search be tested against a model
//! scheduler rather than only against the machine.
//!
//! Two rules the code cannot state for itself:
//!
//! - **Pruning must never skip an interleaving that could expose a distinct
//!   outcome.** A race the search cannot reach is worse than no search, because
//!   it produces false confidence. Where soundness and reduction disagree here,
//!   reduction loses; [`backtracks`] documents each place that happens.
//! - **A step's footprint must be complete.** An empty footprint means "touched
//!   nothing shared", and this module prunes on that. A scheduler that forgets
//!   to record a cell access does not produce a smaller number, it produces a
//!   wrong one — and the symptom is a *better* reduction.
//!
//! Nothing here may name a hash-based collection or read anything the seed does
//! not name: which interleaving runs next is as much a part of a run's
//! determinism as which task runs next. `hygiene` at the bottom checks the rule
//! that is checkable.

use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::cont::SimId;
use crate::sched::{Stamp, StepRecord, happens_before};
use crate::sim::{Exploration, Naive, Plan, Race, RaceSite, Seed, SimMode, StepFootprint, TaskId};

/// One step of one task, as the search reads it.
///
/// A step runs from the scheduler's resumption of a task up to and including
/// that task's next perform, so the boundary between two steps is a scheduling
/// point and `steps[i]` is what ran at point `i`.
///
/// [`StepRecord`] is the scheduler's own record of the same step and
/// [`Step::from_record`] adopts one. The two differ in exactly the site a race
/// is printed at, which the scheduler does not carry and the trace does.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step {
    /// Which of the entry point's `simulate` regions took this step.
    ///
    /// A test may enter several regions in sequence — only *nesting* is
    /// `E0416` — and they run one after another, never interleaved. Two steps
    /// from different regions can therefore never be reordered, and a task id
    /// means a different task in each, so the search must not pair them.
    pub region: SimId,
    pub task: TaskId,
    /// Every task the scheduler could have resumed at this point, in its
    /// canonical order. That order is part of what a seed *means* — a choice is
    /// an index into it — so a scheduler that orders it by anything the seed
    /// does not fix has already broken replay, and [`explore`] reports
    /// [`codes::SIMULATION_DIVERGENCE`] when it catches that.
    pub enabled: Vec<TaskId>,
    /// The index into `enabled` that was taken; `enabled[choice] == task`.
    pub choice: u16,
    /// What the step touched, **excluding** the terminating `task.*` / `clock.*`
    /// atom and **including** every cell and every `random.write`. This is the
    /// dependence relation's whole input.
    pub accesses: StepFootprint,
    /// The definition the step was inside. `None` when the run was not traced —
    /// never guessed.
    pub definition: Option<Symbol>,
    pub span: Span,
    /// The acting task's vector clock, which says which earlier steps this one
    /// had already observed. Empty means the recording carries no
    /// synchronization at all, and then no pair is ordered and every dependent
    /// pair is a candidate — the behaviour this search had before clocks
    /// existed.
    pub stamp: Stamp,
}

impl Step {
    /// Adopt the scheduler's record of a step. `definition` and `span` are where
    /// the step was standing, which a failure artifact prints and the scheduler
    /// does not know; `None` and [`Span::DUMMY`] are honest when the run was not
    /// traced, and cost only the location on a race report.
    pub fn from_record(record: &StepRecord, definition: Option<Symbol>, span: Span) -> Step {
        Step {
            region: record.region,
            task: record.task,
            enabled: record.enabled.clone(),
            choice: record.choice,
            accesses: record.accesses.clone(),
            definition,
            span,
            stamp: record.stamp.clone(),
        }
    }
}

/// How one interleaving ended. A deadlock, a runtime error and a failed
/// assertion are all `Failed`: the search's only question is whether this
/// interleaving is the one to report.
#[derive(Clone, Debug)]
pub enum Verdict {
    Passed,
    Failed(Diagnostic),
}

/// One interleaving, as the scheduler ran it.
#[derive(Clone, Debug)]
pub struct Interleaving {
    pub steps: Vec<Step>,
    pub verdict: Verdict,
    /// Nanoseconds of virtual time the run consumed.
    pub virtual_time: i64,
}

impl Interleaving {
    pub fn passed(steps: Vec<Step>) -> Interleaving {
        Interleaving {
            steps,
            verdict: Verdict::Passed,
            virtual_time: 0,
        }
    }

    pub fn failed(steps: Vec<Step>, diagnostic: Diagnostic) -> Interleaving {
        Interleaving {
            steps,
            verdict: Verdict::Failed(diagnostic),
            virtual_time: 0,
        }
    }

    /// The choice sequence actually taken, which is not the seed's path: beyond
    /// the path the `sched` stream chose, and a backtrack point is named
    /// relative to what ran rather than to what was fixed.
    fn choices(&self) -> Vec<u16> {
        self.steps.iter().map(|s| s.choice).collect()
    }
}

/// Whole-test replay at one seed. The implementer is the scheduler.
///
/// Three obligations, each of which the search would otherwise silently rely on:
///
/// - **`run` is a pure function of the definition set and `seed`.** Calling it
///   twice with one seed must produce identical steps, identical enabled sets
///   and an identical verdict. The search re-runs prefixes and checks this.
/// - **The enabled set is in a canonical order** that is itself a function of
///   the run so far. A choice is an index into it.
/// - **Every access is in the footprint.** Pruning is sound only over a complete
///   access set; see this module's header.
pub trait Simulation {
    fn run(&mut self, seed: &Seed) -> Interleaving;
}

impl<F: FnMut(&Seed) -> Interleaving> Simulation for F {
    fn run(&mut self, seed: &Seed) -> Interleaving {
        self(seed)
    }
}

/// Which dependence relation the search runs over.
///
/// [`Dependence::All`] is not a fallback — it is the measurement. Forcing every
/// pair dependent degenerates this search into exhaustive enumeration of every
/// schedule respecting per-task order and enabledness, which is exactly the
/// naive scheduler the reduction is claimed against. Same code, one flag, no
/// second implementation to disagree with the first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dependence {
    Exact,
    All,
}

impl Dependence {
    fn dependent(self, a: &StepFootprint, b: &StepFootprint) -> bool {
        match self {
            Dependence::Exact => a.conflicts_with(b),
            Dependence::All => true,
        }
    }
}

/// The budget the naive search gets under [`measure_reduction`]. Larger than a
/// real search's, because the number being measured is how much larger the
/// unpruned space is; when it is spent, the count is a lower bound and says so.
pub const NAIVE_BUDGET: u32 = 4096;

/// What a search produced.
#[derive(Clone, Debug)]
pub struct Explored {
    pub exploration: Exploration,
    /// The failing interleaving's diagnostic. `Some` exactly when
    /// `exploration.failure` is `Some`.
    pub diagnostic: Option<Diagnostic>,
    /// The interleavings run, in the order they were run. The evidence behind
    /// `exploration.explored`, and what a replay of the whole search compares
    /// against.
    pub seeds: Vec<Seed>,
}

impl Explored {
    pub fn passed(&self) -> bool {
        self.exploration.failure.is_none()
    }
}

/// Run `plan` against `driver`.
///
/// Under [`SimMode::Once`] and [`SimMode::Random`] this is one interleaving per
/// seed and no search — there is nothing to prune and nothing to report as
/// exhaustive. Under [`SimMode::Dpor`] it is the search of ADR 0006 §6.2.
pub fn explore(plan: &Plan, driver: &mut dyn Simulation) -> Explored {
    explore_under(plan, Dependence::Exact, driver)
}

/// [`explore`] with the dependence relation chosen explicitly.
///
/// [`Dependence::All`] is the unpruned enumeration [`measure_reduction`] counts
/// against. It is an entry point of its own because the interesting audit is not
/// the ratio: it is that the two searches observe the **same set of outcomes**,
/// which is the property pruning has to preserve and the only one worth
/// checking against a real corpus.
pub fn explore_under(plan: &Plan, dependence: Dependence, driver: &mut dyn Simulation) -> Explored {
    let plan = plan.clone().normalized();
    match plan.mode {
        SimMode::Once | SimMode::Random => sample(&plan, driver),
        SimMode::Dpor => search(&plan, dependence, driver),
    }
}

/// [`explore`], then the same search again with the dependence relation forced
/// to `true`, filling [`Exploration::naive`].
///
/// Off by default: the claim is a benchmark, and no ordinary run should pay
/// double for it.
pub fn measure_reduction(plan: &Plan, driver: &mut dyn Simulation) -> Explored {
    let mut explored = explore(plan, driver);
    if plan.mode != SimMode::Dpor || explored.exploration.failure.is_some() {
        // Nothing was pruned, or both searches stopped at a failure rather than
        // at their frontier. Neither is a ratio, so neither is reported as one.
        return explored;
    }
    let naive_plan = Plan {
        budget: plan.budget.max(NAIVE_BUDGET),
        ..plan.clone()
    }
    .normalized();
    let naive = search(&naive_plan, Dependence::All, driver);
    explored.exploration.naive = Some(Naive {
        explored: naive.exploration.explored,
        bounded: naive.exploration.exhausted || naive.exploration.failure.is_some(),
    });
    if let Some(seed) = naive.exploration.failure {
        // The pruned search called this program green and the unpruned one
        // failed it. That is not a ratio to report, it is a defect in the
        // dependence relation, and the failure is the actionable half.
        explored.exploration.failure = Some(seed);
        explored.exploration.exhaustive = false;
        explored.diagnostic = naive.diagnostic.map(|d| {
            d.note(
                "this interleaving was reached only with the dependence relation forced to true, \
                 so the pruned search skipped it: a step's recorded footprint is missing an \
                 access it made",
            )
        });
    }
    explored
}

fn sample(plan: &Plan, driver: &mut dyn Simulation) -> Explored {
    let mut exploration = Exploration::default();
    let mut seeds = Vec::new();
    let mut diagnostic = None;
    for seed in plan.seeds() {
        let run = driver.run(&seed);
        exploration.explored += 1;
        exploration.steps += run.steps.len() as u64;
        exploration.virtual_time = run.virtual_time;
        seeds.push(seed.clone());
        if let Err(d) = check_recording(&seed, &run.steps) {
            exploration.failure = Some(seed);
            diagnostic = Some(d);
            break;
        }
        if let Verdict::Failed(d) = run.verdict {
            exploration.failure = Some(seed);
            diagnostic = Some(d);
            break;
        }
    }
    Explored {
        exploration,
        diagnostic,
        seeds,
    }
}

fn search(plan: &Plan, dependence: Dependence, driver: &mut dyn Simulation) -> Explored {
    let mut exploration = Exploration {
        exhaustive: true,
        ..Exploration::default()
    };
    let mut seeds = Vec::new();
    let mut diagnostic = None;
    for &root in &plan.roots {
        let report = search_root(root, plan.budget, dependence, driver);
        exploration.explored += report.explored;
        exploration.steps += report.steps;
        exploration.virtual_time = report.virtual_time;
        exploration.exhausted |= report.exhausted;
        if report.exhausted {
            exploration.exhaustive = false;
        }
        seeds.extend(report.seeds);
        if let Some(failure) = report.failure {
            exploration.failure = Some(failure.seed);
            exploration.race = failure.race;
            exploration.exhaustive = false;
            diagnostic = Some(failure.diagnostic);
            break;
        }
    }
    Explored {
        exploration,
        diagnostic,
        seeds,
    }
}

/// Where a queued interleaving came from: the run it branched off, the
/// scheduling point it diverges at, and — when the branch was taken to reverse a
/// specific pair of steps — which pair.
struct Branch {
    trace: Rc<Vec<Step>>,
    at: usize,
    choice: u16,
    /// The step the search meant to reorder against `trace[at]`. `None` when the
    /// branch was queued conservatively rather than to reverse an observed race,
    /// in which case there is no pair to report and none is invented.
    against: Option<usize>,
}

struct Work {
    path: Vec<u16>,
    /// `None` for a root's first interleaving, which branched from nothing.
    branch: Option<Branch>,
}

struct Failure {
    seed: Seed,
    diagnostic: Diagnostic,
    race: Option<Race>,
}

#[derive(Default)]
struct RootReport {
    explored: u32,
    steps: u64,
    virtual_time: i64,
    exhausted: bool,
    failure: Option<Failure>,
    seeds: Vec<Seed>,
}

fn search_root(
    root: u64,
    budget: u32,
    dependence: Dependence,
    driver: &mut dyn Simulation,
) -> RootReport {
    let mut report = RootReport::default();
    let mut frontier = vec![Work {
        path: Vec::new(),
        branch: None,
    }];
    // Every (prefix, choice) this search has run or queued. A run is a pure
    // function of (root, path), so a claimed pair names an interleaving that has
    // already been accounted for and re-running it would be the one kind of work
    // this module exists to not do.
    let mut claimed: BTreeMap<Vec<u16>, BTreeSet<u16>> = BTreeMap::new();

    while let Some(work) = frontier.pop() {
        if report.explored >= budget {
            // The frontier is not empty, so the interleavings still on it were
            // never run and nothing may be claimed about them.
            report.exhausted = true;
            break;
        }
        let seed = Seed::at(root, work.path.clone());
        let run = driver.run(&seed);
        report.explored += 1;
        report.steps += run.steps.len() as u64;
        report.virtual_time = run.virtual_time;
        report.seeds.push(seed.clone());

        if let Err(diagnostic) = check_recording(&seed, &run.steps) {
            report.failure = Some(Failure {
                seed,
                diagnostic,
                race: None,
            });
            break;
        }
        if let Some(branch) = &work.branch
            && let Err(diagnostic) = check_replay(&seed, branch, &run.steps)
        {
            report.failure = Some(Failure {
                seed,
                diagnostic,
                race: None,
            });
            break;
        }
        if let Verdict::Failed(diagnostic) = run.verdict {
            let race = work.branch.as_ref().and_then(race_of);
            report.failure = Some(Failure {
                seed,
                diagnostic,
                race,
            });
            break;
        }

        let choices = run.choices();
        for (i, step) in run.steps.iter().enumerate() {
            claimed
                .entry(choices[..i].to_vec())
                .or_default()
                .insert(step.choice);
        }

        let trace = Rc::new(run.steps);
        for (at, tasks) in backtracks(&trace, dependence) {
            let prefix = &choices[..at];
            for (task, against) in tasks {
                let Some(choice) = choice_of(&trace[at].enabled, task) else {
                    continue;
                };
                if !claimed.entry(prefix.to_vec()).or_default().insert(choice) {
                    continue;
                }
                let mut path = prefix.to_vec();
                path.push(choice);
                frontier.push(Work {
                    path,
                    branch: Some(Branch {
                        trace: Rc::clone(&trace),
                        at,
                        choice,
                        against,
                    }),
                });
            }
        }
    }
    report
}

/// The backtrack points a completed interleaving reveals: at scheduling point
/// `at`, each task worth resuming instead, and the step the search means to
/// reorder against `trace[at]`.
///
/// ADR 0006 §6.2 states the rule as: for each `i`, for each `j < i` that is
/// dependent with it, from a different task, **with no step of `task(sᵢ)`
/// between them**, and **with `task(sᵢ)` enabled at `j`**, add `task(sᵢ)` to
/// `backtrack[j]`. Both emphasized conditions are unsound as written, and this
/// implementation drops the first and adds an else branch to the second. Each
/// deviation explores more interleavings than the ADR's text implies and never
/// fewer.
///
/// **"No step of `task(sᵢ)` between them" loses races.** Take `main` writing a
/// counter while `late` is blocked in a `join`, and `late` reading that counter
/// after its barrier releases. `late`'s `join` step sits between the write and
/// the read, so the scan stops before it reaches the write, no backtrack point
/// is generated anywhere, and the search reports *one* interleaving and
/// *exhaustive* over a program with an ordinary lost update in it. That is the
/// worst failure this milestone can ship: a proof of the wrong thing. The
/// condition is trying to express Flanagan and Godefroid's "the last transition
/// dependent with the *next* transition of `p`", where the intervening steps of
/// `p` are exactly what the algorithm looks past, and no per-trace pairwise scan
/// can spell it that way. Every dependent pair from two different tasks is a
/// backtrack point here, which is a superset of both that rule and the ADR's.
///
/// **`task(sᵢ)` not enabled at `j` needs the else branch.** When it had not been
/// spawned yet, or was blocked, dropping the pair loses the race for the same
/// reason: reaching the other order means first running some *third* task to
/// unblock it. Adding every task enabled at `j` is what makes that reachable.
///
/// **A pair the join graph already ordered is not a race, and this is where
/// that is spent.** A dependent pair whose earlier step *happens before* the
/// later one cannot appear in the other order under any schedule, so a
/// backtrack point for it queues an interleaving that does not exist. Nothing
/// downstream notices — the branch runs, produces the same trace it branched
/// from, and is counted. On the ordinary shape of a concurrent test, where the
/// parent asserts after `join`ing every child, that pair is *every* child step
/// against *every* parent assertion, and the else branch above then queues an
/// alternative for each: measured over the corpus, one interleaving becomes
/// nine hundred and ninety-two at five tasks. [`crate::sched::happens_before`]
/// is what makes the ordering visible, and it is a filter rather than an
/// approximation — the reversal it refuses to queue is unreachable rather than
/// unlikely.
///
/// The cost of the two deviations above is duplicate work, not wrong work: two
/// branches that name one `(prefix, choice)` collapse to one interleaving, and a
/// branch queued conservatively carries no `against`, so it can never be
/// reported as an observed race.
fn backtracks(
    steps: &[Step],
    dependence: Dependence,
) -> BTreeMap<usize, BTreeMap<TaskId, Option<usize>>> {
    let mut out: BTreeMap<usize, BTreeMap<TaskId, Option<usize>>> = BTreeMap::new();
    for i in (1..steps.len()).rev() {
        let later = &steps[i];
        for j in (0..i).rev() {
            let earlier = &steps[j];
            if earlier.region != later.region {
                // Two regions of one entry point run in sequence, never
                // interleaved, so no schedule puts these two in the other order
                // — and a task id means a different task in each of them.
                continue;
            }
            if earlier.task == later.task {
                // Program order, not a race.
                continue;
            }
            if !dependence.dependent(&earlier.accesses, &later.accesses) {
                continue;
            }
            if dependence == Dependence::Exact
                && happens_before(&earlier.stamp, earlier.task, &later.stamp)
            {
                continue;
            }
            let at = out.entry(j).or_default();
            if earlier.enabled.contains(&later.task) {
                record(at, later.task, Some(i));
            } else {
                for &task in &earlier.enabled {
                    record(at, task, None);
                }
            }
        }
    }
    out
}

/// A task queued twice at one point keeps whichever pair was observed, so a
/// conservative branch never erases the race a real one names.
fn record(at: &mut BTreeMap<TaskId, Option<usize>>, task: TaskId, against: Option<usize>) {
    let entry = at.entry(task).or_insert(None);
    if entry.is_none() {
        *entry = against;
    }
}

/// `check_recording` has already refused an enabled set too large to index, so
/// the conversion cannot silently drop a backtrack point.
fn choice_of(enabled: &[TaskId], task: TaskId) -> Option<u16> {
    let index = enabled.iter().position(|&t| t == task)?;
    u16::try_from(index).ok()
}

/// The two steps whose reordering the failing branch was queued to perform.
///
/// `None` unless the search actually observed both of them contending: a branch
/// queued conservatively, or one whose steps share no access to name, reports
/// no race rather than a guessed one.
fn race_of(branch: &Branch) -> Option<Race> {
    let against = branch.against?;
    let left = branch.trace.get(branch.at)?;
    let right = branch.trace.get(against)?;
    Some(Race {
        left: site(left, right)?,
        right: site(right, left)?,
        at: u32::try_from(branch.at).ok()?,
    })
}

fn site(step: &Step, other: &Step) -> Option<RaceSite> {
    Some(RaceSite {
        task: step.task,
        definition: step.definition.clone(),
        access: step
            .accesses
            .contention(&other.accesses)
            .first()?
            .to_string(),
        span: step.span,
    })
}

/// A recording that does not describe a schedule is Ply's fault, and it is
/// caught before the search reasons over it: every later conclusion — which
/// interleavings exist, which were pruned, whether the search was exhaustive —
/// is derived from these fields.
fn check_recording(seed: &Seed, steps: &[Step]) -> Result<(), Diagnostic> {
    for (i, step) in steps.iter().enumerate() {
        if step.enabled.len() > usize::from(u16::MAX) {
            return Err(defect(
                seed,
                step,
                format!(
                    "scheduling point {i} offered {} enabled tasks, and a seed's choice sequence \
                     cannot name more than {}",
                    step.enabled.len(),
                    u16::MAX
                ),
            ));
        }
        match step.enabled.get(usize::from(step.choice)) {
            Some(&task) if task == step.task => {}
            _ => {
                return Err(defect(
                    seed,
                    step,
                    format!(
                        "scheduling point {i} resumed {} as choice {}, which its enabled set {} \
                         does not offer",
                        step.task,
                        step.choice,
                        render(&step.enabled)
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Replay is self-checking: re-running a prefix must reproduce the enabled set
/// at every scheduling point that prefix names.
///
/// A mismatch means the run was not a function of its seed, which invalidates
/// every artifact this milestone produces — the replay a failure prints, the
/// exhaustiveness claim, the cached pass. It is Ply's fault, not the program's,
/// and it is the same class of defect as an engine divergence.
fn check_replay(seed: &Seed, branch: &Branch, steps: &[Step]) -> Result<(), Diagnostic> {
    for point in 0..=branch.at {
        let expected = match branch.trace.get(point) {
            Some(step) => step,
            None => break,
        };
        let Some(actual) = steps.get(point) else {
            return Err(divergence(
                seed,
                expected.span,
                format!(
                    "the run stopped after {} scheduling points, and this seed names a choice at \
                     point {point}",
                    steps.len()
                ),
            ));
        };
        if actual.enabled != expected.enabled {
            return Err(divergence(
                seed,
                expected.span,
                format!(
                    "at scheduling point {point} the recorded enabled set was {} and the replay \
                     offered {}",
                    render(&expected.enabled),
                    render(&actual.enabled)
                ),
            ));
        }
        let wanted = if point == branch.at {
            branch.choice
        } else {
            expected.choice
        };
        if actual.choice != wanted {
            return Err(divergence(
                seed,
                expected.span,
                format!(
                    "at scheduling point {point} this seed names choice {wanted} and the replay \
                     took {}",
                    actual.choice
                ),
            ));
        }
    }
    Ok(())
}

fn divergence(seed: &Seed, span: Span, what: String) -> Diagnostic {
    Diagnostic::error(
        codes::SIMULATION_DIVERGENCE,
        format!("replaying seed {seed} did not reproduce the recorded schedule"),
    )
    .primary(span, "this scheduling point")
    .note(what)
    .note(
        "a simulated run must be a pure function of its definition set and its seed; this is a \
         defect in Ply rather than in the program under test",
    )
    .note(format!(
        "reproduce with `--sim once --seed {seed}`, and report it with the test's source"
    ))
}

fn defect(seed: &Seed, step: &Step, what: String) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the scheduler recorded a step that does not describe a schedule",
    )
    .primary(step.span, "this step")
    .note(what)
    .note(format!(
        "reproduce with `--sim once --seed {seed}`, and report it with the test's source"
    ))
}

fn render(tasks: &[TaskId]) -> String {
    if tasks.is_empty() {
        return "{}".to_string();
    }
    tasks
        .iter()
        .map(TaskId::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{Access, Domain, Stream};
    use crate::world::CellId;
    use ply_core::{EffectAtom, Resource};
    use ply_syntax::ast::Mode;

    /// A model scheduler: enough of ADR 0006 §3 to exercise the search, and
    /// none of the machine. A task is a list of operations and one operation is
    /// one step, so every operation here is a scheduling point. That is finer
    /// than the machine's §3.3 granularity on purpose — the search is what is
    /// under test, and it must be exercised on traces it could be handed.
    #[derive(Clone, Debug)]
    enum Op {
        /// Read a cell into this task's register.
        Load(u32),
        /// Write `register + 1` into a cell.
        Store(u32),
        /// `db.<mode>[resource]`.
        Perform(&'static str, Mode),
        Spawn(&'static str, Vec<Op>),
        /// Join this task's `n`th child, by spawn order rather than by id, so a
        /// program means the same thing under every interleaving.
        Join(usize),
        Yield,
    }

    #[derive(Clone)]
    struct ModelTask {
        name: Symbol,
        ops: Vec<Op>,
        pc: usize,
        register: i64,
        children: Vec<usize>,
        blocked: Option<usize>,
        /// The same vector clock the real scheduler keeps, so the search's
        /// happens-before filter is exercised here rather than only against the
        /// machine.
        clock: Stamp,
    }

    #[derive(Clone)]
    struct Model {
        tasks: Vec<ModelTask>,
        cells: BTreeMap<u32, i64>,
        expect: Vec<(u32, i64)>,
        runs: usize,
        /// Every interleaving this model was asked for, as the task order it
        /// produced. The evidence for "both orders were explored".
        traces: Vec<Vec<TaskId>>,
        /// The final world of each interleaving. Two searches over one program
        /// must observe the same *set* of these, whatever they explored.
        outcomes: Vec<Vec<(u32, i64)>>,
    }

    const STEP_CAP: usize = 4096;

    impl Model {
        fn new(main: Vec<Op>) -> Model {
            Model {
                tasks: vec![ModelTask {
                    name: Symbol::new("main"),
                    ops: main,
                    pc: 0,
                    register: 0,
                    children: Vec::new(),
                    blocked: None,
                    clock: vec![0],
                }],
                cells: BTreeMap::new(),
                expect: Vec::new(),
                runs: 0,
                traces: Vec::new(),
                outcomes: Vec::new(),
            }
        }

        fn expecting(mut self, cells: &[(u32, i64)]) -> Model {
            self.expect = cells.to_vec();
            self
        }

        fn finished(tasks: &[ModelTask], t: usize) -> bool {
            tasks[t].pc == tasks[t].ops.len()
                && tasks[t].blocked.is_none_or(|on| Model::finished(tasks, on))
        }

        fn tick(tasks: &mut [ModelTask], t: usize) {
            let width = tasks.len();
            for task in tasks.iter_mut() {
                task.clock.resize(width, 0);
            }
            tasks[t].clock[t] += 1;
        }

        fn absorb(tasks: &mut [ModelTask], into: usize, from: usize) {
            let source = tasks[from].clock.clone();
            let target = &mut tasks[into].clock;
            if target.len() < source.len() {
                target.resize(source.len(), 0);
            }
            for (slot, seen) in target.iter_mut().zip(source) {
                *slot = (*slot).max(seen);
            }
        }

        fn enabled(tasks: &[ModelTask]) -> Vec<usize> {
            (0..tasks.len())
                .filter(|&t| {
                    tasks[t].pc < tasks[t].ops.len()
                        && tasks[t].blocked.is_none_or(|on| Model::finished(tasks, on))
                })
                .collect()
        }
    }

    impl Simulation for Model {
        fn run(&mut self, seed: &Seed) -> Interleaving {
            self.runs += 1;
            let mut tasks = self.tasks.clone();
            let mut cells = self.cells.clone();
            let mut sched = Stream::new(seed.root, Domain::Sched);
            let mut steps = Vec::new();
            let mut order = Vec::new();

            loop {
                let enabled = Model::enabled(&tasks);
                if enabled.is_empty() {
                    break;
                }
                if steps.len() >= STEP_CAP {
                    return Interleaving::failed(
                        steps,
                        Diagnostic::error(codes::DEADLOCK, "the model ran out of steps"),
                    );
                }
                let point = steps.len();
                let chosen = match seed.choice(point) {
                    Some(fixed) => usize::from(fixed),
                    None => sched.below(enabled.len() as u64).unwrap_or(0) as usize,
                };
                let chosen = chosen.min(enabled.len() - 1);
                let t = enabled[chosen];
                let op = tasks[t].ops[tasks[t].pc].clone();
                tasks[t].pc += 1;
                Model::tick(&mut tasks, t);
                let mut accesses = StepFootprint::new();
                match op {
                    Op::Load(cell) => {
                        tasks[t].register = *cells.get(&cell).unwrap_or(&0);
                        accesses.insert(Access::Cell {
                            id: CellId(cell),
                            mode: Mode::Read,
                        });
                    }
                    Op::Store(cell) => {
                        cells.insert(cell, tasks[t].register + 1);
                        accesses.insert(Access::Cell {
                            id: CellId(cell),
                            mode: Mode::Write,
                        });
                    }
                    Op::Perform(resource, mode) => {
                        accesses.insert(Access::Atom(EffectAtom::new(
                            "db",
                            Resource::Named(Symbol::new(resource)),
                            mode,
                        )));
                    }
                    Op::Spawn(name, ops) => {
                        let child = tasks.len();
                        let inherited = tasks[t].clock.clone();
                        tasks.push(ModelTask {
                            name: Symbol::new(name),
                            ops,
                            pc: 0,
                            register: 0,
                            children: Vec::new(),
                            blocked: None,
                            clock: inherited,
                        });
                        tasks[t].children.push(child);
                    }
                    Op::Join(nth) => {
                        let child = tasks[t].children[nth];
                        if Model::finished(&tasks, child) {
                            Model::absorb(&mut tasks, t, child);
                        } else {
                            tasks[t].blocked = Some(child);
                        }
                    }
                    Op::Yield => {}
                }
                // A task blocked on a join observes everything its target did,
                // the moment the target finishes.
                for i in 0..tasks.len() {
                    if let Some(on) = tasks[i].blocked
                        && Model::finished(&tasks, on)
                    {
                        Model::absorb(&mut tasks, i, on);
                    }
                }
                order.push(TaskId(t as u32));
                steps.push(Step {
                    region: SimId(0),
                    task: TaskId(t as u32),
                    enabled: enabled.iter().map(|&t| TaskId(t as u32)).collect(),
                    choice: chosen as u16,
                    accesses,
                    definition: Some(tasks[t].name.clone()),
                    span: Span::DUMMY,
                    stamp: tasks[t].clock.clone(),
                });
            }

            self.traces.push(order);
            self.outcomes
                .push(cells.iter().map(|(&c, &v)| (c, v)).collect());
            if (0..tasks.len()).any(|t| !Model::finished(&tasks, t)) {
                return Interleaving::failed(
                    steps,
                    Diagnostic::error(codes::DEADLOCK, "no task can make progress"),
                );
            }
            for (cell, want) in &self.expect {
                let got = cells.get(cell).copied().unwrap_or(0);
                if got != *want {
                    return Interleaving::failed(
                        steps,
                        Diagnostic::error(
                            codes::ASSERTION_FAILED,
                            format!("expected {want} in cell #{cell}, found {got}"),
                        ),
                    );
                }
            }
            Interleaving::passed(steps)
        }
    }

    fn dpor(budget: u32) -> Plan {
        Plan {
            budget,
            ..Plan::default()
        }
    }

    fn read(resource: &'static str) -> Op {
        Op::Perform(resource, Mode::Read)
    }

    fn write(resource: &'static str) -> Op {
        Op::Perform(resource, Mode::Write)
    }

    /// Did the two tasks appear in both orders across the interleavings run?
    fn both_orders(model: &Model, a: TaskId, b: TaskId) -> bool {
        let order = |trace: &Vec<TaskId>| {
            let ia = trace.iter().position(|&t| t == a);
            let ib = trace.iter().position(|&t| t == b);
            match (ia, ib) {
                (Some(ia), Some(ib)) => Some(ia < ib),
                _ => None,
            }
        };
        model.traces.iter().filter_map(order).any(|x| x)
            && model.traces.iter().filter_map(order).any(|x| !x)
    }

    /// The headline. Two tasks whose footprints do not conflict commute, so one
    /// interleaving is the whole story and a second would be provably redundant
    /// work.
    #[test]
    fn tasks_that_never_conflict_explore_exactly_one_interleaving() {
        let mut model = Model::new(vec![
            Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
            Op::Spawn("b", vec![Op::Load(2), Op::Store(2)]),
            Op::Join(0),
            Op::Join(1),
        ])
        .expecting(&[(1, 1), (2, 1)]);

        let explored = explore(&dpor(64), &mut model);
        assert_eq!(explored.exploration.explored, 1);
        assert!(explored.exploration.exhaustive);
        assert!(!explored.exploration.exhausted);
        assert!(explored.passed());
    }

    /// ...and the same program under an unpruned search runs seventy-one, which
    /// is the measurement the whole claim rests on. Pinned rather than bounded:
    /// this number moving is the news, whichever direction it moves in.
    #[test]
    fn the_naive_count_for_the_same_program_is_larger() {
        let mut model = Model::new(vec![
            Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
            Op::Spawn("b", vec![Op::Load(2), Op::Store(2)]),
            Op::Join(0),
            Op::Join(1),
        ])
        .expecting(&[(1, 1), (2, 1)]);

        let explored = measure_reduction(&dpor(NAIVE_BUDGET), &mut model);
        assert_eq!(explored.exploration.explored, 1);
        assert_eq!(
            explored.exploration.naive,
            Some(Naive {
                explored: 71,
                bounded: false
            })
        );
        assert_eq!(explored.exploration.reduction(), Some(71.0));
    }

    /// Two tasks that *do* share a cell: the search runs nine interleavings
    /// against the unpruned seventy-one. Six of the nine are the equivalence
    /// classes — `a`'s two steps against `b`'s two — and the other three are
    /// what §6.2's rule costs where this module refuses to trust it. The
    /// reduction is real either way, and the overhead is measured rather than
    /// asserted away.
    #[test]
    fn a_shared_cell_costs_interleavings_and_still_reduces() {
        let program = || {
            Model::new(vec![
                Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
                Op::Spawn("b", vec![Op::Load(1), Op::Store(1)]),
                Op::Join(0),
                Op::Join(1),
            ])
        };
        let mut model = program();
        let explored = explore(&dpor(NAIVE_BUDGET), &mut model);
        assert_eq!(explored.exploration.explored, 9);
        assert!(explored.exploration.exhaustive);

        let mut whole = program();
        let unpruned = explore_under(&dpor(NAIVE_BUDGET), Dependence::All, &mut whole);
        assert_eq!(unpruned.exploration.explored, 71);
    }

    /// Small enough to enumerate by hand. `main` spawns `a` then `b`; `a` and
    /// `b` are one step each and touch nothing in common. With `main` first,
    /// the schedules respecting per-task order and enabledness are
    /// `m,m,a,b` · `m,m,b,a` · `m,a,m,b`: three, because `b` cannot run before
    /// the spawn that creates it. The pruned search runs one.
    #[test]
    fn the_naive_count_is_exact_on_a_fixture_that_can_be_counted_by_hand() {
        let mut model = Model::new(vec![
            Op::Spawn("a", vec![read("x")]),
            Op::Spawn("b", vec![read("y")]),
        ]);
        let explored = measure_reduction(&dpor(64), &mut model);
        assert_eq!(explored.exploration.explored, 1);
        assert_eq!(
            explored.exploration.naive,
            Some(Naive {
                explored: 3,
                bounded: false
            })
        );
    }

    /// A budget the naive search spends is reported as a lower bound and never
    /// as an exact number nobody observed.
    #[test]
    fn a_spent_naive_budget_is_reported_as_a_bound() {
        let mut model = Model::new(vec![
            Op::Spawn("a", vec![read("x"), read("x"), read("x"), read("x")]),
            Op::Spawn("b", vec![read("y"), read("y"), read("y"), read("y")]),
            Op::Spawn("c", vec![read("z"), read("z"), read("z"), read("z")]),
            Op::Join(0),
            Op::Join(1),
            Op::Join(2),
        ]);
        let plan = Plan {
            budget: 8,
            ..Plan::default()
        };
        let mut explored = measure_reduction(&plan, &mut model);
        // The naive budget is the larger of the plan's and NAIVE_BUDGET, so
        // shrink the space instead of the budget by asserting on the flag.
        let naive = explored.exploration.naive.take().expect("measured");
        assert!(naive.bounded, "expected a bounded count, got {naive}");
        assert_eq!(naive.explored, NAIVE_BUDGET);
        assert!(naive.to_string().starts_with(">= "));
    }

    /// Two writes to one cell. The outcome is the same either way, so the
    /// search passes — but it must still have run both orders, because the
    /// relation says the order is observable and the search's job is to make
    /// that concrete rather than to argue about it.
    #[test]
    fn a_conflicting_pair_is_explored_in_both_orders() {
        let mut model = Model::new(vec![
            Op::Spawn("a", vec![Op::Store(1)]),
            Op::Spawn("b", vec![Op::Store(1)]),
            Op::Join(0),
            Op::Join(1),
        ])
        .expecting(&[(1, 1)]);

        let explored = explore(&dpor(64), &mut model);
        assert!(explored.passed());
        assert!(explored.exploration.exhaustive);
        assert!(explored.exploration.explored >= 2);
        assert!(
            both_orders(&model, TaskId(1), TaskId(2)),
            "explored {:?}",
            model.traces
        );
    }

    /// The lost update, as two steps of the model above: the search finds the
    /// interleaving in which both tasks load before either stores, and reports
    /// the seed that reproduces it.
    #[test]
    fn a_genuine_race_is_found_and_named() {
        let mut model = Model::new(vec![
            Op::Spawn("credit", vec![Op::Load(1), Op::Store(1)]),
            Op::Spawn("debit", vec![Op::Load(1), Op::Store(1)]),
            Op::Join(0),
            Op::Join(1),
        ])
        .expecting(&[(1, 2)]);

        let explored = explore(&dpor(256), &mut model);
        assert!(!explored.passed(), "the lost update was not found");
        let seed = explored
            .exploration
            .failure
            .clone()
            .expect("a failing seed");
        let race = explored.exploration.race.clone().expect("an observed race");
        assert_ne!(race.left.task, race.right.task);
        assert!(race.left.access.starts_with("cell."));
        assert!(race.right.access.starts_with("cell."));
        assert!(
            race.left.definition.is_some() && race.right.definition.is_some(),
            "the race sites name the definitions they were in"
        );
        assert_eq!(
            explored.diagnostic.expect("a diagnostic").code,
            codes::ASSERTION_FAILED
        );

        // The seed the artifact prints is the seed that reproduces it.
        let mut replay = Model::new(model.tasks[0].ops.clone()).expecting(&[(1, 2)]);
        let again = explore(&Plan::once(seed), &mut replay);
        assert!(!again.passed());
        assert_eq!(again.exploration.explored, 1);
        // A sampled run observed no flip, so it reports no race rather than an
        // inferred one.
        assert_eq!(again.exploration.race, None);
    }

    /// The pair conflicts on one resource and not on the other. The search must
    /// separate the two: `x` is contended and its order is explored, `y` and
    /// `z` are private and contribute nothing.
    #[test]
    fn footprints_that_conflict_on_one_resource_only() {
        let disjoint = vec![
            Op::Spawn("a", vec![read("x"), write("y")]),
            Op::Spawn("b", vec![read("x"), write("z")]),
            Op::Join(0),
            Op::Join(1),
        ];
        let mut shared_read = Model::new(disjoint.clone());
        let read_only = explore(&dpor(64), &mut shared_read);
        assert_eq!(read_only.exploration.explored, 1, "two readers commute");
        assert!(read_only.exploration.exhaustive);

        let contended = vec![
            Op::Spawn("a", vec![write("x"), write("y")]),
            Op::Spawn("b", vec![read("x"), write("z")]),
            Op::Join(0),
            Op::Join(1),
        ];
        let mut shared_write = Model::new(contended);
        let contested = explore(&dpor(64), &mut shared_write);
        assert!(
            contested.exploration.explored > read_only.exploration.explored,
            "a write against a read of the same resource is a real difference"
        );
        assert!(contested.exploration.exhaustive);
        assert!(both_orders(&shared_write, TaskId(1), TaskId(2)));
        // ...and still far short of the unpruned space, because `y` and `z`
        // conflict with nothing.
        let mut measured = Model::new(disjoint);
        let naive = measure_reduction(&dpor(64), &mut measured)
            .exploration
            .naive
            .expect("measured");
        assert!(naive.explored > contested.exploration.explored);
    }

    /// The property pruning has to preserve, checked directly rather than
    /// argued: over each program, the interleavings the pruned search runs
    /// produce **the same set of final worlds** as the interleavings an
    /// unpruned enumeration runs. A missing outcome here is a race the search
    /// cannot reach, which is the one defect this module must not ship, and it
    /// is invisible in the counts — the pruned search would simply look
    /// cheaper.
    #[test]
    fn pruning_preserves_every_outcome_the_unpruned_search_observes() {
        let programs: Vec<(&str, Vec<Op>)> = vec![
            (
                "lost update",
                vec![
                    Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
                    Op::Spawn("b", vec![Op::Load(1), Op::Store(1)]),
                    Op::Join(0),
                    Op::Join(1),
                ],
            ),
            (
                "a racer behind a join",
                vec![
                    Op::Spawn(
                        "late",
                        vec![
                            Op::Spawn("barrier", vec![Op::Yield]),
                            Op::Join(0),
                            Op::Load(1),
                            Op::Store(1),
                        ],
                    ),
                    Op::Load(1),
                    Op::Store(1),
                    Op::Join(0),
                ],
            ),
            (
                "a nested spawn racing its parent's sibling",
                vec![
                    Op::Spawn(
                        "outer",
                        vec![Op::Spawn("inner", vec![Op::Store(1)]), Op::Join(0)],
                    ),
                    Op::Spawn("other", vec![Op::Load(1)]),
                    Op::Join(0),
                    Op::Join(1),
                ],
            ),
            (
                "two resources, one contended",
                vec![
                    Op::Spawn("a", vec![write("x"), Op::Store(1)]),
                    Op::Spawn("b", vec![read("x"), Op::Store(2)]),
                    Op::Join(0),
                    Op::Join(1),
                ],
            ),
        ];

        for (name, ops) in programs {
            let plan = Plan {
                budget: NAIVE_BUDGET,
                ..Plan::default()
            };
            let mut pruned_model = Model::new(ops.clone());
            let pruned = explore_under(&plan, Dependence::Exact, &mut pruned_model);
            let mut whole_model = Model::new(ops);
            let whole = explore_under(&plan, Dependence::All, &mut whole_model);
            assert!(
                pruned.exploration.exhaustive && whole.exploration.exhaustive,
                "{name}: both searches must reach their frontier for the comparison to mean \
                 anything"
            );
            let seen: BTreeSet<&Vec<(u32, i64)>> = pruned_model.outcomes.iter().collect();
            let all: BTreeSet<&Vec<(u32, i64)>> = whole_model.outcomes.iter().collect();
            assert_eq!(
                seen, all,
                "{name}: pruning hid an outcome ({} interleavings against {})",
                pruned.exploration.explored, whole.exploration.explored
            );
            assert!(
                pruned.exploration.explored <= whole.exploration.explored,
                "{name}: pruning must not cost more than not pruning"
            );
        }
    }

    /// Required test 20, at the granularity the relation actually uses. A
    /// read/read pair is one interleaving exactly; a read/write pair is both
    /// orders, and costs at most one run more than the two equivalence classes
    /// it covers — the extra one is the branch that has to run `main` far enough
    /// to have spawned the second task at all.
    #[test]
    fn a_read_read_pair_is_one_interleaving_and_a_read_write_pair_is_both_orders() {
        let mut readers = Model::new(vec![
            Op::Spawn("a", vec![read("x")]),
            Op::Spawn("b", vec![read("x")]),
            Op::Join(0),
            Op::Join(1),
        ]);
        assert_eq!(explore(&dpor(64), &mut readers).exploration.explored, 1);

        let mut writer = Model::new(vec![
            Op::Spawn("a", vec![read("x")]),
            Op::Spawn("b", vec![write("x")]),
            Op::Join(0),
            Op::Join(1),
        ]);
        let explored = explore(&dpor(64), &mut writer);
        assert!((2..=3).contains(&explored.exploration.explored));
        assert!(explored.exploration.exhaustive);
        assert!(both_orders(&writer, TaskId(1), TaskId(2)));
    }

    /// The relation is at cell granularity, so two cells that would share one
    /// `[r]` label are two locations and do not contend.
    #[test]
    fn two_cells_under_one_label_do_not_contend() {
        let mut model = Model::new(vec![
            Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
            Op::Spawn("b", vec![Op::Load(2), Op::Store(2)]),
            Op::Join(0),
            Op::Join(1),
        ]);
        assert_eq!(explore(&dpor(64), &mut model).exploration.explored, 1);
    }

    /// Tasks that appear part way through a run: the enabled set grows, so a
    /// choice index at one scheduling point means something different from the
    /// same index at another, and a backtrack point is only meaningful against
    /// the enabled set that was recorded with it.
    #[test]
    fn nested_spawns_are_explored() {
        let mut model = Model::new(vec![
            Op::Spawn(
                "outer",
                vec![
                    Op::Spawn("inner", vec![Op::Load(1), Op::Store(1)]),
                    Op::Join(0),
                ],
            ),
            Op::Spawn("other", vec![Op::Load(1), Op::Store(1)]),
            Op::Join(0),
            Op::Join(1),
        ])
        .expecting(&[(1, 2)]);

        let explored = explore(&dpor(256), &mut model);
        assert!(
            !explored.passed(),
            "the grandchild races the sibling and the search must reach it"
        );
        assert!(explored.exploration.race.is_some());
        // @3 is the grandchild: spawned by @1, so it exists in no enabled set
        // until @1 has run twice.
        let race = explored.exploration.race.expect("a race");
        assert!(
            [race.left.task, race.right.task].contains(&TaskId(3)),
            "the race names the nested task, not its parent"
        );
    }

    /// A passing nested program is still enumerated to a frontier rather than
    /// sampled: exhaustiveness is the headline, and it must survive tasks that
    /// did not exist when the search started.
    #[test]
    fn a_nested_spawn_that_conflicts_with_nothing_is_one_interleaving() {
        let mut model = Model::new(vec![
            Op::Spawn(
                "outer",
                vec![Op::Spawn("inner", vec![Op::Store(2)]), Op::Join(0)],
            ),
            Op::Spawn("other", vec![Op::Store(3)]),
            Op::Join(0),
            Op::Join(1),
        ])
        .expecting(&[(2, 1), (3, 1)]);

        let explored = explore(&dpor(64), &mut model);
        assert!(explored.passed());
        assert_eq!(explored.exploration.explored, 1);
        assert!(explored.exploration.exhaustive);
    }

    /// The soundness case ADR 0006 §6.2's pseudocode drops. `late` is blocked in
    /// a `join` at the scheduling point where `main` loads the counter, so
    /// `task(sᵢ) ∈ Eⱼ` is false and the literal rule adds no backtrack point at
    /// all — leaving the lost update between `main` and `late` unreachable. The
    /// else branch resumes the task that unblocks it instead, and the race is
    /// found.
    #[test]
    fn a_race_with_a_task_that_was_blocked_at_the_backtrack_point_is_still_found() {
        for root in 0..8u64 {
            let mut model = Model::new(vec![
                Op::Spawn(
                    "late",
                    vec![
                        Op::Spawn("barrier", vec![Op::Yield]),
                        Op::Join(0),
                        Op::Load(1),
                        Op::Store(1),
                    ],
                ),
                Op::Load(1),
                Op::Store(1),
                Op::Join(0),
            ])
            .expecting(&[(1, 2)]);
            let plan = Plan {
                roots: vec![root],
                budget: 512,
                ..Plan::default()
            };
            let explored = explore(&plan, &mut model);
            assert!(
                !explored.passed(),
                "root {root} missed the lost update behind a join",
            );
        }
    }

    /// The same case, as a unit of the rule rather than of the search: when the
    /// racing task is not enabled at the backtrack point, the alternatives that
    /// could unblock it are queued.
    #[test]
    fn the_backtrack_rule_queues_alternatives_when_the_racer_is_not_enabled() {
        let cell = |id: u32, mode: Mode| {
            StepFootprint::from_accesses([Access::Cell {
                id: CellId(id),
                mode,
            }])
        };
        let step = |task: u32, enabled: &[u32], choice: u16, accesses: StepFootprint| Step {
            region: SimId(0),
            task: TaskId(task),
            enabled: enabled.iter().map(|&t| TaskId(t)).collect(),
            choice,
            accesses,
            definition: None,
            span: Span::DUMMY,
            stamp: Stamp::new(),
        };
        // @2 is blocked at point 0 and writes the same cell at point 2; only @1
        // running at point 0 can ever unblock it.
        let blocked = vec![
            step(0, &[0, 1], 0, cell(1, Mode::Write)),
            step(1, &[0, 1], 1, StepFootprint::new()),
            step(2, &[0, 1, 2], 2, cell(1, Mode::Write)),
        ];
        let out = backtracks(&blocked, Dependence::Exact);
        let at_zero = out.get(&0).expect("a backtrack point at 0");
        assert_eq!(
            at_zero.get(&TaskId(1)),
            Some(&None),
            "the alternative that can unblock @2 is queued, and names no race because \
             none was observed"
        );
        assert!(
            !at_zero.contains_key(&TaskId(2)),
            "@2 could not have run at point 0, so scheduling it there is not a schedule"
        );

        // Where the racer *is* enabled, the pair is named exactly, and that is
        // the pair the failure artifact prints.
        let enabled = vec![
            step(0, &[0, 1], 0, cell(1, Mode::Write)),
            step(1, &[0, 1], 1, cell(1, Mode::Write)),
        ];
        let direct = backtracks(&enabled, Dependence::Exact);
        assert_eq!(
            direct.get(&0).and_then(|m| m.get(&TaskId(1))),
            Some(&Some(1))
        );
    }

    /// The shape of nearly every concurrent test there is: spawn, join, then
    /// assert on what the children wrote. Those reads conflict with the
    /// children's writes and can never be reordered against them, because the
    /// join already ordered them — so the whole assertion must cost the search
    /// nothing.
    ///
    /// Without the clocks it costs a great deal. This is the regression guard
    /// for the measured gap: two disjoint tasks are one interleaving with them
    /// and twenty-nine without, and the number grows with the task count.
    #[test]
    fn asserting_on_what_a_joined_task_wrote_costs_no_interleavings() {
        let program = || {
            Model::new(vec![
                Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
                Op::Spawn("b", vec![Op::Load(2), Op::Store(2)]),
                Op::Join(0),
                Op::Join(1),
                // The assertions: the parent reads both cells after joining.
                Op::Load(1),
                Op::Load(2),
            ])
            .expecting(&[(1, 1), (2, 1)])
        };

        let mut model = program();
        let explored = explore(&dpor(NAIVE_BUDGET), &mut model);
        assert!(explored.passed());
        assert_eq!(
            explored.exploration.explored, 1,
            "a read the join already ordered is not a race"
        );
        assert!(explored.exploration.exhaustive);

        // The same program with the recording's clocks withheld, which is what
        // this search did before it read them.
        struct Blind(Model);
        impl Simulation for Blind {
            fn run(&mut self, seed: &Seed) -> Interleaving {
                let mut run = self.0.run(seed);
                for step in &mut run.steps {
                    step.stamp.clear();
                }
                run
            }
        }
        let mut blind = Blind(program());
        let unsynchronized = explore(&dpor(NAIVE_BUDGET), &mut blind);
        assert!(unsynchronized.passed());
        // Pinned rather than bounded: this number moving is the news. On the
        // real machine, where a task is several steps rather than two, the same
        // shape costs 29 interleavings at two tasks and 992 at five.
        assert_eq!(unsynchronized.exploration.explored, 6);
    }

    /// A stamp orders two steps only when the later task really had observed the
    /// earlier one. The empty case is the one that matters most: a recording
    /// with no clocks must order nothing, or a search over it prunes reorderings
    /// that are perfectly reachable.
    #[test]
    fn an_absent_clock_orders_nothing_and_a_present_one_orders_what_it_saw() {
        assert!(!happens_before(&Stamp::new(), TaskId(0), &vec![3, 1]));
        assert!(!happens_before(&vec![3, 1], TaskId(0), &Stamp::new()));
        // @0 has taken three steps; @1 has seen all three.
        assert!(happens_before(&vec![3, 0], TaskId(0), &vec![3, 1]));
        // ...and not when it has only seen two of them.
        assert!(!happens_before(&vec![3, 0], TaskId(0), &vec![2, 1]));
        // A task that has taken no step of its own orders nothing by it.
        assert!(!happens_before(&vec![0, 2], TaskId(0), &vec![0, 5]));
        // A shorter clock is read as zeroes rather than as agreement.
        assert!(!happens_before(&vec![0, 0, 2], TaskId(2), &vec![1, 1]));
    }

    /// Enabledness carries synchronization, not the dependence relation. A join
    /// keeps a task out of the enabled set, so no schedule that runs it early is
    /// ever *generated* — as opposed to generated and then pruned, which is what
    /// encoding the join as a conflict would produce.
    #[test]
    fn a_joined_task_is_never_scheduled_before_its_target() {
        let mut model = Model::new(vec![
            Op::Spawn("producer", vec![Op::Store(1)]),
            Op::Join(0),
            Op::Spawn("consumer", vec![Op::Load(1)]),
            Op::Join(1),
        ]);
        let explored = explore(&dpor(64), &mut model);
        assert!(explored.passed());
        assert!(
            !both_orders(&model, TaskId(1), TaskId(2)),
            "the join orders the write before the read in every schedule: {:?}",
            model.traces
        );
    }

    /// Budgets bound the search, and a search that did not empty its frontier
    /// says so — an exhausted run proved nothing about the interleavings it did
    /// not reach, and `Exploration::is_cacheable` is what acts on that.
    #[test]
    fn a_spent_budget_is_exhausted_and_not_exhaustive() {
        let mut model = Model::new(vec![
            Op::Spawn("a", vec![write("x"), write("x"), write("x")]),
            Op::Spawn("b", vec![write("x"), write("x"), write("x")]),
            Op::Spawn("c", vec![write("x"), write("x"), write("x")]),
            Op::Join(0),
            Op::Join(1),
            Op::Join(2),
        ]);
        let explored = explore(&dpor(4), &mut model);
        assert_eq!(explored.exploration.explored, 4);
        assert!(explored.exploration.exhausted);
        assert!(!explored.exploration.exhaustive);
        assert!(!explored.exploration.is_cacheable());
    }

    /// The search is itself a function of the seed: two runs of one plan visit
    /// the same interleavings in the same order. Without this the reduction
    /// number is not reproducible and neither is the failure it reports.
    #[test]
    fn the_search_is_deterministic() {
        let program = || {
            Model::new(vec![
                Op::Spawn("a", vec![Op::Load(1), Op::Store(1), write("x")]),
                Op::Spawn("b", vec![Op::Load(1), Op::Store(1), read("x")]),
                Op::Join(0),
                Op::Join(1),
            ])
        };
        let mut first = program();
        let mut second = program();
        let a = explore(&dpor(64), &mut first);
        let b = explore(&dpor(64), &mut second);
        assert_eq!(a.seeds, b.seeds);
        assert_eq!(a.exploration.explored, b.exploration.explored);
        assert_eq!(a.exploration.failure, b.exploration.failure);
        assert_eq!(first.runs, second.runs);
    }

    /// Every interleaving the search runs is a distinct seed. Re-running one
    /// would be exactly the redundant work the reduction exists to delete.
    #[test]
    fn no_interleaving_is_run_twice() {
        let mut model = Model::new(vec![
            Op::Spawn("a", vec![write("x"), write("y")]),
            Op::Spawn("b", vec![write("x"), write("y")]),
            Op::Spawn("c", vec![write("y")]),
            Op::Join(0),
            Op::Join(1),
            Op::Join(2),
        ]);
        let explored = explore(&dpor(256), &mut model);
        let unique: BTreeSet<&Seed> = explored.seeds.iter().collect();
        assert_eq!(unique.len(), explored.seeds.len());
        assert_eq!(explored.seeds.len(), explored.exploration.explored as usize);
    }

    /// A scheduler whose replay does not reproduce the recorded enabled set is
    /// Ply's fault, and it is caught rather than silently searched over.
    #[test]
    fn a_replay_that_does_not_reproduce_the_enabled_set_is_a_divergence() {
        let mut calls = 0u32;
        let mut driver = |seed: &Seed| {
            calls += 1;
            let enabled = if seed.is_root() {
                vec![TaskId(0), TaskId(1)]
            } else {
                // The replay offers a different enabled set at the point the
                // seed names, which makes the choice mean something else.
                vec![TaskId(0), TaskId(1), TaskId(2)]
            };
            let write = |resource| {
                StepFootprint::from_accesses([Access::Atom(EffectAtom::new(
                    "db",
                    Resource::Named(Symbol::new(resource)),
                    Mode::Write,
                ))])
            };
            let step = |task: u32, choice: u16, accesses| Step {
                region: SimId(0),
                task: TaskId(task),
                enabled: enabled.clone(),
                choice,
                accesses,
                definition: None,
                span: Span::DUMMY,
                stamp: Stamp::new(),
            };
            Interleaving::passed(vec![
                step(
                    seed.choice(0).unwrap_or(0) as u32,
                    seed.choice(0).unwrap_or(0),
                    write("x"),
                ),
                step(1, 1, write("x")),
            ])
        };
        let explored = explore(&dpor(8), &mut driver);
        let diagnostic = explored.diagnostic.expect("a divergence");
        assert_eq!(diagnostic.code, codes::SIMULATION_DIVERGENCE);
        assert!(explored.exploration.failure.is_some());
    }

    /// A recording that does not describe a schedule is Ply's fault too, and it
    /// is refused before the search draws conclusions from it.
    #[test]
    fn a_step_that_its_enabled_set_does_not_offer_is_an_internal_error() {
        let mut driver = |_: &Seed| {
            Interleaving::passed(vec![Step {
                region: SimId(0),
                task: TaskId(7),
                enabled: vec![TaskId(0), TaskId(1)],
                choice: 0,
                accesses: StepFootprint::new(),
                definition: None,
                span: Span::DUMMY,
                stamp: Stamp::new(),
            }])
        };
        let explored = explore(&dpor(8), &mut driver);
        assert_eq!(
            explored.diagnostic.expect("a defect").code,
            codes::INTERNAL_ERROR
        );
    }

    /// `once` is the replay path: exactly the interleaving the seed names, no
    /// search, and no claim of exhaustiveness from a sample of one.
    #[test]
    fn once_runs_exactly_the_interleaving_its_seed_names() {
        let mut model = Model::new(vec![
            Op::Spawn("a", vec![write("x")]),
            Op::Spawn("b", vec![write("x")]),
            Op::Join(0),
            Op::Join(1),
        ]);
        let seed = Seed::at(3, vec![0, 0, 1]);
        let explored = explore(&Plan::once(seed.clone()), &mut model);
        assert_eq!(explored.seeds, vec![seed]);
        assert_eq!(explored.exploration.explored, 1);
        assert!(!explored.exploration.exhaustive);
        assert!(!explored.exploration.exhausted);
        assert!(explored.exploration.is_cacheable());
    }

    /// `random` is one interleaving per root and no state between them.
    #[test]
    fn random_runs_one_interleaving_per_root() {
        let mut model = Model::new(vec![
            Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
            Op::Spawn("b", vec![Op::Load(1), Op::Store(1)]),
            Op::Join(0),
            Op::Join(1),
        ])
        .expecting(&[(1, 2)]);
        let explored = explore(&Plan::random(16), &mut model);
        assert!(explored.exploration.explored <= 16);
        assert!(!explored.exploration.exhaustive);
        // A sample that happens to find the race reports no race pair, because
        // nothing flipped: there was no earlier passing interleaving to flip.
        assert_eq!(explored.exploration.race, None);
    }

    /// A pruned search that passes where the unpruned one fails means the
    /// relation missed an access. The measurement is where that shows up, and
    /// it must report the failure rather than a flattering ratio.
    #[test]
    fn a_failure_only_the_unpruned_search_reaches_is_reported() {
        // A driver that lies: every step reports an empty footprint, so the
        // exact relation prunes everything, while the program's outcome really
        // does depend on the order.
        struct Liar(Model);
        impl Simulation for Liar {
            fn run(&mut self, seed: &Seed) -> Interleaving {
                let mut run = self.0.run(seed);
                for step in &mut run.steps {
                    step.accesses = StepFootprint::new();
                }
                run
            }
        }
        let mut liar = Liar(
            Model::new(vec![
                Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
                Op::Spawn("b", vec![Op::Load(1), Op::Store(1)]),
                Op::Join(0),
                Op::Join(1),
            ])
            .expecting(&[(1, 2)]),
        );
        let honest = explore(&dpor(256), &mut liar);
        assert!(honest.passed(), "an empty footprint prunes everything");

        let mut liar = Liar(
            Model::new(vec![
                Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
                Op::Spawn("b", vec![Op::Load(1), Op::Store(1)]),
                Op::Join(0),
                Op::Join(1),
            ])
            .expecting(&[(1, 2)]),
        );
        let measured = measure_reduction(&dpor(256), &mut liar);
        assert!(!measured.passed(), "the unpruned search reaches it");
        let diagnostic = measured.diagnostic.expect("a diagnostic");
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|n| n.contains("missing an access")),
            "the report must say the relation was wrong, not that the program is fine"
        );
    }

    /// A rule about how a type is *used* is a rule nobody enforces; a rule about
    /// which types may be *named* is greppable. Which interleaving runs next is
    /// as much a part of a seeded run as which task runs next.
    #[test]
    fn this_module_names_no_hash_based_collection_and_reads_no_clock() {
        let source = include_str!("explore.rs");
        let body = source
            .split_once("mod tests {")
            .map(|(body, _)| body)
            .unwrap_or(source);
        for banned in [
            "HashMap",
            "HashSet",
            "FxHashMap",
            "FxHashSet",
            "SystemTime",
            "Instant",
            "thread::",
            "rayon",
            "as_ptr",
            "strong_count",
        ] {
            assert!(
                !body.contains(banned),
                "`{banned}` appears in ply_eval::explore; which interleaving runs next must be a \
                 function of the definitions and the seed and nothing else"
            );
        }
    }
}

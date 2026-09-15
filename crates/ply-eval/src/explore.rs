//! Footprint-guided interleaving exploration.

use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::cont::SimId;
use crate::sched::{Stamp, StepRecord, happens_before};
use crate::sim::{Exploration, Naive, Plan, Race, RaceSite, Seed, SimMode, StepFootprint, TaskId};

/// One step of one task, as the search reads it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step {
    /// Which of the entry point's `simulate` regions took this step.
    pub region: SimId,
    pub task: TaskId,
    /// Every task the scheduler could have resumed at this point, in its canonical order.
    pub enabled: Vec<TaskId>,
    /// The index into `enabled` that was taken; `enabled[choice] == task`.
    pub choice: u16,
    /// What the step touched, **excluding** the terminating `task.*` / `clock.*` atom and
    /// **including** every cell and every `random.write`.
    pub accesses: StepFootprint,
    /// The definition the step was inside.
    pub definition: Option<Symbol>,
    pub span: Span,
    /// The acting task's vector clock, which says which earlier steps this one had already
    /// observed.
    pub stamp: Stamp,
}

impl Step {
    /// Adopt the scheduler's record of a step.
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

/// How one interleaving ended.
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

    /// The choice sequence actually taken, which is not the seed's path: beyond the path the
    /// `sched` stream chose, and a backtrack point is named relative to what ran rather than to
    /// what was fixed.
    fn choices(&self) -> Vec<u16> {
        self.steps.iter().map(|s| s.choice).collect()
    }
}

/// Whole-test replay at one seed.
pub trait Simulation {
    fn run(&mut self, seed: &Seed) -> Interleaving;
}

impl<F: FnMut(&Seed) -> Interleaving> Simulation for F {
    fn run(&mut self, seed: &Seed) -> Interleaving {
        self(seed)
    }
}

/// Which dependence relation the search runs over.
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

/// The budget the naive search gets under [`measure_reduction`].
pub const NAIVE_BUDGET: u32 = 4096;

/// What a search produced.
#[derive(Clone, Debug)]
pub struct Explored {
    pub exploration: Exploration,
    /// The failing interleaving's diagnostic.
    pub diagnostic: Option<Diagnostic>,
    /// The interleavings run, in the order they were run.
    pub seeds: Vec<Seed>,
}

impl Explored {
    pub fn passed(&self) -> bool {
        self.exploration.failure.is_none()
    }
}

/// Run `plan` against `driver`.
pub fn explore(plan: &Plan, driver: &mut dyn Simulation) -> Explored {
    explore_under(plan, Dependence::Exact, driver)
}

/// [`explore`] with the dependence relation chosen explicitly.
pub fn explore_under(plan: &Plan, dependence: Dependence, driver: &mut dyn Simulation) -> Explored {
    let plan = plan.clone().normalized();
    match plan.mode {
        SimMode::Once | SimMode::Random => sample(&plan, driver),
        SimMode::Dpor => search(&plan, dependence, driver),
    }
}

/// [`explore`], then the same search again with the dependence relation forced to `true`, filling
/// [`Exploration::naive`].
pub fn measure_reduction(plan: &Plan, driver: &mut dyn Simulation) -> Explored {
    let mut explored = explore(plan, driver);
    if plan.mode != SimMode::Dpor || explored.exploration.failure.is_some() {
        // Nothing was pruned, or both searches stopped at a failure rather than at their frontier.
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
        // The pruned search called this program green and the unpruned one failed it.
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

/// Where a queued interleaving came from: the run it branched off, the scheduling point it diverges
/// at, and — when the branch was taken to reverse a specific pair of steps — which pair.
struct Branch {
    trace: Rc<Vec<Step>>,
    at: usize,
    choice: u16,
    /// The step the search meant to reorder against `trace[at]`.
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
    // Every (prefix, choice) this search has run or queued.
    let mut claimed: BTreeMap<Vec<u16>, BTreeSet<u16>> = BTreeMap::new();

    while let Some(work) = frontier.pop() {
        if report.explored >= budget {
            // The frontier is not empty, so the interleavings still on it were never run and
            // nothing may be claimed about them.
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

/// The backtrack points a completed interleaving reveals: at scheduling point `at`, each task worth
/// resuming instead, and the step the search means to reorder against `trace[at]`.
pub fn backtracks(
    steps: &[Step],
    dependence: Dependence,
) -> BTreeMap<usize, BTreeMap<TaskId, Option<usize>>> {
    let mut out: BTreeMap<usize, BTreeMap<TaskId, Option<usize>>> = BTreeMap::new();
    for i in (1..steps.len()).rev() {
        let later = &steps[i];
        for j in (0..i).rev() {
            let earlier = &steps[j];
            if earlier.region != later.region {
                // Two regions of one entry point run in sequence, never interleaved, so no schedule
                // puts these two in the other order — and a task id means a different task in each
                // of them.
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

/// A task queued twice at one point keeps whichever pair was observed, so a conservative branch
/// never erases the race a real one names.
fn record(at: &mut BTreeMap<TaskId, Option<usize>>, task: TaskId, against: Option<usize>) {
    let entry = at.entry(task).or_insert(None);
    if entry.is_none() {
        *entry = against;
    }
}

/// `check_recording` has already refused an enabled set too large to index, so the conversion
/// cannot silently drop a backtrack point.
fn choice_of(enabled: &[TaskId], task: TaskId) -> Option<u16> {
    let index = enabled.iter().position(|&t| t == task)?;
    u16::try_from(index).ok()
}

/// The two steps whose reordering the failing branch was queued to perform.
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

/// A recording that does not describe a schedule is Ply's fault, and it is caught before the search
/// reasons over it: every later conclusion — which interleavings exist, which were pruned, whether
/// the search was exhaustive — is derived from these fields.
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

/// Replay is self-checking: re-running a prefix must reproduce the enabled set at every scheduling
/// point that prefix names.
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

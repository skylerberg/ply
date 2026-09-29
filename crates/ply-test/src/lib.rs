//! Scheduling and running the tests the program chose, and filing each result under the keys it
//! handed over.

pub mod bisect;
pub mod hybrid;
pub mod obligation;
pub mod region;
pub mod report;
pub mod schedule;
pub mod sim;
pub mod slice;

use ply_eval::explore::{Interleaving, explore, measure_reduction};
use ply_eval::host::{HostBinding, HostRuntime};
use ply_eval::{Arena, Exploration, Machine, Plan, Race, Seed, TaskRegions, Value};
use ply_span::{Diagnostic, Symbol, codes};
use ply_store::{Outcome, PassRecord, Store};
use ply_ty::{CheckOutput, Footprint};
use ply_ty::{DefHash, HashOutput};
use serde::Serialize;
use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub use bisect::{
    Baseline, Bisection, ChangeSet, Classify, Confidence, DefKey, Ns, Regression, Rehashed, Row,
    SearchStats, Skipped, StoreClassify, Trial, TrialOutcome, Unresolved, Verdict, change_set,
};
pub use hybrid::{BodyHybrid, Mixture, Signature};
pub use region::GroupRegion;
pub use schedule::{
    AMBIENT, Isolation, Parallelism, REGION_SCOPED, SIM_EFFECT, SIMULATED, contends,
    contends_only_over_regions, group_by_conflict, is_ambient, is_region_scoped, is_seeded,
    parallelism, region_isolated, shared_footprint,
};
pub use sim::{Record, SimSummary, record_under, replay_command};
pub use slice::{
    Assertion, AssertionKind, CausalSlice, Difference, Entered, Event, Frame, SliceBuilder, Tracing,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The hash is absent from the store: this exact test has never gone green.
    New,
    /// `test/nondet` opts out of the cache in both directions.
    Nondet,
    PreviousFailure,
    /// Present and green; re-running cannot reveal anything new.
    Cached,
    Unhashed,
}

impl Reason {
    pub fn runs(self) -> bool {
        !matches!(self, Reason::Cached)
    }

    /// The word a reason crosses as, and back: the program prints these and the runtime reads them.
    pub fn parse(word: &str) -> Option<Reason> {
        match word {
            "new" => Some(Reason::New),
            "nondet" => Some(Reason::Nondet),
            "previous failure" => Some(Reason::PreviousFailure),
            "cached" => Some(Reason::Cached),
            "unhashed" => Some(Reason::Unhashed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Reason::New => "new",
            Reason::Nondet => "nondet",
            Reason::PreviousFailure => "previous failure",
            Reason::Cached => "cached",
            Reason::Unhashed => "unhashed",
        }
    }
}

/// What a run decided, sent by the program that decided it. The runtime applies its own `--filter`
/// and `--std` to it, executes exactly what is left of `runs`, schedules by `groups`, and writes a
/// pass under the keys `filed` names for it.
#[derive(Clone, Debug, Default)]
pub struct Choice {
    /// Test indices to execute, ascending.
    pub runs: Vec<usize>,
    /// The reason for every test the keys row named, indexed by test index. The program decides for
    /// all of them, whether or not this run's filter will report on one.
    pub reasons: Vec<Reason>,
    /// The roots a seeded test still owes, when the cache already answered some of the plan's.
    pub narrowed: BTreeMap<usize, Vec<u64>>,
    /// Concurrency classes over `runs`, as the program coloured them.
    pub groups: Vec<Vec<usize>>,
    /// Every key each running test's pass is written under. A test with none is never written.
    pub filed: BTreeMap<usize, Vec<DefHash>>,
}

#[derive(Clone)]
pub struct Selection {
    pub total: usize,
    pub cached: Vec<(usize, Outcome)>,
    pub to_run: Vec<usize>,
    /// Concurrency groups over `to_run`; every pair within a group has non-conflicting footprints.
    pub groups: Vec<Vec<usize>>,
    /// Indexed by test index, length `total`.
    pub reasons: Vec<Reason>,
    /// Indexed by test index, length `total`.
    pub isolation: Vec<Isolation>,
    pub parallelism: Parallelism,
    /// The search this selection was made against; a seeded test's result is published under it.
    pub plan: Plan,
    /// What a seeded test still owes, when the cache already covers part of the plan.
    pub narrowed: BTreeMap<usize, Plan>,
    /// Every key each running test's pass is written under.
    pub filed: BTreeMap<usize, Vec<DefHash>>,
    /// Test indices this run was never asked to decide: a shipped module's tests without `--std`.
    pub out_of_scope: BTreeSet<usize>,
}

impl Selection {
    /// The runtime's view of what the program decided: the same fields `select` used to build, laid
    /// out from the choice. The evidence a cached test is reported with is always a pass — a stored
    /// failure is never `Cached` — so nothing here has to read the store again.
    pub fn chosen(
        choice: &Choice,
        check: &CheckOutput,
        visible: &[usize],
        plan: &Plan,
    ) -> Selection {
        let plan = plan.clone().normalized();
        let total = check.tests.len();
        let cached: Vec<(usize, Outcome)> = (0..total)
            .filter(|i| choice.reasons.get(*i) == Some(&Reason::Cached))
            .map(|i| (i, Outcome::Pass))
            .collect();
        let footprints: Vec<(usize, Footprint)> = choice
            .runs
            .iter()
            .filter_map(|&i| check.tests.get(i).map(|t| (i, t.footprint.clone())))
            .collect();
        let narrowed: BTreeMap<usize, Plan> = choice
            .narrowed
            .iter()
            .map(|(&index, roots)| {
                (
                    index,
                    Plan {
                        roots: roots.clone(),
                        ..plan.clone()
                    }
                    .normalized(),
                )
            })
            .collect();
        Selection {
            total,
            cached,
            to_run: choice.runs.clone(),
            groups: choice.groups.clone(),
            reasons: choice.reasons.clone(),
            isolation: check
                .tests
                .iter()
                .map(|t| Isolation::of(&t.footprint))
                .collect(),
            // Over the tests this run reports on, so every count shares one denominator.
            parallelism: parallelism(
                visible
                    .iter()
                    .filter_map(|&i| check.tests.get(i))
                    .map(|t| &t.footprint),
                &footprints,
                &choice.groups,
            ),
            plan,
            narrowed,
            filed: choice.filed.clone(),
            out_of_scope: BTreeSet::new(),
        }
    }

    /// The same selection over the tests a filter keeps. `--filter` cannot change which tests
    /// conflict, so a class only loses members; a cached result or a narrowed plan for a test the
    /// run does not report on goes with it.
    pub fn keep(&self, visible: &[usize]) -> Selection {
        let keeps = |i: &usize| visible.binary_search(i).is_ok();
        let mut out = self.clone();
        out.total = visible.len();
        out.cached.retain(|(i, _)| keeps(i));
        out.to_run.retain(keeps);
        out.groups = self
            .groups
            .iter()
            .map(|class| class.iter().copied().filter(keeps).collect::<Vec<usize>>())
            .filter(|class| !class.is_empty())
            .collect();
        out.narrowed.retain(|index, _| keeps(index));
        out.filed.retain(|index, _| keeps(index));
        out
    }

    pub fn reason(&self, index: usize) -> Option<Reason> {
        self.reasons.get(index).copied()
    }

    /// The run's plan, unless the cache already answered for some of this test's roots.
    pub fn plan_for(&self, index: usize) -> &Plan {
        self.narrowed.get(&index).unwrap_or(&self.plan)
    }

    pub fn isolation_of(&self, index: usize) -> Option<Isolation> {
        self.isolation.get(index).copied()
    }

    pub fn group_of(&self, index: usize) -> Option<usize> {
        self.groups.iter().position(|g| g.contains(&index))
    }

    pub fn is_empty(&self) -> bool {
        self.to_run.is_empty()
    }
}

/// Hand-written so that `Selection` stays printable no matter what `Outcome` derives.
impl fmt::Debug for Selection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cached: Vec<usize> = self.cached.iter().map(|(i, _)| *i).collect();
        f.debug_struct("Selection")
            .field("total", &self.total)
            .field("cached", &cached)
            .field("to_run", &self.to_run)
            .field("groups", &self.groups)
            .field("reasons", &self.reasons)
            .field("isolation", &self.isolation)
            .field("parallelism", &self.parallelism)
            .field("plan", &self.plan)
            .field("narrowed", &self.narrowed)
            .field("filed", &self.filed)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passed,
    Failed,
    /// Ply failed rather than the program: the evaluator unwound or reported a broken invariant.
    Panicked,
    /// The harness stopped the run at its wall clock. The test decided nothing, so nothing about
    /// it is recorded and the next run starts it again.
    Abandoned,
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub index: usize,
    pub name: String,
    pub hash: Option<DefHash>,
    pub group: usize,
    pub duration: Duration,
    pub status: Status,
    pub failure: Option<Diagnostic>,
    pub simulation: Option<Exploration>,
    /// Absent when nothing was written: a spent budget or an unobserved search proved nothing.
    pub recorded: Option<Record>,
    pub backend: Option<BackendUse>,
}

impl TestResult {
    pub fn passed(&self) -> bool {
        self.status == Status::Passed
    }

    pub fn green_but_uncached(&self) -> bool {
        self.passed() && matches!(self.recorded, Some(Record::Exhausted | Record::Unobserved))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suspect {
    pub name: Symbol,
    pub hash: Option<DefHash>,
    /// `None` when the failing execution was not traced.
    pub ran: Option<bool>,
    /// Distance above the failing frame, zero being where it happened.
    pub depth: Option<usize>,
    /// Whether bisection put it in the minimal failure-inducing set.
    pub culprit: bool,
}

impl Suspect {
    pub fn new(name: Symbol, hash: Option<DefHash>) -> Suspect {
        Suspect {
            name,
            hash,
            ran: None,
            depth: None,
            culprit: false,
        }
    }

    /// Most-likely-first: a bisected culprit, the stack innermost first, whatever else ran, then
    /// the name.
    fn rank(&self) -> (u8, usize, &str) {
        let tier = match (self.culprit, self.ran, self.depth) {
            (true, ..) => 0,
            (false, _, Some(_)) => 1,
            (false, Some(true), None) => 2,
            (false, None, None) => 3,
            (false, Some(false), None) => 4,
        };
        (tier, self.depth.unwrap_or(usize::MAX), self.name.as_str())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Attribution {
    /// The same set as [`Failure::suspects`], ranked and annotated.
    pub suspects: Vec<Suspect>,
    pub bisection: Bisection,
    /// `None` until a traced re-run has happened.
    pub slice: Option<CausalSlice>,
}

impl Attribution {
    pub fn from_suspects(names: &[Symbol], hashes: &HashOutput) -> Attribution {
        let mut suspects: Vec<Suspect> = names
            .iter()
            .map(|name| {
                let hash = hashes
                    .defs
                    .get(name)
                    .or_else(|| hashes.decls.get(name))
                    .copied();
                Suspect::new(name.clone(), hash)
            })
            .collect();
        suspects.sort_by(|a, b| a.rank().cmp(&b.rank()));
        Attribution {
            suspects,
            bisection: Bisection::default(),
            slice: None,
        }
    }

    /// Folds a bisection and a trace into the suspects, then re-ranks.
    pub fn resolve(&mut self, bisection: Bisection, slice: Option<CausalSlice>) {
        let culprits = bisection.culprits();
        for suspect in &mut self.suspects {
            suspect.culprit = culprits.contains(&suspect.name);
            if let Some(slice) = &slice
                && slice.traced
                && slice.reproduced
            {
                suspect.ran = slice.did_run(&suspect.name);
                suspect.depth = slice.depth_of(&suspect.name);
            }
        }
        // A culprit outside the suspect set is still the answer: a cause need not look changed.
        for name in culprits {
            if !self.suspects.iter().any(|s| s.name == name) {
                let mut extra = Suspect::new(name, None);
                extra.culprit = true;
                self.suspects.push(extra);
            }
        }
        self.suspects.sort_by(|a, b| a.rank().cmp(&b.rank()));
        self.bisection = bisection;
        self.slice = slice;
    }

    pub fn culprits(&self) -> Vec<Symbol> {
        self.bisection.culprits()
    }
}

#[derive(Clone, Debug)]
pub struct Failure {
    /// The label as the source wrote it.
    pub name: String,
    /// `<module>.<label>`, and the key this failure's closure is looked up by.
    pub key: Symbol,
    pub diagnostic: Diagnostic,
    /// Ply's fault rather than the program's: nothing in the definition graph to attribute.
    pub defect: bool,
    /// This failing run reached a host handler, so re-running it acts on the world again.
    pub host: bool,
    /// Definitions in this test's closure whose hash is not in the store.
    pub suspects: Vec<Symbol>,
    pub assertion: Option<Assertion>,
    pub attribution: Attribution,
    pub seed: Option<Seed>,
    /// The two steps whose reordering flipped a passing interleaving to this one.
    pub race: Option<Race>,
}

impl Failure {
    pub fn replay(&self) -> Option<String> {
        Some(replay_command(self.seed.as_ref()?, &self.name))
    }
}

#[derive(Clone, Debug)]
pub struct RunReport {
    pub passed: usize,
    pub failed: usize,
    /// Tests the wall clock stopped. They are not failures, but the run did not decide them, so
    /// it is not a success either.
    pub abandoned: usize,
    pub cached: usize,
    pub failures: Vec<Failure>,
    pub duration: Duration,
    pub parallelism: Parallelism,
    /// Every test that actually ran, in execution order.
    pub results: Vec<TestResult>,
    /// Problems with the run itself rather than with any test.
    pub warnings: Vec<Diagnostic>,
    pub simulation: SimSummary,
}

impl RunReport {
    pub fn is_success(&self) -> bool {
        self.failed == 0 && self.abandoned == 0
    }
}

pub trait Executor: Sync {
    type Worker;

    fn worker(&self) -> Self::Worker;

    fn execute(&self, worker: &mut Self::Worker, index: usize) -> Result<(), Diagnostic>;

    /// What the search the last [`Executor::execute`] performed did.
    fn exploration(&self, _worker: &Self::Worker) -> Option<Exploration> {
        None
    }

    fn host_use(&self, _worker: &Self::Worker) -> Option<ply_eval::host::HostUse> {
        None
    }

    fn backend_use(&self, _worker: &Self::Worker) -> Option<BackendUse> {
        None
    }

    /// What the host runtime reported while closing the entry point; forgotten once read.
    fn teardown(&self, _worker: &mut Self::Worker) -> Vec<Diagnostic> {
        Vec::new()
    }
}

/// The search each test runs, and whether to measure what an unpruned one would have cost.
#[derive(Clone, Debug, Default)]
pub struct Search {
    pub plan: Plan,
    pub narrowed: BTreeMap<usize, Plan>,
    /// Re-run the search with dependence forced to `true`, to measure the reduction.
    pub measure_reduction: bool,
}

impl Search {
    pub fn of(selection: &Selection) -> Search {
        Search {
            plan: selection.plan.clone(),
            narrowed: selection.narrowed.clone(),
            measure_reduction: false,
        }
    }

    pub fn measuring(mut self, measure: bool) -> Search {
        self.measure_reduction = measure;
        self
    }

    pub fn plan_for(&self, index: usize) -> &Plan {
        self.narrowed.get(&index).unwrap_or(&self.plan)
    }
}

#[derive(Default)]
pub struct Hosting<'a> {
    binding: Option<Arc<HostBinding>>,
    /// A factory: a runtime handle belongs to one thread, and the runner has a machine per worker.
    runtime: Option<&'a (dyn Fn() -> Rc<dyn HostRuntime> + Sync)>,
}

impl<'a> Hosting<'a> {
    pub fn hermetic() -> Hosting<'a> {
        Hosting::default()
    }

    pub fn with_binding(mut self, binding: Arc<HostBinding>) -> Hosting<'a> {
        self.binding = Some(binding);
        self
    }

    /// What a [`ply_eval::host::HostAnswer::Pending`] is polled on.
    pub fn with_runtime(
        mut self,
        runtime: &'a (dyn Fn() -> Rc<dyn HostRuntime> + Sync),
    ) -> Hosting<'a> {
        self.runtime = Some(runtime);
        self
    }
}

pub struct InterpExecutor<'a> {
    front: &'a ply_ty::Front,
    fixture: Option<&'a (dyn Fn(&mut TaskRegions) -> Value + Sync)>,
    hosts: Hosting<'a>,
    /// The backend this run installs.
    backend: Option<&'static dyn ply_eval::Provider>,
    search: Search,
}

pub struct Worker<'a> {
    pub machine: Box<Machine<'a>>,
    exploration: Option<Exploration>,
    host: Option<ply_eval::host::HostUse>,
    /// The region this worker's tests run in, built once and mutated in place.
    region: GroupRegion,
    /// Built once and installed on every machine the worker builds, per-interleaving ones too.
    backend: Option<Rc<dyn ply_eval::Compiled>>,
    backend_use: Option<BackendUse>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackendUse {
    /// Bodies this test ran natively instead of evaluating.
    pub entries: u64,
    /// Calls the backend was offered and declined.
    pub declines: u64,
}

impl<'a> Worker<'a> {
    pub fn new(machine: Box<Machine<'a>>) -> Worker<'a> {
        Worker {
            machine,
            exploration: None,
            host: None,
            region: GroupRegion::empty(),
            backend: None,
            backend_use: None,
        }
    }

    fn backed(&self) -> Option<&Machine<'a>> {
        match (&self.machine, self.backend.is_some()) {
            (m, true) => Some(m.as_ref()),
            _ => None,
        }
    }

    pub fn in_region(machine: Box<Machine<'a>>, region: GroupRegion) -> Worker<'a> {
        Worker {
            region,
            ..Worker::new(machine)
        }
    }

    /// The fixture, plus every write the tests run so far made to it.
    pub fn region(&self) -> &GroupRegion {
        &self.region
    }

    fn open_region(&mut self) {
        if self.region.is_empty() {
            return;
        }
        self.machine.set_regions(self.region.open().0);
    }

    /// Returns the test's own slots to the bump pointer and carries its fixture writes here.
    fn close_region(&mut self) {
        if self.region.is_empty() {
            return;
        }
        let m = &self.machine;
        self.region.close(m.cells());
    }

    /// The cells of the machine whose verdict is reported.
    pub fn cells_mut(&mut self) -> &mut Arena {
        let m = &mut self.machine;
        m.cells_mut()
    }

    pub fn cells(&self) -> &Arena {
        let m = &self.machine;
        m.cells()
    }
}

impl<'a> InterpExecutor<'a> {
    pub fn new(front: &'a ply_ty::Front) -> InterpExecutor<'a> {
        InterpExecutor {
            front,
            fixture: None,
            hosts: Hosting::hermetic(),
            backend: None,
            search: Search::default(),
        }
    }

    pub fn with_fixture(mut self, fixture: &'a (dyn Fn(&mut TaskRegions) -> Value + Sync)) -> Self {
        self.fixture = Some(fixture);
        self
    }

    pub fn with_backend(mut self, provider: &'static dyn ply_eval::Provider) -> Self {
        self.backend = Some(provider);
        self
    }

    pub fn with_hosts(mut self, hosts: Hosting<'a>) -> Self {
        self.hosts = hosts;
        self
    }

    pub fn with_search(mut self, search: Search) -> Self {
        self.search = search;
        self
    }

    /// The region this worker's group runs in, built on the worker's own thread.
    fn build_region(&self) -> GroupRegion {
        match self.fixture {
            Some(build) => GroupRegion::build(build),
            None => GroupRegion::empty(),
        }
    }

    fn backend(&self) -> Option<Rc<dyn ply_eval::Compiled>> {
        Some(self.backend?.attach())
    }

    fn machine(&self, backend: Option<Rc<dyn ply_eval::Compiled>>) -> Box<Machine<'a>> {
        let mut machine = Machine::new(self.front);
        if let Some(backend) = backend {
            machine.set_compiled(backend);
        }
        if let Some(binding) = &self.hosts.binding {
            machine.set_host_binding(Arc::clone(binding));
        }
        if let Some(runtime) = self.hosts.runtime {
            machine.set_host_runtime(runtime());
        }
        Box::new(machine)
    }

    /// States this entry point's footprint claim, so a host answer outside it is `E0427`.
    fn arm_footprint_check(&self, machine: &mut Machine<'a>, index: usize) {
        if let Some(test) = self.front.check.tests.get(index) {
            machine.set_declared_footprint(test.footprint.clone());
        }
    }

    /// Whether this test's outcome depends on a seed, and so is searched rather than run.
    fn searches(&self, index: usize) -> bool {
        self.front
            .check
            .tests
            .get(index)
            .is_some_and(|t| is_seeded(&t.footprint))
    }

    /// The whole test, once per interleaving, each opening the group's region as the test found it.
    #[allow(clippy::type_complexity)]
    fn search(
        &self,
        worker: &Worker<'a>,
        index: usize,
    ) -> (
        Result<(), Diagnostic>,
        Option<Exploration>,
        Option<ply_eval::host::HostUse>,
        Option<BackendUse>,
    ) {
        let plan = self.search.plan_for(index);
        // A search re-runs the whole test, so any host operation runs once per interleaving.
        let re_executed = plan.re_executes() || self.search.measure_reduction;
        let mut observed = true;
        // Every interleaving's, unioned.
        let mut host: Option<ply_eval::host::HostUse> = None;
        // Every interleaving's, summed.
        let mut used: Option<BackendUse> = None;
        let region = &worker.region;
        let backend = worker.backend.clone();
        let mut interleaving = |seed: &Seed| {
            let mut machine = self.machine(backend.clone());
            if !region.is_empty() {
                machine.set_regions(region.open().0);
            }
            self.arm_footprint_check(machine.as_mut(), index);
            machine.set_re_executed(re_executed);
            sim::seed_run(machine.as_mut(), seed, plan.steps);
            let outcome = machine.eval_test(index);
            if let Some(reached) = machine.host_use() {
                let into = host.get_or_insert_with(Default::default);
                into.atoms = into.atoms.union(&reached.atoms);
                into.operations = into.operations.saturating_add(reached.operations);
            }
            if backend.is_some() {
                let (entries, declines) = machine.compiled_counts();
                let into = used.get_or_insert_with(Default::default);
                into.entries = into.entries.saturating_add(entries);
                into.declines = into.declines.saturating_add(declines);
            }
            match sim::interleaving_of(machine.as_ref(), &outcome) {
                Some(interleaving) => interleaving,
                None => {
                    observed = false;
                    match outcome {
                        Ok(()) => Interleaving::passed(Vec::new()),
                        Err(diagnostic) => Interleaving::failed(Vec::new(), diagnostic),
                    }
                }
            }
        };

        let explored = if self.search.measure_reduction {
            measure_reduction(plan, &mut interleaving)
        } else {
            explore(plan, &mut interleaving)
        };
        let outcome = match explored.diagnostic {
            Some(diagnostic) => Err(diagnostic),
            None => Ok(()),
        };
        (
            outcome,
            observed.then_some(explored.exploration),
            host,
            used,
        )
    }
}

impl<'a> Executor for InterpExecutor<'a> {
    type Worker = Worker<'a>;

    fn worker(&self) -> Worker<'a> {
        let backend = self.backend();
        let mut worker = Worker::in_region(self.machine(backend.clone()), self.build_region());
        worker.backend = backend;
        // So the worker holds the group's region from creation, not only from its first test.
        worker.open_region();
        worker
    }

    fn exploration(&self, worker: &Worker<'a>) -> Option<Exploration> {
        worker.exploration.clone()
    }

    fn host_use(&self, worker: &Worker<'a>) -> Option<ply_eval::host::HostUse> {
        worker.host.clone()
    }

    fn backend_use(&self, worker: &Worker<'a>) -> Option<BackendUse> {
        worker.backend_use
    }

    fn teardown(&self, worker: &mut Worker<'a>) -> Vec<Diagnostic> {
        let mut out = ply_eval::rc::take_cycles();
        let m = &mut worker.machine;
        out.extend(m.take_teardown_warnings());
        out
    }

    fn execute(&self, worker: &mut Worker<'a>, index: usize) -> Result<(), Diagnostic> {
        worker.exploration = None;
        worker.host = None;
        // Cumulative over the machine's life, so this test's own is the difference.
        let before = worker.backed().map(Machine::compiled_counts);
        worker.backend_use = None;
        if self.searches(index) {
            let (outcome, exploration, host, searched) = self.search(worker, index);
            worker.exploration = exploration;
            worker.host = host;
            // The worker's counters never moved; the search reports its own.
            worker.backend_use = searched;
            return outcome;
        }
        worker.open_region();
        let outcome = self.execute_directly(worker, index);
        worker.backend_use = match (before, worker.backed().map(Machine::compiled_counts)) {
            (Some((e0, d0)), Some((e1, d1))) => Some(BackendUse {
                entries: e1.saturating_sub(e0),
                declines: d1.saturating_sub(d0),
            }),
            _ => None,
        };
        worker.host = worker.machine.host_use().cloned();
        // A failing test still closes its region so the next test does not inherit it.
        worker.close_region();
        outcome
    }
}

impl<'a> InterpExecutor<'a> {
    fn execute_directly(&self, worker: &mut Worker<'a>, index: usize) -> Result<(), Diagnostic> {
        let m = &mut worker.machine;
        self.arm_footprint_check(m.as_mut(), index);
        m.eval_test(index)
    }
}

fn test_hash(hashes: &HashOutput, index: usize) -> Option<DefHash> {
    hashes.tests.get(index).copied()
}

/// Every test the run reports on, nothing answered from the cache, in one class. A caller with no
/// program — a measurement, an audit — wants the same rows every time, and "all of them, fresh" is
/// what it says in the vocabulary a program uses to decide. The store is neither read nor written:
/// the keys a result is filed under are a program's to name.
pub fn fresh(check: &CheckOutput, visible: &[usize], plan: &Plan) -> Selection {
    Selection::chosen(
        &Choice {
            runs: visible.to_vec(),
            groups: vec![visible.to_vec()],
            reasons: Vec::new(),
            narrowed: BTreeMap::new(),
            filed: BTreeMap::new(),
        },
        check,
        visible,
        plan,
    )
}

/// What the runtime keeps so a program can try a mixture after the run that collected it: the
/// bodies this run introduced (a definition the run added is not in the store until the flush), and
/// per failure what a mixture needs.
pub struct Hybrids {
    pub fresh: ply_store::body::BodySet,
    pub per_failure: Vec<Option<HybridInput>>,
}

pub struct HybridInput {
    /// What the runtime knows about each definition in the failing test's closure, either era.
    pub facts: ChangeSet,
    /// What a mixture needs, when one can be built at all.
    pub runnable: Option<(Mixture, ply_store::body::StoredBody)>,
    pub signature: Signature,
    pub seed: Option<Seed>,
    /// Why no mixture can be tried, exactly when none can.
    pub absent: Option<Skipped>,
}

/// What each failure's cause is decided from, for the program that reads the report to decide it:
/// the facts its change set is classified from, and whether a mixture of the two eras can be built.
/// A failure that never passed has no baseline, and so no facts.
pub fn diagnose_failures(
    report: &RunReport,
    sources: &[(String, String)],
    front: &ply_ty::Front,
    store: &Store,
) -> Hybrids {
    let check = &front.check;
    let hashes = &front.hashes;
    let fresh = ply_store::body::of_front(front);
    let per_failure = report
        .failures
        .iter()
        .map(|failure| {
            let record = store.pass_record(&failure.key)?;
            let baseline = Baseline::with_decls(
                record.test_hash,
                record.closure.clone(),
                record.decls.clone(),
            );
            let test_hash = hashes
                .tests
                .iter()
                .zip(check.tests.iter())
                .find(|(_, t)| t.key == failure.key)
                .map(|(hash, _)| *hash);
            let mixture = hybrid::mixture_for(hashes, &failure.key, &baseline);
            let complete = hybrid::bodies_available(store, &fresh, &mixture);
            let test_body = test_hash.and_then(|hash| BodyHybrid::test_body(&fresh, hash));
            // Runnable when every body the mixture needs is available and the failing run reached
            // nothing outside the program.
            let runnable = match test_body {
                Some(test) if complete && !failure.host => Some((mixture, test)),
                _ => None,
            };
            let absent = match (&runnable, complete) {
                (Some(_), _) => None,
                (None, false) => Some(Skipped::NoBodies),
                (None, true) => Some(Skipped::NoHybrids),
            };
            // Unclassified, every change stays a candidate: a wider answer, never a wrong one.
            let rehashed =
                Rehashed::under(sources, &baseline, &front.packages, &front.mod_pkg).ok();
            let regression = Regression {
                key: &failure.key,
                test_hash,
                baseline: &baseline,
                hashes,
            };
            let facts = match rehashed {
                Some(rehashed) => {
                    change_set(&regression, &mut StoreClassify::new(rehashed, store, check))
                }
                None => change_set(&regression, &mut bisect::Unknown),
            };
            Some(HybridInput {
                facts,
                runnable,
                signature: Signature::of(&failure.diagnostic),
                seed: failure.seed.clone(),
                absent,
            })
        })
        .collect();
    Hybrids { fresh, per_failure }
}

pub fn run_with<E: Executor>(
    selection: &Selection,
    check: &CheckOutput,
    hashes: &HashOutput,
    store: &mut Store,
    executor: &E,
) -> RunReport {
    let started = Instant::now();
    let mut warnings = Vec::new();

    let changed = changed_definitions(hashes, store);

    let mut results: Vec<TestResult> = Vec::new();
    let mut failures = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut abandoned = 0usize;

    for (group_index, group) in schedule_of(selection, &mut warnings).iter().enumerate() {
        let mut live = Vec::with_capacity(group.len());
        for &index in group {
            if index < check.tests.len() {
                live.push(index);
            } else {
                warnings.push(
                    Diagnostic::warning(
                        codes::INTERNAL_ERROR,
                        format!(
                            "selection names test {index}, but the module defines {}",
                            check.tests.len()
                        ),
                    )
                    .note("re-run `select` against this module; the stale index was skipped"),
                );
            }
        }

        for executed in execute_group(executor, &live, check) {
            let index = executed.index;
            warnings.extend(executed.teardown);
            let test = &check.tests[index];
            let hash = test_hash(hashes, index);
            let seeded = is_seeded(&test.footprint);
            let host_backed = executed.host.is_some();
            let abandoned_run = executed.failure.as_ref().is_some_and(is_abandoned);
            let defect = executed
                .failure
                .as_ref()
                .is_some_and(|d| executed.panicked || is_defect(d));
            let status = match (&executed.failure, defect) {
                (None, _) => Status::Passed,
                (Some(_), _) if abandoned_run => Status::Abandoned,
                (Some(_), false) => Status::Failed,
                (Some(_), true) => Status::Panicked,
            };
            let exploration = executed.exploration;
            let mut recorded = None;

            if abandoned_run {
                // The clock is a fact about this machine: no verdict, no suspects, nothing stored.
                abandoned += 1;
            } else if let Some(diagnostic) = &executed.failure {
                failed += 1;
                let suspects = suspects_for(hashes, &test.key, &changed);
                let attribution = Attribution::from_suspects(&suspects, hashes);
                failures.push(Failure {
                    name: test.name.clone(),
                    key: test.key.clone(),
                    diagnostic: diagnostic.clone(),
                    defect,
                    host: host_backed,
                    suspects,
                    assertion: None,
                    attribution,
                    seed: exploration.as_ref().and_then(|e| e.failure.clone()),
                    race: exploration.as_ref().and_then(|e| e.race.clone()),
                });
            } else if executed.host.is_some() {
                // This run reached a socket: its green verdict is about that moment only.
                passed += 1;
                recorded = Some(Record::Host);
            } else {
                passed += 1;
                if let Some(filed) = selection.filed.get(&index) {
                    let record = record_under(filed, seeded, exploration.as_ref());
                    if record == Record::Unobserved {
                        warnings.push(unobserved_search(&test.key));
                    }
                    for key in record.keys() {
                        store.put(*key, Outcome::Pass);
                    }
                    // Only the evaluator writes the name-keyed baseline.
                    if record.is_written()
                        && let Some(hash) = hash
                    {
                        let (closure, decls) = closure_hashes(hashes, &test.key);
                        store.put_pass_record(
                            test.key.clone(),
                            PassRecord {
                                test_hash: hash,
                                closure,
                                decls,
                            },
                        );
                    }
                    recorded = Some(record);
                }
            }

            results.push(TestResult {
                index,
                name: test.name.clone(),
                hash,
                group: group_index,
                duration: executed.duration,
                status,
                failure: executed.failure,
                simulation: exploration,
                recorded,
                backend: executed.backend,
            });
        }
    }

    observe_definitions(store, hashes, check, selection, &results);

    if let Err(e) = store.flush() {
        warnings.push(
            Diagnostic::warning(
                codes::CACHE_UNREADABLE,
                format!("could not write the test cache: {e}"),
            )
            .note("the run itself is valid; every test will simply be re-run next time"),
        );
    }

    let simulation = summarize_simulation(selection, &results);

    RunReport {
        passed,
        failed,
        abandoned,
        cached: selection.cached.len(),
        failures,
        duration: started.elapsed(),
        parallelism: selection.parallelism,
        results,
        warnings,
        simulation,
    }
}

fn summarize_simulation(selection: &Selection, results: &[TestResult]) -> SimSummary {
    let mut summary = SimSummary {
        total: results.len(),
        ..SimSummary::default()
    };
    for result in results {
        let Some(exploration) = &result.simulation else {
            continue;
        };
        summary.simulated += 1;
        summary.seeds += selection.plan_for(result.index).roots.len();
        summary.interleavings += u64::from(exploration.explored);
        summary.exhaustive += usize::from(exploration.exhaustive);
        summary.exhausted += usize::from(exploration.exhausted);
        summary.failed += usize::from(exploration.failure.is_some());
    }
    summary
}

/// Something in the closure entered a `simulate` region but the evaluator reported no search.
fn unobserved_search(key: &Symbol) -> Diagnostic {
    Diagnostic::warning(
        codes::INTERNAL_ERROR,
        format!("`{key}` reads a simulation seed, but the run reported no search"),
    )
    .note("the test passed and its result was not cached, so it re-runs next time")
    .note("this is a defect in Ply rather than in the test; please report it")
}

/// A selected test no group claims would be silently skipped, which a runner must never do.
fn schedule_of(selection: &Selection, warnings: &mut Vec<Diagnostic>) -> Vec<Vec<usize>> {
    let scheduled: BTreeSet<usize> = selection.groups.iter().flatten().copied().collect();
    let orphans: Vec<usize> = selection
        .to_run
        .iter()
        .copied()
        .filter(|i| !scheduled.contains(i))
        .collect();
    if orphans.is_empty() {
        return selection.groups.clone();
    }
    warnings.push(
        Diagnostic::warning(
            codes::INTERNAL_ERROR,
            format!(
                "{} selected tests were in no concurrency group",
                orphans.len()
            ),
        )
        .note("they were run one at a time; rebuild the selection with `select`"),
    );
    let mut groups = selection.groups.clone();
    groups.extend(orphans.into_iter().map(|i| vec![i]));
    groups
}

struct Executed {
    index: usize,
    duration: Duration,
    failure: Option<Diagnostic>,
    panicked: bool,
    exploration: Option<Exploration>,
    /// What this test reached across the boundary, which decides whether its pass may be written.
    host: Option<ply_eval::host::HostUse>,
    teardown: Vec<Diagnostic>,
    backend: Option<BackendUse>,
}

/// One worker per pool thread, built lazily so a small group builds no idle interpreters.
fn execute_group<E: Executor>(
    executor: &E,
    indices: &[usize],
    check: &CheckOutput,
) -> Vec<Executed> {
    if indices.is_empty() {
        return Vec::new();
    }

    let next = AtomicUsize::new(0);
    let per_thread = rayon::broadcast(|_| {
        let mut worker: Option<E::Worker> = None;
        let mut out: Vec<Executed> = Vec::new();
        loop {
            let Some(&index) = indices.get(next.fetch_add(1, Ordering::Relaxed)) else {
                return out;
            };
            let w = worker.get_or_insert_with(|| executor.worker());
            let started = Instant::now();
            let result = catch_unwind(AssertUnwindSafe(|| executor.execute(w, index)));
            let duration = started.elapsed();

            let (failure, panicked) = match result {
                Ok(Ok(())) => (None, false),
                Ok(Err(d)) => (Some(d), false),
                Err(payload) => {
                    // Unwinding leaves its invariants unknown; the next test gets a fresh worker.
                    worker = None;
                    (Some(panic_diagnostic(payload, check, index)), true)
                }
            };
            // After the unwind check: a worker with unknown invariants has nothing to report.
            let exploration = worker.as_ref().and_then(|w| executor.exploration(w));
            let host = worker.as_ref().and_then(|w| executor.host_use(w));
            let backend = worker.as_ref().and_then(|w| executor.backend_use(w));
            let teardown = worker
                .as_mut()
                .map(|w| executor.teardown(w))
                .unwrap_or_default();
            out.push(Executed {
                index,
                duration,
                failure,
                panicked,
                exploration,
                host,
                teardown,
                backend,
            });
        }
    });

    let mut out: Vec<Executed> = per_thread.into_iter().flatten().collect();
    out.sort_by_key(|e| e.index);
    out
}

fn is_defect(d: &Diagnostic) -> bool {
    d.code == codes::INTERNAL_ERROR
        || d.code == codes::HOST_FOOTPRINT_ESCAPE
        || d.code == codes::SECRET_TO_HOST
        || d.code == codes::SIMULATION_DIVERGENCE
}

/// The wall clock stopped this run: it is about the machine, so it is no verdict on the test.
fn is_abandoned(d: &Diagnostic) -> bool {
    d.code == codes::RUN_ABANDONED
}

fn panic_diagnostic(payload: Box<dyn Any + Send>, check: &CheckOutput, index: usize) -> Diagnostic {
    let message = if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic with a non-string payload".to_string()
    };

    let (name, span) = match check.tests.get(index) {
        Some(t) => (t.key.to_string(), t.span),
        None => (format!("test {index}"), ply_span::Span::DUMMY),
    };

    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("test `{name}` panicked: {message}"),
    )
    .primary(span, "the interpreter panicked while running this test")
    .note("a panic is a defect in Ply itself, not in the test; please report it with this source")
    .note("the other tests still ran, and this one was not cached")
}

/// Definitions the store has never recorded seeing.
fn changed_definitions(hashes: &HashOutput, store: &Store) -> BTreeSet<Symbol> {
    hashes
        .defs
        .iter()
        .filter(|(_, hash)| !store.knows_definition(**hash))
        .map(|(name, _)| name.clone())
        .collect()
}

/// The single place a test's key becomes a hash-graph key, so callers cannot disagree on it.
fn closure_of<'a>(hashes: &'a HashOutput, key: &Symbol) -> Option<&'a BTreeSet<Symbol>> {
    hashes.closure.get(key)
}

/// Names are how two eras of a program are lined up; hashes are what they are compared by.
fn closure_hashes(
    hashes: &HashOutput,
    key: &Symbol,
) -> (BTreeMap<Symbol, DefHash>, BTreeMap<Symbol, DefHash>) {
    let Some(closure) = closure_of(hashes, key) else {
        return (BTreeMap::new(), BTreeMap::new());
    };
    let mut defs = BTreeMap::new();
    let mut decls = BTreeMap::new();
    for name in closure {
        if let Some(hash) = hashes.defs.get(name) {
            defs.insert(name.clone(), *hash);
        }
        if let Some(hash) = hashes.decls.get(name) {
            decls.insert(name.clone(), *hash);
        }
    }
    (defs, decls)
}

fn suspects_for(hashes: &HashOutput, key: &Symbol, changed: &BTreeSet<Symbol>) -> Vec<Symbol> {
    match closure_of(hashes, key) {
        Some(closure) => closure
            .intersection(changed)
            .filter(|s| *s != key)
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

/// Hands the store every definition except those a failed or never-executed test reached.
fn observe_definitions(
    store: &mut Store,
    hashes: &HashOutput,
    check: &CheckOutput,
    selection: &Selection,
    results: &[TestResult],
) {
    let proven: BTreeSet<usize> = results
        .iter()
        .filter(|r| r.passed())
        .map(|r| r.index)
        .collect();
    let implicated: BTreeSet<&Symbol> = (0..check.tests.len())
        .filter(|index| !selection.out_of_scope.contains(index))
        .filter(|index| {
            let green = selection.reason(*index) == Some(Reason::Cached);
            !green && !proven.contains(index)
        })
        .filter_map(|index| closure_of(hashes, &check.tests[index].key))
        .flatten()
        .collect();

    store.observe_definitions(
        hashes
            .defs
            .iter()
            .filter(|(name, _)| !implicated.contains(name))
            .map(|(_, hash)| *hash),
    );
}

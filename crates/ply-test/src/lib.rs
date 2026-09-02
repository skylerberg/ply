//! Selection, scheduling, and running — where "a test re-runs iff its hash is absent from the
//! cache" stops being a claim and becomes an observable.

pub mod bisect;
pub mod diagnose;
pub mod hybrid;
pub mod key;
pub mod obligation;
pub mod region;
pub mod report;
pub mod schedule;
pub mod sim;
pub mod slice;

#[cfg(test)]
mod tests;

use ply_core::{CheckOutput, Footprint};
use ply_eval::explore::{Interleaving, explore, measure_reduction};
use ply_eval::host::{HostBinding, HostRuntime};
use ply_eval::{
    Arena, Exploration, Lowering, Machine, Plan, Race, Seed, TaskRegions, Value, compare_outcomes,
};
use ply_hash::{DefHash, HashOutput};
use ply_span::{Diagnostic, Symbol, codes};
use ply_store::{Outcome, PassRecord, Store};
use ply_syntax::ast::Program;
use ply_syntax::resolve::Resolved;
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
    Baseline, Bisection, Budget, Change, ChangeKind, Classify, Cluster, Confidence, DefKey, Delta,
    DepEdges, Diff, EraTable, FusionReason, Gate, Hybrid, Mode, Ns, Regression, Renormalizer,
    SearchStats, Skipped, StoreClassify, Trial, TrialOutcome, Unresolved, Verdict, bisect, diff,
    precheck,
};
pub use diagnose::{Evidence, Options, diagnose};
pub use hybrid::{BodyHybrid, Mixture, Signature};
pub use key::{Engine, result_key, seed_key, sim_key, writes_seed_keys};
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

/// Why a test was or was not selected.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The hash is absent from the store: this exact test has never gone green.
    New,
    /// `test/nondet` opts out of the cache in both directions.
    Nondet,
    /// The store holds a failure for this hash.
    PreviousFailure,
    /// The hash is present and green; re-running cannot reveal anything new.
    Cached,
    /// No hash was produced for this test, so nothing can be concluded about it.
    Unhashed,
}

impl Reason {
    pub fn runs(self) -> bool {
        !matches!(self, Reason::Cached)
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
    /// The search this selection was made against, and the key a seeded test's result is published
    /// under.
    pub plan: Plan,
    /// What a seeded test still owes, when the cache already covers part of the plan.
    pub narrowed: BTreeMap<usize, Plan>,
    /// Test indices this run was never asked to decide: a shipped module's tests without `--std`.
    pub out_of_scope: BTreeSet<usize>,
}

impl Selection {
    pub fn reason(&self, index: usize) -> Option<Reason> {
        self.reasons.get(index).copied()
    }

    /// What this test will actually search: the run's plan, unless the cache already answered for
    /// some of its roots.
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
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passed,
    Failed,
    /// Ply failed rather than the program: the evaluator unwound, or it reported one of its own
    /// invariants broken.
    Panicked,
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
    /// What the search did.
    pub simulation: Option<Exploration>,
    /// Absent when nothing was written: a spent budget proved nothing, and a seeded test whose
    /// search went unobserved is a run nobody watched.
    pub recorded: Option<Record>,
    /// Whether the backend oracle actually compared a pair on this test.
    pub audited: Option<bool>,
    /// What this test asked of a compiled backend, and the fact the cache rule's stage two reads: a
    /// written `Pass` beside a non-zero `entries` is a run that cached a third execution strategy's
    /// verdict.
    pub backend: Option<BackendUse>,
}

impl TestResult {
    pub fn passed(&self) -> bool {
        self.status == Status::Passed
    }

    /// Green, and re-runs next time anyway.
    pub fn green_but_uncached(&self) -> bool {
        self.passed() && matches!(self.recorded, Some(Record::Exhausted | Record::Unobserved))
    }
}

/// A bare name is a list to read; these fields are what turn it into a ranking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suspect {
    /// The program-wide name.
    pub name: Symbol,
    pub hash: Option<DefHash>,
    /// Its hash when the test last passed.
    pub before: Option<DefHash>,
    /// `None` when the two configurations were never compared, so nothing distinguishes an edit
    /// from a hash that merely moved underneath it.
    pub change: Option<ChangeKind>,
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
            before: None,
            change: None,
            ran: None,
            depth: None,
            culprit: false,
        }
    }

    /// Most-likely-first: a bisected culprit, then whatever was on the stack when it blew up
    /// (innermost first), then whatever else ran, then an edit over a hash that only moved, then
    /// the name.
    fn rank(&self) -> (u8, usize, u8, &str) {
        let tier = match (self.culprit, self.ran, self.depth) {
            (true, ..) => 0,
            (false, _, Some(_)) => 1,
            (false, Some(true), None) => 2,
            (false, None, None) => 3,
            (false, Some(false), None) => 4,
        };
        let inherited = u8::from(self.change == Some(ChangeKind::Derived));
        (
            tier,
            self.depth.unwrap_or(usize::MAX),
            inherited,
            self.name.as_str(),
        )
    }
}

/// The diagnosis the system is in a position to do, so that a consumer does not have to re-derive
/// it.
#[derive(Clone, Debug, Default)]
pub struct Attribution {
    /// The same set as [`Failure::suspects`], ranked and annotated.
    pub suspects: Vec<Suspect>,
    pub bisection: Bisection,
    /// `None` until a traced re-run has happened.
    pub slice: Option<CausalSlice>,
}

impl Attribution {
    /// What a run knows before any bisection or tracing: the names, and their hashes now.
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
        // A culprit bisection found outside the suspect set is still the answer: the suspect set is
        // "changed and in the closure", and a definition can be a cause without the store having
        // noticed it change.
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
    /// Ply's fault rather than the program's, so there is nothing in the definition graph to
    /// attribute it to.
    pub defect: bool,
    /// This failing run reached a host handler, so re-running it acts on the world again.
    pub host: bool,
    /// Definitions in this test's closure whose hash is not in the store — the suspects for this
    /// failure.
    pub suspects: Vec<Symbol>,
    /// What failed, structured.
    pub assertion: Option<Assertion>,
    pub attribution: Attribution,
    /// The interleaving this failure happened in.
    pub seed: Option<Seed>,
    /// The two steps whose reordering flipped a passing interleaving to this one.
    pub race: Option<Race>,
}

impl Failure {
    /// The command that reproduces exactly this failure, when one exists.
    pub fn replay(&self) -> Option<String> {
        Some(replay_command(self.seed.as_ref()?, &self.name))
    }
}

#[derive(Clone, Debug)]
pub struct RunReport {
    pub passed: usize,
    pub failed: usize,
    pub cached: usize,
    pub failures: Vec<Failure>,
    pub duration: Duration,
    /// Carried through from the selection this run executed, so a consumer of the report alone can
    /// still see how much of the corpus is trivially parallel.
    pub parallelism: Parallelism,
    /// Every test that actually ran, in execution order.
    pub results: Vec<TestResult>,
    /// Problems with the run itself rather than with any test — a cache that could not be written,
    /// a selection naming a test that does not exist.
    pub warnings: Vec<Diagnostic>,
    /// What the run's simulated tests searched.
    pub simulation: SimSummary,
    /// How much of this run the differential oracle actually covered.
    pub audit: Option<AuditSummary>,
    /// Which engine answered, and therefore whose namespace this run's passes went into. A caller
    /// that selected against an engine of its own compares the two: the selection and the run
    /// name the engine in different places, and a disagreement would mean a run skipped what one
    /// engine proved and recorded it as another's.
    pub engine: Engine,
}

/// What `--audit-backend` compared, and what it could not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AuditSummary {
    /// Tests the pair ran and whose outcomes were compared.
    pub compared: usize,
    /// Tests one engine could not run, and which the oracle therefore says nothing about.
    pub unaudited: usize,
}

impl AuditSummary {
    pub fn total(&self) -> usize {
        self.compared + self.unaudited
    }

    /// The summary line, or nothing when there is no coverage to report.
    pub fn line(&self) -> Option<String> {
        if self.total() == 0 {
            return None;
        }
        let mut line = format!("audited {} of {}", self.compared, self.total());
        if self.unaudited > 0 {
            line.push_str(&format!(" · {} ran unpaired", self.unaudited));
        }
        Some(line)
    }
}

impl RunReport {
    pub fn is_success(&self) -> bool {
        self.failed == 0
    }
}

/// Runs a single test.
pub trait Executor: Sync {
    type Worker;

    fn worker(&self) -> Self::Worker;

    fn execute(&self, worker: &mut Self::Worker, index: usize) -> Result<(), Diagnostic>;

    /// Which engine answers, and therefore whose claim a `Pass` this run records is. The
    /// evaluator unless something else was installed.
    fn engine(&self) -> Engine {
        Engine::Evaluator
    }

    /// What the search the last [`Executor::execute`] performed did, read off the worker that
    /// performed it.
    fn exploration(&self, _worker: &Self::Worker) -> Option<Exploration> {
        None
    }

    /// What the last [`Executor::execute`] reached across the host boundary.
    fn host_use(&self, _worker: &Self::Worker) -> Option<ply_eval::host::HostUse> {
        None
    }

    /// Whether the last [`Executor::execute`] compared a pair.
    fn audited(&self, _worker: &Self::Worker) -> Option<bool> {
        None
    }

    /// What the last [`Executor::execute`] asked of a compiled backend.
    fn backend_use(&self, _worker: &Self::Worker) -> Option<BackendUse> {
        None
    }

    /// What the host runtime reported while closing the entry point, and forgotten by the worker
    /// once read.
    fn teardown(&self, _worker: &mut Self::Worker) -> Vec<Diagnostic> {
        Vec::new()
    }
}

/// The search each test runs, and whether to measure what an unpruned one would have cost.
#[derive(Clone, Debug, Default)]
pub struct Search {
    pub plan: Plan,
    pub narrowed: BTreeMap<usize, Plan>,
    /// Run the same search a second time with the dependence relation forced to `true`, so the
    /// reduction is a measured number rather than a slogan.
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

/// What a run may reach outside the program.
#[derive(Default)]
pub struct Hosting<'a> {
    binding: Option<Arc<HostBinding>>,
    /// A factory rather than a value, for the reason [`InterpExecutor::with_fixture`] is one: a
    /// runtime handle belongs to the one thread its machine runs on, and the runner has a machine
    /// per worker.
    runtime: Option<&'a (dyn Fn() -> Rc<dyn HostRuntime> + Sync)>,
}

impl<'a> Hosting<'a> {
    /// Nothing bound and nothing to name.
    pub fn hermetic() -> Hosting<'a> {
        Hosting::default()
    }

    /// The binding the run's machines get.
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
    pub program: &'a Program,
    pub resolved: &'a Resolved,
    pub check: &'a CheckOutput,
    /// Per test, its module and its position among that module's tests.
    addresses: Vec<(Symbol, usize)>,
    fixture: Option<&'a (dyn Fn(&mut TaskRegions) -> Value + Sync)>,
    hosts: Hosting<'a>,
    /// Whether to run the plain machine beside the backed one and compare.
    audit_backend: bool,
    /// The backend this run installs, and which of the eight corruptions it is wearing.
    backend: Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>,
    search: Search,
    /// This program's region kinds, shared by every engine this run builds.
    region_kinds: ply_eval::region_kind::Kinds,
}

/// One test's evaluator.
pub enum Engines<'a> {
    One(Box<Machine<'a>>),
    Audited(Box<Machine<'a>>, Box<Machine<'a>>),
}

/// One pool thread's evaluators, plus what its last test searched.
pub struct Worker<'a> {
    pub engines: Engines<'a>,
    exploration: Option<Exploration>,
    /// What the last test reached across the host boundary.
    host: Option<ply_eval::host::HostUse>,
    /// The region this worker's tests run in, built once and mutated in place.
    region: GroupRegion,
    /// Whether the last test was actually compared as a pair.
    audited: Option<bool>,
    /// This worker's backend, built once and installed on every machine it builds — including the
    /// machines a search rebuilds per interleaving, which would otherwise each construct their own
    /// evaluator.
    backend: Option<Rc<dyn ply_eval::Compiled>>,
    /// What the last test entered natively.
    backend_use: Option<BackendUse>,
}

/// What one test asked of the backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackendUse {
    /// Bodies this test ran natively instead of evaluating.
    pub entries: u64,
    /// Calls the backend was offered and declined.
    pub declines: u64,
}

impl<'a> Worker<'a> {
    pub fn new(engines: Engines<'a>) -> Worker<'a> {
        Worker {
            engines,
            exploration: None,
            host: None,
            region: GroupRegion::empty(),
            audited: None,
            backend: None,
            backend_use: None,
        }
    }

    /// The machine carrying this run's backend, if one is installed.
    fn backed(&self) -> Option<&Machine<'a>> {
        match (&self.engines, self.backend.is_some()) {
            (Engines::One(m), true) => Some(m.as_ref()),
            (Engines::Audited(_, b), _) => Some(b.as_ref()),
            _ => None,
        }
    }

    pub fn in_region(engines: Engines<'a>, region: GroupRegion) -> Worker<'a> {
        Worker {
            region,
            ..Worker::new(engines)
        }
    }

    /// The group's region as it stands: the fixture, plus every write the tests run so far made to
    /// it.
    pub fn region(&self) -> &GroupRegion {
        &self.region
    }

    /// What this worker's machine has already lowered, for the machines a search builds per
    /// interleaving.
    fn lowering(&self) -> Option<Rc<Lowering<'a>>> {
        let (Engines::One(m) | Engines::Audited(m, _)) = &self.engines;
        Some(m.share_lowering())
    }

    /// Hands each machine a stack seeded from the group's region, one each, so that the two under
    /// `--audit-backend` cannot write through to one another.
    fn open_region(&mut self) {
        if self.region.is_empty() {
            return;
        }
        match &mut self.engines {
            Engines::One(m) => m.set_regions(self.region.open().0),
            Engines::Audited(m, backed) => {
                m.set_regions(self.region.open().0);
                backed.set_regions(self.region.open().0);
            }
        }
    }

    /// Closes the test's region: its own slots go back to the bump pointer at the next entry point,
    /// and its writes to the fixture are carried here.
    fn close_region(&mut self) {
        if self.region.is_empty() {
            return;
        }
        let (Engines::One(m) | Engines::Audited(m, _)) = &self.engines;
        self.region.close(m.cells());
    }

    /// The cells of the machine whose verdict is reported.
    pub fn cells_mut(&mut self) -> &mut Arena {
        let (Engines::One(m) | Engines::Audited(m, _)) = &mut self.engines;
        m.cells_mut()
    }

    pub fn cells(&self) -> &Arena {
        let (Engines::One(m) | Engines::Audited(m, _)) = &self.engines;
        m.cells()
    }
}

impl<'a> InterpExecutor<'a> {
    pub fn new(
        program: &'a Program,
        resolved: &'a Resolved,
        check: &'a CheckOutput,
    ) -> InterpExecutor<'a> {
        let mut seen: std::collections::BTreeMap<Symbol, usize> = Default::default();
        let addresses = check
            .tests
            .iter()
            .map(|t| {
                let module = t.module.as_symbol().clone();
                let ordinal = seen.entry(module.clone()).or_default();
                let at = *ordinal;
                *ordinal += 1;
                (module, at)
            })
            .collect();
        InterpExecutor {
            program,
            resolved,
            check,
            addresses,
            fixture: None,
            hosts: Hosting::hermetic(),
            audit_backend: false,
            backend: None,
            search: Search::default(),
            region_kinds: ply_eval::region_kind::Kinds::default(),
        }
    }

    /// The fixture a group's tests share.
    pub fn with_fixture(mut self, fixture: &'a (dyn Fn(&mut TaskRegions) -> Value + Sync)) -> Self {
        self.fixture = Some(fixture);
        self
    }

    pub fn with_backend_audit(mut self, audit: bool) -> Self {
        self.audit_backend = audit;
        self
    }

    /// Install a backend on every machine this run builds.
    pub fn with_backend(
        mut self,
        provider: &'static dyn ply_eval::Provider,
        spec: ply_eval::BackendSpec,
    ) -> Self {
        self.backend = Some((provider, spec));
        self
    }

    /// What this run may reach outside the program.
    pub fn with_hosts(mut self, hosts: Hosting<'a>) -> Self {
        self.hosts = hosts;
        self
    }

    /// The search every simulated test in this run performs.
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

    /// The run's one answer about this program's regions.
    pub fn shared_region_kinds(&self) -> ply_eval::region_kind::Kinds {
        ply_eval::region_kind::Kinds::clone(&self.region_kinds)
    }

    /// A machine, with the run's binding and — on this worker's own thread — its own handle on the
    /// reactor.
    fn machine(&self) -> Box<Machine<'a>> {
        self.machine_lowering(None, None)
    }

    /// A backend for one worker, or `None` when this run installs none.
    fn backend(&self) -> Option<Rc<dyn ply_eval::Compiled>> {
        let (provider, spec) = self.backend.as_ref()?;
        Some(provider.attach(spec))
    }

    /// The same machine, lowering into `lowering` rather than into a cache of its own.
    fn machine_lowering(
        &self,
        lowering: Option<Rc<Lowering<'a>>>,
        backend: Option<Rc<dyn ply_eval::Compiled>>,
    ) -> Box<Machine<'a>> {
        let mut machine = Machine::new(self.program, self.resolved, self.check);
        if let Some(backend) = backend {
            machine.set_compiled(backend);
        }
        machine.share_region_kinds(self.shared_region_kinds());
        if let Some(lowering) = lowering {
            machine.set_lowering(lowering);
        }
        if let Some(binding) = &self.hosts.binding {
            machine.set_host_binding(Arc::clone(binding));
        }
        if let Some(runtime) = self.hosts.runtime {
            machine.set_host_runtime(runtime());
        }
        Box::new(machine)
    }

    fn run_one<E: ply_eval::Evaluator>(&self, e: &mut E, index: usize) -> Result<(), Diagnostic> {
        match self.addresses.get(index) {
            Some((module, ordinal)) => e.eval_test_in(module, *ordinal),
            None => e.eval_test(index),
        }
    }

    /// State this entry point's footprint claim, so a host answer outside it is `E0427` rather than
    /// a quiet uncached pass.
    fn arm_footprint_check(&self, machine: &mut Machine<'a>, index: usize) {
        if let Some(test) = self.check.tests.get(index) {
            machine.set_declared_footprint(test.footprint.clone());
        }
    }

    /// Whether this test's outcome is a function of a seed as well as of its definitions, and
    /// therefore something to search rather than to run.
    fn searches(&self, index: usize) -> bool {
        self.check
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
        // A search re-runs the test whole, so a host operation anywhere in it — not only inside the
        // region — is performed once per interleaving.
        let re_executed = plan.re_executes() || self.search.measure_reduction;
        let mut observed = true;
        // Every interleaving's, unioned.
        let mut host: Option<ply_eval::host::HostUse> = None;
        // Every interleaving's, summed.
        let mut used: Option<BackendUse> = None;
        let region = &worker.region;
        let lowering = worker.lowering();
        let backend = worker.backend.clone();
        let mut interleaving = |seed: &Seed| {
            let mut machine = self.machine_lowering(lowering.clone(), backend.clone());
            if !region.is_empty() {
                machine.set_regions(region.open().0);
            }
            self.arm_footprint_check(machine.as_mut(), index);
            machine.set_re_executed(re_executed);
            sim::seed_run(machine.as_mut(), seed, plan.steps);
            let outcome = self.run_one(machine.as_mut(), index);
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
                // The verdict is still the run's own.
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

    /// A run with a backend installed records in that backend's namespace, auditing or not. Under
    /// `--audit-backend` a plain test runs on both engines and they must agree, so its pass is
    /// true of the backend as well; a *searched* test under audit runs on machines the backend is
    /// installed on and never on the pair, so its pass is true of the backend alone. The backend's
    /// namespace is the one claim that holds in every case.
    fn engine(&self) -> Engine {
        let Some((provider, spec)) = &self.backend else {
            return Engine::Evaluator;
        };
        Engine::of_backend(provider.name(), &provider.variant(), spec)
    }

    fn worker(&self) -> Worker<'a> {
        let backend = self.backend();
        let mut worker = Worker::in_region(
            match (self.audit_backend, backend.clone()) {
                (true, Some(b)) => {
                    Engines::Audited(self.machine(), self.machine_lowering(None, Some(b)))
                }
                (_, b) => Engines::One(self.machine_lowering(None, b)),
            },
            self.build_region(),
        );
        worker.backend = backend;
        // So that a worker holds the group's region from the moment it exists, rather than only
        // from its first test.
        worker.open_region();
        worker
    }

    fn exploration(&self, worker: &Worker<'a>) -> Option<Exploration> {
        worker.exploration.clone()
    }

    fn host_use(&self, worker: &Worker<'a>) -> Option<ply_eval::host::HostUse> {
        worker.host.clone()
    }

    fn audited(&self, worker: &Worker<'a>) -> Option<bool> {
        worker.audited
    }

    fn backend_use(&self, worker: &Worker<'a>) -> Option<BackendUse> {
        worker.backend_use
    }

    fn teardown(&self, worker: &mut Worker<'a>) -> Vec<Diagnostic> {
        let mut out = ply_eval::rc::take_cycles();
        let (Engines::One(m) | Engines::Audited(m, _)) = &mut worker.engines;
        out.extend(m.take_teardown_warnings());
        out
    }

    fn execute(&self, worker: &mut Worker<'a>, index: usize) -> Result<(), Diagnostic> {
        worker.exploration = None;
        worker.host = None;
        let auditing = matches!(worker.engines, Engines::Audited(_, _));
        worker.audited = None;
        // Cumulative over the machine's life, so this test's own is the difference.
        let before = worker.backed().map(Machine::compiled_counts);
        worker.backend_use = None;
        if self.searches(index) {
            // A searched test is re-run per interleaving on a machine built for the schedule, so
            // the pair never runs and there is no second answer to compare against.
            worker.audited = auditing.then_some(false);
            let (outcome, exploration, host, searched) = self.search(worker, index);
            worker.exploration = exploration;
            worker.host = host;
            // A searched test runs on machines built per interleaving, so the worker's own counters
            // never moved and the search reports its own.
            worker.backend_use = searched;
            return outcome;
        }
        worker.open_region();
        let (outcome, audited) = self.execute_directly(worker, index);
        worker.audited = auditing.then_some(audited);
        worker.backend_use = match (before, worker.backed().map(Machine::compiled_counts)) {
            (Some((e0, d0)), Some((e1, d1))) => Some(BackendUse {
                entries: e1.saturating_sub(e0),
                declines: d1.saturating_sub(d0),
            }),
            _ => None,
        };
        // The machine whose verdict is reported, never the backed one: a backend reaches no host
        // handler, so the two cannot disagree here.
        worker.host = {
            let (Engines::One(m) | Engines::Audited(m, _)) = &worker.engines;
            m.host_use().cloned()
        };
        // A failing test closes its region like a passing one: what it allocated is still gone, and
        // the next test in the group must not inherit it because this one was red.
        worker.close_region();
        outcome
    }
}

impl<'a> InterpExecutor<'a> {
    /// The verdict, and whether a pair produced it.
    fn execute_directly(
        &self,
        worker: &mut Worker<'a>,
        index: usize,
    ) -> (Result<(), Diagnostic>, bool) {
        match &mut worker.engines {
            Engines::One(m) => {
                self.arm_footprint_check(m.as_mut(), index);
                (self.run_one(m.as_mut(), index), false)
            }
            Engines::Audited(m, backed) => {
                self.arm_footprint_check(m.as_mut(), index);
                self.arm_footprint_check(backed.as_mut(), index);
                // Both are stepped even when the first has already failed: a backed machine that
                // skipped a test has a different constant memo and a different stale answer from
                // here on.
                let plain = self.run_one(m.as_mut(), index);
                // A host handler is not a function, so a test that reached the boundary is run
                // once and counted unaudited rather than having its handler called twice.
                if m.host_use().is_some() {
                    return (plain, false);
                }
                let with_backend = self.run_one(backed.as_mut(), index);
                let subject = self
                    .check
                    .tests
                    .get(index)
                    .map(|t| t.key.to_string())
                    .unwrap_or_else(|| format!("test {index}"));
                let span = self
                    .check
                    .tests
                    .get(index)
                    .map_or(ply_span::Span::DUMMY, |t| t.span);
                // Taken whatever the plain machine said: comparing only where the plain run
                // passed would make a backend that turns a red test *green* the one thing this
                // cannot see.
                let verdict = match compare_outcomes(
                    m.as_ref(),
                    backed.as_ref(),
                    &subject,
                    Some(index),
                    &plain,
                    &with_backend,
                ) {
                    Some(d) => Err(d.to_backend_diagnostic(span)),
                    // The plain machine's answer, so that what a run reports never depends on
                    // whether auditing was switched on.
                    None => plain,
                };
                (verdict, true)
            }
        }
    }
}

fn test_hash(hashes: &HashOutput, index: usize) -> Option<DefHash> {
    hashes.tests.get(index).copied()
}

/// `plan` is what a seeded test's cache entry is keyed on, so a selection made against one plan
/// says nothing about another.
/// What this run must execute, read against `engine`'s own history: a `Pass` another engine
/// recorded is a claim about that engine and is not read here.
pub fn select(
    check: &CheckOutput,
    hashes: &HashOutput,
    store: &Store,
    plan: &Plan,
    engine: &Engine,
) -> Selection {
    let plan = plan.clone().normalized();
    let total = check.tests.len();
    let mut reasons = Vec::with_capacity(total);
    let mut cached = Vec::new();
    let mut to_run = Vec::new();
    let mut narrowed: BTreeMap<usize, Plan> = BTreeMap::new();

    for (index, test) in check.tests.iter().enumerate() {
        let seeded = is_seeded(&test.footprint);
        let hash = test_hash(hashes, index);
        let stored = hash.map(|hash| store.get(result_key(hash, seeded, &plan, engine)));

        // A `random` plan decomposes into one standalone claim per root, so a widened root set owes
        // only the roots nothing has answered for.
        let owed = match (seeded, hash) {
            (true, Some(hash)) if writes_seed_keys(&plan) => plan
                .roots
                .iter()
                .copied()
                .filter(|&root| {
                    !matches!(
                        store.get(seed_key(hash, &Seed::root(root), engine)),
                        Some(Outcome::Pass)
                    )
                })
                .collect(),
            _ => plan.roots.clone(),
        };

        let reason = if test.nondet {
            Reason::Nondet
        } else {
            match stored {
                None => Reason::Unhashed,
                // Every root already passed on its own, so the widened plan is proved by the roots
                // it is made of and nothing needs running.
                Some(None) if owed.is_empty() => Reason::Cached,
                Some(None) => Reason::New,
                Some(Some(Outcome::Pass)) => Reason::Cached,
                // Never trust a stored failure.
                Some(Some(Outcome::Fail { .. })) => Reason::PreviousFailure,
            }
        };

        match (reason, stored) {
            (Reason::Cached, Some(Some(outcome))) => cached.push((index, outcome)),
            (Reason::Cached, _) => cached.push((index, Outcome::Pass)),
            _ => {
                if owed.len() < plan.roots.len() {
                    narrowed.insert(
                        index,
                        Plan {
                            roots: owed,
                            ..plan.clone()
                        }
                        .normalized(),
                    );
                }
                to_run.push(index)
            }
        }
        reasons.push(reason);
    }

    let footprints: Vec<(usize, Footprint)> = to_run
        .iter()
        .map(|&i| (i, check.tests[i].footprint.clone()))
        .collect();
    let groups = group_by_conflict(&footprints);
    let parallelism = parallelism(
        check.tests.iter().map(|t| &t.footprint),
        &footprints,
        &groups,
    );

    Selection {
        total,
        cached,
        to_run,
        groups,
        reasons,
        isolation: check
            .tests
            .iter()
            .map(|t| Isolation::of(&t.footprint))
            .collect(),
        parallelism,
        plan,
        narrowed,
        out_of_scope: BTreeSet::new(),
    }
}

/// Turns each failure's raw suspect list into the ranked, annotated attribution of the failure artifact.
pub fn diagnose_failures(
    report: &mut RunReport,
    program: &Program,
    resolved: &Resolved,
    check: &CheckOutput,
    hashes: &HashOutput,
    store: &mut Store,
    options: &Options,
) -> Vec<Diagnostic> {
    if report.failures.is_empty() {
        return Vec::new();
    }

    // Built once for the whole run and shared: it re-normalizes the entire program, which is the
    // expensive half of deciding `Edited` from `Derived`.
    let test_keys: Vec<Symbol> = check.tests.iter().map(|t| t.key.clone()).collect();
    let (renormalizer, mut warnings) =
        match Renormalizer::new(program, resolved, hashes, &test_keys) {
            Ok(renormalizer) => (Some(renormalizer), Vec::new()),
            Err(diagnostics) => (None, diagnostics),
        };
    let edges = DepEdges::from(hashes);

    // This run's own normalized bytes.
    let fresh = ply_hash::hash_program_with_bodies(program, resolved)
        .map(|(_, bodies)| bodies)
        .unwrap_or_default();

    // A hybrid that went green is a true claim about exactly its own closure, so it may be cached —
    // but only after the borrow the search holds on the store has ended.
    let mut proved: Vec<DefHash> = Vec::new();

    for failure in &mut report.failures {
        let baseline = store.pass_record(&failure.key).map(|record| {
            Baseline::with_decls(
                record.test_hash,
                record.closure.clone(),
                record.decls.clone(),
            )
        });
        let nondet = check
            .tests
            .iter()
            .find(|t| t.key == failure.key)
            .is_some_and(|t| t.nondet);

        let test_hash = hashes
            .tests
            .iter()
            .zip(check.tests.iter())
            .find(|(_, t)| t.key == failure.key)
            .map(|(hash, _)| *hash);

        let evidence = Evidence {
            key: &failure.key,
            test_hash,
            nondet,
            defect: failure.defect,
            host: failure.host,
            suspects: &failure.suspects,
            hashes,
            baseline: baseline.as_ref(),
            slice: failure.attribution.slice.clone(),
        };

        // Everything the mixture could need, on either side.
        let mixture = baseline
            .as_ref()
            .map(|baseline| hybrid::mixture_for(hashes, &failure.key, baseline));
        let complete = mixture
            .as_ref()
            .is_some_and(|m| hybrid::bodies_available(store, &fresh, m));
        let test_body = test_hash.and_then(|hash| BodyHybrid::test_body(&fresh, hash));
        let absent = match (&mixture, complete) {
            (Some(_), false) => Skipped::NoBodies,
            _ => Skipped::NoHybrids,
        };
        // Every hybrid runs at the interleaving this failure happened in.
        let seed = failure.seed.clone();
        let mut builder = match (mixture, test_body, complete) {
            // A host-backed failure gets no builder at all.
            _ if failure.host => None,
            (Some(mixture), Some(test), true) => {
                let hybrid = BodyHybrid::new(
                    store,
                    &fresh,
                    mixture,
                    test,
                    Signature::of(&failure.diagnostic),
                );
                Some(match &seed {
                    Some(seed) => hybrid.at_seed(seed),
                    None => hybrid,
                })
            }
            _ => None,
        };

        // Without a renormalizer nothing can be told apart from a hash that merely moved, so every
        // change stays a candidate: a wider answer, never a wrong one.
        let mut unknown = bisect::Unknown;
        let mut store_classify;
        let classify: &mut dyn Classify = match (&renormalizer, &baseline) {
            (Some(renormalizer), Some(baseline)) => {
                store_classify = StoreClassify::new(renormalizer, baseline, store, check);
                &mut store_classify
            }
            _ => &mut unknown,
        };

        failure.attribution = diagnose(
            evidence,
            options,
            &edges,
            classify,
            builder.as_mut().map(|b| b as &mut dyn Hybrid),
            absent,
        );
        if let Some(builder) = &mut builder {
            // Never the failing test's own key.
            let forbidden: Vec<DefHash> = test_hash
                .into_iter()
                .flat_map(|hash| {
                    let plan = match &seed {
                        Some(seed) => Plan::once(seed.clone()),
                        None => Plan::once(Seed::default()),
                    };
                    [hash, sim_key(hash, &plan)]
                })
                .collect();
            proved.extend(
                builder
                    .take_proved()
                    .into_iter()
                    .filter(|hash| !forbidden.contains(hash)),
            );
        }
    }

    // A hybrid's test hash covers its whole closure, so `Pass` under it is true of that
    // configuration and of nothing else.
    for hash in proved {
        store.put(hash, Outcome::Pass);
    }

    warnings.retain(|d| d.severity != ply_span::Severity::Error);
    warnings
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    selection: &Selection,
    program: &Program,
    resolved: &Resolved,
    check: &CheckOutput,
    hashes: &HashOutput,
    store: &mut Store,
    audit_backend: bool,
    search: Search,
    hosts: Hosting<'_>,
) -> RunReport {
    let executor = InterpExecutor::new(program, resolved, check)
        .with_backend_audit(audit_backend)
        .with_search(search)
        .with_hosts(hosts);
    run_with(selection, check, hashes, store, &executor)
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
    let engine = executor.engine();

    let changed = changed_definitions(hashes, store);

    let mut results: Vec<TestResult> = Vec::new();
    let mut failures = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;

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
            let defect = executed
                .failure
                .as_ref()
                .is_some_and(|d| executed.panicked || is_defect(d));
            let status = match (&executed.failure, defect) {
                (None, _) => Status::Passed,
                (Some(_), false) => Status::Failed,
                (Some(_), true) => Status::Panicked,
            };
            let exploration = executed.exploration;
            let mut recorded = None;

            if let Some(diagnostic) = &executed.failure {
                failed += 1;
                let suspects = suspects_for(hashes, &test.key, &changed);
                let mut attribution = Attribution::from_suspects(&suspects, hashes);
                // The same order `precheck` applies, so a report nobody diagnosed says what a
                // diagnosed one would have.
                if defect {
                    attribution.bisection = Bisection::not_attempted(Skipped::Panicked);
                } else if host_backed {
                    attribution.bisection = Bisection::not_attempted(Skipped::Host);
                } else if test.nondet {
                    attribution.bisection = Bisection::not_attempted(Skipped::Nondet);
                }
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
                // The runtime is authoritative: this run reached a socket, so its green verdict is
                // a statement about that socket at that moment and about nothing the next run will
                // face.
                passed += 1;
                recorded = Some(Record::Host);
            } else {
                passed += 1;
                if !test.nondet
                    && let Some(hash) = hash
                {
                    let record = record_under(
                        hash,
                        seeded,
                        &selection.plan,
                        selection.plan_for(index),
                        exploration.as_ref(),
                        &engine,
                    );
                    if record == Record::Unobserved {
                        warnings.push(unobserved_search(&test.key));
                    }
                    for key in record.keys() {
                        store.put(*key, Outcome::Pass);
                    }
                    // The baseline a later failure is bisected against, and it is written only
                    // by the evaluator: it is keyed by the test's name rather than by a key an
                    // engine can qualify, and `diagnose_failures` re-runs every hybrid on the
                    // evaluator, so a baseline another engine earned would be a claim this record
                    // does not make.
                    if record.is_written() && engine.is_evaluator() {
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
                audited: executed.audited,
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
    let audit = summarize_audit(&results);

    RunReport {
        passed,
        failed,
        cached: selection.cached.len(),
        failures,
        duration: started.elapsed(),
        parallelism: selection.parallelism,
        results,
        warnings,
        simulation,
        audit,
        engine,
    }
}

/// `None` unless at least one test knew whether it had been audited, so a run with no oracle
/// reports no coverage rather than a coverage of zero.
fn summarize_audit(results: &[TestResult]) -> Option<AuditSummary> {
    let mut summary = AuditSummary::default();
    let mut seen = false;
    for result in results {
        let Some(audited) = result.audited else {
            continue;
        };
        seen = true;
        if audited {
            summary.compared += 1;
        } else {
            summary.unaudited += 1;
        }
    }
    seen.then_some(summary)
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

/// The test's row says something in its closure entered a `simulate` region and the evaluator
/// reported no search.
fn unobserved_search(key: &Symbol) -> Diagnostic {
    Diagnostic::warning(
        codes::INTERNAL_ERROR,
        format!("`{key}` reads a simulation seed, but the run reported no search"),
    )
    .note("the test passed and its result was not cached, so it re-runs next time")
    .note("this is a defect in Ply rather than in the test; please report it")
}

/// A selected test that no group claims would be silently skipped, which is the one outcome a test
/// runner may never produce.
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
    /// What this test actually reached across the boundary, which decides whether its pass may be
    /// written.
    host: Option<ply_eval::host::HostUse>,
    /// What the host runtime reported while closing the entry point.
    teardown: Vec<Diagnostic>,
    /// Whether a pair produced this verdict.
    audited: Option<bool>,
    /// What this test asked of a compiled backend.
    backend: Option<BackendUse>,
}

/// One worker per pool thread, built lazily so a group smaller than the pool does not pay to
/// construct interpreters that never run anything.
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
                    // Unwinding out of the middle of a worker leaves its invariants unknown, so the
                    // next test gets a fresh one.
                    worker = None;
                    (Some(panic_diagnostic(payload, check, index)), true)
                }
            };
            // After the unwind check: a worker whose invariants are unknown has nothing to report
            // about what it searched.
            let exploration = worker.as_ref().and_then(|w| executor.exploration(w));
            let host = worker.as_ref().and_then(|w| executor.host_use(w));
            let audited = worker.as_ref().and_then(|w| executor.audited(w));
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
                audited,
                backend,
            });
        }
    });

    let mut out: Vec<Executed> = per_thread.into_iter().flatten().collect();
    out.sort_by_key(|e| e.index);
    out
}

/// Two evaluators of one language disagreeing is a defect in Ply by definition: whatever the
/// program means, at most one of the answers is it, and nothing in the user's definition graph
/// decides which.
fn is_divergence(d: &Diagnostic) -> bool {
    d.code == codes::ENGINE_DIVERGENCE || d.code == codes::SIMULATION_DIVERGENCE
}

/// Ply's fault rather than the program's, read off the diagnostic.
fn is_defect(d: &Diagnostic) -> bool {
    d.code == codes::INTERNAL_ERROR
        || d.code == codes::HOST_FOOTPRINT_ESCAPE
        || d.code == codes::SECRET_TO_HOST
        || is_divergence(d)
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

/// The single place a test's key becomes a key into the hash graph: two callers disagreeing about
/// that convention would silently mis-attribute a failure rather than fail.
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

/// Hands the store every definition except those an *unresolved* test reached: one that failed, or
/// that was selected and never executed because a filter or a stale selection dropped it.
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

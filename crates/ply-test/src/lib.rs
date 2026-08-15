//! Selection, scheduling, and running — where "a test re-runs iff its hash is
//! absent from the cache" stops being a claim and becomes an observable.

pub mod bisect;
pub mod diagnose;
pub mod hybrid;
pub mod key;
pub mod obligation;
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
    Engine, EngineChoice, Exploration, Interp, Machine, Plan, Race, Seed, World, compare_outcomes,
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
pub use key::{result_key, seed_key, sim_key, writes_seed_keys};
pub use schedule::{
    AMBIENT, Isolation, Parallelism, SIM_EFFECT, SIMULATED, WORLD_BACKED, contends,
    group_by_conflict, is_ambient, is_seeded, is_world_backed, parallelism, shared_footprint,
    world_isolated,
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
    /// The store holds a failure for this hash. A red test re-runs until green.
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
    /// Concurrency groups over `to_run`; every pair within a group has
    /// non-conflicting footprints. This is the schedule of record — `run`
    /// executes these, in order.
    pub groups: Vec<Vec<usize>>,
    /// Indexed by test index, length `total`.
    pub reasons: Vec<Reason>,
    /// Indexed by test index, length `total`. Covers cached tests too: whether
    /// a test could disturb another is a fact about the test, not about this
    /// run.
    pub isolation: Vec<Isolation>,
    pub parallelism: Parallelism,
    /// The search this selection was made against, and the key a seeded test's
    /// result is published under. A selection read against one plan says nothing
    /// about another, which is why it travels with the answer.
    pub plan: Plan,
    /// What a seeded test still owes, when the cache already covers part of the
    /// plan. Only `random` decomposes, so only `random` ever narrows: sixty-four
    /// roots become a hundred and twenty-eight for the cost of sixty-four runs.
    pub narrowed: BTreeMap<usize, Plan>,
    /// Test indices this run was never asked to decide: a shipped module's
    /// tests without `--std`. They are not `filtered out` and not in `total`,
    /// and — the part that matters — they withhold nothing from
    /// [`Store::observe_definitions`]. A filter-dropped test still implicates
    /// its closure, because the project chose not to run a test it owns; a
    /// shipped test was never in the denominator, so leaving `std.http`'s
    /// definitions permanently unrecorded would put all of them in every
    /// suspect set for the life of the cache.
    pub out_of_scope: BTreeSet<usize>,
}

impl Selection {
    pub fn reason(&self, index: usize) -> Option<Reason> {
        self.reasons.get(index).copied()
    }

    /// What this test will actually search: the run's plan, unless the cache
    /// already answered for some of its roots.
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

/// Hand-written so that `Selection` stays printable no matter what `Outcome`
/// derives.
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
    /// Ply failed rather than the program: the evaluator unwound, or it reported
    /// one of its own invariants broken. Kept distinct from `Failed` so a defect
    /// in Ply cannot be read as a red test the user should go and fix.
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
    /// What the search did. `None` — never a zeroed [`Exploration`] — when this
    /// test reached no `simulate` region, because a consumer cannot tell a zero
    /// from a test that never simulated anything.
    pub simulation: Option<Exploration>,
    /// Absent when nothing was written: a spent budget proved nothing, and a
    /// seeded test whose search went unobserved is a run nobody watched.
    pub recorded: Option<Record>,
}

impl TestResult {
    pub fn passed(&self) -> bool {
        self.status == Status::Passed
    }

    /// Green, and re-runs next time anyway. The one place a passing `det` test
    /// is not cacheable, and the summary says so rather than leaving a reader to
    /// wonder why selection did not shrink.
    pub fn green_but_uncached(&self) -> bool {
        self.passed() && matches!(self.recorded, Some(Record::Exhausted | Record::Unobserved))
    }
}

/// A bare name is a list to read; these fields are what turn it into a ranking.
/// Every one of them is tri-state where the evidence may be missing, because a
/// consumer that cannot tell "did not run" from "was not traced" will act on the
/// wrong one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suspect {
    /// The program-wide name.
    pub name: Symbol,
    pub hash: Option<DefHash>,
    /// Its hash when the test last passed. `None` when no baseline is known.
    pub before: Option<DefHash>,
    /// `None` when the two configurations were never compared, so nothing
    /// distinguishes an edit from a hash that merely moved underneath it.
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

    /// Most-likely-first: a bisected culprit, then whatever was on the stack when
    /// it blew up (innermost first), then whatever else ran, then an edit over a
    /// hash that only moved, then the name. Total and deterministic, because two
    /// runs over the same failure have to produce the same artifact.
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

/// The diagnosis the system is in a position to do, so that a consumer does not
/// have to re-derive it.
#[derive(Clone, Debug, Default)]
pub struct Attribution {
    /// The same set as [`Failure::suspects`], ranked and annotated. The order
    /// differs deliberately: `Failure::suspects` is the raw intersection and this
    /// is the answer.
    pub suspects: Vec<Suspect>,
    pub bisection: Bisection,
    /// `None` until a traced re-run has happened.
    pub slice: Option<CausalSlice>,
}

impl Attribution {
    /// What a run knows before any bisection or tracing: the names, and their
    /// hashes now.
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

    /// Folds a bisection and a trace into the suspects, then re-ranks. Called
    /// once both are in hand; the ordering is not stable under partial updates
    /// and is not meant to be read before this.
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
        // A culprit bisection found outside the suspect set is still the answer:
        // the suspect set is "changed and in the closure", and a definition can
        // be a cause without the store having noticed it change.
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
    /// The label as the source wrote it. Not unique program-wide; `key` is.
    pub name: String,
    /// `<module>.<label>`, and the key this failure's closure is looked up by.
    pub key: Symbol,
    pub diagnostic: Diagnostic,
    /// Ply's fault rather than the program's, so there is nothing in the
    /// definition graph to attribute it to.
    ///
    /// Four things say so, and no fifth: the run watched the evaluator unwind,
    /// the evaluator said `INTERNAL_ERROR`, the two engines disagreed, or a host
    /// answer landed outside the entry point's declared footprint. Reading it off
    /// `RUNTIME_ERROR` is what made a runaway recursion — a documented limit, and
    /// as bisectable a regression as any assertion — report itself as a defect in
    /// Ply and decline to be bisected.
    pub defect: bool,
    /// This failing run reached a host handler, so re-running it acts on the
    /// world again. Read off what the runtime did, never off the prediction
    /// selection made from footprints.
    pub host: bool,
    /// Definitions in this test's closure whose hash is not in the store —
    /// the suspects for this failure.
    pub suspects: Vec<Symbol>,
    /// What failed, structured. `None` until the evaluator carries the payload
    /// rather than rendering it into the diagnostic's notes.
    pub assertion: Option<Assertion>,
    pub attribution: Attribution,
    /// The interleaving this failure happened in. The whole point of M7 is that
    /// the repro handed to an agent is this rather than a stack trace, so it is
    /// the field the artifact leads the reproduction with.
    ///
    /// `None` on a failure no simulation produced — never a default seed, which
    /// would replay a different run.
    pub seed: Option<Seed>,
    /// The two steps whose reordering flipped a passing interleaving to this
    /// one. `Some` only when the search actually observed the flip: under
    /// `once` and `random` there is nothing to observe.
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
    /// Carried through from the selection this run executed, so a consumer of
    /// the report alone can still see how much of the corpus is trivially
    /// parallel.
    pub parallelism: Parallelism,
    /// Every test that actually ran, in execution order.
    pub results: Vec<TestResult>,
    /// Problems with the run itself rather than with any test — a cache that
    /// could not be written, a selection naming a test that does not exist.
    pub warnings: Vec<Diagnostic>,
    /// What the run's simulated tests searched. Zeroed, not absent: it is
    /// aggregated over the tests that ran, and `simulated == 0` is the honest
    /// answer for a corpus with no `simulate` region.
    pub simulation: SimSummary,
}

impl RunReport {
    pub fn is_success(&self) -> bool {
        self.failed == 0
    }
}

/// Runs a single test. `run` supplies the tree-walking interpreter; splitting it
/// out lets the scheduler, the cache rules, and panic containment be tested
/// without an evaluator in the loop.
///
/// `Worker` deliberately carries no `Send` bound: it is built on the thread that
/// will use it and never crosses one, which is what lets an interpreter full of
/// `Rc`s be a worker.
pub trait Executor: Sync {
    type Worker;

    fn worker(&self) -> Self::Worker;

    fn execute(&self, worker: &mut Self::Worker, index: usize) -> Result<(), Diagnostic>;

    /// What the search the last [`Executor::execute`] performed did, read off
    /// the worker that performed it.
    ///
    /// `None` means the test reached no `simulate` region, which is how a test
    /// that simulates nothing pays nothing. It is deliberately not a zeroed
    /// [`Exploration`]: the cache rules branch on the difference.
    fn exploration(&self, _worker: &Self::Worker) -> Option<Exploration> {
        None
    }

    /// What the last [`Executor::execute`] reached across the host boundary.
    ///
    /// `None` means it reached nothing, and it is the **authority** on whether
    /// the pass may be cached — the footprint-based prediction decides only what
    /// runs. Never a zeroed [`HostUse`]: "reached no host handler" and "reached
    /// one that touched nothing" are different claims and the cache branches on
    /// the difference.
    fn host_use(&self, _worker: &Self::Worker) -> Option<ply_eval::host::HostUse> {
        None
    }

    /// What the host runtime reported while closing the entry point, and
    /// forgotten by the worker once read.
    ///
    /// Warnings, never failures, and never the test's verdict. A connection the
    /// driver had to discard because its `ROLLBACK` failed is the *run's* own
    /// resource: the program asked for nothing and did nothing wrong, and
    /// attributing it to whichever test happened to be running would send a
    /// reader looking for a defect in their own program.
    fn teardown(&self, _worker: &mut Self::Worker) -> Vec<Diagnostic> {
        Vec::new()
    }
}

/// The search each test runs, and whether to measure what an unpruned one would
/// have cost.
///
/// `narrowed` is [`Selection::narrowed`], carried here because the plan a test
/// searches is decided by what the cache already covers and the executor is what
/// installs it.
#[derive(Clone, Debug, Default)]
pub struct Search {
    pub plan: Plan,
    pub narrowed: BTreeMap<usize, Plan>,
    /// Run the same search a second time with the dependence relation forced to
    /// `true`, so the reduction is a measured number rather than a slogan. Off
    /// by default: the claim is a benchmark, not something every run pays double
    /// for.
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
///
/// Hermetic by default, and the default is the guarantee: a suite that acquires
/// a live dependency without anyone deciding to is the failure mode this
/// language exists to prevent, so binding a real handler is something a caller
/// has to write down.
///
/// A hermetic binding is **not** an absent one. [`HostBinding::hermetic_with`]
/// carries the registry without binding it, which is what lets the refusal be
/// `E0424` — naming the handler that would have served the operation under
/// `--host` — rather than `E0303`, which means inference should have prevented
/// the perform and did not. The two call for opposite responses.
#[derive(Default)]
pub struct Hosting<'a> {
    binding: Option<Arc<HostBinding>>,
    /// A factory rather than a value, for the reason [`InterpExecutor::with_fixture`]
    /// is one: a runtime handle belongs to the one thread its machine runs on,
    /// and the runner has a machine per worker.
    runtime: Option<&'a (dyn Fn() -> Rc<dyn HostRuntime> + Sync)>,
}

impl<'a> Hosting<'a> {
    /// Nothing bound and nothing to name. What every caller that does not
    /// mention the host gets.
    pub fn hermetic() -> Hosting<'a> {
        Hosting::default()
    }

    /// The binding the run's machines get. Pass
    /// `HostBinding::hermetic_with(registry)` to stay hermetic and still be able
    /// to name what would have served a perform.
    pub fn with_binding(mut self, binding: Arc<HostBinding>) -> Hosting<'a> {
        self.binding = Some(binding);
        self
    }

    /// What a [`ply_eval::host::HostAnswer::Pending`] is polled on. Absent for a
    /// hermetic run, where nothing can answer `Pending` in the first place.
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
    ///
    /// The evaluator indexes tests within the AST it was given, and the
    /// incremental front end reports tests from modules it never parsed, so a
    /// position in `check.tests` is not a position in `program`. This is the
    /// key that means the same thing in both.
    addresses: Vec<(Symbol, usize)>,
    fixture: Option<&'a (dyn Fn() -> World + Sync)>,
    hosts: Hosting<'a>,
    engine: EngineChoice,
    search: Search,
}

/// One test's evaluator. Under [`EngineChoice::Both`] it is two of them, run
/// over the same test so that a disagreement fails the run at the test that
/// produced it.
pub enum Engines<'a> {
    Treewalk(Box<Interp<'a>>),
    Machine(Box<Machine<'a>>),
    Both(Box<Interp<'a>>, Box<Machine<'a>>),
}

/// One pool thread's evaluators, plus what its last test searched.
///
/// The search lives here rather than in the return of `execute` because
/// `Executor::execute` answers the same question it always did — did this test
/// pass — and a runner that has no simulation to report should not have to
/// mention one.
pub struct Worker<'a> {
    pub engines: Engines<'a>,
    exploration: Option<Exploration>,
    /// What the last test reached across the host boundary. Held here rather
    /// than read off `engines` because a searched test runs on a machine built
    /// per interleaving, and reading the worker's own machine would answer with
    /// whatever the *previous* test did.
    host: Option<ply_eval::host::HostUse>,
}

impl<'a> Worker<'a> {
    pub fn new(engines: Engines<'a>) -> Worker<'a> {
        Worker {
            engines,
            exploration: None,
            host: None,
        }
    }

    /// The world of the engine whose verdict is reported.
    pub fn world(&self) -> &World {
        match &self.engines {
            Engines::Treewalk(i) | Engines::Both(i, _) => i.world(),
            Engines::Machine(m) => m.world(),
        }
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
            engine: EngineChoice::default(),
            search: Search::default(),
        }
    }

    /// The world every test forks from. A `World` holds `Rc` and cannot cross a
    /// thread, so this is a factory called on each worker's own thread rather
    /// than a value — one fixture per worker, one fork per test.
    ///
    /// A test's writes therefore reach neither the base nor a sibling worker,
    /// which is the premise the scheduler's world-backed exemption rests on: a
    /// shared mutable fixture here would make `cell` atoms contend again.
    pub fn with_fixture(mut self, fixture: &'a (dyn Fn() -> World + Sync)) -> Self {
        self.fixture = Some(fixture);
        self
    }

    pub fn with_engine(mut self, engine: EngineChoice) -> Self {
        self.engine = engine;
        self
    }

    /// What this run may reach outside the program. Hermetic unless said
    /// otherwise, in every constructor and at every call site.
    pub fn with_hosts(mut self, hosts: Hosting<'a>) -> Self {
        self.hosts = hosts;
        self
    }

    /// The search every simulated test in this run performs. Installed on the
    /// machine per test rather than per worker, because a narrowed plan is a
    /// fact about one test.
    pub fn with_search(mut self, search: Search) -> Self {
        self.search = search;
        self
    }

    fn interp(&self) -> Box<Interp<'a>> {
        let mut interp = Interp::new(self.program, self.resolved, self.check);
        if let Some(fixture) = self.fixture {
            interp.set_base_world(fixture());
        }
        if let Some(binding) = &self.hosts.binding {
            interp.set_host_binding(Arc::clone(binding));
        }
        Box::new(interp)
    }

    /// A machine, with the run's binding and — on this worker's own thread — its
    /// own handle on the reactor.
    fn machine(&self) -> Box<Machine<'a>> {
        let mut machine = Machine::new(self.program, self.resolved, self.check);
        if let Some(fixture) = self.fixture {
            machine.set_base_world(fixture());
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

    /// State this entry point's footprint claim, so a host answer outside it is
    /// `E0427` rather than a quiet uncached pass.
    ///
    /// Without this the check is inert in the one command that runs a corpus. A
    /// footprint is an upper bound on what a test performs, so a host answer
    /// outside it is the run observing its own two answers disagree — and it is
    /// reachable without any dishonest handler: a `handle` whose clause set
    /// covers some but not all operations of an atom discharges the atom out of
    /// the row and leaves the uncovered operation to fall through to the
    /// binding. That test is then scheduled as world-isolated over a footprint
    /// that says it touches nothing, while it touches a socket.
    ///
    /// Restated per test because one `Machine` serves many of them: a claim that
    /// outlived its entry point would judge the next test by the last one's row.
    fn arm_footprint_check(&self, machine: &mut Machine<'a>, index: usize) {
        if let Some(test) = self.check.tests.get(index) {
            machine.set_declared_footprint(test.footprint.clone());
        }
    }

    /// Whether this test's outcome is a function of a seed as well as of its
    /// definitions, and therefore something to search rather than to run.
    ///
    /// Under `--engine treewalk` it never is: `simulate` is machine-only, and
    /// the tree-walker's job there is to refuse the region with `E0504` rather
    /// than to have the region scheduled around it. Under `--engine both` the
    /// search runs once, on the machine, for the same reason.
    fn searches(&self, index: usize) -> bool {
        self.engine.primary() == Engine::Machine
            && self
                .check
                .tests
                .get(index)
                .is_some_and(|t| is_seeded(&t.footprint))
    }

    /// The whole test, once per interleaving, each from a fresh fork of the base
    /// world.
    ///
    /// Whole-test replay rather than re-entering the region: restoring the world
    /// as of region entry is the snapshot/restore capability ADR 0005 refused as
    /// having no type-level account. A test is re-run, so its writes are re-done
    /// rather than un-done, and the monotone world survives. It costs re-doing
    /// whatever setup precedes the region, per interleaving.
    #[allow(clippy::type_complexity)]
    fn search(
        &self,
        index: usize,
    ) -> (
        Result<(), Diagnostic>,
        Option<Exploration>,
        Option<ply_eval::host::HostUse>,
    ) {
        let plan = self.search.plan_for(index);
        // A search re-runs the test whole, so a host operation anywhere in it —
        // not only inside the region — is performed once per interleaving.
        // `measure_reduction` runs the whole search a second time, unpruned,
        // which doubles it again.
        let re_executed = plan.re_executes() || self.search.measure_reduction;
        let mut observed = true;
        // Every interleaving's, unioned. A search re-runs the whole test per
        // schedule, so an operation reached in one of them is one this run
        // performed, and reporting only the last would let the cache believe a
        // pass that a socket answered.
        let mut host: Option<ply_eval::host::HostUse> = None;
        let mut interleaving = |seed: &Seed| {
            let mut machine = self.machine();
            self.arm_footprint_check(machine.as_mut(), index);
            machine.set_re_executed(re_executed);
            sim::seed_run(machine.as_mut(), seed, plan.steps);
            let outcome = self.run_one(machine.as_mut(), index);
            if let Some(used) = machine.host_use() {
                let into = host.get_or_insert_with(Default::default);
                into.atoms = into.atoms.union(&used.atoms);
                into.operations = into.operations.saturating_add(used.operations);
            }
            match sim::interleaving_of(machine.as_ref(), &outcome) {
                Some(interleaving) => interleaving,
                // The verdict is still the run's own. An unobserved search must
                // report nothing about interleavings and must not turn a red
                // test green on the way.
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
        (outcome, observed.then_some(explored.exploration), host)
    }
}

impl<'a> Executor for InterpExecutor<'a> {
    type Worker = Worker<'a>;

    fn worker(&self) -> Worker<'a> {
        Worker::new(match self.engine {
            EngineChoice::Treewalk => Engines::Treewalk(self.interp()),
            EngineChoice::Machine => Engines::Machine(self.machine()),
            EngineChoice::Both => Engines::Both(self.interp(), self.machine()),
        })
    }

    fn exploration(&self, worker: &Worker<'a>) -> Option<Exploration> {
        worker.exploration.clone()
    }

    fn host_use(&self, worker: &Worker<'a>) -> Option<ply_eval::host::HostUse> {
        worker.host.clone()
    }

    /// Only the machine has a runtime to close anything on: the tree-walker
    /// refuses a bound host operation rather than driving one, so there is never
    /// a scope of its to hear about.
    fn teardown(&self, worker: &mut Worker<'a>) -> Vec<Diagnostic> {
        match &mut worker.engines {
            Engines::Machine(m) | Engines::Both(_, m) => m.take_teardown_warnings(),
            Engines::Treewalk(_) => Vec::new(),
        }
    }

    fn execute(&self, worker: &mut Worker<'a>, index: usize) -> Result<(), Diagnostic> {
        worker.exploration = None;
        worker.host = None;
        if self.searches(index) {
            let (outcome, exploration, host) = self.search(index);
            worker.exploration = exploration;
            worker.host = host;
            return outcome;
        }
        let outcome = self.execute_directly(worker, index);
        // Only the machine can reach a host handler: the tree-walker refuses one
        // as machine-only rather than driving it, so under `--engine both` there
        // is one answer here and never two to reconcile.
        worker.host = match &worker.engines {
            Engines::Machine(m) | Engines::Both(_, m) => m.host_use().cloned(),
            Engines::Treewalk(_) => None,
        };
        outcome
    }
}

impl<'a> InterpExecutor<'a> {
    fn execute_directly(&self, worker: &mut Worker<'a>, index: usize) -> Result<(), Diagnostic> {
        match &mut worker.engines {
            Engines::Treewalk(i) => self.run_one(i.as_mut(), index),
            Engines::Machine(m) => {
                self.arm_footprint_check(m.as_mut(), index);
                self.run_one(m.as_mut(), index)
            }
            Engines::Both(i, m) => {
                self.arm_footprint_check(m.as_mut(), index);
                // Both are stepped even when the first has already failed: an
                // engine that skipped a test is at a different point in the
                // corpus, and every later comparison becomes meaningless.
                let left = self.run_one(i.as_mut(), index);
                let right = self.run_one(m.as_mut(), index);
                // Comparing a refusal against an answer reports a divergence
                // between an engine that declined to start and one that did the
                // work, which is not a disagreement about what the program means.
                if matches!(&left, Err(d) if ply_eval::is_machine_only(d)) {
                    return right;
                }
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
                match compare_outcomes(i.as_ref(), m.as_ref(), &subject, Some(index), &left, &right)
                {
                    Some(d) => Err(d.to_diagnostic(Engine::Treewalk, Engine::Machine, span)),
                    // The authoritative engine's answer, so that which engine a
                    // run reports never depends on whether auditing was on. The
                    // two have just been proved equal, so this is a statement
                    // about provenance rather than about the value.
                    None => match EngineChoice::Both.primary() {
                        Engine::Machine => right,
                        Engine::Treewalk => left,
                    },
                }
            }
        }
    }
}

fn test_hash(hashes: &HashOutput, index: usize) -> Option<DefHash> {
    hashes.tests.get(index).copied()
}

/// `plan` is what a seeded test's cache entry is keyed on, so a selection made
/// against one plan says nothing about another. It is a parameter rather than a
/// second entry point beside the old one for exactly that reason: a caller that
/// kept the old signature while running a widened search would read and write
/// the wrong entry, silently.
pub fn select(check: &CheckOutput, hashes: &HashOutput, store: &Store, plan: &Plan) -> Selection {
    let plan = plan.clone().normalized();
    let total = check.tests.len();
    let mut reasons = Vec::with_capacity(total);
    let mut cached = Vec::new();
    let mut to_run = Vec::new();
    let mut narrowed: BTreeMap<usize, Plan> = BTreeMap::new();

    for (index, test) in check.tests.iter().enumerate() {
        let seeded = is_seeded(&test.footprint);
        let hash = test_hash(hashes, index);
        let stored = hash.map(|hash| store.get(result_key(hash, seeded, &plan)));

        // A `random` plan decomposes into one standalone claim per root, so a
        // widened root set owes only the roots nothing has answered for. The
        // plan key is what publishes the widened claim, and this run writes it.
        let owed = match (seeded, hash) {
            (true, Some(hash)) if writes_seed_keys(&plan) => plan
                .roots
                .iter()
                .copied()
                .filter(|&root| {
                    !matches!(
                        store.get(seed_key(hash, &Seed::root(root))),
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
                // Every root already passed on its own, so the widened plan is
                // proved by the roots it is made of and nothing needs running.
                Some(None) if owed.is_empty() => Reason::Cached,
                Some(None) => Reason::New,
                Some(Some(Outcome::Pass)) => Reason::Cached,
                // Never trust a stored failure. Nothing here writes one, so it
                // can only have come from an older or foreign writer, and
                // re-running is the only safe reading of it.
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

/// Turns each failure's raw suspect list into the ranked, annotated attribution
/// of ADR 0004. Separate from [`run_with`] because it needs the AST — a delta is
/// decided by re-normalizing bodies, not by comparing hashes — and because a
/// caller with no evaluator still wants the answers that need no hybrid.
///
/// Returns the diagnoser's own warnings, which are never failures: a diagnosis
/// that cannot be made leaves the failure exactly as it was.
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

    // Built once for the whole run and shared: it re-normalizes the entire
    // program, which is the expensive half of deciding `Edited` from `Derived`.
    let test_keys: Vec<Symbol> = check.tests.iter().map(|t| t.key.clone()).collect();
    let (renormalizer, mut warnings) =
        match Renormalizer::new(program, resolved, hashes, &test_keys) {
            Ok(renormalizer) => (Some(renormalizer), Vec::new()),
            Err(diagnostics) => (None, diagnostics),
        };
    let edges = DepEdges::from(hashes);

    // This run's own normalized bytes. A definition it introduced has no stored
    // body until the cache is flushed, and a hybrid that could not reach the
    // *current* side of a change would have nothing to flip to. The bytes are
    // only used where they reproduce the hash the run published, so a program
    // the front end assembled from cached pieces contributes nothing rather than
    // something stale.
    let fresh = ply_hash::hash_program_with_bodies(program, resolved)
        .map(|(_, bodies)| bodies)
        .unwrap_or_default();

    // A hybrid that went green is a true claim about exactly its own closure, so
    // it may be cached — but only after the borrow the search holds on the store
    // has ended.
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

        // Everything the mixture could need, on either side. `NoBodies` is a
        // cache somebody pruned — go and stop pruning; `NoHybrids` is a build
        // that cannot mix eras at all, which nothing outside can fix.
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
        // Every hybrid runs at the interleaving this failure happened in. A
        // hybrid that searches for its own answers a different question, and the
        // bisection then names whichever definition the other interleaving ran
        // through.
        let seed = failure.seed.clone();
        let mut builder = match (mixture, test_body, complete) {
            // A host-backed failure gets no builder at all. `precheck` already
            // refuses to search one, and this is the second lock: a trial is an
            // evaluation of the failing test, and the run that failed reached
            // the world outside the program. Nothing here may be reachable by
            // threading a binding into the trials to "make bisection work under
            // `--host`" — that is the change this arm exists to stop.
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

        // Without a renormalizer nothing can be told apart from a hash that
        // merely moved, so every change stays a candidate: a wider answer, never
        // a wrong one.
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
            // Never the failing test's own key. `H(all)` *is* the current
            // program, so a replay that goes green would otherwise cache a
            // `Pass` for the test this run just watched fail — and a red test
            // has to re-run until it goes green. Under a pinned seed the key is
            // derived, so the bare hash alone is not the whole of it.
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

    // A hybrid's test hash covers its whole closure, so `Pass` under it is true
    // of that configuration and of nothing else. `observe_definitions` is a
    // different claim entirely and must never be made here: a definition proved
    // fine *in a hybrid* has not been vindicated in the real program, and
    // recording it would empty the next run's suspect set.
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
    engine: EngineChoice,
    search: Search,
    hosts: Hosting<'_>,
) -> RunReport {
    let executor = InterpExecutor::new(program, resolved, check)
        .with_engine(engine)
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
                // The same order `precheck` applies, so a report nobody
                // diagnosed says what a diagnosed one would have.
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
                // The runtime is authoritative: this run reached a socket, so
                // its green verdict is a statement about that socket at that
                // moment and about nothing the next run will face. Nothing is
                // written, in either direction, whatever the prediction said.
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
                    );
                    if record == Record::Unobserved {
                        warnings.push(unobserved_search(&test.key));
                    }
                    for key in record.keys() {
                        store.put(*key, Outcome::Pass);
                    }
                    if record.is_written() {
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

/// The test's row says something in its closure entered a `simulate` region and
/// the evaluator reported no search. Nothing is known about what actually ran,
/// so nothing is written — and a green run that quietly stopped caching would
/// look like a bug in selection, which is the one thing this system asks to be
/// trusted on.
fn unobserved_search(key: &Symbol) -> Diagnostic {
    Diagnostic::warning(
        codes::INTERNAL_ERROR,
        format!("`{key}` reads a simulation seed, but the run reported no search"),
    )
    .note("the test passed and its result was not cached, so it re-runs next time")
    .note("this is a defect in Ply rather than in the test; please report it")
}

/// A selected test that no group claims would be silently skipped, which is the
/// one outcome a test runner may never produce.
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
    /// What this test actually reached across the boundary, which decides
    /// whether its pass may be written.
    host: Option<ply_eval::host::HostUse>,
    /// What the host runtime reported while closing the entry point. A run-level
    /// warning rather than part of the verdict.
    teardown: Vec<Diagnostic>,
}

/// One worker per pool thread, built lazily so a group smaller than the pool
/// does not pay to construct interpreters that never run anything.
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
                    // Unwinding out of the middle of a worker leaves its
                    // invariants unknown, so the next test gets a fresh one.
                    worker = None;
                    (Some(panic_diagnostic(payload, check, index)), true)
                }
            };
            // After the unwind check: a worker whose invariants are unknown has
            // nothing to report about what it searched.
            let exploration = worker.as_ref().and_then(|w| executor.exploration(w));
            let host = worker.as_ref().and_then(|w| executor.host_use(w));
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
            });
        }
    });

    let mut out: Vec<Executed> = per_thread.into_iter().flatten().collect();
    out.sort_by_key(|e| e.index);
    out
}

/// Two evaluators of one language disagreeing is a defect in Ply by definition:
/// whatever the program means, at most one of the answers is it, and nothing in
/// the user's definition graph decides which. Bisecting it would name whichever
/// definition the disagreement happened to run through.
///
/// One seed producing two runs is the same class of defect and gets the same
/// treatment: a simulated run is meant to be a pure function of its definitions
/// and its seed, so a replay that reproduced something else is Ply's fault, and
/// no definition in the program decides which of the two runs was meant.
///
/// The two codes that must be read rather than observed. An unwind and an
/// `INTERNAL_ERROR` are things the run watched happen; both of these are
/// comparisons the run made and reported as an ordinary `Err`, so there is no
/// observation to read them off.
fn is_divergence(d: &Diagnostic) -> bool {
    d.code == codes::ENGINE_DIVERGENCE || d.code == codes::SIMULATION_DIVERGENCE
}

/// Ply's fault rather than the program's, read off the diagnostic.
///
/// `E0427` belongs here for the reason the divergence codes do: a host answer
/// outside the entry point's declared footprint is the run knowing that two of
/// its own answers disagree, and nothing in the definition graph decides which
/// was meant. Bisecting it would name whichever definition the disagreement
/// happened to run through — and the diagnostic's own last note already tells
/// the reader this is Ply's fault, so classifying it as the program's would
/// contradict the text the run prints.
///
/// `E0439` belongs here for the same reason and one more: bisecting a credential
/// that reached the boundary would re-run the program that carried it, once per
/// candidate, against the handler that must not receive it.
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

/// Definitions the store has never recorded seeing. Against a warm cache that
/// is exactly what the last edit touched, transitively.
fn changed_definitions(hashes: &HashOutput, store: &Store) -> BTreeSet<Symbol> {
    hashes
        .defs
        .iter()
        .filter(|(_, hash)| !store.knows_definition(**hash))
        .map(|(name, _)| name.clone())
        .collect()
}

/// The single place a test's key becomes a key into the hash graph: two callers
/// disagreeing about that convention would silently mis-attribute a failure
/// rather than fail.
fn closure_of<'a>(hashes: &'a HashOutput, key: &Symbol) -> Option<&'a BTreeSet<Symbol>> {
    hashes.closure.get(key)
}

/// Names are how two eras of a program are lined up; hashes are what they are
/// compared by. A baseline needs both — and it needs one entry per *namespace*,
/// because a `fn` and a `type` may share a name and preferring either drops the
/// other's hash from the record for good.
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

/// Hands the store every definition except those an *unresolved* test reached:
/// one that failed, or that was selected and never executed because a filter or
/// a stale selection dropped it.
///
/// Recording a definition retires it as a suspect, and neither of those
/// outcomes is evidence that anything under it is fine. Recording them anyway is
/// what empties the suspect set on the second `ply test` of the same red code.
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

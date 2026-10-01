//! Running a test once, or one interleaving of it, on whichever thread asks, and filing what a run
//! of the tests the program chose came to under the keys it handed over. Which tests run, in which
//! classes, at once or not, and which interleavings a seeded test is searched at, is the program's.

pub mod bisect;
pub mod hybrid;
pub mod obligation;
pub mod sim;

use ply_eval::host::{HostBinding, HostUse};
use ply_eval::{
    CheckOutput, DefHash, Diagnostic, HashOutput, Interleaving, Machine, Seed, Span, Symbol, codes,
};
use ply_store::{Outcome, PassRecord, Store};
use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use bisect::{
    Absent, Baseline, ChangeSet, Classify, DefKey, Ns, Regression, Rehashed, Row, StoreClassify,
    Trial, TrialOutcome, Unresolved, change_set,
};
pub use hybrid::{BodyHybrid, Mixture, Signature};
pub use sim::{Record, SimSummary, is_seeded, record_under};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
/// and `--std` to it, reports on exactly what is left of `runs` by `groups`, and writes a pass under
/// the keys `filed` names for it.
#[derive(Clone, Debug, Default)]
pub struct Choice {
    /// Test indices to execute, ascending.
    pub runs: Vec<usize>,
    /// The reason for every test the keys row named, indexed by test index. The program decides for
    /// all of them, whether or not this run's filter will report on one.
    pub reasons: Vec<Reason>,
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
    /// Every key each running test's pass is written under.
    pub filed: BTreeMap<usize, Vec<DefHash>>,
    /// Test indices this run was never asked to decide: a test outside the root package, or a
    /// shipped module's without `--std`.
    pub out_of_scope: BTreeSet<usize>,
}

impl Selection {
    /// The runtime's view of what the program decided. The evidence a cached test is reported with is
    /// always a pass — a stored failure is never `Cached` — so nothing here has to read the store.
    pub fn chosen(choice: &Choice, check: &CheckOutput) -> Selection {
        let total = check.tests.len();
        let cached: Vec<(usize, Outcome)> = (0..total)
            .filter(|i| choice.reasons.get(*i) == Some(&Reason::Cached))
            .map(|i| (i, Outcome::Pass))
            .collect();
        Selection {
            total,
            cached,
            to_run: choice.runs.clone(),
            groups: choice.groups.clone(),
            reasons: choice.reasons.clone(),
            filed: choice.filed.clone(),
            out_of_scope: BTreeSet::new(),
        }
    }

    /// The same selection over the tests a filter keeps. `--filter` cannot change which tests
    /// conflict, so a class only loses members; a cached result for a test the run does not report
    /// on goes with it.
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
        out.filed.retain(|index, _| keeps(index));
        out
    }

    pub fn reason(&self, index: usize) -> Option<Reason> {
        self.reasons.get(index).copied()
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
            .field("filed", &self.filed)
            .finish()
    }
}

/// What a search run beside the pruned one explored; `bounded` when a spent budget or a failure
/// stopped it short of its frontier, so the count is a lower bound.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cost {
    pub explored: u32,
    pub bounded: bool,
}

impl fmt::Display for Cost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bounded {
            write!(f, ">= {}", self.explored)
        } else {
            write!(f, "{}", self.explored)
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RaceSite {
    pub task: u64,
    pub definition: Option<Symbol>,
    pub access: String,
    pub span: Span,
}

/// Two steps whose reordering at scheduling point `at` turned a pass into a failure.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Race {
    pub left: RaceSite,
    pub right: RaceSite,
    pub at: u32,
}

/// What a seeded test's search ran, as the program that searched it reports it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Searched {
    pub explored: u32,
    /// Every interleaving ran, up to reordering steps that do not conflict.
    pub exhaustive: bool,
    /// The budget was spent.
    pub exhausted: bool,
    /// `--measure-reduction` only: the same search with every pair of steps dependent.
    pub naive: Option<Cost>,
    /// `--measure-reduction` only: the same search with every step's vector clock withheld.
    pub blind: Option<Cost>,
    pub steps: u64,
    /// Nanoseconds of virtual time the last interleaving consumed.
    pub virtual_time: i64,
    pub failure: Option<Seed>,
    pub race: Option<Race>,
}

impl Searched {
    /// How many times more an unpruned search would have run.
    pub fn reduction(&self) -> Option<f64> {
        let naive = self.naive?;
        (self.explored > 0).then(|| f64::from(naive.explored) / f64::from(self.explored))
    }

    pub fn is_cacheable(&self) -> bool {
        self.failure.is_none() && !self.exhausted
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    pub simulation: Option<Searched>,
    /// Absent when nothing was written: a spent budget or an unobserved search proved nothing.
    pub recorded: Option<Record>,
    pub backend: Option<BackendUse>,
    /// The operations the test performed, handled ones included, summed over every interleaving
    /// a search ran.
    pub performs: u64,
}

impl TestResult {
    pub fn passed(&self) -> bool {
        self.status == Status::Passed
    }

    pub fn green_but_uncached(&self) -> bool {
        self.passed() && matches!(self.recorded, Some(Record::Exhausted | Record::Unobserved))
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
    pub seed: Option<Seed>,
    /// The two steps whose reordering flipped a passing interleaving to this one.
    pub race: Option<Race>,
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

#[derive(Default, Clone)]
pub struct Hosting {
    binding: Option<Arc<HostBinding>>,
    /// A factory: a runtime handle belongs to one thread, and a test runs on whichever asks.
    runtime: Option<ply_eval::RuntimeFactory>,
}

impl Hosting {
    pub fn hermetic() -> Hosting {
        Hosting::default()
    }

    pub fn with_binding(mut self, binding: Arc<HostBinding>) -> Hosting {
        self.binding = Some(binding);
        self
    }

    /// What a [`ply_eval::host::HostAnswer::Pending`] is polled on.
    pub fn with_runtime(mut self, runtime: ply_eval::RuntimeFactory) -> Hosting {
        self.runtime = Some(runtime);
        self
    }
}

/// What a test runs on: the program, the unit built from it, and the host it may reach.
pub struct InterpExecutor<'a> {
    front: &'a ply_eval::Front,
    hosts: Hosting,
    provider: &'static dyn ply_eval::Provider,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackendUse {
    /// Bodies this test ran natively instead of evaluating.
    pub entries: u64,
    /// Calls the backend was offered and declined.
    pub declines: u64,
}

impl<'a> InterpExecutor<'a> {
    /// Every test runs on a tier attached from `provider`, the unit built from `front`.
    pub fn new(
        front: &'a ply_eval::Front,
        provider: &'static dyn ply_eval::Provider,
    ) -> InterpExecutor<'a> {
        InterpExecutor {
            front,
            hosts: Hosting::hermetic(),
            provider,
        }
    }

    pub fn with_hosts(mut self, hosts: Hosting) -> Self {
        self.hosts = hosts;
        self
    }

    /// The tier this thread runs the unit on, attached once per thread: a test is a fresh machine
    /// over it, never a fresh attachment.
    fn tier(&self) -> Rc<dyn ply_eval::Compiled> {
        thread_local! {
            static ATTACHED: std::cell::RefCell<Vec<(usize, Rc<dyn ply_eval::Compiled>)>> =
                const { std::cell::RefCell::new(Vec::new()) };
        }
        let key = std::ptr::from_ref(self.provider).cast::<()>() as usize;
        ATTACHED.with(|attached| {
            if let Some((_, tier)) = attached.borrow().iter().find(|(k, _)| *k == key) {
                return Rc::clone(tier);
            }
            let tier = self.provider.attach();
            attached.borrow_mut().push((key, Rc::clone(&tier)));
            tier
        })
    }

    fn machine(&self) -> Result<Machine<'a>, Diagnostic> {
        let mut machine = Machine::new(self.front, self.tier())?;
        if let Some(binding) = &self.hosts.binding {
            machine.set_host_binding(Arc::clone(binding));
        }
        if let Some(runtime) = &self.hosts.runtime {
            machine.set_host_runtime(Arc::clone(runtime));
        }
        Ok(machine)
    }

    /// States this entry point's footprint claim, so a host answer outside it is `E0427`.
    fn arm_footprint_check(&self, machine: &mut Machine<'a>, index: usize) {
        if let Some(test) = self.front.check.tests.get(index) {
            machine.set_declared_footprint(test.footprint.clone());
        }
    }
}

/// What running one test, or settling one search, came to.
#[derive(Clone, Debug)]
pub struct Executed {
    pub index: usize,
    pub duration: Duration,
    pub failure: Option<Diagnostic>,
    /// Ply unwound rather than the program failing.
    pub panicked: bool,
    pub searched: Option<Searched>,
    /// The roots a seeded test's search started from.
    pub seeds: usize,
    /// What this test reached across the boundary, which decides whether its pass may be written.
    pub host: Option<HostUse>,
    pub teardown: Vec<Diagnostic>,
    pub backend: Option<BackendUse>,
    pub performs: u64,
}

/// What one machine's entry came to, before it is anyone's report.
struct Entered {
    outcome: Result<(), Diagnostic>,
    interleaving: Option<Interleaving>,
    performs: u64,
    host: Option<HostUse>,
    backend: BackendUse,
    teardown: Vec<Diagnostic>,
}

fn entered<'a>(
    executor: &InterpExecutor<'a>,
    index: usize,
    seeded: Option<(&Seed, u32, bool)>,
) -> Result<Entered, Diagnostic> {
    let mut machine = executor.machine()?;
    executor.arm_footprint_check(&mut machine, index);
    if let Some((seed, steps, re_executed)) = seeded {
        machine.set_re_executed(re_executed);
        sim::seed_run(&mut machine, seed, steps);
    }
    let outcome = machine.eval_test(index);
    let (entries, declines) = machine.compiled_counts();
    let mut teardown = ply_eval::rc::take_cycles();
    teardown.extend(machine.take_teardown_warnings());
    Ok(Entered {
        interleaving: seeded.and_then(|_| sim::interleaving_of(&machine, &outcome)),
        outcome,
        performs: machine.trace().performs(),
        host: machine.host_use().cloned(),
        backend: BackendUse { entries, declines },
        teardown,
    })
}

/// One test run once on this thread, as a test with no `simulate` region in its closure runs.
pub fn executed(executor: &InterpExecutor<'_>, check: &CheckOutput, index: usize) -> Executed {
    contained(check, index, || {
        let started = Instant::now();
        match entered(executor, index, None) {
            Ok(e) => Executed {
                index,
                duration: started.elapsed(),
                failure: e.outcome.err(),
                panicked: false,
                searched: None,
                seeds: 0,
                host: e.host,
                teardown: e.teardown,
                backend: Some(e.backend),
                performs: e.performs,
            },
            Err(refused) => Executed::refused(index, refused),
        }
    })
}

/// `run` on this thread, an unwind out of it reported as Ply's defect at the test's source.
pub fn contained(check: &CheckOutput, index: usize, run: impl FnOnce() -> Executed) -> Executed {
    let started = Instant::now();
    match catch_unwind(AssertUnwindSafe(run)) {
        Ok(executed) => executed,
        Err(payload) => Executed {
            panicked: true,
            duration: started.elapsed(),
            ..Executed::refused(index, panic_diagnostic(payload, check, index))
        },
    }
}

/// One interleaving of a seeded test, the one `seed` names, on this thread.
pub struct Interleaved {
    pub interleaving: Interleaving,
    /// The test entered a `simulate` region, and so had a schedule to vary.
    pub observed: bool,
    pub panicked: bool,
    pub duration: Duration,
    pub host: Option<HostUse>,
    pub backend: BackendUse,
    pub performs: u64,
    pub teardown: Vec<Diagnostic>,
}

pub fn interleaved(
    executor: &InterpExecutor<'_>,
    check: &CheckOutput,
    index: usize,
    seed: &Seed,
    steps: u32,
    re_executed: bool,
) -> Interleaved {
    let started = Instant::now();
    let result = catch_unwind(AssertUnwindSafe(|| {
        entered(executor, index, Some((seed, steps, re_executed)))
    }));
    let duration = started.elapsed();
    let failed = |diagnostic: Diagnostic, panicked: bool| Interleaved {
        panicked,
        duration,
        ..Interleaved::refused(diagnostic)
    };
    match result {
        Ok(Ok(e)) => Interleaved {
            observed: e.interleaving.is_some(),
            interleaving: match e.interleaving {
                Some(interleaving) => interleaving,
                None => match e.outcome {
                    Ok(()) => Interleaving::passed(Vec::new()),
                    Err(diagnostic) => Interleaving::failed(Vec::new(), diagnostic),
                },
            },
            panicked: false,
            duration,
            host: e.host,
            backend: e.backend,
            performs: e.performs,
            teardown: e.teardown,
        },
        Ok(Err(refused)) => failed(refused, false),
        Err(payload) => failed(panic_diagnostic(payload, check, index), true),
    }
}

/// What a seeded test's interleavings came to as each one ran: what they reached and cost between
/// them, and each failing one's diagnostic, held by the order it failed in.
#[derive(Default)]
pub struct Interleavings {
    failures: Vec<Diagnostic>,
    /// Some interleaving never entered a `simulate` region, so it had no schedule to vary.
    unobserved: bool,
    panicked: bool,
    duration: Duration,
    host: Option<HostUse>,
    backend: BackendUse,
    performs: u64,
    teardown: Vec<Diagnostic>,
}

impl Interleavings {
    /// Folds one more in, answering the id a failing one's diagnostic is held under.
    pub fn add(&mut self, run: &Interleaved) -> Option<usize> {
        self.unobserved |= !run.observed;
        self.panicked |= run.panicked;
        self.duration += run.duration;
        if let Some(reached) = &run.host {
            let into = self.host.get_or_insert_with(Default::default);
            into.atoms = into.atoms.union(&reached.atoms);
            into.operations = into.operations.saturating_add(reached.operations);
        }
        self.backend.entries = self.backend.entries.saturating_add(run.backend.entries);
        self.backend.declines = self.backend.declines.saturating_add(run.backend.declines);
        self.performs = self.performs.saturating_add(run.performs);
        self.teardown.extend(run.teardown.iter().cloned());
        match &run.interleaving.verdict {
            ply_eval::Verdict::Failed(diagnostic) => {
                self.failures.push(diagnostic.clone());
                Some(self.failures.len() - 1)
            }
            ply_eval::Verdict::Passed => None,
        }
    }

    /// Each failing interleaving's diagnostic, by its id.
    pub fn held(&self) -> &[Diagnostic] {
        &self.failures
    }

    /// The test's result once the search that ran these settled on `searched`, stopping at
    /// `failure` if it stopped at one, from `seeds` roots. An unobserved search is no search.
    pub fn settled(
        self,
        index: usize,
        searched: Searched,
        failure: Option<Diagnostic>,
        seeds: usize,
    ) -> Executed {
        Executed {
            index,
            duration: self.duration,
            failure,
            panicked: self.panicked,
            searched: (!self.unobserved).then_some(searched),
            seeds,
            host: self.host,
            teardown: self.teardown,
            backend: Some(self.backend),
            performs: self.performs,
        }
    }
}

impl Interleaved {
    /// An interleaving nothing could run: only the refusal is known.
    pub fn refused(refusal: Diagnostic) -> Interleaved {
        Interleaved {
            interleaving: Interleaving::failed(Vec::new(), refusal),
            observed: false,
            panicked: false,
            duration: Duration::ZERO,
            host: None,
            backend: BackendUse::default(),
            performs: 0,
            teardown: Vec::new(),
        }
    }
}

impl Executed {
    /// A test nothing could run: only the refusal is known.
    pub fn refused(index: usize, refusal: Diagnostic) -> Executed {
        Executed {
            index,
            duration: Duration::ZERO,
            failure: Some(refusal),
            panicked: false,
            searched: None,
            seeds: 0,
            host: None,
            teardown: Vec::new(),
            backend: None,
            performs: 0,
        }
    }
}

fn test_hash(hashes: &HashOutput, index: usize) -> Option<DefHash> {
    hashes.tests.get(index).copied()
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
    pub absent: Option<Absent>,
}

/// What each failure's cause is decided from, for the program that reads the report to decide it:
/// the facts its change set is classified from, and whether a mixture of the two eras can be built.
/// A failure that never passed has no baseline, and so no facts.
pub fn diagnose_failures(
    report: &RunReport,
    sources: &[(String, String)],
    front: &ply_eval::Front,
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
            // Refusing a host-backed failure is the program's gate; a mixture runs hermetically.
            let runnable = match test_body {
                Some(test) if complete => Some((mixture, test)),
                _ => None,
            };
            let absent = match (&runnable, complete) {
                (Some(_), _) => None,
                (None, false) => Some(Absent::NoBodies),
                (None, true) => Some(Absent::NoHybrids),
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

/// What the program's run came to, in test order, filed under the keys it named: what a run prints,
/// written into the store as the report is assembled. `duration` is the run's wall clock.
pub fn concluded(
    selection: &Selection,
    check: &CheckOutput,
    hashes: &HashOutput,
    store: &mut Store,
    mut ran: Vec<Executed>,
    duration: Duration,
) -> RunReport {
    let mut warnings = Vec::new();
    let changed = changed_definitions(hashes, store);
    ran.sort_by_key(|e| e.index);
    let seeds: BTreeMap<usize, usize> = ran.iter().map(|e| (e.index, e.seeds)).collect();
    let group_of: BTreeMap<usize, usize> = selection
        .groups
        .iter()
        .enumerate()
        .flat_map(|(g, class)| class.iter().map(move |&i| (i, g)))
        .collect();

    let mut results: Vec<TestResult> = Vec::new();
    let mut failures = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut abandoned = 0usize;

    for executed in ran {
        let index = executed.index;
        let Some(test) = check.tests.get(index) else {
            warnings.push(
                Diagnostic::warning(
                    codes::INTERNAL_ERROR,
                    format!(
                        "a run named test {index}, but the module defines {}",
                        check.tests.len()
                    ),
                )
                .note("the choice was made against another program; the stale index was skipped"),
            );
            continue;
        };
        warnings.extend(executed.teardown);
        let hash = test_hash(hashes, index);
        let seeded = is_seeded(&test.footprint);
        let host_backed = executed.host.is_some();
        let abandoned_run = executed.failure.as_ref().is_some_and(is_abandoned);
        let defect = executed
            .failure
            .as_ref()
            .is_some_and(|d| executed.panicked || codes::is_defect(d.code));
        let status = match (&executed.failure, defect) {
            (None, _) => Status::Passed,
            (Some(_), _) if abandoned_run => Status::Abandoned,
            (Some(_), false) => Status::Failed,
            (Some(_), true) => Status::Panicked,
        };
        let searched = executed.searched;
        let mut recorded = None;

        if abandoned_run {
            // The clock is a fact about this machine: no verdict, no suspects, nothing stored.
            abandoned += 1;
        } else if let Some(diagnostic) = &executed.failure {
            failed += 1;
            failures.push(Failure {
                name: test.name.clone(),
                key: test.key.clone(),
                diagnostic: diagnostic.clone(),
                defect,
                host: host_backed,
                suspects: suspects_for(hashes, &test.key, &changed),
                seed: searched.as_ref().and_then(|e| e.failure.clone()),
                race: searched.as_ref().and_then(|e| e.race.clone()),
            });
        } else if executed.host.is_some() {
            // This run reached a socket: its green verdict is about that moment only.
            passed += 1;
            recorded = Some(Record::Host);
        } else {
            passed += 1;
            if let Some(filed) = selection.filed.get(&index) {
                let record = record_under(filed, seeded, searched.as_ref());
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
            group: group_of
                .get(&index)
                .copied()
                .unwrap_or(selection.groups.len()),
            duration: executed.duration,
            status,
            failure: executed.failure,
            simulation: searched,
            recorded,
            backend: executed.backend,
            performs: executed.performs,
        });
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

    let simulation = summarize_simulation(&results, &seeds);

    RunReport {
        passed,
        failed,
        abandoned,
        cached: selection.cached.len(),
        failures,
        duration,
        results,
        warnings,
        simulation,
    }
}

fn summarize_simulation(results: &[TestResult], seeds: &BTreeMap<usize, usize>) -> SimSummary {
    let mut summary = SimSummary {
        total: results.len(),
        ..SimSummary::default()
    };
    for result in results {
        let Some(searched) = &result.simulation else {
            continue;
        };
        summary.simulated += 1;
        summary.seeds += seeds.get(&result.index).copied().unwrap_or(0);
        summary.interleavings += u64::from(searched.explored);
        summary.exhaustive += usize::from(searched.exhaustive);
        summary.exhausted += usize::from(searched.exhausted);
        summary.failed += usize::from(searched.failure.is_some());
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
        None => (format!("test {index}"), ply_eval::Span::DUMMY),
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

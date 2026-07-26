//! Selection, scheduling, and running — where "a test re-runs iff its hash is
//! absent from the cache" stops being a claim and becomes an observable.

pub mod bisect;
pub mod diagnose;
pub mod hybrid;
pub mod report;
pub mod schedule;
pub mod slice;

#[cfg(test)]
mod tests;

use ply_core::{CheckOutput, Footprint};
use ply_eval::Interp;
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub use bisect::{
    Baseline, Bisection, Budget, Change, ChangeKind, Classify, Cluster, Confidence, DefKey, Delta,
    DepEdges, Diff, EraTable, FusionReason, Hybrid, Mode, Ns, Regression, Renormalizer,
    SearchStats, Skipped, StoreClassify, Trial, TrialOutcome, Unresolved, Verdict, bisect, diff,
    precheck,
};
pub use diagnose::{Evidence, Options, diagnose};
pub use hybrid::{BodyHybrid, Mixture, Signature};
pub use schedule::group_by_conflict;
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
}

impl Selection {
    pub fn reason(&self, index: usize) -> Option<Reason> {
        self.reasons.get(index).copied()
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
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passed,
    Failed,
    /// Kept distinct from `Failed` so a defect in Ply cannot be read as a red
    /// test.
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
}

impl TestResult {
    pub fn passed(&self) -> bool {
        self.status == Status::Passed
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
    /// Definitions in this test's closure whose hash is not in the store —
    /// the suspects for this failure.
    pub suspects: Vec<Symbol>,
    /// What failed, structured. `None` until the evaluator carries the payload
    /// rather than rendering it into the diagnostic's notes.
    pub assertion: Option<Assertion>,
    pub attribution: Attribution,
}

#[derive(Clone, Debug)]
pub struct RunReport {
    pub passed: usize,
    pub failed: usize,
    pub cached: usize,
    pub failures: Vec<Failure>,
    pub duration: Duration,
    /// Every test that actually ran, in execution order.
    pub results: Vec<TestResult>,
    /// Problems with the run itself rather than with any test — a cache that
    /// could not be written, a selection naming a test that does not exist.
    pub warnings: Vec<Diagnostic>,
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
        }
    }
}

impl<'a> Executor for InterpExecutor<'a> {
    type Worker = Interp<'a>;

    fn worker(&self) -> Interp<'a> {
        Interp::new(self.program, self.resolved, self.check)
    }

    fn execute(&self, worker: &mut Interp<'a>, index: usize) -> Result<(), Diagnostic> {
        match self.addresses.get(index) {
            Some((module, ordinal)) => worker.eval_test_in(module, *ordinal),
            None => worker.eval_test(index),
        }
    }
}

fn test_hash(hashes: &HashOutput, index: usize) -> Option<DefHash> {
    hashes.tests.get(index).copied()
}

pub fn select(check: &CheckOutput, hashes: &HashOutput, store: &Store) -> Selection {
    let total = check.tests.len();
    let mut reasons = Vec::with_capacity(total);
    let mut cached = Vec::new();
    let mut to_run = Vec::new();

    for (index, test) in check.tests.iter().enumerate() {
        let stored = test_hash(hashes, index).map(|hash| store.get(hash));

        let reason = if test.nondet {
            Reason::Nondet
        } else {
            match stored {
                None => Reason::Unhashed,
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
            _ => to_run.push(index),
        }
        reasons.push(reason);
    }

    let footprints: Vec<(usize, Footprint)> = to_run
        .iter()
        .map(|&i| (i, check.tests[i].footprint.clone()))
        .collect();
    let groups = group_by_conflict(&footprints);

    Selection {
        total,
        cached,
        to_run,
        groups,
        reasons,
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
        let panicked = failure.diagnostic.code == codes::RUNTIME_ERROR;
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
            panicked,
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
        let test_body = check
            .tests
            .iter()
            .position(|t| t.key == failure.key)
            .and_then(|index| BodyHybrid::test_body(&fresh, hashes, index));
        let absent = match (&mixture, complete) {
            (Some(_), false) => Skipped::NoBodies,
            _ => Skipped::NoHybrids,
        };
        let mut builder = match (mixture, test_body, complete) {
            (Some(mixture), Some(test), true) => Some(BodyHybrid::new(
                store,
                &fresh,
                mixture,
                test,
                Signature::of(&failure.diagnostic),
            )),
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
            // Never the failing test's own hash. `H(all)` *is* the current
            // program, so a replay that goes green would otherwise cache a
            // `Pass` for the test this run just watched fail — and a red test
            // has to re-run until it goes green.
            proved.extend(
                builder
                    .take_proved()
                    .into_iter()
                    .filter(|hash| Some(*hash) != test_hash),
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

pub fn run(
    selection: &Selection,
    program: &Program,
    resolved: &Resolved,
    check: &CheckOutput,
    hashes: &HashOutput,
    store: &mut Store,
) -> RunReport {
    let executor = InterpExecutor::new(program, resolved, check);
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
                        codes::RUNTIME_ERROR,
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
            let test = &check.tests[index];
            let hash = test_hash(hashes, index);
            let status = match (&executed.failure, executed.panicked) {
                (None, _) => Status::Passed,
                (Some(_), false) => Status::Failed,
                (Some(_), true) => Status::Panicked,
            };

            if let Some(diagnostic) = &executed.failure {
                failed += 1;
                let suspects = suspects_for(hashes, &test.key, &changed);
                let mut attribution = Attribution::from_suspects(&suspects, hashes);
                if executed.panicked {
                    attribution.bisection = Bisection::not_attempted(Skipped::Panicked);
                } else if test.nondet {
                    attribution.bisection = Bisection::not_attempted(Skipped::Nondet);
                }
                failures.push(Failure {
                    name: test.name.clone(),
                    key: test.key.clone(),
                    diagnostic: diagnostic.clone(),
                    suspects,
                    assertion: None,
                    attribution,
                });
            } else {
                passed += 1;
                if !test.nondet
                    && let Some(hash) = hash
                {
                    store.put(hash, Outcome::Pass);
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
            }

            results.push(TestResult {
                index,
                name: test.name.clone(),
                hash,
                group: group_index,
                duration: executed.duration,
                status,
                failure: executed.failure,
            });
        }
    }

    observe_definitions(store, hashes, check, selection, &results);

    if let Err(e) = store.flush() {
        warnings.push(
            Diagnostic::warning(
                codes::RUNTIME_ERROR,
                format!("could not write the test cache: {e}"),
            )
            .note("the run itself is valid; every test will simply be re-run next time"),
        );
    }

    RunReport {
        passed,
        failed,
        cached: selection.cached.len(),
        failures,
        duration: started.elapsed(),
        results,
        warnings,
    }
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
            codes::RUNTIME_ERROR,
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
            out.push(Executed {
                index,
                duration,
                failure,
                panicked,
            });
        }
    });

    let mut out: Vec<Executed> = per_thread.into_iter().flatten().collect();
    out.sort_by_key(|e| e.index);
    out
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
        codes::RUNTIME_ERROR,
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

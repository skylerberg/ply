//! Selection, scheduling, and running — where "a test re-runs iff its hash is
//! absent from the cache" stops being a claim and becomes an observable.

pub mod report;
pub mod schedule;

#[cfg(test)]
mod tests;

use ply_core::{CheckOutput, Footprint};
use ply_eval::Interp;
use ply_hash::{DefHash, HashOutput};
use ply_span::{Diagnostic, Symbol, codes};
use ply_store::{Outcome, Store};
use ply_syntax::ast::Module;
use serde::Serialize;
use std::any::Any;
use std::collections::BTreeSet;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub use schedule::group_by_conflict;

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

#[derive(Clone, Debug)]
pub struct Failure {
    pub name: String,
    pub diagnostic: Diagnostic,
    /// Definitions in this test's closure whose hash is not in the store —
    /// the suspects for this failure.
    pub suspects: Vec<Symbol>,
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
    pub module: &'a Module,
    pub check: &'a CheckOutput,
}

impl<'a> Executor for InterpExecutor<'a> {
    type Worker = Interp<'a>;

    fn worker(&self) -> Interp<'a> {
        Interp::new(self.module, self.check)
    }

    fn execute(&self, worker: &mut Interp<'a>, index: usize) -> Result<(), Diagnostic> {
        worker.eval_test(index)
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

    let footprints: Vec<(usize, Footprint)> =
        to_run.iter().map(|&i| (i, check.tests[i].footprint.clone())).collect();
    let groups = group_by_conflict(&footprints);

    Selection { total, cached, to_run, groups, reasons }
}

pub fn run(
    selection: &Selection,
    module: &Module,
    check: &CheckOutput,
    hashes: &HashOutput,
    store: &mut Store,
) -> RunReport {
    run_with(selection, check, hashes, store, &InterpExecutor { module, check })
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

    // Snapshotted before the first write: a test that passes in group 0 records
    // the hashes in its closure, and a definition they share would then stop
    // looking changed to a test that fails in group 3.
    let changed = absent_definitions(hashes, store);

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
                failures.push(Failure {
                    name: test.name.clone(),
                    diagnostic: diagnostic.clone(),
                    suspects: suspects_for(hashes, &test.name, &changed),
                });
            } else {
                passed += 1;
                if !test.nondet {
                    record_pass(store, hashes, check, index, hash);
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
    let orphans: Vec<usize> =
        selection.to_run.iter().copied().filter(|i| !scheduled.contains(i)).collect();
    if orphans.is_empty() {
        return selection.groups.clone();
    }
    warnings.push(
        Diagnostic::warning(
            codes::RUNTIME_ERROR,
            format!("{} selected tests were in no concurrency group", orphans.len()),
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
            out.push(Executed { index, duration, failure, panicked });
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
        Some(t) => (t.name.clone(), t.span),
        None => (format!("test {index}"), ply_span::Span::DUMMY),
    };

    Diagnostic::error(codes::RUNTIME_ERROR, format!("test `{name}` panicked: {message}"))
        .primary(span, "the interpreter panicked while running this test")
        .note("a panic is a defect in Ply itself, not in the test; please report it with this source")
        .note("the other tests still ran, and this one was not cached")
}

/// Definitions whose hash the store has never seen green. Against a warm cache
/// that is exactly what the last edit touched, transitively.
fn absent_definitions(hashes: &HashOutput, store: &Store) -> BTreeSet<Symbol> {
    hashes
        .defs
        .iter()
        .filter(|(_, hash)| store.get(**hash).is_none())
        .map(|(name, _)| name.clone())
        .collect()
}

fn suspects_for(hashes: &HashOutput, test_name: &str, changed: &BTreeSet<Symbol>) -> Vec<Symbol> {
    match hashes.closure.get(&Symbol::new(test_name)) {
        Some(closure) => closure
            .intersection(changed)
            .filter(|s| s.as_str() != test_name)
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

/// A green test vouches for every definition it reached, so their hashes are
/// recorded alongside its own. That record is what later lets a failure name the
/// handful of definitions that changed underneath it rather than its whole
/// closure.
fn record_pass(
    store: &mut Store,
    hashes: &HashOutput,
    check: &CheckOutput,
    index: usize,
    hash: Option<DefHash>,
) {
    if let Some(hash) = hash {
        store.put(hash, Outcome::Pass);
    }
    if let Some(closure) = hashes.closure.get(&Symbol::new(&check.tests[index].name)) {
        for name in closure {
            if let Some(&def) = hashes.defs.get(name) {
                store.put(def, Outcome::Pass);
            }
        }
    }
}

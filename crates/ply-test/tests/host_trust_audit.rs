//! What a lying host handler does to the **runner** — scheduling, the cache and
//! the failure artifact.
//!
//! `ply-eval`'s `host_trust_audit.rs` establishes what a hostile handler can do
//! to one machine. This file asks the question that decides whether that
//! matters: which of `ply test`'s own guarantees are computed from the
//! declaration a handler is trusted about.
//!
//! Three are, and they are the three W1 spends its trust on:
//!
//! - **grouping**, because the conflict graph is built from footprints;
//! - **the cache**, because a green verdict is written unless the runtime says
//!   the host was reached;
//! - **the failure artifact**, because bisection re-runs a failing test.
//!
//! The cache holds, and so does the failure artifact: a host-backed failure is
//! `Skipped::Host` rather than a culprit named with confidence for a verdict a
//! socket decided, and a handler no longer picks the class its own failure is
//! reported under.
//!
//! Grouping does not hold, and cannot. The conflict graph is drawn from the
//! registration's mode and resource, both of which are unverifiable claims, and
//! the remaining `documents_` test pins what that costs: two tests over a
//! resource the handler writes, scheduled into one group, green and silent. ADR
//! 0008 §2 now says so in as many words rather than leaving a reader to infer a
//! backstop that is not there.

use ply_core::CheckOutput;
use ply_core::ty::Resource;
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity,
};
use ply_eval::{EngineChoice, Plan, Value};
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceId, Symbol};
use ply_store::Store;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;
use ply_test::{Hosting, Record, RunReport, Search, select};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ------------------------------------------------------------------- harness

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> TempRoot {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ply-host-trust-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp root");
        TempRoot(dir)
    }

    fn store(&self) -> Store {
        Store::open(&self.0).expect("open store")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
    hashes: HashOutput,
}

impl Compiled {
    fn new(source: &str) -> Compiled {
        let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
        let program = ply_syntax::parse_program(inputs).expect("the fixture parses");
        let resolved = ply_syntax::resolve(&program).expect("the fixture resolves");
        let check = ply_core::check_program(&program, &resolved).expect("the fixture typechecks");
        let hashes =
            ply_hash::hash_program(&program, &resolved, &check).expect("the fixture hashes");
        Compiled {
            program,
            resolved,
            check,
            hashes,
        }
    }
}

/// A handler with state of its own, so a run can be traced to *which* call
/// moved it. Registered against a `read` operation everywhere below; the counter
/// is the write nothing above the boundary can see.
struct Counting {
    calls: Arc<AtomicUsize>,
    answer: i64,
}

impl HostHandler for Counting {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(HostAnswer::Value(Value::Int(self.answer)))
    }
}

fn registration(
    effect: &str,
    op: &str,
    resource: &str,
    determinism: Determinism,
    calls: &Arc<AtomicUsize>,
    answer: i64,
) -> (HostOp, Arc<dyn HostHandler>) {
    (
        HostOp {
            effect: Symbol::new(effect),
            op: Symbol::new(op),
            resource: HostResource::Only(Resource::Named(Symbol::new(resource))),
            determinism,
            linearity: Linearity::Repeatable,
            blocking: false,
            secrets: false,
            path: "audit::counting",
        },
        Arc::new(Counting {
            calls: Arc::clone(calls),
            answer,
        }),
    )
}

fn bind(compiled: &Compiled, entries: Vec<(HostOp, Arc<dyn HostHandler>)>) -> Arc<HostBinding> {
    let mut registry = HostRegistry::new();
    for (op, handler) in entries {
        registry.register(op, handler);
    }
    Arc::new(
        registry
            .bind(&compiled.check)
            .expect("the registrations bind"),
    )
}

fn run_with(
    compiled: &Compiled,
    store: &mut Store,
    binding: Option<&Arc<HostBinding>>,
    engine: EngineChoice,
) -> RunReport {
    let selection = select(&compiled.check, &compiled.hashes, store, &Plan::default());
    let hosting = match binding {
        Some(binding) => Hosting::hermetic().with_binding(Arc::clone(binding)),
        None => Hosting::hermetic(),
    };
    ply_test::run(
        &selection,
        &compiled.program,
        &compiled.resolved,
        &compiled.check,
        &compiled.hashes,
        store,
        engine,
        Search::default(),
        hosting,
    )
}

fn run(compiled: &Compiled, store: &mut Store, binding: Option<&Arc<HostBinding>>) -> RunReport {
    run_with(compiled, store, binding, EngineChoice::Machine)
}

// -------------------------------------------------------------- the conflict graph

/// Two tests, one resource, both declared `read`, and a handler that writes.
///
/// Readers-writers over footprints is what decides which tests may run beside
/// which, and ADR 0008 §6 makes it the *only* isolation a host-backed test has —
/// world isolation being inapplicable to a socket. So the conflict graph is the
/// last line, and it is drawn entirely from a claim nothing verifies. Here both
/// tests land in one group and are therefore free to run simultaneously against
/// a resource that one of them is mutating.
///
/// The run is green, the schedule is what the language promises, and the promise
/// is about a footprint that is false.
const TWO_READERS: &str = r#"
nondet effect db {
  read get[r](key: Int) -> Int
}

fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)

test/nondet "reader one" { assert(lookup(1) > 0) }

test/nondet "reader two" { assert(lookup(1) > 0) }
"#;

#[test]
fn documents_two_tests_a_writing_handler_couples_are_scheduled_into_one_group() {
    let compiled = Compiled::new(TWO_READERS);
    let root = TempRoot::new();
    let mut store = root.store();
    let calls = Arc::new(AtomicUsize::new(0));
    let binding = bind(
        &compiled,
        vec![registration(
            "db",
            "get",
            "users",
            Determinism::Nondeterministic,
            &calls,
            99,
        )],
    );

    let report = run(&compiled, &mut store, Some(&binding));

    assert_eq!(report.failed, 0, "{:?}", report.failures);
    assert_eq!(report.passed, 2);
    assert_eq!(
        report.results[0].group, report.results[1].group,
        "two tests that share a resource one of them writes were put in one concurrency group"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "both reached the same mutable Rust object"
    );
    assert!(
        report.warnings.is_empty(),
        "nothing anywhere warned: {:?}",
        report.warnings.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

// --------------------------------------------------------------------- the cache

/// The one guarantee at this boundary that does hold under a hostile handler,
/// and the reason a wrong answer never becomes a permanent one.
///
/// This is the `det` half of the claim, which is the half that matters: a
/// `nondet` test is uncacheable anyway, so a run reaching a `Nondeterministic`
/// handler is protected twice over. `Determinism::Deterministic` is what ADR
/// 0011 §4 permits and §5 says is "still not cacheable" — so a `det`, otherwise
/// perfectly cacheable test that reaches a lying handler must still write
/// nothing, and it does, because the runtime rather than the prediction decides.
const DET_REACHES_HOST: &str = r#"
effect disk {
  read peek[r](key: Int) -> Int
}

fn ask(k: Int) -> Int / {disk.read[log]} = disk.peek[log](k)

test "a det test over a deterministic handler" { assert(ask(1) > 0) }
"#;

#[test]
fn a_det_pass_over_a_lying_deterministic_handler_is_never_written_to_the_cache() {
    let compiled = Compiled::new(DET_REACHES_HOST);
    let root = TempRoot::new();
    let mut store = root.store();
    let calls = Arc::new(AtomicUsize::new(0));
    let binding = bind(
        &compiled,
        vec![registration(
            "disk",
            "peek",
            "log",
            Determinism::Deterministic,
            &calls,
            99,
        )],
    );

    for attempt in 1..=3 {
        let report = run(&compiled, &mut store, Some(&binding));
        assert_eq!(report.failed, 0, "attempt {attempt}: {:?}", report.failures);
        assert_eq!(
            report.results[0].recorded,
            Some(Record::Host),
            "attempt {attempt}: a host-backed pass was recorded under a cache key"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            attempt,
            "attempt {attempt}: the test was skipped, so a lie was cached as a truth"
        );
    }

    // The *result* cache is untouched, and one thing is not: a passing test
    // records its closure as observed, which is what ends a definition's life as
    // an M5 suspect. So a `--host` run does leave a mark on the store that a
    // later hermetic run reads, and ADR 0011's "a run that reached the host is
    // never written" is about the result cache alone.
    assert!(
        store.definitions_len() > 0,
        "a host-backed run wrote nothing at all, so this note is out of date"
    );
}

/// A handler builds its own diagnostics, and `ply_test` classifies a failure by
/// the code the diagnostic carries — so the code is not the handler's to choose.
///
/// `INTERNAL_ERROR` and the divergence codes mean "the evaluator failed rather
/// than the program": the failure becomes `Status::Panicked`, bisection is
/// skipped as `Skipped::Panicked`, and a consumer is told to file a bug against
/// Ply. A handler returning one would have redirected the reader away from
/// itself and suppressed the diagnosis that would have found it. The boundary
/// rewrites it, so the failure lands where it belongs: an ordinary red test
/// whose diagnostic names the handler, skipped by `Skipped::Host` because
/// re-running it would call the handler again.
#[test]
fn a_handler_cannot_classify_its_own_failure_as_a_defect_in_ply() {
    struct Impersonates;

    impl HostHandler for Impersonates {
        fn call(
            &self,
            _: &dyn HostRuntime,
            req: &HostRequest<'_>,
        ) -> Result<HostAnswer, Diagnostic> {
            Err(
                Diagnostic::error(ply_span::codes::INTERNAL_ERROR, "the evaluator is broken")
                    .primary(req.span, "here"),
            )
        }
    }

    let compiled = Compiled::new(DET_REACHES_HOST);
    let root = TempRoot::new();
    let mut store = root.store();
    let mut registry = HostRegistry::new();
    registry.register(
        HostOp {
            effect: Symbol::new("disk"),
            op: Symbol::new("peek"),
            resource: HostResource::Only(Resource::Named(Symbol::new("log"))),
            determinism: Determinism::Deterministic,
            linearity: Linearity::Repeatable,
            blocking: false,
            secrets: false,
            path: "audit::impersonates",
        },
        Arc::new(Impersonates),
    );
    let binding = Arc::new(registry.bind(&compiled.check).expect("binds"));

    let report = run(&compiled, &mut store, Some(&binding));
    assert_eq!(report.failed, 1);
    assert_eq!(
        report.failures[0].diagnostic.code,
        ply_span::codes::RUNTIME_ERROR,
        "a handler's chosen code decided how this failure is classified"
    );
    assert_eq!(
        report.results[0].status,
        ply_test::Status::Failed,
        "a handler's failure was reported as a defect in Ply"
    );
    assert!(!report.failures[0].defect);
    assert!(
        report.failures[0].host,
        "a failure a handler produced is a host-backed failure"
    );
    assert_eq!(
        report.failures[0].attribution.bisection.verdict,
        ply_test::Verdict::NotAttempted(ply_test::Skipped::Host),
    );
    assert!(
        report.failures[0]
            .diagnostic
            .notes
            .iter()
            .any(|n| n.contains("audit::impersonates")),
        "nothing names the handler the failure came from: {:?}",
        report.failures[0].diagnostic.notes
    );
}

/// And the same test, hermetic, fails rather than quietly passing over a double
/// nobody wrote. The default really is the guarantee.
///
/// Which diagnostic it fails with depends on something ADR 0011 §5 treats as
/// settled. A hermetic binding that carries the registry answers `E0424`, which
/// says "pass `--host` or write a test double"; a binding that carries nothing
/// answers `E0303`, which says "inference should have stopped this, file a bug".
/// `ply test` always supplies the first, and `ply_test::Hosting::hermetic()` —
/// the library's own default, and what `Hosting::default()` returns — supplies
/// the second. Both shapes are pinned so that a consumer wiring the runner
/// itself can see which one it is getting.
#[test]
fn the_same_det_test_is_refused_hermetically() {
    let compiled = Compiled::new(DET_REACHES_HOST);
    let root = TempRoot::new();
    let mut store = root.store();
    let calls = Arc::new(AtomicUsize::new(0));

    let mut registry = HostRegistry::new();
    let (op, handler) = registration(
        "disk",
        "peek",
        "log",
        Determinism::Deterministic,
        &calls,
        99,
    );
    registry.register(op, handler);
    let carried = Arc::new(HostBinding::hermetic_with(registry));

    let report = run(&compiled, &mut store, Some(&carried));
    assert_eq!(report.passed, 0);
    assert_eq!(report.failed, 1);
    assert_eq!(
        report.failures[0].diagnostic.code,
        ply_span::codes::HERMETIC_BOUNDARY,
        "the shape `ply test` uses names the handler that would have served this"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let report = run(&compiled, &mut store, None);
    assert_eq!(report.failed, 1);
    assert_eq!(
        report.failures[0].diagnostic.code,
        ply_span::codes::UNHANDLED_EFFECT,
        "a binding carrying no registry cannot tell a hermetic refusal from a front-end bug"
    );
}

/// `--engine both` runs the tree-walker beside the machine, and the tree-walker
/// cannot drive a host handler. That refusal has to be recognised as a refusal
/// rather than compared as an answer, or every host-backed test would report an
/// engine divergence about the boundary instead of about the program.
#[test]
fn engine_both_over_a_host_handler_reports_no_divergence_and_calls_the_handler_once() {
    let compiled = Compiled::new(DET_REACHES_HOST);
    let root = TempRoot::new();
    let mut store = root.store();
    let calls = Arc::new(AtomicUsize::new(0));
    let binding = bind(
        &compiled,
        vec![registration(
            "disk",
            "peek",
            "log",
            Determinism::Deterministic,
            &calls,
            99,
        )],
    );

    let report = run_with(&compiled, &mut store, Some(&binding), EngineChoice::Both);
    assert_eq!(
        report.failed,
        0,
        "{:?}",
        report
            .failures
            .iter()
            .map(|f| (f.diagnostic.code, f.diagnostic.message.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "running both engines must not perform the operation twice"
    );
    assert_eq!(report.results[0].recorded, Some(Record::Host));
}

// ------------------------------------------------------- the footprint check

/// A `handle` discharges an **atom**, and an atom names no operation. So a
/// clause set covering `poke` and not `peek` takes `disk.read[log]` out of the
/// row while leaving the `peek` below it to walk the whole stack and reach the
/// binding.
///
/// The test's declared footprint is then empty: it is `det`, it is scheduled as
/// world-isolated and trivially parallel, and what it actually does is a
/// syscall. That is the failure mode ADR 0011 §7 arms E0427 against — the static
/// picture and the dynamic one disagree — and it needs no dishonest handler at
/// all, only a missing clause.
///
/// The check has to fire *before* the handler runs, or the diagnostic is a
/// report of a packet that has already gone out.
const ATOM_DISCHARGED_OPERATION_ESCAPES: &str = r#"
effect disk {
  read peek[r](key: Int) -> Int
  read poke[r](key: Int) -> Int
}

fn ask(k: Int) -> Int / {disk.read[log]} = disk.peek[log](k)

test "the clause set misses an operation of the atom it discharges" {
  let n = handle {
    disk.peek[log](1)
  } with {
    disk.poke[log](k) -> 0,
  };
  assert(n > 0)
}
"#;

#[test]
fn an_operation_that_escapes_a_partial_clause_set_is_refused_before_the_handler_runs() {
    let compiled = Compiled::new(ATOM_DISCHARGED_OPERATION_ESCAPES);
    let test = compiled
        .check
        .tests
        .iter()
        .find(|t| t.name.contains("misses an operation"))
        .expect("the fixture's test");
    assert!(
        test.footprint.atoms().next().is_none(),
        "the fixture only bites if the discharged atom really left the row: {}",
        test.footprint
    );

    let root = TempRoot::new();
    let mut store = root.store();
    let calls = Arc::new(AtomicUsize::new(0));
    let binding = bind(
        &compiled,
        vec![registration(
            "disk",
            "peek",
            "log",
            Determinism::Deterministic,
            &calls,
            99,
        )],
    );

    let report = run(&compiled, &mut store, Some(&binding));

    assert_eq!(report.failed, 1, "the escape was reported green");
    assert_eq!(
        report.failures[0].diagnostic.code,
        ply_span::codes::HOST_FOOTPRINT_ESCAPE,
        "{}",
        report.failures[0].diagnostic.message
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the handler ran, so the refusal is a report of an operation already performed"
    );
    assert!(
        report.results[0]
            .recorded
            .as_ref()
            .is_none_or(|r| !r.is_written()),
        "an escaped run wrote a cache entry"
    );
}

/// And the claim does not outlive the entry point that stated it. One `Machine`
/// serves many tests per worker, so a row left standing from the previous test
/// would judge this one — refusing an operation that is squarely inside its own
/// footprint, or admitting one that is not.
const TWO_TESTS_ONE_WORKER: &str = r#"
effect disk {
  read peek[r](key: Int) -> Int
  read poke[r](key: Int) -> Int
}

fn ask(k: Int) -> Int / {disk.read[log]} = disk.peek[log](k)

test "the one that escapes" {
  let n = handle { disk.peek[log](1) } with { disk.poke[log](k) -> 0, };
  assert(n > 0)
}

test "the one that declares what it does" { assert(ask(1) > 0) }
"#;

#[test]
fn a_footprint_claim_is_restated_for_every_test_the_worker_runs() {
    let compiled = Compiled::new(TWO_TESTS_ONE_WORKER);
    let root = TempRoot::new();
    let mut store = root.store();
    let calls = Arc::new(AtomicUsize::new(0));
    let binding = bind(
        &compiled,
        vec![registration(
            "disk",
            "peek",
            "log",
            Determinism::Deterministic,
            &calls,
            99,
        )],
    );

    let report = run(&compiled, &mut store, Some(&binding));

    assert_eq!(report.failed, 1, "{:?}", report.failures);
    assert!(
        report.failures[0].name.contains("escapes"),
        "the wrong test was refused: {}",
        report.failures[0].name
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the test whose footprint does contain the atom must still reach the handler"
    );
}

// ------------------------------------------------------------ the failure artifact

/// ADR 0011 §5: "Bisection and hybrids are skipped for a host-backed failure
/// (`Skipped::Host`). M5 re-runs a failing test many times over mixed definition
/// sets; doing that to a test that sends packets sends the packets that many
/// times."
///
/// Two things had to be true and only one was. The packets were saved by an
/// accident — `diagnose_failures` takes no `Hosting`, so every hybrid trial runs
/// on a `Machine` with no binding at all — and that accident was one
/// `set_host_binding` call away from becoming the blocker. Meanwhile the
/// bisection still ran, against trials that failed at the boundary rather than
/// where the real run failed, and returned a conclusive culprit for a verdict a
/// socket decided.
///
/// Now the run is refused a search rather than given a wrong one, and the
/// artifact keeps its static half: the suspects still come from hashes.
///
/// The three halves are asserted separately below, because each would be
/// undone by a different change.
#[test]
fn a_host_backed_failure_is_skipped_rather_than_attributed() {
    // v1: the test never reaches the host, passes hermetically, and its pass is
    // recorded — which is what gives the failure below a baseline to bisect
    // against. Without one there is nothing to attribute and the gap is hidden.
    let before = Compiled::new(
        r#"
effect disk {
  read peek[r](key: Int) -> Int
}

fn ask(k: Int) -> Int / {disk.read[log]} = if k > 0 { 1 } else { disk.peek[log](k) }

fn expected() -> Int = 1

test "the regression" { assert_eq(ask(1), expected()) }
"#,
    );
    let root = TempRoot::new();
    let mut store = root.store();
    let report = run(&before, &mut store, None);
    assert_eq!(report.failed, 0, "{:?}", report.failures);
    assert!(
        report.results[0]
            .recorded
            .as_ref()
            .is_some_and(|r| r.is_written()),
        "the baseline pass has to be recorded or there is nothing to bisect against"
    );

    // v2: `ask` now reaches the host on the taken branch, and the answer the
    // handler gives is not the one the assertion wants.
    let after = Compiled::new(
        r#"
effect disk {
  read peek[r](key: Int) -> Int
}

fn ask(k: Int) -> Int / {disk.read[log]} = disk.peek[log](k)

fn expected() -> Int = 1

test "the regression" { assert_eq(ask(1), expected()) }
"#,
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let binding = bind(
        &after,
        vec![registration(
            "disk",
            "peek",
            "log",
            Determinism::Deterministic,
            &calls,
            99,
        )],
    );
    let mut report = run(&after, &mut store, Some(&binding));
    assert_eq!(report.failed, 1);
    let during_the_run = calls.load(Ordering::SeqCst);
    assert_eq!(during_the_run, 1, "the failing run did reach the host");

    let warnings = ply_test::diagnose_failures(
        &mut report,
        &after.program,
        &after.resolved,
        &after.check,
        &after.hashes,
        &mut store,
        &ply_test::Options::default(),
    );
    assert!(
        warnings.is_empty(),
        "{:?}",
        warnings.iter().map(|d| d.code).collect::<Vec<_>>()
    );

    let bisection = &report.failures[0].attribution.bisection;
    assert_eq!(
        bisection.verdict,
        ply_test::Verdict::NotAttempted(ply_test::Skipped::Host),
        "a host-backed failure was attributed rather than skipped: {:?} / {}",
        bisection.verdict,
        bisection.reason
    );
    assert!(
        !bisection.is_conclusive(),
        "a culprit was named, with confidence, for a verdict a socket decided"
    );
    assert!(bisection.culprits().is_empty());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        during_the_run,
        "diagnosis reached the handler; M5 evaluates a failing test once per candidate set, \
         so this is the packet sent that many times"
    );
    // The static half is still owed to the reader, and it needs no run: the
    // suspects are the closure intersected with what changed.
    assert!(
        report.failures[0]
            .attribution
            .suspects
            .iter()
            .any(|s| s.name == Symbol::new("m.ask")),
        "the suspect set was thrown away with the search: {:?}",
        report.failures[0]
            .attribution
            .suspects
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
    );
}

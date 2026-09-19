use crate::fixture::{Compiled, TierExecutor};
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity,
};
use ply_eval::{Plan, Value};
use ply_span::{Diagnostic, Symbol};
use ply_store::Store;
use ply_test::{Hosting, InterpExecutor, Record, RunReport, Search, select};
use ply_ty::Resource;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// A handler with state of its own, so a run can be traced to *which* call moved it.
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

fn run(compiled: &Compiled, store: &mut Store, binding: Option<&Arc<HostBinding>>) -> RunReport {
    let selection = select(
        &compiled.check,
        &compiled.hashes,
        store,
        &Plan::default(),
        &ply_test::Engine::Evaluator,
    );
    let hosting = match binding {
        Some(binding) => Hosting::hermetic().with_binding(Arc::clone(binding)),
        None => Hosting::hermetic(),
    };
    let (unit, spec) = compiled.tier();
    let executor = TierExecutor(
        InterpExecutor::new(&compiled.program, &compiled.resolved, &compiled.check)
            .with_backend(unit, spec)
            .with_search(Search::default())
            .with_hosts(hosting),
    );
    ply_test::run_with(
        &selection,
        &compiled.check,
        &compiled.hashes,
        store,
        &executor,
    )
}

/// Two tests, one resource, both declared `read`, and a handler that writes.
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

    // A pass still records its closure as observed, which ends each definition's life as a suspect.
    assert!(
        store.definitions_len() > 0,
        "a host-backed run wrote nothing at all, so this note is out of date"
    );
}

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

/// A `handle` discharges an **atom**, and an atom names no operation.
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

#[test]
fn a_host_backed_failure_is_skipped_rather_than_attributed() {
    // v1 never reaches the host, so its hermetic pass is the baseline the failure below bisects against.
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

    // v2: `ask` reaches the host on the taken branch, and the handler's answer fails the assertion.
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

    let mut sources: Vec<(String, String)> = after.texts.clone().into_iter().collect();
    sources.sort();
    ply_test::diagnose_failures(
        &mut report,
        &sources,
        &after.front(),
        &mut store,
        &ply_test::Options::default(),
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
    // Suspects need no run: they are the closure intersected with what changed.
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

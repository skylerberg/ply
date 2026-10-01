use crate::fixture::{Compiled, Seeds};
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity,
};
use ply_eval::{Diagnostic, Resource, Symbol, Value};
use ply_store::{Outcome, Store};
use ply_test::Hosting;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> TempRoot {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ply-host-selection-{}-{}",
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

impl Compiled {
    fn footprint_of_test(&self, name: &str) -> &ply_eval::Footprint {
        &self
            .check
            .tests
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named `{name}`"))
            .footprint
    }
}

struct Counting {
    calls: Arc<AtomicUsize>,
}

impl HostHandler for Counting {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(HostAnswer::Value(Value::Int(99)))
    }
}

fn registry(
    effect: &str,
    op: &str,
    determinism: Determinism,
    calls: &Arc<AtomicUsize>,
) -> HostRegistry {
    let mut registry = HostRegistry::new();
    registry.register(
        HostOp {
            effect: Symbol::new(effect),
            op: Symbol::new(op),
            resource: HostResource::Only(Resource::Named(Symbol::new("log"))),
            determinism,
            linearity: Linearity::AtMostOnce,
            blocking: false,
            secrets: false,
            path: "audit::counting",
        },
        Arc::new(Counting {
            calls: Arc::clone(calls),
        }),
    );
    registry
}

fn run(
    compiled: &Compiled,
    store: &mut Store,
    binding: Option<&Arc<HostBinding>>,
) -> ply_test::RunReport {
    let hosting = match binding {
        Some(binding) => Hosting::hermetic().with_binding(Arc::clone(binding)),
        None => Hosting::hermetic(),
    };
    compiled.run(&compiled.every(), hosting, store, &Seeds::default())
}

/// Whether the store holds a pass under the test's own key, whatever key a program handed over.
fn on_file(compiled: &Compiled, store: &Store) -> bool {
    matches!(store.get(compiled.hashes.tests[0]), Some(Outcome::Pass))
}

const NONDET: &str = r#"
nondet effect wire {
  read peek[r](k: Int) -> Int
}

fn ask(k: Int) -> Int / {wire.read[log]} = wire.peek[log](k)

test/nondet "reaches the host" { assert_eq(ask(1), 99) }
"#;

#[test]
fn a_pass_earned_over_a_host_handler_is_never_written_to_the_cache() {
    let compiled = Compiled::new(NONDET);
    let root = TempRoot::new();
    let mut store = root.store();
    let calls = Arc::new(AtomicUsize::new(0));
    let binding = Arc::new(
        registry("wire", "peek", Determinism::Nondeterministic, &calls)
            .bind(&compiled.check)
            .expect("the registration binds"),
    );

    for attempt in 0..2 {
        let report = run(&compiled, &mut store, Some(&binding));
        assert_eq!(report.failed, 0, "attempt {attempt}: {:?}", report.failures);
        assert_eq!(report.passed, 1, "attempt {attempt}");
        assert_eq!(
            calls.load(Ordering::Relaxed),
            attempt + 1,
            "attempt {attempt}: the host was not consulted, so the pass proves nothing"
        );
    }

    assert!(
        !on_file(&compiled, &store),
        "a host-backed pass was written under the key it was handed"
    );
}

#[test]
fn the_same_test_reaches_nothing_when_nothing_is_bound() {
    let compiled = Compiled::new(NONDET);
    let root = TempRoot::new();
    let mut store = root.store();
    let calls = Arc::new(AtomicUsize::new(0));

    let report = run(&compiled, &mut store, None);
    assert_eq!(report.passed, 0);
    assert_eq!(report.failed, 1);
    assert_eq!(
        report.failures[0].diagnostic.code,
        ply_eval::codes::UNHANDLED_EFFECT,
        "the operation, not the seam, is what failed: {:?}",
        report.failures[0].diagnostic
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

const DETERMINISTIC: &str = r#"
effect disk {
  read peek[r](k: Int) -> Int
}

fn ask(k: Int) -> Int / {disk.read[log]} =
  if k > 0 { 1 } else { disk.peek[log](k) }

test "its footprint reaches the host, its path does not" { assert_eq(ask(1), 1) }
"#;

#[test]
fn a_deterministic_registration_binds_and_the_test_footprint_reaches_it() {
    let compiled = Compiled::new(DETERMINISTIC);
    let calls = Arc::new(AtomicUsize::new(0));
    let binding = registry("disk", "peek", Determinism::Deterministic, &calls)
        .bind(&compiled.check)
        .expect(
            "a deterministic handler over a `det` effect binds; determinism propagation permits it",
        );

    assert!(
        binding.reaches(
            compiled.footprint_of_test("its footprint reaches the host, its path does not")
        ),
        "the binding does not agree this test can reach it, so the rest of this file is about nothing"
    );
}

/// `suite.select` is handed what the store holds, and no reach: a hermetic pass of a test the
/// binding reaches is still on file, so a `--host` run reports it cached and never consults the host.
#[test]
fn documents_a_hermetic_pass_stays_on_file_for_a_test_the_binding_reaches() {
    let compiled = Compiled::new(DETERMINISTIC);
    let name = "its footprint reaches the host, its path does not";
    let root = TempRoot::new();
    let mut store = root.store();
    let calls = Arc::new(AtomicUsize::new(0));

    let report = run(&compiled, &mut store, None);
    assert_eq!(report.failed, 0, "{:?}", report.failures);
    assert_eq!(report.passed, 1);

    let binding = registry("disk", "peek", Determinism::Deterministic, &calls)
        .bind(&compiled.check)
        .expect("the registration binds");
    assert!(binding.reaches(compiled.footprint_of_test(name)));
    assert!(
        on_file(&compiled, &store),
        "the hermetic pass is gone, so the gap this documents is closed"
    );
}

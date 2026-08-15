//! What decides whether a host-backed test runs at all.
//!
//! ADR 0011 §5 makes one claim about selection under `--host`:
//!
//! > the tests that can reach the host are exactly those whose footprint
//! > intersects the binding's. Those get `Reason::Host`: they always run and are
//! > never written to the cache, in either direction.
//!
//! Two halves. The **write** half is implemented and is implemented well: the
//! runtime is authoritative, a run that reached a host handler records
//! `Record::Host` and nothing is stored, and `ply test` audits every written
//! result against the binding afterwards. The tests here pin that.
//!
//! The **read** half is not implemented. [`select`] takes no binding — it is
//! `select(check, hashes, store, plan)` — so no test is ever selected *because*
//! it can reach the host. A host-reaching test runs today only because `Reason::
//! Nondet` covers it, and it covers it only because every handler in W1's
//! trusted computing base is registered `Determinism::Nondeterministic`.
//!
//! ADR 0011 §4 explicitly permits a `Determinism::Deterministic` registration
//! and §5 says such a handler is "still not cacheable". The last test here shows
//! that it is: a hermetic pass by a test whose footprint reaches the binding is
//! read back under `--host`, and the test is skipped without the host being
//! consulted. It is named `documents_` because it pins present behaviour so a
//! fix shows up as a diff — it is not an endorsement.

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
use ply_test::{Hosting, Reason, Search, select};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ------------------------------------------------------------------ harness

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

    fn footprint_of_test(&self, name: &str) -> &ply_core::Footprint {
        &self
            .check
            .tests
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named `{name}`"))
            .footprint
    }
}

/// A handler that answers a constant and counts how often it was asked.
///
/// The count is the point: "the host was consulted" is a claim a passing test
/// cannot make on its own, because the fixtures below pass either way.
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
        EngineChoice::Machine,
        Search::default(),
        hosting,
    )
}

fn reason(compiled: &Compiled, store: &Store, name: &str) -> Reason {
    let selection = select(&compiled.check, &compiled.hashes, store, &Plan::default());
    let index = compiled
        .check
        .tests
        .iter()
        .position(|t| t.name == name)
        .expect("the test exists");
    selection.reasons[index]
}

// --------------------------------------------------- the write half, which holds

/// A `nondet` effect, which is every effect W1's trusted computing base serves.
const NONDET: &str = r#"
nondet effect wire {
  read peek[r](k: Int) -> Int
}

fn ask(k: Int) -> Int / {wire.read[log]} = wire.peek[log](k)

test/nondet "reaches the host" { assert_eq(ask(1), 99) }
"#;

/// The claim ADR 0008 §3 and ADR 0011 §5 are built on, end to end: a run that
/// reached a real handler writes nothing, so the next run cannot believe it.
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

    assert_eq!(
        reason(&compiled, &store, "reaches the host"),
        Reason::Nondet,
        "a host-backed pass was written and read back"
    );
}

/// And the flag really is what changed: with nothing bound the same test cannot
/// reach the handler at all.
#[test]
fn the_same_test_reaches_nothing_when_nothing_is_bound() {
    let compiled = Compiled::new(NONDET);
    let root = TempRoot::new();
    let mut store = root.store();
    let calls = Arc::new(AtomicUsize::new(0));

    let report = run(&compiled, &mut store, None);
    assert_eq!(report.passed, 0);
    assert_eq!(report.failed, 1);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

// ------------------------------------------- the read half, which does not hold

/// A **deterministic** effect, which ADR 0011 §4 permits a host handler to serve
/// and which nothing in W1's trusted computing base currently does.
///
/// The test's footprint carries `disk.read[log]` — inference cannot know the
/// branch is never taken — so it is exactly the "footprint intersects the
/// binding" case §5 is about. It is `det`, because `disk` is not `nondet`, so it
/// is cacheable.
const DETERMINISTIC: &str = r#"
effect disk {
  read peek[r](k: Int) -> Int
}

fn ask(k: Int) -> Int / {disk.read[log]} =
  if k > 0 { 1 } else { disk.peek[log](k) }

test "its footprint reaches the host, its path does not" { assert_eq(ask(1), 1) }
"#;

/// The binding is what decides, and it agrees this test can reach it.
#[test]
fn a_deterministic_registration_binds_and_the_test_footprint_reaches_it() {
    let compiled = Compiled::new(DETERMINISTIC);
    let calls = Arc::new(AtomicUsize::new(0));
    let binding = registry("disk", "peek", Determinism::Deterministic, &calls)
        .bind(&compiled.check)
        .expect("a deterministic handler over a `det` effect binds; ADR 0011 §4 permits it");

    assert!(
        binding.reaches(
            compiled.footprint_of_test("its footprint reaches the host, its path does not")
        ),
        "the binding does not agree this test can reach it, so the rest of this file is about nothing"
    );
}

/// **The gap.** A hermetic pass is read back under `--host`, so the test is
/// skipped and the host is never consulted.
///
/// ADR 0011 §5 requires the opposite: such a test "always runs and is never
/// written to the cache, in either direction". Selection cannot implement that
/// because it is never given the binding — `select(check, hashes, store, plan)`
/// has no parameter for one — so the only thing making host-backed tests re-run
/// today is `Reason::Nondet`, which is a property of the *program's*
/// declaration rather than of the binding.
///
/// Why it matters, in the shape this project's defects always take: the run is
/// green, the count of cached tests looks right, and the resource the test
/// exists to exercise was never touched. Nothing anywhere says so.
///
/// This is unreachable through the shipped `ply` binary, because every
/// registration in `ply_host::registry` is `Nondeterministic`. It becomes
/// reachable the moment one is not.
#[test]
fn documents_a_host_reaching_test_is_skipped_when_its_hermetic_pass_was_cached() {
    let compiled = Compiled::new(DETERMINISTIC);
    let name = "its footprint reaches the host, its path does not";
    let root = TempRoot::new();
    let mut store = root.store();
    let calls = Arc::new(AtomicUsize::new(0));

    // A hermetic run. The branch that would perform `disk.peek` is not taken, so
    // the test passes and its pass is cached — correctly, hermetically.
    let report = run(&compiled, &mut store, None);
    assert_eq!(report.failed, 0, "{:?}", report.failures);
    assert_eq!(report.passed, 1);

    let binding = Arc::new(
        registry("disk", "peek", Determinism::Deterministic, &calls)
            .bind(&compiled.check)
            .expect("the registration binds"),
    );

    assert_eq!(
        reason(&compiled, &store, name),
        Reason::Cached,
        "ADR 0011 §5 requires `Reason::Host` here: a test whose footprint reaches the binding \
         always runs"
    );

    let report = run(&compiled, &mut store, Some(&binding));
    assert_eq!(
        report.passed + report.failed,
        0,
        "the test ran, so this file is out of date and the gap it documents is closed"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "the host was consulted, so the gap this documents is closed"
    );
}

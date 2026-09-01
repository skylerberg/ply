//! Adversarial audit of the scheduler split, and of what re-execution does to a host operation.

use ply_core::CheckOutput;
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity,
};
use ply_eval::{Plan, Value};
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_store::Store;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;
use ply_test::{Hosting, RunReport, Search, select};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> TempRoot {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ply-host-sched-{}-{}",
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

fn compile(source: &str) -> Compiled {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let mut program = ply_syntax::parse_program(inputs).expect("the fixture parses");
    let resolved = ply_syntax::resolve(&mut program).expect("the fixture resolves");
    let check = ply_core::check_program(&program, &resolved)
        .unwrap_or_else(|d| panic!("the fixture typechecks: {d:#?}"));
    let hashes = ply_hash::hash_program(&program, &resolved, &check).expect("the fixture hashes");
    Compiled {
        program,
        resolved,
        check,
        hashes,
    }
}

/// Counts every call.
#[derive(Default)]
struct Counting {
    calls: AtomicUsize,
}

impl Counting {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl HostHandler for Counting {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(HostAnswer::Value(Value::Int(1)))
    }
}

fn op(effect: &str, name: &str) -> HostOp {
    HostOp {
        effect: Symbol::new(effect),
        op: Symbol::new(name),
        resource: HostResource::Any,
        determinism: Determinism::Nondeterministic,
        linearity: Linearity::AtMostOnce,
        blocking: false,
        secrets: false,
        path: "test::send",
    }
}

/// `net.send` plus the three `task` operations, so a fixture can reach the production scheduler and
/// a socket in the same binding.
fn registry(handler: Arc<Counting>, tasks: bool) -> HostRegistry {
    let mut registry = HostRegistry::new();
    registry.register(op("net", "send"), handler.clone());
    if tasks {
        for name in ["spawn", "join", "yield"] {
            let mut task = op("task", name);
            task.linearity = Linearity::Repeatable;
            registry.register(task, handler.clone());
        }
    }
    registry
}

struct Ran {
    report: RunReport,
    sends: usize,
}

impl Ran {
    fn failure(&self) -> Option<&Diagnostic> {
        self.report.failures.first().map(|f| &f.diagnostic)
    }

    #[track_caller]
    fn refused(&self, code: &str) {
        let d = self
            .failure()
            .unwrap_or_else(|| panic!("the run was expected to fail; it passed"));
        assert_eq!(d.code, code, "{}: {}", d.code, d.message);
    }
}

/// The whole runner, over one fixture, with the binding actually bound — `--host`, in other words,
/// which is the only configuration in which any of these questions has an answer.
fn run_hosted(source: &str, tasks: bool) -> Ran {
    let compiled = compile(source);
    let counter = Arc::new(Counting::default());
    let binding = registry(counter.clone(), tasks)
        .bind(&compiled.check)
        .unwrap_or_else(|d| panic!("the registry binds: {d:#?}"));
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    let search = Search::of(&selection);
    let report = ply_test::run(
        &selection,
        &compiled.program,
        &compiled.resolved,
        &compiled.check,
        &compiled.hashes,
        &mut store,
        false,
        search,
        Hosting::hermetic().with_binding(Arc::new(binding)),
    );
    Ran {
        report,
        sends: counter.calls(),
    }
}

/// A `--host` run and a `simulate` region in one suite is an ordinary configuration, and the region
/// has to keep answering `task` itself.
#[test]
fn a_hosted_run_still_gives_simulate_the_seeded_scheduler() {
    let ran = run_hosted(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test "a seeded region under a bound registry" {
  let n = simulate {
    let a = task.spawn(|| 1);
    let b = task.spawn(|| 2);
    task.join(a) + task.join(b)
  };
  assert_eq(n, 3)
}
"#,
        true,
    );
    assert!(
        ran.report.is_success(),
        "{:?}",
        ran.failure().map(|d| (d.code, d.message.clone()))
    );
    assert_eq!(
        ran.sends, 0,
        "the region answered `task` itself; the bound handler was never called"
    );
    let result = &ran.report.results[0];
    assert!(
        result.simulation.is_some(),
        "a seeded region reports an exploration"
    );
}

/// Lock 2, from the other side.
#[test]
fn a_simulate_inside_a_spawned_production_task_is_refused() {
    let ran = run_hosted(
        r#"
test/nondet "a region inside a spawned task" {
  let t = task.spawn(|| simulate { task.join(task.spawn(|| 2)) });
  assert_eq(task.join(t), 2)
}
"#,
        true,
    );
    ran.refused(codes::NESTED_SIMULATION);
}

/// A production task that outlives its region — the shape that would leave real work running past
/// the scope that owns it.
#[test]
fn a_handler_cannot_discard_a_production_region_and_orphan_its_tasks() {
    let ran = run_hosted(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect bail {
  read out() -> Int
}

test/nondet "a clause that never resumes, over a spawned task" {
  let value = handle {
    let t = task.spawn(|| net.send[socket](1));
    bail.out()
  } with {
    bail.out() resume k -> 0
  };
  assert_eq(value, 0)
}
"#,
        true,
    );
    assert!(
        ran.report.is_success(),
        "{:?}",
        ran.failure().map(|d| (d.code, d.message.clone()))
    );
    assert_eq!(
        ran.sends, 1,
        "the spawned task ran to completion rather than being dropped on the floor"
    );
}

/// The seeded region *can* be discarded that way, and when it is, the machine must not answer the
/// next `task.*` by opening a production region beside the corpse.
#[test]
fn a_task_after_an_abandoned_seeded_region_does_not_open_a_production_one() {
    let ran = run_hosted(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect bail {
  read out() -> Int
}

test/nondet "a production region beside an abandoned seeded one" {
  let escaped = handle {
    simulate {
      let t = task.spawn(|| bail.out());
      task.join(t)
    }
  } with {
    bail.out() resume k -> 0
  };
  let after = task.spawn(|| net.send[socket](1));
  assert_eq(escaped + task.join(after), 1)
}
"#,
        true,
    );
    let d = ran
        .failure()
        .unwrap_or_else(|| panic!("a discarded region must be reported, not run beside a second"));
    // `Machine::innermost_simulation` looks for a live seeded region rather than for one the stack
    // still holds, so the abandoned region shadows the boundary and the second region is never
    // opened.
    assert_eq!(
        d.code,
        codes::HOST_IN_SIMULATION,
        "{}: {}",
        d.code,
        d.message
    );
    assert_eq!(ran.sends, 0, "nothing reached the socket");
}

/// The same shadowing, from the other direction: an ordinary host operation after a region whose
/// control a handler discarded.
#[test]
fn a_send_after_an_abandoned_seeded_region_is_refused_rather_than_performed() {
    let ran = run_hosted(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect bail {
  read out() -> Int
}

test/nondet "a socket after a discarded region" {
  let escaped = handle {
    simulate {
      let t = task.spawn(|| bail.out());
      task.join(t)
    }
  } with {
    bail.out() resume k -> 0
  };
  assert_eq(escaped + net.send[socket](1), 1)
}
"#,
        false,
    );
    let d = ran.failure().expect("the run is refused");
    assert_eq!(d.code, codes::HOST_IN_SIMULATION, "{}", d.message);
    assert_eq!(ran.sends, 0);
}

/// A production region and a `simulate` in *sequence*, which is not nesting in any reading of the
/// source.
#[test]
fn a_simulate_after_a_production_region_is_refused_as_nesting() {
    let ran = run_hosted(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "spawn, join, then simulate" {
  let a = task.spawn(|| 1);
  let first = task.join(a);
  let second = simulate {
    let b = task.spawn(|| 1);
    let c = task.spawn(|| 2);
    task.join(b) + task.join(c)
  };
  assert_eq(first + second, 4)
}
"#,
        true,
    );
    ran.refused(codes::NESTED_SIMULATION);
}

/// A bound host operation is performed once.
#[test]
fn engine_both_over_a_bound_host_sends_once_and_does_not_diverge() {
    let compiled = compile(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "one socket, one send" {
  assert_eq(net.send[socket](1), 1)
}
"#,
    );
    let counter = Arc::new(Counting::default());
    let binding = registry(counter.clone(), false)
        .bind(&compiled.check)
        .unwrap_or_else(|d| panic!("the registry binds: {d:#?}"));
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    let search = Search::of(&selection);
    let report = ply_test::run(
        &selection,
        &compiled.program,
        &compiled.resolved,
        &compiled.check,
        &compiled.hashes,
        &mut store,
        true,
        search,
        Hosting::hermetic().with_binding(Arc::new(binding)),
    );
    assert!(
        report.is_success(),
        "{:?}",
        report
            .failures
            .iter()
            .map(|f| (f.diagnostic.code, f.diagnostic.message.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(counter.calls(), 1, "only the machine reached the boundary");
    assert_eq!(report.results[0].recorded, Some(ply_test::Record::Host));
}

/// ADR 0011 §7 refuses a host operation inside a `simulate` region with `E0425`, on the ground that
/// "DPOR re-runs a test whole per interleaving; a region that reaches a socket would send one
/// packet per interleaving explored and call the result a proof".
const SEND_BESIDE_A_REGION: &str = r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "one send, several schedules" {
  let sent = net.send[socket](1);
  let raced = simulate {
    with_cell[n](0) { c -> {
      let a = task.spawn(|| cell_set(c, cell_get(c) + 1));
      let b = task.spawn(|| cell_set(c, cell_get(c) + 2));
      task.join(a);
      task.join(b);
      cell_get(c)
    } }
  };
  assert_eq(sent + raced, 4)
}
"#;

#[test]
fn a_send_beside_a_region_is_refused_before_the_first_packet() {
    let ran = run_hosted(SEND_BESIDE_A_REGION, false);
    ran.refused(codes::HOST_IN_SIMULATION);
    assert_eq!(
        ran.sends, 0,
        "the refusal is a report of a packet that had already gone out"
    );
    let d = ran.failure().expect("refused");
    assert!(
        d.notes.iter().any(|n| n.contains("`--sim once`")),
        "the refusal must name the one search a host-backed test may have: {:?}",
        d.notes
    );
    assert!(
        ran.report.results[0]
            .recorded
            .as_ref()
            .is_none_or(|r| !r.is_written()),
        "a refused run wrote a cache entry"
    );
}

/// And the escape hatch the refusal names really is one.
#[test]
fn under_simulation_once_the_same_send_runs_exactly_once_and_is_not_cached() {
    let compiled = compile(SEND_BESIDE_A_REGION);
    let counter = Arc::new(Counting::default());
    let binding = registry(counter.clone(), false)
        .bind(&compiled.check)
        .unwrap_or_else(|d| panic!("the registry binds: {d:#?}"));
    let root = TempRoot::new();
    let mut store = root.store();
    let plan = Plan::once(ply_eval::Seed::default());
    let selection = select(&compiled.check, &compiled.hashes, &store, &plan);
    assert!(
        !plan.re_executes(),
        "the fixture only bites if this plan really runs the test once"
    );
    let report = ply_test::run(
        &selection,
        &compiled.program,
        &compiled.resolved,
        &compiled.check,
        &compiled.hashes,
        &mut store,
        false,
        Search::of(&selection),
        Hosting::hermetic().with_binding(Arc::new(binding)),
    );
    assert!(
        report.is_success(),
        "{:?}",
        report
            .failures
            .iter()
            .map(|f| (f.diagnostic.code, f.diagnostic.message.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        counter.calls(),
        1,
        "the source performs `net.send` once, and so did the run"
    );
    assert_eq!(
        report.results[0].recorded,
        Some(ply_test::Record::Host),
        "a run that reached the host is recorded as such and stored nowhere"
    );
}

/// Hermetically the same program is `E0424`, and the diagnostic says that `--host` — the second of
/// the two remedies it names — would not help.
#[test]
fn a_hermetic_refusal_says_that_host_would_not_repair_a_searched_test() {
    let compiled = compile(SEND_BESIDE_A_REGION);
    let counter = Arc::new(Counting::default());
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    let report = ply_test::run(
        &selection,
        &compiled.program,
        &compiled.resolved,
        &compiled.check,
        &compiled.hashes,
        &mut store,
        false,
        Search::of(&selection),
        // The shape `ply test` uses: the registry is carried, nothing is bound.
        Hosting::hermetic().with_binding(Arc::new(HostBinding::hermetic_with(registry(
            counter.clone(),
            false,
        )))),
    );
    let d = &report.failures[0].diagnostic;
    assert_eq!(d.code, codes::HERMETIC_BOUNDARY, "{}", d.message);
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("`--host` would then refuse this")),
        "the refusal sends the reader to a flag that will refuse them: {:?}",
        d.notes
    );
    assert_eq!(counter.calls(), 0);
}

/// `--simulation measure-reduction` runs the whole search a second time with the dependence
/// relation forced to true, so it doubles whatever the first search did.
#[test]
fn measure_reduction_re_executes_a_once_plan_and_is_refused() {
    let compiled = compile(SEND_BESIDE_A_REGION);
    let counter = Arc::new(Counting::default());
    let binding = registry(counter.clone(), false)
        .bind(&compiled.check)
        .unwrap_or_else(|d| panic!("the registry binds: {d:#?}"));
    let root = TempRoot::new();
    let mut store = root.store();
    let plan = Plan::once(ply_eval::Seed::default());
    let selection = select(&compiled.check, &compiled.hashes, &store, &plan);
    let report = ply_test::run(
        &selection,
        &compiled.program,
        &compiled.resolved,
        &compiled.check,
        &compiled.hashes,
        &mut store,
        false,
        Search::of(&selection).measuring(true),
        Hosting::hermetic().with_binding(Arc::new(binding)),
    );
    let d = &report.failures[0].diagnostic;
    assert_eq!(d.code, codes::HOST_IN_SIMULATION, "{}", d.message);
    assert_eq!(counter.calls(), 0);
}

/// The other re-executing consumer: M5's bisection, which runs a failing test once per mixed
/// definition set.
#[test]
fn a_bisection_hybrid_re_evaluates_a_host_test_without_reaching_the_host() {
    use ply_test::bisect::{Delta, Hybrid};
    use ply_test::hybrid::{BodyHybrid, Mixture, Signature};

    let source = r#"
effect net {
  write send[s](payload: Int) -> Int
}

fn body() -> Int / {net.write[socket]} = net.send[socket](1)

test "a det test over a deterministic host handler" {
  assert_eq(body(), 99)
}
"#;
    let compiled = compile(source);
    let counter = Arc::new(Counting::default());
    let mut registry = HostRegistry::new();
    let mut deterministic = op("net", "send");
    deterministic.determinism = Determinism::Deterministic;
    registry.register(deterministic, counter.clone());
    let binding = registry
        .bind(&compiled.check)
        .unwrap_or_else(|d| panic!("the registry binds: {d:#?}"));

    let root = TempRoot::new();
    let mut store = root.store();
    let selection = select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    let search = Search::of(&selection);
    let report = ply_test::run(
        &selection,
        &compiled.program,
        &compiled.resolved,
        &compiled.check,
        &compiled.hashes,
        &mut store,
        false,
        search,
        Hosting::hermetic().with_binding(Arc::new(binding)),
    );
    assert_eq!(counter.calls(), 1, "the run itself reached the socket once");
    let signature = Signature::of(&report.failures[0].diagnostic);

    let (_, fresh) = ply_hash::hash_program_with_bodies(&compiled.program, &compiled.resolved)
        .expect("the fixture's bodies are reconstructible");
    let test_hash = compiled.hashes.tests[0];
    let test_body = BodyHybrid::test_body(&fresh, test_hash).expect("the test has a stored body");
    let mut mixture = Mixture::new();
    for name in compiled
        .hashes
        .closure
        .get(&compiled.check.tests[0].key)
        .into_iter()
        .flatten()
    {
        if let Some(hash) = compiled.hashes.defs.get(name) {
            let key = ply_test::bisect::DefKey::value(name.clone());
            mixture.baseline(key.clone(), *hash);
            mixture.current(key, *hash);
        }
    }
    let mut hybrid = BodyHybrid::new(&store, &fresh, mixture, test_body, signature);
    let delta = Delta {
        test: None,
        changes: Vec::new(),
        clusters: Vec::new(),
        unclassified: 0,
    };
    let trial = hybrid.trial(&delta, &[]);

    assert_eq!(
        counter.calls(),
        1,
        "a hybrid re-evaluated the test and did not send a second packet"
    );
    assert_ne!(
        trial.outcome,
        ply_test::bisect::TrialOutcome::Fails,
        "the trial cannot reproduce a failure it cannot reach the host to produce"
    );
}

/// Lock 3 at the runner rather than at the machine: with no binding, a `task.*` that reaches the
/// boundary is `E0424` and no production scheduler is built.
#[test]
fn a_hermetic_run_cannot_build_a_production_scheduler() {
    let compiled = compile(
        r#"
test/nondet "spawns without a binding" {
  assert_eq(task.join(task.spawn(|| 1)), 1)
}
"#,
    );
    let counter = Arc::new(Counting::default());
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    let search = Search::of(&selection);
    let report = ply_test::run(
        &selection,
        &compiled.program,
        &compiled.resolved,
        &compiled.check,
        &compiled.hashes,
        &mut store,
        false,
        search,
        Hosting::hermetic().with_binding(Arc::new(HostBinding::hermetic_with(registry(
            counter.clone(),
            true,
        )))),
    );
    let d = &report.failures[0].diagnostic;
    assert_eq!(d.code, codes::HERMETIC_BOUNDARY, "{}", d.message);
    assert!(d.message.contains("task.spawn"), "{}", d.message);
    assert_eq!(counter.calls(), 0);
}

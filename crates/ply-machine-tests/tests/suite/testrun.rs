//! One test, or one interleaving of one, as the runtime runs it under the binding it is handed: what
//! reaches a handler and what is refused before it can, how a failure is classed, what an entry
//! leaves behind, and isolated tests running at once without observing each other. What a run's
//! results establish is `suite.conclude`'s, and its tests pin that.

use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity, MachineId, Pending,
};
use ply_eval::{Diagnostic, Front, Resource, Seed, SourceId, Span, Symbol, Value, codes};
use ply_machine::testrun::{self, Executor, Hosting};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Compiled {
    front: Front,
    unit: &'static ply_codegen::Unit,
}

impl Compiled {
    #[track_caller]
    fn new(src: &str) -> Compiled {
        ply_codegen::c::producer::ensure_default();
        let sources = vec![("m".to_string(), src.to_string())];
        let front = ply_codegen::c::producer::checked_front(&sources, &[SourceId(0)])
            .unwrap_or_else(|e| panic!("the fixture must typecheck: {e:#}"));
        let unit = ply_codegen::Unit::over_front(&front, sources.into_iter().collect())
            .expect("this host has a C compiler");
        Compiled { front, unit }
    }

    fn index_of(&self, name: &str) -> usize {
        self.front
            .check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }

    /// Something in its closure entered a `simulate` region, so a search runs it per interleaving.
    fn seeded(&self, index: usize) -> bool {
        self.front.check.tests[index]
            .footprint
            .atoms()
            .any(|a| a.effect.as_str() == "sim")
    }
}

/// The interleavings a seeded test runs at -- one per root, each taking `path` first -- and
/// whether the test may run more than once, which a search from several roots does.
#[derive(Clone)]
struct Seeds {
    roots: Vec<u64>,
    path: Vec<u16>,
    re_executed: bool,
}

impl Default for Seeds {
    fn default() -> Seeds {
        Seeds {
            roots: vec![0],
            path: Vec::new(),
            re_executed: true,
        }
    }
}

impl Seeds {
    /// Exactly the interleaving `seed` names, run once.
    fn once(seed: Seed) -> Seeds {
        Seeds {
            roots: vec![seed.root],
            path: seed.path,
            re_executed: false,
        }
    }
}

/// How one test ran: once, or a seeded one an interleaving at a time until one failed.
struct Ran {
    status: &'static str,
    failure: Option<Diagnostic>,
    host: bool,
    performs: u64,
    teardown: Vec<Diagnostic>,
    /// Every interleaving entered a region, so the run was a search.
    searched: bool,
}

impl Ran {
    #[track_caller]
    fn refused(&self, code: &str) {
        let d = self
            .failure
            .as_ref()
            .unwrap_or_else(|| panic!("the run was expected to fail; it passed"));
        assert_eq!(d.code, code, "{}: {}", d.code, d.message);
    }
}

fn run(compiled: &Compiled, index: usize, hosting: &Hosting, seeds: &Seeds) -> Ran {
    let executor = Executor {
        front: &compiled.front,
        hosting: hosting.clone(),
        provider: compiled.unit,
    };
    if !compiled.seeded(index) {
        let e = testrun::executed(&executor, index);
        return Ran {
            status: testrun::status_word(e.failure.as_ref(), e.panicked),
            failure: e.failure,
            host: e.usage.host,
            performs: e.usage.performs,
            teardown: e.usage.teardown,
            searched: false,
        };
    }
    let mut ran = Ran {
        status: "passed",
        failure: None,
        host: false,
        performs: 0,
        teardown: Vec::new(),
        searched: true,
    };
    for &root in &seeds.roots {
        let seed = Seed::at(root, seeds.path.clone());
        let one = testrun::interleaved(&executor, index, &seed, 100_000, seeds.re_executed);
        ran.host |= one.usage.host;
        ran.performs += one.usage.performs;
        ran.teardown.extend(one.usage.teardown);
        ran.searched &= one.observed;
        if let ply_eval::Verdict::Failed(d) = &one.interleaving.verdict {
            ran.status = testrun::status_word(Some(d), one.panicked);
            ran.failure = Some(d.clone());
            break;
        }
    }
    ran
}

fn run_all(compiled: &Compiled, hosting: &Hosting, seeds: &Seeds) -> Vec<Ran> {
    (0..compiled.front.check.tests.len())
        .map(|i| run(compiled, i, hosting, seeds))
        .collect()
}

fn bound(binding: HostBinding) -> Hosting {
    Hosting {
        binding: Some(Arc::new(binding)),
        runtime: None,
    }
}

/// A handler with state of its own, so a run can be traced to which call moved it.
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

fn op(effect: &str, name: &str, resource: Option<&str>, determinism: Determinism) -> HostOp {
    HostOp {
        effect: Symbol::new(effect),
        op: Symbol::new(name),
        resource: match resource {
            Some(r) => HostResource::Only(Resource::Named(Symbol::new(r))),
            None => HostResource::Any,
        },
        determinism,
        linearity: Linearity::AtMostOnce,
        blocking: false,
        secrets: false,
        path: "audit::counting",
    }
}

fn counting(calls: &Arc<AtomicUsize>, answer: i64) -> Arc<dyn HostHandler> {
    Arc::new(Counting {
        calls: Arc::clone(calls),
        answer,
    })
}

fn one_op(
    effect: &str,
    name: &str,
    determinism: Determinism,
    calls: &Arc<AtomicUsize>,
) -> HostRegistry {
    let mut registry = HostRegistry::new();
    let mut registered = op(effect, name, Some("log"), determinism);
    registered.linearity = Linearity::Repeatable;
    registry.register(registered, counting(calls, 99));
    registry
}

// --- What a binding lets a test reach -----------------------------------------------------------

const NONDET: &str = r#"
nondet effect wire {
  read peek[r](k: Int) -> Int
}

fn ask(k: Int) -> Int / {wire.read[log]} = wire.peek[log](k)

test/nondet "reaches the host" { assert_eq(ask(1), 99) }
"#;

#[test]
fn a_pass_earned_over_a_host_handler_says_it_reached_the_host_every_time() {
    let compiled = Compiled::new(NONDET);
    let calls = Arc::new(AtomicUsize::new(0));
    let hosting = bound(
        one_op("wire", "peek", Determinism::Nondeterministic, &calls)
            .bind(&compiled.front.check)
            .expect("the registration binds"),
    );
    for attempt in 0..2 {
        let ran = run(&compiled, 0, &hosting, &Seeds::default());
        assert_eq!(ran.status, "passed", "attempt {attempt}: {:?}", ran.failure);
        assert!(
            ran.host,
            "attempt {attempt}: a pass over a handler was not marked as one, so it could be filed"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            attempt + 1,
            "attempt {attempt}"
        );
    }
}

#[test]
fn the_same_test_reaches_nothing_when_nothing_is_bound() {
    let compiled = Compiled::new(NONDET);
    let ran = run(&compiled, 0, &Hosting::default(), &Seeds::default());
    ran.refused(codes::UNHANDLED_EFFECT);
    assert!(!ran.host);
}

const DET_REACHES_HOST: &str = r#"
effect disk {
  read peek[r](key: Int) -> Int
}

fn ask(k: Int) -> Int / {disk.read[log]} = disk.peek[log](k)

test "a det test over a deterministic handler" { assert(ask(1) > 0) }
"#;

#[test]
fn a_det_pass_over_a_lying_deterministic_handler_is_marked_as_reaching_the_host() {
    let compiled = Compiled::new(DET_REACHES_HOST);
    let calls = Arc::new(AtomicUsize::new(0));
    let hosting = bound(
        one_op("disk", "peek", Determinism::Deterministic, &calls)
            .bind(&compiled.front.check)
            .expect("a deterministic handler over a `det` effect binds"),
    );
    for attempt in 1..=3 {
        let ran = run(&compiled, 0, &hosting, &Seeds::default());
        assert_eq!(ran.status, "passed", "attempt {attempt}: {:?}", ran.failure);
        assert!(
            ran.host,
            "attempt {attempt}: a lie could be filed as a truth"
        );
        assert_eq!(calls.load(Ordering::SeqCst), attempt, "attempt {attempt}");
    }
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
                Diagnostic::error(codes::INTERNAL_ERROR, "the evaluator is broken")
                    .primary(req.span, "here"),
            )
        }
    }

    let compiled = Compiled::new(DET_REACHES_HOST);
    let mut registry = HostRegistry::new();
    let mut registered = op("disk", "peek", Some("log"), Determinism::Deterministic);
    registered.path = "audit::impersonates";
    registered.linearity = Linearity::Repeatable;
    registry.register(registered, Arc::new(Impersonates));
    let hosting = bound(registry.bind(&compiled.front.check).expect("binds"));

    let ran = run(&compiled, 0, &hosting, &Seeds::default());
    ran.refused(codes::RUNTIME_ERROR);
    assert_eq!(
        ran.status, "failed",
        "a handler's failure was reported as a defect in Ply"
    );
    assert!(
        ran.host,
        "a failure a handler produced is a host-backed failure"
    );
    let d = ran.failure.expect("refused");
    assert!(
        d.notes.iter().any(|n| n.contains("audit::impersonates")),
        "nothing names the handler the failure came from: {:?}",
        d.notes
    );
}

#[test]
fn the_same_det_test_is_refused_hermetically() {
    let compiled = Compiled::new(DET_REACHES_HOST);
    let calls = Arc::new(AtomicUsize::new(0));
    let carried = bound(HostBinding::hermetic_with(one_op(
        "disk",
        "peek",
        Determinism::Deterministic,
        &calls,
    )));
    // The shape `ply test` uses names the handler that would have served this.
    run(&compiled, 0, &carried, &Seeds::default()).refused(codes::HERMETIC_BOUNDARY);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // A binding carrying no registry cannot tell a hermetic refusal from a front-end bug.
    run(&compiled, 0, &Hosting::default(), &Seeds::default()).refused(codes::UNHANDLED_EFFECT);
}

#[test]
fn an_operation_a_partial_clause_set_leaves_is_refused_by_the_checker() {
    ply_codegen::c::producer::ensure_default();
    let diagnostics = ply_codegen::c::producer::front(
        &[(
            "m".to_string(),
            r#"
effect disk {
  read peek[r](key: Int) -> Int
  read poke[r](key: Int) -> Int
}

test "the clause set misses an operation it performs" {
  let n = handle {
    disk.peek[log](1)
  } with {
    disk.poke[log](k) -> 0,
  };
  assert(n > 0)
}
"#
            .to_string(),
        )],
        &[SourceId(0)],
    )
    .expect("the front end answers")
    .diagnostics;
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        [codes::HANDLER_CLAUSE_MISSING],
        "{diagnostics:?}"
    );
}

#[test]
fn a_footprint_claim_is_restated_for_every_test_the_thread_runs() {
    let compiled = Compiled::new(
        r#"
effect disk {
  read peek[r](key: Int) -> Int
  read poke[r](key: Int) -> Int
}

fn ask(k: Int) -> Int / {disk.read[log]} = disk.peek[log](k)

test "the one that handles it" {
  let n = handle { disk.peek[log](1) } with { disk.peek[log](k) -> 1, };
  assert(n > 0)
}

test "the one that declares what it does" { assert(ask(1) > 0) }
"#,
    );
    assert!(
        compiled.front.check.tests[0]
            .footprint
            .atoms()
            .next()
            .is_none(),
        "the first test's claim is empty"
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let hosting = bound(
        one_op("disk", "peek", Determinism::Deterministic, &calls)
            .bind(&compiled.front.check)
            .expect("binds"),
    );
    let ran = run_all(&compiled, &hosting, &Seeds::default());
    assert!(ran.iter().all(|r| r.status == "passed"));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the test whose footprint holds the atom reaches the handler after one whose claim is empty"
    );
}

#[test]
fn a_test_run_without_the_binding_never_reaches_its_handler() {
    // A mixture is run hermetically: it re-evaluates a host-backed test without a second packet.
    let compiled = Compiled::new(
        r#"
effect net {
  write send[s](payload: Int) -> Int
}

fn body() -> Int / {net.write[socket]} = net.send[socket](1)

test "a det test over a deterministic host handler" { assert_eq(body(), 99) }
"#,
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = HostRegistry::new();
    registry.register(
        op("net", "send", None, Determinism::Deterministic),
        counting(&calls, 99),
    );
    let hosting = bound(registry.bind(&compiled.front.check).expect("binds"));
    assert_eq!(
        run(&compiled, 0, &hosting, &Seeds::default()).status,
        "passed"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let hermetic = run(&compiled, 0, &Hosting::default(), &Seeds::default());
    assert_ne!(hermetic.status, "passed");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the hermetic run sent a second packet"
    );
}

// --- Schedulers under a binding ------------------------------------------------------------------

#[derive(Default)]
struct Sends {
    calls: AtomicUsize,
}

impl HostHandler for Sends {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(HostAnswer::Value(Value::Int(1)))
    }
}

/// `net.send`, and the `task` operations when `tasks`, so one binding reaches a socket and the
/// production scheduler.
fn scheduling(handler: &Arc<Sends>, tasks: bool) -> HostRegistry {
    let mut registry = HostRegistry::new();
    registry.register(
        op("net", "send", None, Determinism::Nondeterministic),
        Arc::clone(handler) as Arc<dyn HostHandler>,
    );
    if tasks {
        for name in ["spawn", "join", "yield"] {
            let mut task = op("task", name, None, Determinism::Nondeterministic);
            task.linearity = Linearity::Repeatable;
            registry.register(task, Arc::clone(handler) as Arc<dyn HostHandler>);
        }
    }
    registry
}

fn hosted(source: &str, tasks: bool, seeds: &Seeds) -> (Ran, usize) {
    let compiled = Compiled::new(source);
    let sends = Arc::new(Sends::default());
    let hosting = bound(
        scheduling(&sends, tasks)
            .bind(&compiled.front.check)
            .unwrap_or_else(|d| panic!("the registry binds: {d:#?}")),
    );
    let ran = run(&compiled, 0, &hosting, seeds);
    (ran, sends.calls.load(Ordering::SeqCst))
}

#[test]
fn a_hosted_run_still_gives_simulate_the_seeded_scheduler() {
    let (ran, sends) = hosted(
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
        &Seeds::default(),
    );
    assert_eq!(ran.status, "passed", "{:?}", ran.failure);
    assert_eq!(sends, 0, "the region answered `task` itself");
    assert!(ran.searched, "a seeded region reports an exploration");
}

#[test]
fn a_simulate_inside_a_spawned_production_task_is_refused() {
    let (ran, _) = hosted(
        r#"
test/nondet "a region inside a spawned task" {
  let t = task.spawn(|| simulate { task.join(task.spawn(|| 2)) });
  assert_eq(task.join(t), 2)
}
"#,
        true,
        &Seeds::default(),
    );
    ran.refused(codes::NESTED_SIMULATION);
}

#[test]
fn a_handler_cannot_discard_a_production_region_and_orphan_its_tasks() {
    let (ran, sends) = hosted(
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
        &Seeds::default(),
    );
    assert_eq!(ran.status, "passed", "{:?}", ran.failure);
    assert_eq!(
        sends, 1,
        "the spawned task ran to completion rather than being dropped"
    );
}

const ABANDONED_REGION: &str = r#"
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
"#;

#[test]
fn a_task_after_an_abandoned_seeded_region_does_not_open_a_production_one() {
    let (ran, sends) = hosted(ABANDONED_REGION, true, &Seeds::default());
    // The abandoned region is still the innermost one, so the boundary refuses under it.
    ran.refused(codes::HOST_IN_SIMULATION);
    assert_eq!(sends, 0, "nothing reached the socket");
}

#[test]
fn a_send_after_an_abandoned_seeded_region_is_refused_rather_than_performed() {
    let (ran, sends) = hosted(
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
        &Seeds::default(),
    );
    ran.refused(codes::HOST_IN_SIMULATION);
    assert_eq!(sends, 0);
}

#[test]
fn a_simulate_after_a_production_region_is_refused_as_nesting() {
    let (ran, _) = hosted(
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
        &Seeds::default(),
    );
    ran.refused(codes::NESTED_SIMULATION);
}

/// `E0425`: a search re-runs a test per interleaving, so the send would repeat per interleaving.
const SEND_BESIDE_A_REGION: &str = r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "one send, several schedules" {
  let sent = net.send[socket](1);
  let raced = with_cell[n](0) { c ->
    simulate {
      let a = task.spawn(|| cell_set(c, cell_get(c) + 1));
      let b = task.spawn(|| cell_set(c, cell_get(c) + 2));
      task.join(a);
      task.join(b);
      cell_get(c)
    }
  };
  assert_eq(sent + raced, 4)
}
"#;

#[test]
fn a_send_beside_a_region_is_refused_before_the_first_packet() {
    let (ran, sends) = hosted(SEND_BESIDE_A_REGION, false, &Seeds::default());
    ran.refused(codes::HOST_IN_SIMULATION);
    assert_eq!(
        sends, 0,
        "the refusal reports a packet that had already gone out"
    );
    let d = ran.failure.expect("refused");
    assert!(
        d.notes.iter().any(|n| n.contains("`--sim once`")),
        "the refusal names the one search a host-backed test may have: {:?}",
        d.notes
    );
}

#[test]
fn under_simulation_once_the_same_send_runs_exactly_once_and_reaches_the_host() {
    let seeds = Seeds::once(Seed::default());
    assert!(!seeds.re_executed, "this plan really runs the test once");
    let (ran, sends) = hosted(SEND_BESIDE_A_REGION, false, &seeds);
    assert_eq!(ran.status, "passed", "{:?}", ran.failure);
    assert_eq!(
        sends, 1,
        "the source performs `net.send` once, and so did the run"
    );
    assert!(
        ran.host,
        "a run that reached the host says so, and is stored nowhere"
    );
}

#[test]
fn a_re_executed_once_plan_is_refused_as_measuring_a_reduction_runs_it() {
    let seeds = Seeds {
        re_executed: true,
        ..Seeds::once(Seed::default())
    };
    let (ran, sends) = hosted(SEND_BESIDE_A_REGION, false, &seeds);
    ran.refused(codes::HOST_IN_SIMULATION);
    assert_eq!(sends, 0);
}

#[test]
fn a_hermetic_refusal_says_that_host_would_not_repair_a_searched_test() {
    let compiled = Compiled::new(SEND_BESIDE_A_REGION);
    let sends = Arc::new(Sends::default());
    let carried = bound(HostBinding::hermetic_with(scheduling(&sends, false)));
    let ran = run(&compiled, 0, &carried, &Seeds::default());
    ran.refused(codes::HERMETIC_BOUNDARY);
    let d = ran.failure.expect("refused");
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("`--host` would then refuse this")),
        "the refusal sends the reader to a flag that will refuse them: {:?}",
        d.notes
    );
    assert_eq!(sends.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_hermetic_run_cannot_build_a_production_scheduler() {
    let compiled = Compiled::new(
        r#"
test/nondet "spawns without a binding" {
  assert_eq(task.join(task.spawn(|| 1)), 1)
}
"#,
    );
    let sends = Arc::new(Sends::default());
    let carried = bound(HostBinding::hermetic_with(scheduling(&sends, true)));
    let ran = run(&compiled, 0, &carried, &Seeds::default());
    ran.refused(codes::HERMETIC_BOUNDARY);
    assert!(
        ran.failure.expect("refused").message.contains("task.spawn"),
        "the refusal names the operation"
    );
    assert_eq!(sends.calls.load(Ordering::SeqCst), 0);
}

// --- What an entry ends with --------------------------------------------------------------------

/// A runtime that warns as every entry point it is told of ends, as one does of spans left open.
struct Warns;

impl HostRuntime for Warns {
    fn watch(&self, _: &Pending) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Ok(Value::Unit)
    }

    fn end_entry_point(&self, _: MachineId) -> Vec<Diagnostic> {
        vec![Diagnostic::warning(
            codes::SPAN_ABANDONED,
            "a span was still open when the entry point ended",
        )]
    }
}

#[test]
fn what_a_test_ended_with_is_answered_whether_it_ran_once_or_was_searched() {
    let compiled = Compiled::new(
        r#"
test "runs once" { assert_eq(1, 1) }

test "runs once per interleaving" {
  let n = simulate {
    let a = task.spawn(|| 1);
    task.join(a)
  };
  assert_eq(n, 1)
}
"#,
    );
    let hosting = Hosting {
        binding: None,
        runtime: Some(Arc::new(|| Rc::new(Warns) as Rc<dyn HostRuntime>)),
    };
    for (name, seeded) in [("runs once", false), ("runs once per interleaving", true)] {
        let index = compiled.index_of(name);
        assert_eq!(
            compiled.seeded(index),
            seeded,
            "{name} is searched: {seeded}"
        );
        let ran = run(&compiled, index, &hosting, &Seeds::default());
        assert_eq!(ran.status, "passed", "{name}: {:?}", ran.failure);
        assert!(
            ran.teardown.iter().any(|w| w.code == codes::SPAN_ABANDONED),
            "{name}: what the test ended with never reached its answer"
        );
    }
}

#[test]
fn a_result_counts_the_operations_its_own_test_performed() {
    let compiled = Compiled::new(
        r#"
effect disk {
  read peek[r](key: Int) -> Int
}

test "peeks three times" {
  let n = handle {
    disk.peek[log](1) + disk.peek[log](2) + disk.peek[log](3)
  } with {
    disk.peek[log](k) -> k,
  };
  assert_eq(n, 6)
}

test "peeks at nothing" { assert_eq(1, 1) }
"#,
    );
    let ran = run_all(&compiled, &Hosting::default(), &Seeds::default());
    assert_eq!(
        ran[0].performs, 3,
        "a handled operation is performed all the same"
    );
    assert_eq!(
        ran[1].performs, 0,
        "a thread's count starts over with each test"
    );
}

// --- How a failure is classed -------------------------------------------------------------------

fn classed(code: &'static str, panicked: bool) -> &'static str {
    let d = Diagnostic::error(code, "the fixture's failure")
        .primary(Span::new(SourceId(0), 0, 1), "here");
    testrun::status_word(Some(&d), panicked)
}

#[test]
fn no_program_level_code_is_read_as_a_defect_in_ply() {
    for code in [
        codes::ASSERTION_FAILED,
        codes::RUNTIME_ERROR,
        codes::NON_EXHAUSTIVE_MATCH,
        codes::ARITY_MISMATCH,
        codes::UNHANDLED_EFFECT,
        codes::RESOURCE_REQUIRED,
        codes::UNKNOWN_NAME,
        codes::UNKNOWN_OPERATION,
        codes::STEP_BUDGET,
    ] {
        assert_eq!(classed(code, false), "failed", "{code}");
    }
}

#[test]
fn an_internal_error_a_divergence_and_an_unwind_are_defects_and_a_clock_is_no_verdict() {
    assert_eq!(classed(codes::INTERNAL_ERROR, false), "panicked");
    assert_eq!(classed(codes::SIMULATION_DIVERGENCE, false), "panicked");
    assert_eq!(
        classed(codes::RUNTIME_ERROR, true),
        "panicked",
        "an unwind is a defect whatever code it carries"
    );
    assert_eq!(classed(codes::RUN_ABANDONED, false), "abandoned");
    let warned = Diagnostic::warning(codes::RUNTIME_ERROR, "odd but red");
    assert_eq!(
        testrun::status_word(Some(&warned), false),
        "failed",
        "a non-error severity is still a failure"
    );
    assert_eq!(testrun::status_word(None, false), "passed");
}

// --- Isolated tests at once ---------------------------------------------------------------------

/// Every test allocates its cell at the same id, writes it, and checks every value it wrote.
fn contending_source(tests: usize, label: impl Fn(usize) -> String) -> String {
    let mut out = String::new();
    let mut declared: Vec<String> = Vec::new();
    for i in 0..tests {
        let label = label(i);
        if !declared.contains(&label) {
            out.push_str(&format!(
                "\nfn touches_{label}(n: Int) -> Int / {{cell.read[{label}], \
                 cell.write[{label}]}} = n\n"
            ));
            declared.push(label.clone());
        }
        out.push_str(&format!(
            r#"
test "contender {label} {i}" {{
  let seen = with_cell[{label}]({i}) {{ c -> {{
    assert_eq(cell_get(c), {i});
    cell_set(c, cell_get(c) * 7);
    assert_eq(cell_get(c), {seven});
    cell_set(c, cell_get(c) + {i});
    cell_get(c)
  }} }};
  assert_eq(touches_{label}(seen), {eight})
}}
"#,
            seven = i * 7,
            eight = i * 8,
        ));
    }
    out
}

/// Each test on its own thread at once, as lanes under a `parallel` block run them.
fn at_once(compiled: &Compiled, tests: &[usize]) -> Vec<&'static str> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = tests
            .iter()
            .map(|&i| {
                std::thread::Builder::new()
                    .stack_size(64 << 20)
                    .spawn_scoped(scope, move || {
                        run(compiled, i, &Hosting::default(), &Seeds::default()).status
                    })
                    .expect("a test thread starts")
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("no test thread panicked"))
            .collect()
    })
}

#[test]
fn isolated_tests_running_at_once_never_observe_each_other() {
    const TESTS: usize = 32;
    let compiled = Compiled::new(&contending_source(TESTS, |i| format!("table{i}")));
    let all: Vec<usize> = (0..TESTS).collect();
    for round in 0..3 {
        let statuses = at_once(&compiled, &all);
        assert!(
            statuses.iter().all(|s| *s == "passed"),
            "round {round}: {statuses:?}"
        );
    }
}

#[test]
fn verdicts_do_not_move_between_one_thread_and_many() {
    let source = format!(
        "{}{}{}",
        contending_source(6, |_| "table".to_string()),
        contending_source(10, |i| format!("own{i}")),
        (0..8)
            .map(|i| format!("\ntest \"pure {i}\" {{ assert_eq({i} + 1, {}) }}\n", i + 1))
            .collect::<String>()
    );
    let compiled = Compiled::new(&source);
    let tests = compiled.front.check.tests.len();
    let one: Vec<&str> = (0..tests)
        .map(|i| run(&compiled, i, &Hosting::default(), &Seeds::default()).status)
        .collect();
    // The tests sharing `table` each in a class of its own, as a program colours them, the rest at once.
    let mut many: Vec<&str> = vec![""; tests];
    let together: Vec<usize> = std::iter::once(0).chain(6..tests).collect();
    for (i, status) in together.iter().zip(at_once(&compiled, &together)) {
        many[*i] = status;
    }
    for (i, status) in many.iter_mut().enumerate().take(6).skip(1) {
        *status = at_once(&compiled, &[i])[0];
    }
    assert!(
        one.iter().all(|s| *s == "passed"),
        "the corpus is green: {one:?}"
    );
    assert_eq!(one, many, "a verdict moved between one thread and many");
}

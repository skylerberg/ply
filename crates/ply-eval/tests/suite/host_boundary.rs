//! The host effect boundary, end to end through the machine.

use crate::fixture::Compiled;
use ply_core::ty::{EffectAtom, Footprint, Resource};
use ply_eval::Value;
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity, Pending,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Mode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A host handler that answers the ordinal of its own call.
#[derive(Default)]
struct Counter {
    calls: AtomicU64,
}

impl Counter {
    fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }
}

impl HostHandler for Counter {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let ordinal = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(HostAnswer::Value(Value::Int(ordinal as i64)))
    }
}

/// A handler that never completes on the spot, which is every operation that waits.
struct Waits;

impl HostHandler for Waits {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        Ok(HostAnswer::Pending(Pending {
            token: 7,
            label: "accept",
        }))
    }
}

/// A runtime whose tokens are already resolved.
struct Resolved7;

impl HostRuntime for Resolved7 {
    fn poll(&self, _: &Pending) -> Result<Option<Value>, Diagnostic> {
        Ok(Some(Value::Int(7)))
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Ok(Value::Int(7))
    }
}

/// `effect` is the name the `effect` declaration writes — `net` — while the program is module `t`,
/// so every atom below is `t.net`.
fn op(effect: &str, name: &str, linearity: Linearity) -> HostOp {
    HostOp {
        effect: Symbol::new(effect),
        op: Symbol::new(name),
        resource: HostResource::Any,
        determinism: Determinism::Nondeterministic,
        linearity,
        blocking: false,
        secrets: false,
        path: "test::send",
    }
}

fn registry_of(entries: Vec<(HostOp, Arc<dyn HostHandler>)>) -> HostRegistry {
    let mut registry = HostRegistry::new();
    for (op, handler) in entries {
        registry.register(op, handler);
    }
    registry
}

fn atom(effect: &str, resource: &str, mode: Mode) -> EffectAtom {
    EffectAtom::new(effect, Resource::Named(Symbol::new(resource)), mode)
}

#[track_caller]
fn diagnostic(outcome: Result<(), Diagnostic>) -> Diagnostic {
    outcome.expect_err("the program was expected to fail")
}

/// One host-backed operation, performed once, with nothing to shadow it.
const SEND: &str = r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "the packet goes out" {
  assert_eq(net.send[socket](1), 1)
}
"#;

/// The default is the guarantee.
#[test]
fn a_hermetic_run_refuses_the_boundary_and_names_the_handler() {
    let compiled = Compiled::named("t", SEND);
    let counter = Arc::new(Counter::default());
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        counter.clone(),
    )]);

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(HostBinding::hermetic_with(registry)));
    let d = diagnostic(machine.eval_test(0));

    assert_eq!(d.code, codes::HERMETIC_BOUNDARY);
    assert!(d.message.contains("net.send[socket]"), "{}", d.message);
    let notes = d.notes.join(" ");
    assert!(notes.contains("test::send"), "{notes}");
    assert!(notes.contains("--host"), "{notes}");
    assert_eq!(counter.calls(), 0, "a hermetic run reaches no handler");
    assert_eq!(machine.host_ops(), 0);
    assert!(machine.host_use().is_none());
}

/// E0424 and E0303 call for opposite responses — pass `--host` or write a test double, versus file
/// a bug — so an operation nothing registered must keep the old code rather than acquire the new
/// one.
#[test]
fn an_operation_no_handler_claims_is_still_e0303() {
    let compiled = Compiled::named("t", SEND);
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(HostBinding::hermetic()));
    assert_eq!(
        diagnostic(machine.eval_test(0)).code,
        codes::UNHANDLED_EFFECT
    );

    // And the same under a real binding that simply does not serve it.
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        Arc::new(Counter::default()),
    )]);
    let idle = Compiled::named(
        "t",
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
  write close[s](conn: Int) -> Unit
}

test/nondet "closes without sending" {
  net.close[socket](1)
}
"#,
    );
    let binding = registry.bind(&idle.check).expect("binds");
    let mut machine = idle.machine();
    machine.set_host_binding(Arc::new(binding));
    assert_eq!(
        diagnostic(machine.eval_test(0)).code,
        codes::UNHANDLED_EFFECT
    );
}

#[test]
fn a_bound_run_reaches_the_handler_and_records_what_it_reached() {
    let compiled = Compiled::named("t", SEND);
    let counter = Arc::new(Counter::default());
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        counter.clone(),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.eval_test(0).expect("the bound run passes");

    assert_eq!(counter.calls(), 1);
    assert_eq!(machine.host_ops(), 1);
    let used = machine.host_use().expect("the run reached the host");
    assert_eq!(used.operations, 1);
    assert!(used.atoms.contains(&atom("t.net", "socket", Mode::Write)));
}

/// The binding is the handler of **last resort**.
#[test]
fn a_handler_in_scope_shadows_the_host() {
    let compiled = Compiled::named(
        "t",
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "the double answers" {
  let answered = handle { net.send[socket](1) } with { net.send[socket](p) -> 99 };
  assert_eq(answered, 99)
}
"#,
    );
    let counter = Arc::new(Counter::default());
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        counter.clone(),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.eval_test(0).expect("the double answers it");

    assert_eq!(counter.calls(), 0, "the host was never reached");
    assert_eq!(machine.host_ops(), 0);
    assert!(machine.host_use().is_none());
}

/// The exact program that would otherwise send the packet twice: a multi-shot Ply handler installed
/// *around* a host operation, so the captured control contains the `perform`.
const MULTI_SHOT_OVER_HOST: &str = r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

test/nondet "resumed twice across a send" {
  handle {
    let n = retry.ask();
    net.send[socket](n)
  } with {
    retry.ask() resume k -> k(1) + k(2)
  }
}
"#;

#[test]
fn a_second_resumption_across_an_at_most_once_operation_is_refused() {
    let compiled = Compiled::named("t", MULTI_SHOT_OVER_HOST);
    let counter = Arc::new(Counter::default());
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        counter.clone(),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    let d = diagnostic(machine.eval_test(0));

    assert_eq!(d.code, codes::HOST_CONTINUATION_RESUMED);
    let notes = d.notes.join(" ");
    assert!(notes.contains("test::send"), "{notes}");
    assert!(notes.contains("at-most-once"), "{notes}");
    assert_eq!(
        counter.calls(),
        1,
        "the refusal happens before the second send, not after it"
    );
}

/// `Repeatable` is what keeps the rule's over-approximation tight, and it is a claim the handler
/// author makes: this replays without changing anything outside the program.
#[test]
fn the_same_program_with_a_repeatable_operation_resumes_twice() {
    let compiled = Compiled::named("t", MULTI_SHOT_OVER_HOST);
    let counter = Arc::new(Counter::default());
    let registry = registry_of(vec![(
        op("net", "send", Linearity::Repeatable),
        counter.clone(),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine
        .eval_test(0)
        .expect("a repeatable operation replays");

    assert_eq!(counter.calls(), 2);
    assert_eq!(machine.host_ops(), 0, "a repeatable answer is not counted");
    assert_eq!(
        machine
            .host_use()
            .expect("the run still reached the host")
            .operations,
        2,
        "`host_use` counts every operation; only the linearity rule is selective"
    );
}

/// The rule refuses a second resumption only when an irreversible operation happened *after* the
/// capture.
#[test]
fn a_continuation_captured_after_the_last_send_resumes_twice() {
    let compiled = Compiled::named(
        "t",
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

test/nondet "captured after the send" {
  let total = handle {
    let sent = net.send[socket](1);
    sent + retry.ask()
  } with {
    retry.ask() resume k -> k(10) + k(20)
  };
  assert_eq(total, 32)
}
"#,
    );
    let counter = Arc::new(Counter::default());
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        counter.clone(),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine
        .eval_test(0)
        .expect("nothing irreversible happened after the capture");
    assert_eq!(counter.calls(), 1);
}

/// The whole reason W1 leaves M6 alone: in a hermetic run the counter is zero for the life of the
/// entry point, so the refusal condition is unreachable and no existing multi-shot program can
/// change behaviour.
#[test]
fn hermetic_multi_shot_is_untouched() {
    let compiled = Compiled::named(
        "t",
        r#"
effect retry {
  read ask() -> Int
}

test "three resumptions" {
  let total = handle { retry.ask() } with { retry.ask() resume k -> k(1) + k(2) + k(3) };
  assert_eq(total, 6)
}
"#,
    );
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(HostBinding::hermetic()));
    machine.eval_test(0).expect("multi-shot is unaffected");
    assert_eq!(machine.host_ops(), 0);
}

/// DPOR re-runs the region whole for every schedule it explores.
#[test]
fn a_host_operation_inside_a_simulate_region_is_refused() {
    for source in [
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "lexically enclosing" {
  simulate { net.send[socket](1) }
}
"#,
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

fn shout() -> Int / {net.write[socket]} = net.send[socket](1)

test/nondet "reached through a call" {
  simulate { shout() }
}
"#,
    ] {
        let compiled = Compiled::named("t", source);
        let counter = Arc::new(Counter::default());
        let registry = registry_of(vec![(
            op("net", "send", Linearity::AtMostOnce),
            counter.clone(),
        )]);
        let binding = registry.bind(&compiled.check).expect("binds");

        let mut machine = compiled.machine();
        machine.set_host_binding(Arc::new(binding));
        let d = diagnostic(machine.eval_test(0));
        assert_eq!(d.code, codes::HOST_IN_SIMULATION, "{}", d.message);
        assert_eq!(counter.calls(), 0, "the refusal precedes the operation");
    }
}

/// E0425 is the terminal answer, so it is reported even when nothing is bound: telling a hermetic
/// run to pass `--host` when `--host` would then refuse it costs the reader a round trip to learn
/// nothing.
#[test]
fn a_host_operation_inside_a_region_is_refused_hermetically_too() {
    let compiled = Compiled::named(
        "t",
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "hermetic, in a region" {
  simulate { net.send[socket](1) }
}
"#,
    );
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        Arc::new(Counter::default()),
    )]);
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(HostBinding::hermetic_with(registry)));
    assert_eq!(
        diagnostic(machine.eval_test(0)).code,
        codes::HOST_IN_SIMULATION
    );
}

/// Lock 2 of the three that keep `simulate` and the production scheduler apart.
#[test]
fn a_spawn_inside_a_region_reaches_the_seeded_scheduler_even_when_task_is_bound() {
    let compiled = Compiled::named(
        "t",
        r#"
fn detached() -> Int / {task.write} = {
  let t = task.spawn(|| 1);
  task.join(t)
}

test/nondet "the region's own scheduler answers" {
  let answered = simulate {
    let a = task.spawn(|| 7);
    task.join(a)
  };
  assert_eq(answered, 7)
}
"#,
    );
    let counter = Arc::new(Counter::default());
    let registry = registry_of(vec![
        (op("task", "spawn", Linearity::Repeatable), counter.clone()),
        (op("task", "join", Linearity::Repeatable), counter.clone()),
    ]);
    let binding = registry.bind(&compiled.check).expect("binds");
    assert!(
        !binding.listing().is_empty(),
        "the fixture exists to have a bound `task` handler to shadow"
    );

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.eval_test(0).expect("the seeded scheduler answers");
    assert_eq!(
        counter.calls(),
        0,
        "the host `task` handler was never reached"
    );
    assert!(
        machine.simulated().is_some(),
        "the run went through a seeded region"
    );
}

/// The one mechanical defence in the system against a footprint that under-reports.
#[test]
fn an_answer_outside_the_declared_footprint_is_refused() {
    let compiled = Compiled::named("t", SEND);
    let counter = Arc::new(Counter::default());
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        counter.clone(),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.set_declared_footprint(Footprint::empty());
    let d = diagnostic(machine.eval_test(0));

    assert_eq!(d.code, codes::HOST_FOOTPRINT_ESCAPE);
    assert!(d.message.contains("net.write[socket]"), "{}", d.message);
    assert_eq!(
        counter.calls(),
        0,
        "the check runs before the handler, or the packet is already out"
    );
}

#[test]
fn an_answer_inside_the_declared_footprint_is_allowed() {
    let compiled = Compiled::named("t", SEND);
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        Arc::new(Counter::default()),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.set_declared_footprint(Footprint::from_atoms([atom(
        "t.net",
        "socket",
        Mode::Write,
    )]));
    machine.eval_test(0).expect("the atom is declared");
}

/// The claim is the caller's and it is made once per entry point, so the reset every entry point
/// performs may not quietly drop it — a footprint check that silently stops checking is worse than
/// no check at all.
#[test]
fn the_declared_footprint_survives_the_next_entry_point() {
    let compiled = Compiled::named("t", SEND);
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        Arc::new(Counter::default()),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.set_declared_footprint(Footprint::empty());
    diagnostic(machine.eval_test(0));
    assert_eq!(
        diagnostic(machine.eval_test(0)).code,
        codes::HOST_FOOTPRINT_ESCAPE,
        "a second entry point is checked against the same claim"
    );
}

/// Outside a scheduler region a `Pending` has nowhere to park, so the machine drives the runtime
/// until the token resolves.
#[test]
fn a_pending_answer_outside_a_region_blocks_and_returns_the_value() {
    let compiled = Compiled::named(
        "t",
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "waits" {
  assert_eq(net.send[socket](1), 7)
}
"#,
    );
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        Arc::new(Waits),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.set_host_runtime(std::rc::Rc::new(Resolved7));
    machine.eval_test(0).expect("the token resolves");
    assert_eq!(machine.host_ops(), 1);
}

/// A machine with a binding and no reactor is a legitimate configuration — a clock read never
/// touches one — so this must be a diagnostic rather than a panic or a hang.
#[test]
fn a_pending_answer_with_no_runtime_is_a_diagnostic() {
    let compiled = Compiled::named("t", SEND);
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        Arc::new(Waits),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    let d = diagnostic(machine.eval_test(0));
    assert_eq!(d.code, codes::INTERNAL_ERROR);
    assert!(d.message.contains("net.send[socket]"), "{}", d.message);
}

/// A binding is a runtime decision, and nothing about it may reach the front end: a `det` test
/// performing a `nondet` operation is E0412 with a binding and without one, and never runs either
/// way.
#[test]
fn a_binding_does_not_move_an_e0412_verdict() {
    let source = r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test "a det test reaching a socket" {
  net.send[socket](1)
}
"#;
    let diagnostics = Compiled::rejected_in("t", source);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == codes::NONDET_IN_DET_TEST),
        "{:?}",
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

/// The counter is per entry point.
#[test]
fn the_host_operation_count_does_not_cross_an_entry_point() {
    let compiled = Compiled::named(
        "t",
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "sends" {
  net.send[socket](1)
}

test/nondet "sends too" {
  net.send[socket](2)
}
"#,
    );
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        Arc::new(Counter::default()),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.eval_test(0).expect("passes");
    assert_eq!(machine.host_ops(), 1);
    machine.eval_test(1).expect("passes");
    assert_eq!(machine.host_ops(), 1, "counted from zero again");
    assert_eq!(
        machine.host_use().expect("reached the host").operations,
        1,
        "and so is what the run reports having reached"
    );
}

#[test]
fn a_span_from_the_perform_reaches_the_handler() {
    struct Spans;

    impl HostHandler for Spans {
        fn call(
            &self,
            _: &dyn HostRuntime,
            req: &HostRequest<'_>,
        ) -> Result<HostAnswer, Diagnostic> {
            assert_ne!(req.span, Span::DUMMY, "a handler can point at Ply source");
            assert_eq!(req.args.len(), 1);
            assert_eq!(req.atom.resource, Resource::Named(Symbol::new("socket")));
            Ok(HostAnswer::Value(Value::Int(1)))
        }
    }

    let compiled = Compiled::named("t", SEND);
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        Arc::new(Spans),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.eval_test(0).expect("passes");
}

/// The registrations `ply_host::sched` makes, as a fixture: three `task` operations, `Repeatable`,
/// over the singleton resource `Any` resolves to.
fn task_registry(handler: Arc<dyn HostHandler>) -> HostRegistry {
    registry_of(
        ["spawn", "join", "yield"]
            .into_iter()
            .map(|name| (op("task", name, Linearity::Repeatable), handler.clone()))
            .collect(),
    )
}

/// The whole point of the production scheduler: it is reachable from a program.
#[test]
fn a_bound_task_perform_opens_a_production_region_rather_than_calling_a_handler() {
    let compiled = Compiled::named(
        "t",
        r#"
test/nondet "two tasks and a join" {
  let a = task.spawn(|| 1);
  let b = task.spawn(|| 2);
  assert_eq(task.join(a) + task.join(b), 3)
}
"#,
    );
    let counter = Arc::new(Counter::default());
    let binding = task_registry(counter.clone())
        .bind(&compiled.check)
        .expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.eval_test(0).expect("the production scheduler runs");
    assert_eq!(
        counter.calls(),
        0,
        "`task.*` is answered by opening a region, never by a handler that sees only values"
    );
    assert!(
        machine.simulated().is_none(),
        "a production region records no step, so it cannot fabricate an exploration"
    );
}

/// Lock 3.
#[test]
fn a_hermetic_run_never_opens_a_production_region() {
    let compiled = Compiled::named(
        "t",
        r#"
test/nondet "spawns" {
  assert_eq(task.join(task.spawn(|| 1)), 1)
}
"#,
    );
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(HostBinding::hermetic_with(task_registry(
        Arc::new(Counter::default()),
    ))));
    let d = diagnostic(machine.eval_test(0));
    assert_eq!(d.code, codes::HERMETIC_BOUNDARY);
    assert!(d.message.contains("task.spawn"), "{}", d.message);
}

/// The two schedulers do not nest in *either* order.
#[test]
fn a_simulate_inside_a_production_region_is_refused() {
    let compiled = Compiled::named(
        "t",
        r#"
test/nondet "a region inside the production one" {
  let t = task.spawn(|| 1);
  let inner = simulate { task.join(task.spawn(|| 2)) };
  assert_eq(task.join(t) + inner, 3)
}
"#,
    );
    let binding = task_registry(Arc::new(Counter::default()))
        .bind(&compiled.check)
        .expect("binds");
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    assert_eq!(
        diagnostic(machine.eval_test(0)).code,
        codes::NESTED_SIMULATION
    );
}

/// A production region answers `task` and nothing else.
#[test]
fn a_production_region_never_answers_clock_from_the_seeded_table() {
    let compiled = Compiled::named(
        "t",
        r#"
test/nondet "reads a clock inside the production region" {
  let t = task.spawn(|| 1);
  let at = clock.now();
  assert_eq(task.join(t) + at, 1)
}
"#,
    );
    let binding = task_registry(Arc::new(Counter::default()))
        .bind(&compiled.check)
        .expect("binds");
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    // Nothing registers `clock`, so it reaches the boundary and is unhandled.
    assert_eq!(
        diagnostic(machine.eval_test(0)).code,
        codes::UNHANDLED_EFFECT
    );
}

/// ADR 0008 §8, which is the reason `HostAnswer::Pending` exists at all: a task waiting on a token
/// leaves the enabled set, and the others keep running.
#[test]
fn a_task_pending_on_a_host_token_parks_and_the_others_run() {
    /// Never completes on the spot.
    struct Once;

    impl HostHandler for Once {
        fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
            Ok(HostAnswer::Pending(Pending {
                token: 1,
                label: "accept",
            }))
        }
    }

    /// Resolves only on the second poll.
    struct Later {
        polls: AtomicU64,
    }

    impl HostRuntime for Later {
        fn poll(&self, _: &Pending) -> Result<Option<Value>, Diagnostic> {
            let n = self.polls.fetch_add(1, Ordering::SeqCst);
            Ok((n > 0).then_some(Value::Int(5)))
        }

        fn park(&self) -> Result<(), Diagnostic> {
            Ok(())
        }

        fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
            panic!("a task inside a production region must park, never block the thread")
        }
    }

    let compiled = Compiled::named(
        "t",
        r#"
nondet effect net {
  write accept[s](listener: Int) -> Int
}

fn waits() -> Int / {net.write[socket]} = net.accept[socket](1)

test/nondet "the sibling runs while one task waits" {
  let slow = task.spawn(|| waits());
  let quick = task.spawn(|| 2);
  assert_eq(task.join(quick) + task.join(slow), 7)
}
"#,
    );
    let mut registry = task_registry(Arc::new(Counter::default()));
    registry.register(op("net", "accept", Linearity::AtMostOnce), Arc::new(Once));
    let binding = registry.bind(&compiled.check).expect("binds");

    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.set_host_runtime(std::rc::Rc::new(Later {
        polls: AtomicU64::new(0),
    }));
    machine.eval_test(0).expect("both tasks finish");
    assert_eq!(
        machine.host_use().expect("reached the host").operations,
        1,
        "the pending operation is charged once, at the perform and not at the wake"
    );
}

/// Outside a region there is nowhere to park, so the machine drives the runtime until the token
/// resolves.
#[test]
fn a_pending_outside_a_region_blocks_the_one_thread_it_is_allowed_to() {
    let compiled = Compiled::named(
        "t",
        r#"
nondet effect net {
  write accept[s](listener: Int) -> Int
}

test/nondet "one operation, no tasks" {
  assert_eq(net.accept[socket](1), 7)
}
"#,
    );
    let registry = registry_of(vec![(
        op("net", "accept", Linearity::AtMostOnce),
        Arc::new(Waits),
    )]);
    let binding = registry.bind(&compiled.check).expect("binds");
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));
    machine.set_host_runtime(std::rc::Rc::new(Resolved7));
    machine.eval_test(0).expect("block_on answers");
}

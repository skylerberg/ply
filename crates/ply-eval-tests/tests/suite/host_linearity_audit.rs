//! Adversarial audit of host linearity — the retries and captures a host operation still permits
//! on the tier. The at-most-once linearity counter (E0426) the tree machine raised is not a tier
//! mechanism, so the cases that turned on it were removed with that machine.

use crate::fixture::Compiled;
use ply_eval::Value;
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity, Pending,
};
use ply_span::{Diagnostic, Symbol, codes};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Answers the ordinal of its own call, so a replay is visible in the value as well as in the
/// count.
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

/// Never completes on the spot.
struct Waits;

impl HostHandler for Waits {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        Ok(HostAnswer::Pending(Pending {
            token: 1,
            label: "accept",
        }))
    }
}

/// A reactor whose tokens are already resolved.
struct Ready;

impl HostRuntime for Ready {
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

/// The three `task` registrations a production region needs in order to be openable at all.
fn with_tasks(mut registry: HostRegistry, handler: Arc<dyn HostHandler>) -> HostRegistry {
    for name in ["spawn", "join", "yield"] {
        registry.register(op("task", name, Linearity::Repeatable), handler.clone());
    }
    registry
}

/// Runs `source` with `net.send` bound at `linearity`, and answers what the run did and how many
/// packets went out.
struct Run {
    outcome: Result<(), Diagnostic>,
    sends: u64,
}

fn run(source: &str, linearity: Linearity, tasks: bool) -> Run {
    run_with(source, linearity, tasks, false)
}

fn run_with(source: &str, linearity: Linearity, tasks: bool, runtime: bool) -> Run {
    let compiled = Compiled::named("t", source);
    let counter = Arc::new(Counter::default());
    let mut registry = registry_of(vec![(
        op("net", "send", linearity),
        counter.clone() as Arc<dyn HostHandler>,
    )]);
    // Registered only where the fixture declares it: a registration for an operation the program
    // does not have is `E0421` before anything runs.
    if source.contains("accept[s]") {
        registry.register(op("net", "accept", linearity), Arc::new(Waits));
    }
    if tasks {
        registry = with_tasks(registry, counter.clone());
    }
    let binding = registry
        .bind(&compiled.check)
        .unwrap_or_else(|d| panic!("the registry binds: {d:#?}"));
    let mut machine = compiled.machine_on_tier();
    machine.set_host_binding(Arc::new(binding));
    if runtime {
        machine.set_host_runtime(std::rc::Rc::new(Ready));
    }
    let outcome = machine.eval_test(0);
    Run {
        outcome,
        sends: counter.calls(),
    }
}

/// The rule is about one `perform` running twice because its control was reinstated, not about a
/// program that performs twice.
#[test]
fn two_ordinary_performs_are_a_retry_and_are_allowed() {
    let run = run(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "retries" {
  let a = net.send[socket](1);
  let b = net.send[socket](2);
  assert_eq(a + b, 3)
}
"#,
        Linearity::AtMostOnce,
        false,
    );
    run.outcome.expect("an ordinary retry is not a replay");
    assert_eq!(run.sends, 2);
}

/// Two performs of one operation capture two continuations, each resumed once.
#[test]
fn a_fresh_capture_after_a_send_may_still_be_resumed_once() {
    let run = run(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

fn once() -> Int / {net.write[socket], retry.read} =
  handle {
    let n = retry.ask();
    net.send[socket](n)
  } with { retry.ask() resume k -> k(1) }

test/nondet "twice over" {
  let a = once();
  let b = once();
  assert_eq(a + b, 3)
}
"#,
        Linearity::AtMostOnce,
        false,
    );
    run.outcome
        .expect("each capture is resumed once, which is what the rule permits");
    assert_eq!(run.sends, 2);
}

/// A `simulate` region and a host operation in the same entry point, with the operation *outside*
/// the region.
#[test]
fn a_send_beside_a_simulate_region_is_performed_once_by_the_machine() {
    let run = run(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "a region and a socket, side by side" {
  let inside = simulate {
    let a = task.spawn(|| 1);
    let b = task.spawn(|| 2);
    task.join(a) + task.join(b)
  };
  let sent = net.send[socket](inside);
  assert_eq(sent, 1)
}
"#,
        Linearity::AtMostOnce,
        false,
    );
    run.outcome.expect("the region ends before the send");
    assert_eq!(run.sends, 1);
}

/// A registry compiled in but not bound is the `ply test` default, and it must leave M6 exactly
/// where it was: `host_ops` stays zero for the life of the entry point, and a three-shot handler
/// still runs three times.
#[test]
fn a_present_but_unbound_registry_leaves_multi_shot_alone() {
    let compiled = Compiled::named(
        "t",
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

test/nondet "three resumptions, nothing bound" {
  let total = handle { retry.ask() } with { retry.ask() resume k -> k(1) + k(2) + k(3) };
  assert_eq(total, 6)
}
"#,
    );
    let counter = Arc::new(Counter::default());
    let registry = registry_of(vec![(
        op("net", "send", Linearity::AtMostOnce),
        counter.clone() as Arc<dyn HostHandler>,
    )]);
    let mut machine = compiled.machine_on_tier();
    machine.set_host_binding(Arc::new(HostBinding::hermetic_with(registry)));
    machine
        .eval_test(0)
        .expect("a compiled-in registry is not a bound one");
    assert_eq!(machine.host_ops(), 0);
    assert_eq!(counter.calls(), 0);
}

/// M8 runs a law's body once per generated case, so a law that could reach a socket would send one
/// packet per case and report the result as a `property` tier over the whole domain.
#[test]
fn a_spec_can_never_reach_the_host_because_a_spec_can_never_perform() {
    for source in [
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

law "sends agree"
  forall (n: Int) {
    net.send[socket](n) == n
  }
"#,
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

fn relay(n: Int) -> Int / {net.write[socket]}
  ensures result == net.send[socket](n)
= net.send[socket](n)
"#,
    ] {
        let diagnostics = Compiled::rejected_in("t", source);
        assert!(
            diagnostics.iter().any(|d| d.code == codes::EFFECT_IN_SPEC),
            "{:?}",
            diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }
}

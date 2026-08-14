//! Adversarial audit of ADR 0008 §7 / ADR 0011 §3 — a host handler's
//! continuation may be resumed at most once.
//!
//! `host_boundary.rs` pins the shape the design describes. This file attacks it:
//! every test here is a way to reach a second resumption *around* the rule
//! rather than through it — through a cell, through another task, through a
//! region that has ended, through an aliased binding, through a re-execution.
//!
//! The assertion in every case is a **count**, never a value. A boundary that
//! sends the packet twice and returns the right number is precisely the defect
//! this milestone exists to prevent, and no assertion on a result can see it.

use ply_core::{CheckOutput, check_program};
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity, Pending,
};
use ply_eval::{Machine, Value};
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// ------------------------------------------------------------------- fixtures

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

fn compile(source: &str) -> Compiled {
    let program =
        ply_syntax::parse_program(vec![(SourceId(0), ModuleName::from_dotted("t"), source)])
            .expect("the fixture parses");
    let resolved = resolve(&program).expect("the fixture resolves");
    let check = check_program(&program, &resolved)
        .unwrap_or_else(|d| panic!("the fixture typechecks: {d:#?}"));
    Compiled {
        program,
        resolved,
        check,
    }
}

impl Compiled {
    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }
}

/// Answers the ordinal of its own call, so a replay is visible in the value as
/// well as in the count.
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

/// Never completes on the spot. Its token is what forces a task inside a
/// production region to leave the enabled set.
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

/// The three `task` registrations a production region needs in order to be
/// openable at all.
fn with_tasks(mut registry: HostRegistry, handler: Arc<dyn HostHandler>) -> HostRegistry {
    for name in ["spawn", "join", "yield"] {
        registry.register(op("task", name, Linearity::Repeatable), handler.clone());
    }
    registry
}

/// Runs `source` with `net.send` bound at `linearity`, and answers what the run
/// did and how many packets went out.
struct Run {
    outcome: Result<(), Diagnostic>,
    sends: u64,
}

impl Run {
    #[track_caller]
    fn refused(&self, code: &str) -> &Diagnostic {
        let d = self
            .outcome
            .as_ref()
            .expect_err("the program was expected to be refused");
        assert_eq!(d.code, code, "{}: {}", d.code, d.message);
        d
    }
}

fn run(source: &str, linearity: Linearity, tasks: bool) -> Run {
    run_with(source, linearity, tasks, false)
}

fn run_with(source: &str, linearity: Linearity, tasks: bool, runtime: bool) -> Run {
    let compiled = compile(source);
    let counter = Arc::new(Counter::default());
    let mut registry = registry_of(vec![(
        op("net", "send", linearity),
        counter.clone() as Arc<dyn HostHandler>,
    )]);
    // Registered only where the fixture declares it: a registration for an
    // operation the program does not have is `E0421` before anything runs.
    if source.contains("accept[s]") {
        registry.register(op("net", "accept", linearity), Arc::new(Waits));
    }
    if tasks {
        registry = with_tasks(registry, counter.clone());
    }
    let binding = registry
        .bind(&compiled.check)
        .unwrap_or_else(|d| panic!("the registry binds: {d:#?}"));
    let mut machine = compiled.machine();
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

// --------------------------------------------------- laundering the same `k`

/// The counter is shared across clones by `Rc`, so a second resumption cannot
/// launder itself through an alias. If it could, every other test in this file
/// would be defeated by one `let`.
#[test]
fn a_second_resumption_through_an_alias_is_still_the_second() {
    let run = run(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

test/nondet "aliased" {
  handle {
    let n = retry.ask();
    net.send[socket](n)
  } with {
    retry.ask() resume k -> {
      let alias = k;
      k(1) + alias(2)
    }
  }
}
"#,
        Linearity::AtMostOnce,
        false,
    );
    run.refused(codes::HOST_CONTINUATION_RESUMED);
    assert_eq!(run.sends, 1, "the alias is the same continuation");
}

/// The rule is about one `perform` running twice because its control was
/// reinstated, not about a program that performs twice. A retry loop is legal
/// and must stay legal, or the boundary is unusable for the thing a network
/// handler most needs to do.
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
/// Refusing the second capture's first resumption would be a false positive on
/// the most ordinary multi-shot program there is.
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

// ---------------------------------------------------- storing a continuation

/// The hazard ADR 0011 §3 names, with the second resumption moved out of the
/// clause entirely: the clause stores `k` in a cell, returns, and the *body*
/// resumes it a second time long after the handler is gone.
///
/// Nothing about the rule depends on where a resumption is written, and this is
/// the test that says so.
#[test]
fn a_continuation_stashed_in_a_cell_cannot_be_resumed_a_second_time() {
    let run = run(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

test/nondet "stashed" {
  with_cell[slot](|n| n) { slot -> {
    let first = handle {
      let n = retry.ask();
      net.send[socket](n)
    } with {
      retry.ask() resume k -> { cell_set(slot, k); k(1) }
    };
    let later = cell_get(slot);
    first + later(2)
  } }
}
"#,
        Linearity::AtMostOnce,
        false,
    );
    run.refused(codes::HOST_CONTINUATION_RESUMED);
    assert_eq!(
        run.sends, 1,
        "the stashed continuation is refused before the second send"
    );
}

// ------------------------------------------------------------ across a region

/// A continuation captured inside a production region and resumed from a
/// *different* task. The two resumptions are separated by a scheduling
/// decision, so nothing lexical connects them — the counter on the continuation
/// is the only thing that can.
#[test]
fn a_continuation_resumed_from_another_task_is_still_counted() {
    let run = run(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

test/nondet "resumed from a sibling" {
  with_cell[slot](|n| n) { slot -> {
    let value = handle {
      let a = task.spawn(|| {
        let n = retry.ask();
        net.send[socket](n)
      });
      let b = task.spawn(|| {
        task.yield();
        let stored = cell_get(slot);
        stored(2)
      });
      task.join(a) + task.join(b)
    } with {
      retry.ask() resume k -> { cell_set(slot, k); k(1) }
    };
    value
  } }
}
"#,
        Linearity::AtMostOnce,
        true,
    );
    run.refused(codes::HOST_CONTINUATION_RESUMED);
    assert_eq!(
        run.sends, 1,
        "a sibling task may not replay a control that already sent"
    );
}

/// A `simulate` region and a host operation in the same entry point, with the
/// operation *outside* the region. E0425 covers only the inside, so this is the
/// residue: whether the machine itself performs the operation once.
///
/// The machine does. What re-runs the whole entry point is the search, and that
/// lives in `ply-test`; see `host_scheduler_audit.rs`.
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

// ------------------------------------------------------------------- pending

/// A task parked on a pending token has already spent its linearity: the
/// operation happened, and only its answer is outstanding. A rule that charged
/// at the wake instead would let a control captured over the *perform* be
/// replayed while the first packet was still in flight.
#[test]
fn a_pending_operation_is_charged_at_the_perform_and_closes_a_replay() {
    let run = run_with(
        r#"
nondet effect net {
  write accept[s](listener: Int) -> Int
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

test/nondet "replay over a token" {
  handle {
    let n = retry.ask();
    net.accept[socket](n)
  } with {
    retry.ask() resume k -> k(1) + k(2)
  }
}
"#,
        Linearity::AtMostOnce,
        false,
        true,
    );
    run.refused(codes::HOST_CONTINUATION_RESUMED);
}

// ------------------------------------------------ the over-approximation

/// The false positive ADR 0011 §3 accepts on purpose, pinned so it stays a
/// decision rather than becoming a discovery.
///
/// The send happens in the *clause*, not inside the captured control, so
/// replaying `k` would repeat nothing. The rule refuses anyway, because the
/// precise version needs a per-resumption liveness scope on the control stack —
/// in the one part of the system where a defect is silent and sends a packet
/// twice. Widening this later is a real improvement; doing it by accident is not.
#[test]
fn a_send_in_the_clause_refuses_a_replay_that_would_repeat_nothing() {
    let run = run(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

test/nondet "the send is not inside the continuation" {
  handle {
    retry.ask()
  } with {
    retry.ask() resume k -> {
      let sent = net.send[socket](1);
      k(sent) + k(2)
    }
  }
}
"#,
        Linearity::AtMostOnce,
        false,
    );
    let d = run.refused(codes::HOST_CONTINUATION_RESUMED);
    assert!(
        d.notes.iter().any(|n| n.contains("conservative")),
        "a refusal a program did not deserve has to say that it is conservative: {:?}",
        d.notes
    );
    assert_eq!(run.sends, 1);
}

/// A registry compiled in but not bound is the `ply test` default, and it must
/// leave M6 exactly where it was: `host_ops` stays zero for the life of the
/// entry point, so the refusal condition is unreachable and a three-shot handler
/// still runs three times.
#[test]
fn a_present_but_unbound_registry_leaves_multi_shot_alone() {
    let compiled = compile(
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
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(HostBinding::hermetic_with(registry)));
    machine
        .eval_test(0)
        .expect("a compiled-in registry is not a bound one");
    assert_eq!(machine.host_ops(), 0);
    assert_eq!(counter.calls(), 0);
}

// ------------------------------------------------------------- M8's re-runs

/// M8 runs a law's body once per generated case, so a law that could reach a
/// socket would send one packet per case and report the result as a `property`
/// tier over the whole domain.
///
/// It cannot, and the exclusion is in the front end rather than at the boundary:
/// a spec expression is pure, so any host-backed row in a law or an `ensures` is
/// `E0417` before `ply prove` ever builds a machine. Checked here because the
/// two facts are only safe together — the purity rule is what makes the second
/// line of defence (every `ply-prove` machine is hermetic) unnecessary rather
/// than load-bearing.
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
        let program =
            ply_syntax::parse_program(vec![(SourceId(0), ModuleName::from_dotted("t"), source)])
                .expect("parses");
        let resolved = resolve(&program).expect("resolves");
        let diagnostics =
            check_program(&program, &resolved).expect_err("a spec may not perform an effect");
        assert!(
            diagnostics.iter().any(|d| d.code == codes::EFFECT_IN_SPEC),
            "{:?}",
            diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }
}

// ------------------------------------------------ the shape of the diagnostic

/// E0426 is the one diagnostic in this milestone whose reader has to act on a
/// *packet*, not on a program. It has to name the operation being protected, the
/// handler that served it, and which resumption was refused, or the reader
/// cannot tell a genuine replay from a false positive of the over-approximation.
#[test]
fn the_refusal_names_the_operation_the_handler_and_the_ordinal() {
    let run = run(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

test/nondet "named" {
  handle {
    let n = retry.ask();
    net.send[socket](n)
  } with {
    retry.ask() resume k -> k(1) + k(2)
  }
}
"#,
        Linearity::AtMostOnce,
        false,
    );
    let d = run.refused(codes::HOST_CONTINUATION_RESUMED);
    let labels: String = d
        .labels
        .iter()
        .map(|l| l.message.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let text = format!("{} {} {labels}", d.message, d.notes.join(" "));
    assert!(text.contains("net.send[socket]"), "{text}");
    assert!(text.contains("test::send"), "{text}");
    assert!(text.contains("at-most-once"), "{text}");
    assert!(
        d.labels.iter().any(|l| l.primary && !l.span.is_dummy()),
        "a refusal with no span cannot be acted on"
    );
}

/// A `Repeatable` claim is the one place a handler author can quietly re-open
/// the boundary, so the flag has to be the *only* thing that changes between
/// these two runs.
#[test]
fn repeatable_is_the_single_switch_between_refused_and_replayed() {
    const SOURCE: &str = r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

test/nondet "switch" {
  handle {
    let n = retry.ask();
    net.send[socket](n)
  } with {
    retry.ask() resume k -> k(1) + k(2)
  }
}
"#;
    let refused = run(SOURCE, Linearity::AtMostOnce, false);
    refused.refused(codes::HOST_CONTINUATION_RESUMED);
    assert_eq!(refused.sends, 1);

    let replayed = run(SOURCE, Linearity::Repeatable, false);
    replayed
        .outcome
        .expect("a repeatable operation is allowed to replay");
    assert_eq!(replayed.sends, 2);
}

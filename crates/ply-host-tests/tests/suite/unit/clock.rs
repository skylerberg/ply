use ply_eval::host::MachineId;
use ply_eval::{HostAnswer, HostBinding, HostRequest, Span, Symbol, Value};
use ply_host::Host;
use ply_host::clock::*;

fn check(source: &str) -> ply_eval::CheckOutput {
    crate::support::answered::checked("m", source).check
}

/// Reads the language's clock and sleeps on it, so both rows bind.
const TIMED: &str = r#"
fn waited() -> Instant / {clock.read, clock.write} = {
  clock.sleep(Duration(1));
  clock.now()
}
"#;

/// Declares a `clock` of its own, which hides the language's in this module.
const OWN: &str = r#"
nondet effect clock {
  read now() -> Int
}

fn stamp() -> Int / {clock.read} = clock.now()
"#;

fn bound(host: &Host, source: &str) -> HostBinding {
    host.registry()
        .bind(&check(source))
        .unwrap_or_else(|d| panic!("the host binds: {d:?}"))
}

fn answer(host: &Host, binding: &HostBinding, op: Op, args: &[Value]) -> Value {
    let bound = binding
        .resolve(&Symbol::new(EFFECT), &Symbol::new(op.name()), None)
        .expect("the language's clock is bound");
    let request = HostRequest {
        machine: MachineId(1),
        atom: bound.atom.clone(),
        op: bound.op,
        args,
        span: Span::DUMMY,
        task: None,
        declared: None,
    };
    match bound.handler.call(&*host.runtime(), &request) {
        Ok(HostAnswer::Value(value)) => value,
        Ok(HostAnswer::Pending(pending)) => panic!("{} waited on `{pending}`", op.what()),
        Err(d) => panic!("{} was refused: {}", op.what(), d.message),
    }
}

fn nanos(value: &Value) -> i64 {
    ply_eval::sim::nanos_of(value, Span::DUMMY, "a reading").expect("an `Instant`")
}

#[test]
fn the_host_serves_the_languages_clock_to_a_program_that_performs_it() {
    let rows: Vec<String> = bound(&Host::new(), TIMED)
        .listing()
        .rows
        .iter()
        .map(|row| row.to_string())
        .collect();
    assert_eq!(rows, ["clock.now", "clock.sleep"]);
}

/// A registration by the language's name is for the language's effect: bound by declared name it
/// would find two declarations here, and answer one of them an `Instant` where it reads an `Int`.
#[test]
fn a_modules_own_clock_is_not_the_one_the_host_serves() {
    let binding = bound(&Host::new(), OWN);
    assert!(
        binding.listing().is_empty(),
        "the host bound a handler to an effect a module declared: {:?}",
        binding.listing().rows
    );
    assert_eq!(
        binding.would_serve(&Symbol::new("m.clock"), &Symbol::new("now"), None),
        None
    );
}

#[test]
fn a_reading_is_an_instant_on_the_clock_the_runtime_keeps() {
    let host = Host::new();
    let binding = bound(&host, TIMED);
    let rt = host.runtime();
    let before = rt.now().expect("the host keeps time");
    let read = answer(&host, &binding, Op::Now, &[]);
    let after = rt.now().expect("the host keeps time");
    assert!(
        matches!(&read, Value::Ctor { name, .. } if name.as_str() == "Instant"),
        "`clock.now` answered a {}",
        read.type_name()
    );
    let at = nanos(&read);
    assert!(
        before <= at && at <= after,
        "{at} is not between two readings of the runtime's clock, {before} and {after}"
    );
}

/// What a sleep is where no task could run meanwhile; a task of a production region never reaches
/// the handler.
#[test]
fn the_handlers_sleep_is_the_threads_wait() {
    let host = Host::new();
    let binding = bound(&host, TIMED);
    let before = nanos(&answer(&host, &binding, Op::Now, &[]));
    let span = Value::ctor("Duration", vec![Value::Int(20_000_000)]);
    let answered = answer(&host, &binding, Op::Sleep, &[span]);
    assert!(
        matches!(answered, Value::Unit),
        "a sleep reads nothing back"
    );
    let after = nanos(&answer(&host, &binding, Op::Now, &[]));
    assert!(
        after - before >= 20_000_000,
        "{before} to {after} is not a 20ms wait"
    );
    let none = Value::ctor("Duration", vec![Value::Int(-1)]);
    answer(&host, &binding, Op::Sleep, &[none]);
}

use ply_core::Mode;
use ply_core::host::{Grants, MachineId, Unserved};
use ply_core::{HostAnswer, HostBinding, HostRequest, Span, Symbol, Value};
use ply_host::Host;
use ply_host::clock::*;

fn bound(host: &Host) -> HostBinding {
    host.registry()
        .granted(Grants::default())
        .unwrap_or_else(|d| panic!("the host binds: {d:?}"))
}

fn answer(host: &Host, binding: &HostBinding, op: Op, args: &[Value]) -> Value {
    let bound = binding
        .resolve(&Symbol::new(EFFECT), &Symbol::new(op.name()), None)
        .expect("the language's clock is bound");
    let request = HostRequest {
        machine: MachineId(1),
        atom: ply_core::EffectAtom::operation(
            EFFECT,
            ply_core::Resource::Singleton,
            Mode::Read,
            op.name(),
        ),
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
    ply_core::sim::nanos_of(value, Span::DUMMY, "a reading").expect("an `Instant`")
}

/// A registration by the language's name is for the language's effect: a module's own `clock`,
/// which hides the language's there, is answered by nothing the host registers, where read by its
/// declared name it would be answered an `Instant` where it reads an `Int`.
#[test]
fn the_host_serves_the_languages_clock_and_not_a_modules_own() {
    let binding = bound(&Host::new());
    for op in ["now", "sleep"] {
        assert!(
            binding
                .resolve(&Symbol::new(EFFECT), &Symbol::new(op), None)
                .is_ok(),
            "`clock.{op}` is not served"
        );
    }
    assert!(matches!(
        binding.resolve(&Symbol::new("m.clock"), &Symbol::new("now"), None),
        Err(Unserved::Unregistered)
    ));
}

#[test]
fn a_reading_is_an_instant_on_the_clock_the_runtime_keeps() {
    let host = Host::new();
    let binding = bound(&host);
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
    let binding = bound(&host);
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

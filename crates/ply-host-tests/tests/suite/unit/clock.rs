use ply_core::host::Grants;
use ply_core::{HostBinding, Span, Symbol, Value};
use ply_host::Host;
use ply_host::clock::*;

fn bound() -> HostBinding {
    ply_host::registry().granted(Grants::default())
}

fn answered(host: &Host, op: Op, args: &[Value]) -> Value {
    answer(op, &*host.runtime(), args, Span::DUMMY)
        .unwrap_or_else(|d| panic!("`clock.{}` was refused: {}", op.name(), d.message))
}

fn nanos(value: &Value) -> i64 {
    ply_core::sim::nanos_of(value, Span::DUMMY, "a reading").expect("an `Instant`")
}

/// A registration by the language's name is for the language's effect: a module's own `clock`,
/// which hides the language's there, is answered by nothing the host registers, where read by its
/// declared name it would be answered an `Instant` where it reads an `Int`.
#[test]
fn the_host_serves_the_languages_clock_and_not_a_modules_own() {
    let binding = bound();
    for op in Op::ALL {
        let declared = binding
            .resolve(&Symbol::new(EFFECT), &Symbol::new(op.name()), None)
            .unwrap_or_else(|| panic!("`clock.{}` is not served", op.name()));
        assert_eq!(Op::of(declared), Some(op));
    }
    assert!(
        binding
            .resolve(&Symbol::new("m.clock"), &Symbol::new("now"), None)
            .is_none()
    );
}

#[test]
fn a_reading_is_an_instant_on_the_clock_the_runtime_keeps() {
    let host = Host::new();
    let rt = host.runtime();
    let before = rt.now().expect("the host keeps time");
    let read = answered(&host, Op::Now, &[]);
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
/// the host.
#[test]
fn a_sleep_is_the_threads_wait() {
    let host = Host::new();
    let before = nanos(&answered(&host, Op::Now, &[]));
    let span = Value::ctor("Duration", vec![Value::Int(20_000_000)]);
    let slept = answered(&host, Op::Sleep, &[span]);
    assert!(matches!(slept, Value::Unit), "a sleep reads nothing back");
    let after = nanos(&answered(&host, Op::Now, &[]));
    assert!(
        after - before >= 20_000_000,
        "{before} to {after} is not a 20ms wait"
    );
    let again = nanos(&answered(&host, Op::Now, &[]));
    let none = Value::ctor("Duration", vec![Value::Int(-1)]);
    answered(&host, Op::Sleep, &[none]);
    assert!(
        nanos(&answered(&host, Op::Now, &[])) - again < 20_000_000,
        "a negative span waited"
    );
}

#[test]
fn a_perform_of_the_wrong_arity_is_refused_as_ply_s_own_fault() {
    let host = Host::new();
    let refused = answer(Op::Now, &*host.runtime(), &[Value::Int(1)], Span::DUMMY)
        .expect_err("`clock.now` takes nothing");
    assert_eq!(refused.code, ply_core::codes::INTERNAL_ERROR);
}

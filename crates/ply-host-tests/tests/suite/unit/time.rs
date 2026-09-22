use ply_eval::host::MachineId;
use ply_eval::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, Linearity, Value,
};
use ply_host::time::*;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_ty::{EffectAtom, Mode, Resource};
use std::sync::Arc;

struct Nothing;

impl HostRuntime for Nothing {
    fn poll(&self, _: &ply_eval::Pending) -> Result<Option<Value>, Diagnostic> {
        Ok(None)
    }
    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }
    fn block_on(&self, _: ply_eval::Pending) -> Result<Value, Diagnostic> {
        Ok(Value::Unit)
    }
}

fn answer(
    handlers: &[(HostOp, Arc<dyn HostHandler>)],
    op: Op,
    args: &[Value],
) -> Result<Value, Diagnostic> {
    let (declaration, handler) = handlers
        .iter()
        .find(|(d, _)| d.op.as_str() == op.name())
        .expect("every operation is registered");
    let answer = handler.call(
        &Nothing,
        &HostRequest {
            atom: EffectAtom::new(Symbol::new(EFFECT), Resource::Singleton, Mode::Read),
            op: declaration,
            args,
            span: Span::DUMMY,
            machine: MachineId(0),
            task: None,
            declared: None,
        },
    )?;
    match answer {
        HostAnswer::Value(v) => Ok(v),
        HostAnswer::Pending(_) => panic!("a time operation waits on nothing the runtime polls"),
    }
}

fn call(
    handlers: &[(HostOp, Arc<dyn HostHandler>)],
    op: Op,
    args: &[Value],
) -> Result<i64, Diagnostic> {
    answer(handlers, op, args)?.as_int(Span::DUMMY, "a reading")
}

fn read(handlers: &[(HostOp, Arc<dyn HostHandler>)], op: Op) -> i64 {
    call(handlers, op, &[]).unwrap_or_else(|d| panic!("refused: {} {}", d.code, d.message))
}

#[test]
fn the_registrations_declare_what_a_reviewer_relies_on() {
    let time = Arc::new(TimeHost::new());
    let handlers = registrations(&time);
    assert_eq!(handlers.len(), Op::ALL.len());
    for (op, _) in &handlers {
        assert_eq!(op.effect.as_str(), EFFECT);
        assert_eq!(op.determinism, Determinism::Nondeterministic);
        assert!(!op.blocking, "{op}");
        assert!(!op.secrets, "a time is never a credential");
        assert!(op.path.starts_with("ply_host::time::"));
    }
    // A reading consumes nothing, so a continuation may cross one more than once; a wait crossed
    // twice waits twice.
    for (op, _) in &handlers {
        let repeatable = op.op.as_str() != Op::SleepMs.name();
        assert_eq!(op.linearity == Linearity::Repeatable, repeatable, "{op}");
    }
    assert!(DECLARATION.contains("pub nondet effect time"));
    for op in Op::ALL {
        let declared = match op.arity() {
            0 => format!(" {}()", op.name()),
            _ => format!(" {}(", op.name()),
        };
        assert!(
            DECLARATION.contains(&declared),
            "`{}` is not declared in std.time",
            op.name()
        );
    }
}

/// The language's `clock` is virtual time, which `simulate` answers by name; this is the host's
/// real time, so its name must not be one the simulator claims or a file would have two of them.
#[test]
fn the_simulator_answers_no_effect_this_module_declares() {
    let simple = EFFECT.rsplit('.').next().expect("a dotted effect name");
    assert_eq!(simple, "time");
    assert!(
        !ply_eval::SEEDED_EFFECTS.contains(&simple),
        "`{simple}` is answered by `simulate`, so binding a host handler to it would be a second clock"
    );
    let time = Arc::new(TimeHost::new());
    for (op, _) in registrations(&time) {
        assert_eq!(op.effect.as_str(), EFFECT, "{op}");
    }
}

#[test]
fn the_wall_clock_reads_milliseconds_since_the_epoch() {
    let time = Arc::new(TimeHost::new());
    let handlers = registrations(&time);
    let now = read(&handlers, Op::NowMs);
    // After 2020 and before 2100: a clock outside that is one this run cannot stamp with.
    assert!(now > 1_577_836_800_000, "{now}");
    assert!(now < 4_102_444_800_000, "{now}");
    // The handler answers what the host reads rather than a number of its own.
    assert!((time.now_ms() - now).abs() < 1_000, "{now}");
}

#[test]
fn the_monotonic_clock_counts_from_the_run_and_never_goes_back() {
    let time = Arc::new(TimeHost::new());
    let handlers = registrations(&time);
    let first = read(&handlers, Op::ElapsedMs);
    assert!((0..1_000).contains(&first), "{first}");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = read(&handlers, Op::ElapsedMs);
    assert!(second >= first + 5, "{second} is not 5ms after {first}");
    // The two clocks are separate: the span since this run started is not a date.
    assert!(read(&handlers, Op::NowMs) > second);
}

/// The wait is the one operation a caller is owed time by, and the only one that takes an argument.
#[test]
fn a_wait_parks_for_the_span_it_was_given_and_answers_nothing() {
    let time = Arc::new(TimeHost::new());
    let handlers = registrations(&time);
    let before = time.elapsed_ms();
    let answered = answer(&handlers, Op::SleepMs, &[Value::Int(20)]).expect("a wait answers");
    assert!(matches!(answered, Value::Unit), "a wait reads nothing back");
    let after = time.elapsed_ms();
    assert!(
        after >= before + 20,
        "{before} to {after} is not a 20ms wait"
    );
    // A span no clock can run backwards over is no wait at all rather than a refusal.
    answer(&handlers, Op::SleepMs, &[Value::Int(-1)]).expect("a negative span waits for nothing");
}

// Arity is inference's, so the wrong count means the module was never checked.
#[test]
fn the_wrong_arity_is_a_dispatch_defect() {
    let time = Arc::new(TimeHost::new());
    let handlers = registrations(&time);
    for op in Op::ALL {
        let given = vec![Value::Unit; op.arity() + 1];
        let refused =
            answer(&handlers, op, &given).expect_err("an operation takes what it declares");
        assert_eq!(refused.code, codes::INTERNAL_ERROR, "{}", op.name());
    }
}

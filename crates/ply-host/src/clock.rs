//! The language's `clock` outside `simulate`: the run's monotonic clock, in nanoseconds since it
//! started. A sleep by a task of a production region never reaches here: the scheduler parks that
//! task against the same clock and runs the others. The wait here is the thread's, for a sleep
//! performed where no task could run meanwhile.

use ply_core::host::HostRegistry;
use ply_core::{
    Determinism, Diagnostic, HostOp, HostResource, HostRuntime, Linearity, Span, Symbol, Value,
    codes,
};
use std::time::Duration;

pub const EFFECT: &str = "clock";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Now,
    Sleep,
}

impl Op {
    pub const ALL: [Op; 2] = [Op::Now, Op::Sleep];

    pub fn name(self) -> &'static str {
        match self {
            Op::Now => "now",
            Op::Sleep => "sleep",
        }
    }

    /// The operation `declared` is, where it is one of the clock's.
    pub fn of(declared: &HostOp) -> Option<Op> {
        let named = |op: &Op| declared.op.as_str() == op.name();
        Op::ALL
            .into_iter()
            .find(|op| declared.effect.as_str() == EFFECT && named(op))
    }

    fn path(self) -> &'static str {
        match self {
            Op::Now => "ply_host::clock::now",
            Op::Sleep => "ply_host::clock::sleep",
        }
    }

    fn arity(self) -> usize {
        match self {
            Op::Now => 0,
            Op::Sleep => 1,
        }
    }

    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            // A reading consumes nothing and a wait changes nothing outside the program, so a
            // continuation that crosses one twice reads twice, or waits twice.
            linearity: Linearity::Repeatable,
            path: self.path(),
        }
    }
}

pub fn register(registry: &mut HostRegistry) {
    for op in Op::ALL {
        registry.register(op.declaration());
    }
}

/// What `op` answers `args`: `now` an `Instant` on the clock `runtime` keeps, `sleep` unit once
/// this thread has waited out its `Duration`, a negative one not at all.
///
/// # Errors
/// `runtime` keeps no clock, or `args` are not what the operation's declaration takes.
pub fn answer(
    op: Op,
    runtime: &dyn HostRuntime,
    args: &[Value],
    span: Span,
) -> Result<Value, Diagnostic> {
    match (op, args) {
        (Op::Now, []) => Ok(Value::ctor("Instant", vec![Value::Int(runtime.now()?)])),
        (Op::Sleep, [span_of_sleep]) => {
            let nanos = ply_core::sim::nanos_of(span_of_sleep, span, "`clock.sleep`")?;
            std::thread::sleep(Duration::from_nanos(u64::try_from(nanos).unwrap_or(0)));
            Ok(Value::Unit)
        }
        _ => Err(arity(op, args.len(), span)),
    }
}

#[cold]
fn arity(op: Op, got: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "`clock.{}` was performed with {got} argument(s) and takes {}",
            op.name(),
            op.arity()
        ),
    )
    .primary(span, "this perform reached the host boundary")
    .note("inference checks a perform's arity, so reaching this means the runtime was handed a module that was never checked")
}

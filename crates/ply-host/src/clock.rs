//! The language's `clock` outside `simulate`: the run's monotonic clock, in nanoseconds since it
//! started. A sleep by a task of a production region never reaches here: the scheduler parks that
//! task against the same clock and runs the others. The wait here is the thread's, for a sleep
//! performed where no task could run meanwhile.

use crate::time::TimeHost;
use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, Diagnostic, HostAnswer, HostHandler, HostOp, HostRequest, HostResource,
    HostRuntime, Linearity, Span, Symbol, Value, codes,
};
use std::sync::Arc;

pub const EFFECT: &str = "clock";

operations! {
    what "clock";
    path "clock";
    Now = "now" / 0,
    Sleep = "sleep" / 1,
}

impl Op {
    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            // A reading consumes nothing and a wait changes nothing outside the program, so a
            // continuation that crosses one twice reads twice, or waits twice.
            linearity: Linearity::Repeatable,
            blocking: false,
            secrets: false,
            path: self.path(),
        }
    }
}

pub fn registrations(time: &Arc<TimeHost>) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    Op::ALL
        .iter()
        .map(|op| {
            let handler: Arc<dyn HostHandler> = Arc::new(Operation {
                op: *op,
                time: Arc::clone(time),
            });
            (op.declaration(), handler)
        })
        .collect()
}

pub fn register(registry: &mut HostRegistry, time: Arc<TimeHost>) {
    for (op, handler) in registrations(&time) {
        registry.register(op, handler);
    }
}

struct Operation {
    op: Op,
    time: Arc<TimeHost>,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        if req.args.len() != self.op.arity() {
            return Err(arity(self.op, req.args.len(), req.span));
        }
        Ok(HostAnswer::Value(match self.op {
            Op::Now => Value::ctor("Instant", vec![Value::Int(self.time.elapsed_ns())]),
            Op::Sleep => {
                self.time.sleep_ns(ply_eval::sim::nanos_of(
                    &req.args[0],
                    req.span,
                    self.op.what(),
                )?);
                Value::Unit
            }
        }))
    }
}

#[cold]
fn arity(op: Op, got: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "{} was performed with {got} argument(s) and takes {}",
            op.what(),
            op.arity()
        ),
    )
    .primary(span, "this perform reached the host handler")
    .note("inference checks a perform's arity, so reaching this means the evaluator was handed a module that was never checked")
}

//! The `time` effect: a wall-clock reading and a monotonic one, both of the host's real time.

use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime,
    Linearity, Value,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The Ply declaration the registrations below are checked against.
pub const DECLARATION: &str = ply_std::TIME;

pub const MODULE: &str = "std.time";

pub const EFFECT: &str = "std.time.time";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    NowMs,
    ElapsedMs,
}

impl Op {
    pub const ALL: [Op; 2] = [Op::NowMs, Op::ElapsedMs];

    pub fn name(self) -> &'static str {
        match self {
            Op::NowMs => "now_ms",
            Op::ElapsedMs => "elapsed_ms",
        }
    }

    pub fn what(self) -> &'static str {
        match self {
            Op::NowMs => "`time.now_ms`",
            Op::ElapsedMs => "`time.elapsed_ms`",
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Op::NowMs => "ply_host::time::now_ms",
            Op::ElapsedMs => "ply_host::time::elapsed_ms",
        }
    }

    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            // A reading consumes nothing, so a continuation may cross one more than once.
            linearity: Linearity::Repeatable,
            blocking: false,
            secrets: false,
            path: self.path(),
        }
    }
}

/// The run's two clocks: the system's, and one counting from the moment this host was built.
pub struct TimeHost {
    started: Instant,
}

impl Default for TimeHost {
    fn default() -> TimeHost {
        TimeHost::new()
    }
}

impl TimeHost {
    pub fn new() -> TimeHost {
        TimeHost {
            started: Instant::now(),
        }
    }

    /// Milliseconds since the Unix epoch; a clock set before it reads `0` rather than refusing.
    pub fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0)
    }

    /// Milliseconds since this run's clock was started; only the difference of two means anything.
    pub fn elapsed_ms(&self) -> i64 {
        i64::try_from(self.started.elapsed().as_millis()).unwrap_or(i64::MAX)
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
        if !req.args.is_empty() {
            return Err(arity(self.op, req.args.len(), req.span));
        }
        Ok(HostAnswer::Value(Value::Int(match self.op {
            Op::NowMs => self.time.now_ms(),
            Op::ElapsedMs => self.time.elapsed_ms(),
        })))
    }
}

#[cold]
fn arity(op: Op, got: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "{} was performed with {got} argument(s) and takes none",
            op.what()
        ),
    )
    .primary(span, "this perform reached the host handler")
    .note("inference checks a perform's arity, so reaching this means the evaluator was handed a module that was never checked")
}

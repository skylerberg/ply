//! The `time` effect: a wall-clock reading, a monotonic one and a wait, all of the host's real
//! time.

use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, Diagnostic, HostAnswer, HostHandler, HostOp, HostRequest, HostResource,
    HostRuntime, Linearity, Span, Symbol, Value, codes,
};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The Ply declaration the registrations below are checked against.
pub const DECLARATION: &str = ply_std::TIME;

pub const MODULE: &str = "std.time";

pub const EFFECT: &str = "std.time.time";

operations! {
    what "time";
    path "time";
    NowMs = "now_ms" / 0,
    ElapsedMs = "elapsed_ms" / 0,
    ElapsedUs = "elapsed_us" / 0,
    SleepMs = "sleep_ms" / 1,
}

impl Op {
    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: match self {
                // A reading consumes nothing, so a continuation may cross one more than once.
                Op::NowMs | Op::ElapsedMs | Op::ElapsedUs => Linearity::Repeatable,
                // A wait crossed twice waits twice, as a line written twice is written twice.
                Op::SleepMs => Linearity::AtMostOnce,
            },
            // The thread that performs the wait is the one that owes it: nothing is dispatched.
            blocking: false,
            secrets: false,
            path: self.path(),
        }
    }
}

/// The run's two clocks: the system's, and one counting from the process's start when it was
/// marked, else from the moment this host was built.
pub struct TimeHost {
    started: Instant,
}

static PROCESS: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Marks when the process started, so a program's first reading of the clock says what came
/// before it. The first mark wins.
pub fn mark_start(at: Instant) {
    let _ = PROCESS.set(at);
}

impl Default for TimeHost {
    fn default() -> TimeHost {
        TimeHost::new()
    }
}

impl TimeHost {
    pub fn new() -> TimeHost {
        TimeHost {
            started: PROCESS.get().copied().unwrap_or_else(Instant::now),
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

    /// Microseconds since this run's clock was started. A benchmark needs a reading finer than a
    /// millisecond: an in-process call is measured in microseconds, and 200 of them are still a
    /// fifth of a millisecond.
    pub fn elapsed_us(&self) -> i64 {
        i64::try_from(self.started.elapsed().as_micros()).unwrap_or(i64::MAX)
    }

    /// Parks this thread for `ms`; a span no clock can run backwards over, so a negative one is no
    /// wait at all rather than a refusal.
    pub fn sleep_ms(&self, ms: i64) {
        std::thread::sleep(Duration::from_millis(u64::try_from(ms).unwrap_or(0)));
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
            Op::NowMs => Value::Int(self.time.now_ms()),
            Op::ElapsedMs => Value::Int(self.time.elapsed_ms()),
            Op::ElapsedUs => Value::Int(self.time.elapsed_us()),
            Op::SleepMs => {
                self.time.sleep_ms(req.args[0].as_int(req.span, "a wait")?);
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

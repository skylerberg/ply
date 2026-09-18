//! The production task scheduler's half of the boundary.

use ply_eval::MachineScheduler;
use ply_eval::sched::{HostPolicy, Scheduler};
use ply_eval::sim::TASK_OPS;
use ply_eval::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRequest, HostResource,
    HostRuntime, Linearity, SimId,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::sync::Arc;

const TASK: &str = "task";

pub fn registrations() -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    TASK_OPS
        .iter()
        .map(|op| {
            (
                HostOp {
                    effect: Symbol::new(TASK),
                    op: Symbol::new(*op),
                    resource: HostResource::Any,
                    determinism: Determinism::Nondeterministic,
                    linearity: Linearity::Repeatable,
                    blocking: false,
                    // `spawn` is handed a closure and `join` a `Task`.
                    secrets: false,
                    path: path_of(op),
                },
                Arc::new(Scheduled) as Arc<dyn HostHandler>,
            )
        })
        .collect()
}

fn path_of(op: &str) -> &'static str {
    match op {
        "spawn" => "ply_host::sched::spawn",
        "join" => "ply_host::sched::join",
        _ => "ply_host::sched::yield",
    }
}

pub fn open(
    binding: &HostBinding,
    region: SimId,
    span: Span,
) -> Result<MachineScheduler, Diagnostic> {
    match HostPolicy::of(binding) {
        Some(permit) => Ok(Scheduler::production(region, span, permit)),
        None => Err(err_hermetic(span, binding)),
    }
}

/// The handler registered against `task.*`, which exists to be listed and to refuse.
struct Scheduled;

impl HostHandler for Scheduled {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!(
                "`{}.{}` was dispatched to a host handler instead of opening a production region",
                req.op.effect, req.op.op
            ),
        )
        .primary(req.span, "performed here")
        .note("a task is a suspended machine state, so `task.*` is answered by the scheduler the machine opens rather than by a handler that sees only values")
        .note("this is a defect in Ply's host dispatch rather than in the program"))
    }
}

#[cold]
#[inline(never)]
fn err_hermetic(span: Span, binding: &HostBinding) -> Diagnostic {
    let spawn = Symbol::new("spawn");
    let effect = Symbol::new(TASK);
    let mut diagnostic = Diagnostic::error(
        codes::HERMETIC_BOUNDARY,
        "`task.spawn` reached the host boundary in a hermetic run",
    )
    .primary(span, "no handler here, and no production scheduler is bound")
    .note("`ply test` is hermetic: it binds simulated handlers and refuses real ones")
    .note("wrap this in `simulate { .. }` to get the seeded scheduler, or run with `--host` for real concurrency");
    if let Some(path) = binding.would_serve(&effect, &spawn, None) {
        diagnostic = diagnostic.note(format!("`{path}` would serve this under `--host`"));
    }
    diagnostic
}

//! The production task scheduler's half of the boundary.

use ply_eval::sim::TASK_OPS;
use ply_eval::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_span::{Diagnostic, Symbol, codes};
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

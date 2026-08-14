//! The production task scheduler's half of the boundary.
//!
//! M7 declared `task.spawn` / `task.join` / `task.yield` and built only the
//! seeded handler. This is the second handler for those same operations, and the
//! decision that makes it small is ADR 0011 §9's: **a Ply task cannot move
//! between OS threads.** A `Value` holds `Rc`, a continuation is `Rc<Vec<Segment>>`
//! and a `Machine` is single-threaded by construction, so the production
//! scheduler is not one task per thread. It is the same cooperative scheduler
//! over the same machine — [`ply_eval::sched::Scheduler`] under
//! [`Policy::Host`] — choosing by real readiness instead of by a seed, with real
//! threads confined to the host runtime, where no Ply value ever goes.
//!
//! So this module holds no scheduling algorithm at all. What it holds is the
//! three things that are the host's rather than the evaluator's: the
//! registrations `ply hosts` prints for `task.*`, the gate that decides whether
//! a production region may be opened, and the reason a `task.*` perform must
//! never reach an ordinary [`HostHandler`].
//!
//! **What makes the two schedulers mutually exclusive.** Four independent locks,
//! and no one of them is load-bearing alone:
//!
//! 1. `task` is declared `nondet`, so a `det` test performing `task.*` without a
//!    handler is `E0412` and never runs. Only a `test/nondet` — never cached,
//!    opted into by hand — can reach here at all.
//! 2. `simulate` pushes a `Delimiter::Sim` and `find_handler` walks the stack
//!    innermost-first before ever consulting the host binding, so a `task.spawn`
//!    inside a region reaches the seeded scheduler always.
//! 3. [`open`] needs a [`HostPolicy`], which only a **bound** [`HostBinding`]
//!    mints, and `ply test` binds `HostBinding::hermetic()`. There is no
//!    expression a hermetic run can write that produces one.
//! 4. [`Scheduler::next`] refuses a production region and
//!    [`Scheduler::next_host`] refuses a seeded one, so a scheduler driven by the
//!    wrong loop is a diagnostic rather than a silently different answer.
//!
//! [`Policy::Host`]: ply_eval::sched::Policy::Host
//! [`Scheduler::next`]: ply_eval::sched::Scheduler::next
//! [`Scheduler::next_host`]: ply_eval::sched::Scheduler::next_host

use ply_eval::sched::{HostPolicy, Scheduler};
use ply_eval::sim::TASK_OPS;
use ply_eval::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRequest, HostResource,
    HostRuntime, Linearity, SimId,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::sync::Arc;

/// The effect the three operations belong to, as every `EffectAtom` names it.
const TASK: &str = "task";

/// The registrations, in the order `ply hosts` will sort them into anyway.
///
/// Every one is `HostResource::Any`, and that is not a widening: `task.*` is
/// declared without `[r]`, so the only label `Any` can ever resolve to is the
/// singleton. What it buys is silence. `Only(Singleton)` is `E0421` for a
/// program whose footprint never mentions `task.write` — a registration for an
/// atom nothing performs — and this registry is compiled into *every* program,
/// including the great majority that never spawn a task. `Any` resolving to
/// nothing is a driver that is idle rather than wrong, which is exactly the case
/// here.
///
/// `Repeatable`, because spawning or joining a Ply task changes nothing outside
/// the program: it creates and observes a machine state. That keeps ADR 0011
/// §3's over-approximation tight — a multi-shot continuation captured across a
/// `task.spawn` is unaffected — and it is a claim a reviewer can check, which is
/// why the column is printed.
///
/// `blocking: false`, because none of the three ever waits: `spawn` and `yield`
/// answer at once and `join` parks the performing task in the scheduler rather
/// than on a thread.
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
                    path: path_of(op),
                },
                Arc::new(Scheduled) as Arc<dyn HostHandler>,
            )
        })
        .collect()
}

/// One path per operation rather than one for the trio: `ply hosts` prints a
/// line per triple, and three lines naming one path would tell a reviewer less
/// than three lines naming three.
fn path_of(op: &str) -> &'static str {
    match op {
        "spawn" => "ply_host::sched::spawn",
        "join" => "ply_host::sched::join",
        _ => "ply_host::sched::yield",
    }
}

/// Opens a production region, or says why it may not be opened.
///
/// Called by the machine at the **first** `task.*` perform that reaches the host
/// binding, rooted at the stack it was performed on — lazily, per ADR 0011 §9,
/// because opening one eagerly around every entry point would make every
/// existing `simulate` nested and `E0416` under `--host`.
///
/// The [`HostPolicy`] this consumes is the whole of lock 3. It exists only for a
/// bound [`HostBinding`], it is not `Clone`, and [`Scheduler::production`] takes
/// it by value — so a permit cannot be minted under `--host` and kept for a run
/// that was configured hermetically.
pub fn open(binding: &HostBinding, region: SimId, span: Span) -> Result<Scheduler, Diagnostic> {
    match HostPolicy::of(binding) {
        Some(permit) => Ok(Scheduler::production(region, span, permit)),
        None => Err(err_hermetic(span, binding)),
    }
}

/// The handler registered against `task.*`, which exists to be listed and to
/// refuse.
///
/// A `task.*` perform that reaches the binding is answered by opening a region
/// ([`open`]), not by calling a handler: a handler is handed argument values and
/// a span, and a task is a suspended machine state that only the machine can
/// build. Reaching [`HostHandler::call`] therefore means the machine did not
/// intercept, and the useful thing to do about it is say so — the alternative is
/// a `task.spawn` that quietly returns a value and a program whose concurrency
/// silently did not happen.
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

/// Reaching the production scheduler with nothing bound.
///
/// `E0424` and deliberately not `E0303`: inference was right, the row was legal,
/// and the run was configured hermetically. The two call for opposite responses
/// — file a bug, versus pass `--host` or write a test double — and a consumer
/// that cannot tell them apart will do the wrong one.
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

#[cfg(test)]
mod tests;

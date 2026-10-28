//! The host boundary from the C backend: an unhandled `perform` reaches the context's binding
//! through the machine's checks, in the machine's order.

use crate::emitted::{Carried, Handed, PlyOp};
use crate::heap::Word;
use crate::rt::Ctx;
use ply_core::handler::err_unhandled;
use ply_core::host::{
    Bound, HostAnswer, HostBinding, HostRequest, Unserved, attribute, carries_secret,
    check_host_answer, err_blocking_answered_inline, err_footprint_escape, err_host_in_search,
    err_host_in_simulation, err_secret_to_host, err_withheld, operation_label, resource_of,
};
use ply_core::{Diagnostic, Span, Symbol, Value, codes};
use ply_core::{EffectAtom, Mode};
use std::sync::Arc;

/// An at-most-once host operation an entry performed, which a second resumption would replay.
pub(crate) struct HostMark {
    pub(crate) operation: String,
    pub(crate) path: &'static str,
    pub(crate) span: Span,
}

/// A second resumption across an at-most-once host operation.
#[cold]
#[inline(never)]
pub(crate) fn err_continuation_resumed(
    span: Span,
    resumes: u32,
    last: Option<&HostMark>,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::HOST_CONTINUATION_RESUMED,
        "this continuation is resumed again across a host operation",
    )
    .primary(span, format!("resumption {resumes} of one continuation"));
    if let Some(mark) = last {
        if mark.span != Span::DUMMY {
            diagnostic = diagnostic.secondary(
                mark.span,
                format!("`{}` was performed here, after the capture", mark.operation),
            );
        }
        diagnostic = diagnostic.note(format!(
            "`{}` is registered `at-most-once`, so replaying this control would perform it again",
            mark.path
        ));
    }
    diagnostic
        .note("multi-shot resumption stays available for pure and in-memory handlers; the restriction is on the boundary, not on the feature")
        .note("resume at most once across a host operation, or -- only if replaying it really changes nothing outside the program -- register the operation `Linearity::Repeatable`")
        .note("the rule is conservative: it refuses when any at-most-once host operation happened after the capture, including in another task")
}

/// What the host answered an operation.
pub(crate) enum Served {
    Value(Value),
    /// 0, with the context failed where the operation failed.
    Word(Word),
}

/// What the binding makes of `effect.op` before any argument is read: refused where a seeded region
/// is in reach of what would serve it, or where nothing it binds does; otherwise what serves it.
fn bound<'b>(
    c: &Ctx,
    binding: &'b HostBinding,
    effect: &Symbol,
    op: &Symbol,
    resource: Option<&Symbol>,
) -> Result<Bound<'b>, Diagnostic> {
    let span = c.site();
    let operation = operation_label(effect, op, resource);
    let resolved = binding.resolve(effect, op, resource);
    // Ahead of every other refusal: under a seed the region is re-run per schedule, so an answer
    // any handler could give, bound or withheld, would differ between interleavings.
    if let Some(region) = crate::tasks::seeded_region(c)
        && !matches!(resolved, Err(Unserved::Unregistered))
    {
        return Err(err_host_in_simulation(span, &operation, region));
    }
    resolved.map_err(|unserved| match unserved {
        Unserved::Unregistered => err_unhandled(span, effect, op, resource),
        Unserved::Withheld { path, served } => err_withheld(span, &operation, effect, path, served),
    })
}

/// Whether `effect.op`, which nothing on the stack answers, is a sleep by a task of the run's
/// region, which waits there while the region's other tasks run. Outside one no task could run
/// meanwhile, and a sleep is the host's.
pub(crate) fn production_scheduled(c: &Ctx, effect: &Symbol, op: &Symbol) -> bool {
    effect.as_str() == "clock"
        && op.as_str() == "sleep"
        && crate::tasks::production_task(c).is_some()
}

/// [`serve`] of a sleep [`production_scheduled`] says is the run's region's: the binding's
/// refusals, then the task waits in its region for the nanoseconds `args` hold, which this takes.
pub(crate) unsafe fn schedule(
    ctx: *mut Ctx,
    described: &PlyOp,
    effect: &Symbol,
    op: &Symbol,
    resource: Option<&Symbol>,
    args: &[Word],
) -> Word {
    let c = unsafe { &mut *ctx };
    let binding = Arc::clone(&c.binding);
    let refused = bound(c, &binding, effect, op, resource).err();
    let lowered: Vec<Result<Value, String>> = args
        .iter()
        .zip(described.params())
        .map(|(w, ty)| {
            let carried = unsafe { Carried::new(Handed::of(*w), *ty) };
            let value = carried.lower(ctx);
            carried.release(ctx);
            value
        })
        .collect();
    let c = unsafe { &mut *ctx };
    if let Some(d) = refused {
        return c.fail(d);
    }
    let span = c.site();
    let nanos = match lowered.first() {
        Some(Ok(d)) => ply_core::sim::nanos_of(d, span, "clock.sleep"),
        _ => Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "a sleep reached the run's region with no duration it could read",
        )),
    };
    match nanos {
        Ok(n) => {
            crate::tasks::waited(c, crate::tasks::ASK_SLEEP, n);
            0
        }
        Err(d) => c.fail(d),
    }
}

/// `effect.op` performed on the host with `values`, through the machine's checks in the machine's
/// order, `mode` as its declaration states it. [`production_scheduled`] operations are
/// [`schedule`]'s.
pub(crate) unsafe fn serve(
    ctx: *mut Ctx,
    effect: &Symbol,
    op: &Symbol,
    resource: Option<&Symbol>,
    mode: Mode,
    values: Vec<Value>,
) -> Served {
    let c = unsafe { &mut *ctx };
    let failed = |c: &mut Ctx, d: Diagnostic| Served::Word(c.fail(d));
    let span = c.site();
    let operation = operation_label(effect, op, resource);
    let binding = Arc::clone(&c.binding);
    let bound = match bound(c, &binding, effect, op, resource) {
        Ok(bound) => bound,
        Err(d) => return failed(c, d),
    };
    let atom = EffectAtom::operation(effect.clone(), resource_of(resource), mode, op.clone());
    let declaration = bound.op.clone();
    let handler = Arc::clone(bound.handler);
    if let Some(declared) = &c.declared
        && !declared.covers(&atom)
    {
        return failed(
            c,
            err_footprint_escape(span, &operation, &atom, declared, declaration.path),
        );
    }
    if c.re_executed {
        return failed(c, err_host_in_search(span, &operation, declaration.path));
    }
    if !declaration.secrets
        && let Some(position) = values.iter().position(carries_secret)
    {
        return failed(
            c,
            err_secret_to_host(span, &operation, position, declaration.path),
        );
    }
    if let Err(d) = ply_core::escape::check_arguments(&operation, declaration.path, &values, span) {
        return failed(c, d);
    }
    let runtime = c.host_runtime();
    let answered = {
        let request = HostRequest {
            atom: atom.clone(),
            op: &declaration,
            args: &values,
            span,
            machine: c.id,
            task: crate::tasks::production_task(c),
            declared: c.declared.as_ref(),
        };
        handler.call(runtime.as_ref(), &request)
    };
    c.host_use.record(declaration.determinism);
    if declaration.linearity.is_linear() {
        c.host_ops = c.host_ops.saturating_add(1);
        c.last_linear = Some(HostMark {
            operation: operation.clone(),
            path: declaration.path,
            span,
        });
    }
    let answer = match answered {
        Ok(answer) => answer,
        Err(d) => return failed(c, attribute(d, declaration.path, &operation, span)),
    };
    let value = match answer {
        HostAnswer::Value(value) => {
            if declaration.blocking {
                return failed(
                    c,
                    err_blocking_answered_inline(span, &operation, declaration.path),
                );
            }
            value
        }
        // A task of the run's region waits there for the answer, which is then at hand.
        HostAnswer::Pending(pending) => {
            match crate::tasks::waited(c, crate::tasks::ASK_TICKET, pending.token as i64) {
                Some(false) => return Served::Word(0),
                Some(true) | None => match runtime.block_on(pending) {
                    Ok(value) => value,
                    Err(d) => return failed(c, d),
                },
            }
        }
    };
    if let Err(d) = check_host_answer(&operation, declaration.path, &value, span) {
        return failed(c, d);
    }
    Served::Value(value)
}

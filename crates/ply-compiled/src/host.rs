//! The host boundary from the C backend: an unhandled `perform` reaches the context's binding
//! through the machine's checks, in the machine's order.

use crate::emitted::{Carried, Handed, PlyOp};
use crate::heap::Word;
use crate::rt::Ctx;
use ply_core::handler::err_unhandled;
use ply_core::host::{
    Bound, HostAnswer, HostBinding, HostRequest, attribute, err_blocking_answered_inline,
    err_hermetic, err_host_in_search, err_withheld, operation_label,
};
use ply_core::sim::TASK_OPS;
use ply_core::{
    Diagnostic, Span, Symbol, Unbound, Value, carries_secret, check_host_answer, codes,
    err_footprint_escape, err_host_in_simulation, err_no_runtime, err_secret_to_host,
    err_unenumerated_atom,
};
use std::sync::Arc;

/// An at-most-once host operation an entry performed, which a second resumption would replay.
pub(crate) struct HostMark {
    operation: String,
    path: &'static str,
    span: Span,
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
    let would = binding.would_serve(effect, op, resource);
    let resolved = binding.resolve(effect, op, resource);
    // Ahead of every other refusal: under a seed the region is re-run per schedule, so an answer
    // any handler could give -- bound, withheld or hermetic -- would differ between interleavings.
    if let Some(region) = crate::simulate::seeded_region(c)
        && (would.is_some() || resolved.is_some())
    {
        return Err(err_host_in_simulation(span, &operation, region));
    }
    resolved.ok_or_else(|| match would {
        None => err_unhandled(span, effect, op, resource),
        Some(path) if binding.is_hermetic() => {
            let hermetic = err_hermetic(span, &operation, path);
            if c.re_executed {
                hermetic.note(
                    "`--host` would then refuse this: the search runs a seeded test whole once per interleaving, so a handler would answer it once per schedule",
                )
            } else {
                hermetic
            }
        }
        Some(path) => match binding.withholds(effect, op, resource) {
            Some(served) => err_withheld(span, &operation, effect, path, served),
            None => err_unenumerated_atom(span, &operation, path),
        },
    })
}

/// Whether `effect.op`, which nothing on the stack answers and the binding serves, is the production
/// scheduler's: a `task` operation, or a sleep inside an open production region, which parks the
/// task alone. Outside one no task could run meanwhile, and a sleep is the host's.
pub(crate) fn production_scheduled(c: &Ctx, effect: &Symbol, op: &Symbol) -> bool {
    match effect.as_str() {
        "task" => TASK_OPS.contains(&op.as_str()),
        "clock" => {
            op.as_str() == "sleep" && crate::simulate::running_task_of_production(c).is_some()
        }
        _ => false,
    }
}

/// [`serve`] of an operation emitted code performed that [`production_scheduled`] says is the
/// production scheduler's: the binding's refusals, the region opened for a `task` operation where
/// none is, and `args` handed to it as they were performed, which this takes.
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
    let opened = refused.is_none()
        && (effect.as_str() != "task"
            || unsafe { crate::simulate::open_production(ctx, effect, op) });
    if !opened {
        for (w, ty) in args.iter().zip(described.params()) {
            unsafe { Carried::new(Handed::of(*w), *ty) }.release(ctx);
        }
        let c = unsafe { &mut *ctx };
        return refused.map_or(0, |d| c.fail(d));
    }
    unsafe { crate::simulate::perform(ctx, effect, op, args, described) }
}

/// `effect.op` performed on the host with `values`, through the machine's checks in the machine's
/// order. [`production_scheduled`] operations are [`schedule`]'s.
pub(crate) unsafe fn serve(
    ctx: *mut Ctx,
    effect: &Symbol,
    op: &Symbol,
    resource: Option<&Symbol>,
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
    let atom = bound.atom.clone();
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
            task: crate::simulate::running_task_of_production(c),
            declared: c.declared.as_ref(),
        };
        match &runtime {
            Some(rt) => handler.call(rt.as_ref(), &request),
            None => handler.call(&Unbound, &request),
        }
    };
    c.host_use.record(&atom, declaration.determinism);
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
        HostAnswer::Pending(pending) => {
            if crate::simulate::running_task_of_production(c).is_some() {
                match unsafe { crate::simulate::park(ctx, pending) } {
                    Some(value) => value,
                    None => return Served::Word(0),
                }
            } else {
                let Some(rt) = runtime else {
                    return failed(
                        c,
                        err_no_runtime(span, &operation, pending, declaration.path),
                    );
                };
                match rt.block_on(pending) {
                    Ok(value) => value,
                    Err(d) => return failed(c, d),
                }
            }
        }
    };
    if let Err(d) = check_host_answer(&operation, declaration.path, &value, span) {
        return failed(c, d);
    }
    Served::Value(value)
}

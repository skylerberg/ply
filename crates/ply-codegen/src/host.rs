//! The host boundary from the compiled tier: a `perform` nothing on the stack answers reaches
//! the binding the context carries, through the machine's checks in the machine's order, and a
//! pending answer is waited on the reactor. What is not here is the production region the host
//! policy opens for a `task` operation outside any `simulate`; the fixpoint leaves such a
//! performer to the machine.

use crate::heap::{self, Word};
use crate::rt::{Ctx, values_taken};
use ply_eval::handler::err_unhandled;
use ply_eval::host::{
    HostAnswer, HostRequest, attribute, err_blocking_answered_inline, err_hermetic,
    err_host_in_search, err_withheld, operation_label,
};
use ply_eval::sim::TASK_OPS;
use ply_eval::{
    Unbound, carries_secret, check_host_answer, err_footprint_escape, err_host_in_simulation,
    err_no_runtime, err_secret_to_host, err_unenumerated_atom,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::sync::Arc;

/// An at-most-once host operation an entry performed: what a second resumption across it would
/// replay.
pub(crate) struct HostMark {
    operation: String,
    path: &'static str,
    span: Span,
}

/// `E0426` -- a second resumption across an at-most-once host operation.
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

pub unsafe fn perform(
    ctx: *mut Ctx,
    effect: &Symbol,
    op: &Symbol,
    resource: Option<&Symbol>,
    args: &[Word],
) -> Word {
    let c = unsafe { &mut *ctx };
    let span = c.site();
    let values = values_taken(c, args);
    let operation = operation_label(effect, op, resource);
    let binding = Arc::clone(&c.binding);
    let would = binding.would_serve(effect, op, resource);
    if would.is_some() && crate::simulate::innermost_is_seeded(c) {
        return c.fail(err_host_in_simulation(span, &operation, Span::DUMMY));
    }
    let Some(bound) = binding.resolve(effect, op, resource) else {
        let d = match would {
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
            Some(path) if binding.withholds(effect, op, resource).is_some() => {
                err_withheld(span, &operation, effect, path)
            }
            Some(path) => err_unenumerated_atom(span, &operation, path),
        };
        return c.fail(d);
    };
    // A `task` operation the binding serves opens the production region with the performer's
    // stack as the root task, and is answered by its scheduler.
    if effect.as_str() == "task" && TASK_OPS.contains(&op.as_str()) {
        let words: Vec<Word> = values.iter().map(|v| c.word(v)).collect();
        if !unsafe { crate::simulate::open_production(ctx, effect, op) } {
            return 0;
        }
        return unsafe { crate::simulate::perform(ctx, effect, op, &words) };
    }
    let atom = bound.atom.clone();
    let declaration = bound.op.clone();
    let handler = Arc::clone(bound.handler);
    if let Some(declared) = &c.declared
        && !declared.contains(&atom)
    {
        return c.fail(err_footprint_escape(
            span,
            &operation,
            &atom,
            declared,
            declaration.path,
        ));
    }
    if c.re_executed {
        return c.fail(err_host_in_search(span, &operation, declaration.path));
    }
    if !declaration.secrets
        && let Some(position) = values.iter().position(carries_secret)
    {
        return c.fail(err_secret_to_host(
            span,
            &operation,
            position,
            declaration.path,
        ));
    }
    if let Err(d) = ply_eval::escape::check_arguments(&operation, declaration.path, &values, span) {
        return c.fail(d);
    }
    let runtime = c.runtime.clone();
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
    c.host_use.record(&atom);
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
        Err(d) => return c.fail(attribute(d, declaration.path, &operation, span)),
    };
    let value = match answer {
        HostAnswer::Value(value) => {
            if declaration.blocking {
                return c.fail(err_blocking_answered_inline(
                    span,
                    &operation,
                    declaration.path,
                ));
            }
            value
        }
        HostAnswer::Pending(pending) => {
            if crate::simulate::running_task_of_production(c).is_some() {
                let w = unsafe { crate::simulate::park(ctx, pending) };
                let c = unsafe { &mut *ctx };
                if c.failed != 0 {
                    return 0;
                }
                let value = c.value(w);
                heap::dec(w);
                value
            } else {
                let Some(rt) = runtime else {
                    return c.fail(err_no_runtime(span, &operation, pending, declaration.path));
                };
                match rt.block_on(pending) {
                    Ok(value) => value,
                    Err(d) => return c.fail(d),
                }
            }
        }
    };
    if let Err(d) = check_host_answer(&operation, declaration.path, &value, span) {
        return c.fail(d);
    }
    c.word(&value)
}

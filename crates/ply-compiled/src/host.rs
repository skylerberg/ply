//! The host boundary from the C backend: an unhandled `perform` reaches the context's binding
//! through the machine's checks, in the machine's order.

use crate::emitted::{Carried, Handed, PlyOp};
use crate::heap::Word;
use crate::rt::Ctx;
use ply_core::handler::err_unhandled;
use ply_core::host::{
    HostBinding, HostOp, err_footprint_escape, err_host_in_search, err_host_in_simulation,
    operation_label, resource_of,
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
/// is in reach of what would serve it, or where nothing it binds does; otherwise its declaration.
fn bound<'b>(
    c: &Ctx,
    binding: &'b HostBinding,
    effect: &Symbol,
    op: &Symbol,
    resource: Option<&Symbol>,
) -> Result<&'b HostOp, Diagnostic> {
    let span = c.site();
    let resolved = binding.resolve(effect, op, resource);
    // Ahead of every other refusal: under a seed the region is re-run per schedule, so what the
    // host answered would differ between interleavings.
    if let Some(region) = crate::tasks::seeded_region(c)
        && resolved.is_some()
    {
        let operation = operation_label(effect, op, resource);
        return Err(err_host_in_simulation(span, &operation, region));
    }
    resolved.ok_or_else(|| err_unhandled(span, effect, op, resource))
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
    let binding = Arc::clone(&c.binding);
    let declaration = match bound(c, &binding, effect, op, resource) {
        Ok(declaration) => declaration,
        Err(d) => return failed(c, d),
    };
    let atom = EffectAtom::operation(effect.clone(), resource_of(resource), mode, op.clone());
    if let Some(declared) = &c.declared
        && !declared.covers(&atom)
    {
        let operation = operation_label(effect, op, resource);
        return failed(
            c,
            err_footprint_escape(span, &operation, &atom, declared, declaration.path),
        );
    }
    if c.re_executed {
        let operation = operation_label(effect, op, resource);
        return failed(c, err_host_in_search(span, &operation, declaration.path));
    }
    let answered = ply_host::registry::answer(declaration, &*c.host_runtime(), &values, span);
    c.host_use.record(declaration.determinism);
    match answered {
        Ok(value) => Served::Value(value),
        Err(d) => failed(c, d),
    }
}

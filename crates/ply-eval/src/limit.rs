//! The bound on runaway recursion.

use ply_span::{Diagnostic, Span, Symbol, codes};

/// The most nested calls a program may hold at once.
pub const DEFAULT_MAX_CALLS: usize = 10_000;

/// The deepest a value may nest before a structural walk over it refuses.
pub const MAX_VALUE_DEPTH: usize = DEFAULT_MAX_CALLS;

/// One step of a native recursion costs several frames and an unoptimized build makes each of them
/// large, so a worker's default 2 MiB runs out long before either bound above does.
pub(crate) fn grow<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, f)
}

/// How many of the innermost calls the diagnostic names.
pub(crate) const NAMED_CALLS: usize = 6;

/// The message keeps the phrase "recursion limit", and names the innermost calls — the actual
/// recursion path.
pub(crate) fn err_recursion_limit(
    span: Span,
    what: &str,
    max: usize,
    innermost: &[Option<Symbol>],
) -> Diagnostic {
    let mut diag = Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("recursion limit of {max} {what} exceeded"),
    )
    .primary(span, "this call is too deeply nested")
    .note("check for a recursive call that never reaches its base case");

    let named: Vec<String> = innermost
        .iter()
        .take(NAMED_CALLS)
        .map(|name| match name {
            Some(n) => format!("`{n}`"),
            None => "an anonymous function".to_string(),
        })
        .collect();
    if !named.is_empty() {
        diag = diag.note(format!("innermost calls: {}", named.join(" from ")));
    }
    diag
}

/// A machine ran out of the frame ceiling it was asked for.
pub(crate) fn err_frame_ceiling(
    span: Span,
    max: usize,
    max_calls: usize,
    innermost: &[Option<Symbol>],
) -> Diagnostic {
    let mut diag = Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("this engine's ceiling of {max} {PENDING_FRAMES} was reached"),
    )
    .primary(span, "evaluating this ran the machine out of frames")
    .note(format!(
        "a frame ceiling is a resource guard on the control-stack machine's own heap, not a \
         bound on the program: the program's bound is {max_calls} {NESTED_CALLS}"
    ));

    let named: Vec<String> = innermost
        .iter()
        .take(NAMED_CALLS)
        .map(|name| match name {
            Some(n) => format!("`{n}`"),
            None => "an anonymous function".to_string(),
        })
        .collect();
    if !named.is_empty() {
        diag = diag.note(format!("innermost calls: {}", named.join(" from ")));
    }
    diag
}

/// An `iterate` whose step never answered `Stop`.
pub(crate) fn err_iterate_budget(span: Span, budget: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`iterate` took its budget of {budget} steps without stopping"),
    )
    .primary(span, "this loop never answered `Stop`")
    .note("raise the budget if the loop is right, or check the step that should have stopped")
}

/// A budget that is not a count of steps.
pub(crate) fn err_iterate_budget_not_a_count(span: Span, budget: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`iterate` was given a budget of {budget}"),
    )
    .primary(span, "a budget is the most steps the loop may take")
    .note("it must be at least 1")
}

/// A value too deep to walk.
pub(crate) fn err_value_depth(span: Span, max: usize) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("recursion limit of {max} {NESTED_VALUES} exceeded"),
    )
    .primary(span, "this value nests too deeply to walk")
    .note("a value this deep is reachable only by iteration; compare its parts instead")
}

/// What each bound counts, in the message.
pub(crate) const NESTED_CALLS: &str = "nested calls";
pub(crate) const PENDING_FRAMES: &str = "pending frames";
pub(crate) const NESTED_VALUES: &str = "nested values";

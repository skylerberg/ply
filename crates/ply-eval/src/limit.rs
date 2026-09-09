//! The bound on runaway recursion.

use ply_span::{Diagnostic, Span, codes};

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
pub(crate) const NESTED_VALUES: &str = "nested values";

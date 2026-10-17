//! The two bounds on a runaway program: how deep it may nest, and how much work it may do.

use crate::{Diagnostic, Span, codes};

/// The most nested calls a program may hold at once.
pub const DEFAULT_MAX_CALLS: usize = 10_000;

/// The calls one entry may make before it is refused more work; 0 is no bound. A tail call is a
/// loop, so a call is the unit of work that is already counted on every path.
pub const DEFAULT_STEP_BUDGET: i64 = 1_000_000_000;

pub const MAX_VALUE_DEPTH: usize = DEFAULT_MAX_CALLS;

/// Grows the stack: unoptimized native recursion exhausts a worker's stack before either bound.
pub fn grow<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, f)
}

/// The lowest address of the stack this frame is on, as [`grow`] knows it: this thread's own, or
/// the segment a growth moved to. `None` where the platform does not say.
pub fn stack_base() -> Option<usize> {
    let here = 0u8;
    let at = std::ptr::from_ref(&here) as usize;
    stacker::remaining_stack().map(|left| at.saturating_sub(left))
}

pub(crate) fn err_value_depth(span: Span, max: usize) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("recursion limit of {max} {NESTED_VALUES} exceeded"),
    )
    .primary(span, "this value nests too deeply to walk")
    .note("a value this deep is reachable only by iteration; compare its parts instead")
}

pub(crate) const NESTED_VALUES: &str = "nested values";

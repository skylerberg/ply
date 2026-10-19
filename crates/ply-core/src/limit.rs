//! The two bounds on a runaway program: how deep it may nest, and how much work it may do.

use crate::{Diagnostic, Span, codes};
use std::cell::Cell;
use std::ptr::NonNull;

/// The most nested calls a program may hold at once.
pub const DEFAULT_MAX_CALLS: usize = 10_000;

/// The calls one entry may make before it is refused more work; 0 is no bound. A tail call is a
/// loop, so a call is the unit of work that is already counted on every path.
pub const DEFAULT_STEP_BUDGET: i64 = 1_000_000_000;

pub const MAX_VALUE_DEPTH: usize = DEFAULT_MAX_CALLS;

/// A stack the runtime reserved whole and switched this thread onto: writable from its top down to
/// what it has committed so far, and never moved.
pub trait Reserved {
    /// The lowest writable address and the top.
    fn writable(&self) -> (usize, usize);

    /// Makes more of the reservation writable below what is; `false` once it is spent or the
    /// system will commit no more.
    fn extend(&self) -> bool;
}

thread_local! {
    static RUNNING: Cell<Option<NonNull<dyn Reserved>>> = const { Cell::new(None) };
}

/// The runtime stack this thread runs on, or `None` while it runs on its own.
pub fn running() -> Option<NonNull<dyn Reserved>> {
    RUNNING.with(Cell::get)
}

/// Names the stack this thread now runs on, as every switch between stacks does.
///
/// # Safety
/// `stack` stays alive for as long as it is the running one.
pub unsafe fn set_running(stack: Option<NonNull<dyn Reserved>>) {
    RUNNING.with(|r| r.set(stack));
}

fn with_running<R>(f: impl FnOnce(&dyn Reserved) -> R) -> Option<R> {
    let stack = running()?;
    // SAFETY: `set_running`'s caller keeps it alive while it is the running one.
    Some(f(unsafe { stack.as_ref() }))
}

/// The lowest writable address of the running stack, where `at` lies on it: `None` on the thread's
/// own stack and on a segment [`grow`] moved to.
pub fn running_low(at: usize) -> Option<usize> {
    with_running(|stack| {
        let (low, top) = stack.writable();
        (low..top).contains(&at).then_some(low)
    })
    .flatten()
}

/// Makes more of the running stack writable; `false` with none running or its reservation spent.
pub fn extend_running() -> bool {
    with_running(|stack| stack.extend()).unwrap_or(false)
}

/// Runs `f` where it has room to recurse: unoptimized native recursion exhausts a worker's stack
/// before either bound. A runtime stack is judged by its own bounds and made writable further down;
/// any other, or one whose reservation is spent, by stacker, which moves to a segment of its own.
pub fn grow<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    let here = 0u8;
    let at = std::ptr::from_ref(&here) as usize;
    match running_low(at) {
        Some(low) if at - low >= RED_ZONE || extend_running() => f(),
        Some(_) => stacker::grow(NEW_SEGMENT, f),
        None => stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, f),
    }
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

//! How deep a program may nest, and the stack it nests on.

use std::cell::Cell;
use std::ptr::NonNull;

/// The most nested calls a program may hold at once.
pub const DEFAULT_MAX_CALLS: usize = 10_000;

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
/// own stack.
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

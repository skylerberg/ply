//! Where a call's argument vector comes from, and where it goes back to.

use crate::value::Value;
use std::cell::RefCell;

/// Arities the free list serves, one class each.
pub const CLASSES: usize = 4;

/// Vectors kept per class per thread.
pub const KEEP: usize = 1024;

thread_local! {
    pub static FREE: RefCell<[Vec<Vec<Value>>; CLASSES]> =
        const { RefCell::new([const { Vec::new() }; CLASSES]) };
}

/// The free list index an `arity`-argument buffer belongs in, if any.
fn class_of(arity: usize) -> Option<usize> {
    (1..=CLASSES).contains(&arity).then(|| arity - 1)
}

/// A vector with room for `arity` arguments and nothing in it.
pub fn take(arity: usize) -> Vec<Value> {
    if let Some(class) = class_of(arity) {
        let recycled = FREE
            .try_with(|free| free.borrow_mut()[class].pop())
            .unwrap_or(None);
        if let Some(buffer) = recycled {
            debug_assert!(buffer.is_empty(), "the free list handed out a full buffer");
            debug_assert!(buffer.capacity() >= arity);
            return buffer;
        }
    }
    Vec::with_capacity(arity)
}

/// A pooled vector holding exactly `values`, for a builtin handing arguments to the function it
/// calls: the callee drains it and gives it back, so a loop's callback steps allocate nothing.
pub fn of<const N: usize>(values: [Value; N]) -> Vec<Value> {
    let mut out = take(N);
    out.extend(values);
    out
}

/// Takes back a vector the callee has finished with.
pub fn give(args: Vec<Value>) {
    if !args.is_empty() {
        return;
    }
    let Some(class) = class_of(args.capacity()) else {
        return;
    };
    let _ = FREE.try_with(|free| {
        let mut free = free.borrow_mut();
        if free[class].len() < KEEP {
            free[class].push(args);
        }
    });
}

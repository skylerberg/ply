//! Where a call's argument vector comes from, and where it goes back to.

use crate::value::Value;
use std::cell::RefCell;

/// Arities the free list serves, one class each.
pub const CLASSES: usize = 4;

/// Vectors kept per class per thread.
const KEEP: usize = 1024;

thread_local! {
    static FREE: RefCell<[Vec<Vec<Value>>; CLASSES]> =
        const { RefCell::new([const { Vec::new() }; CLASSES]) };
}

/// The free list index an `arity`-argument buffer belongs in, if any.
fn class_of(arity: usize) -> Option<usize> {
    (1..=CLASSES).contains(&arity).then(|| arity - 1)
}

/// A vector with room for `arity` arguments and nothing in it.
pub(crate) fn take(arity: usize) -> Vec<Value> {
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

/// Takes back a vector the callee has finished with.
pub(crate) fn give(args: Vec<Value>) {
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

/// Empties every class, so a test that counts hits is not reading whatever the test before it left
/// behind.
#[cfg(test)]
pub(crate) fn drain_the_free_list() {
    let _ = FREE.try_with(|free| {
        for class in free.borrow_mut().iter_mut() {
            class.clear();
        }
    });
}

/// Buffers currently parked in every class.
#[cfg(test)]
pub(crate) fn kept() -> [usize; CLASSES] {
    FREE.try_with(|free| {
        let free = free.borrow();
        std::array::from_fn(|class| free[class].len())
    })
    .unwrap_or([0; CLASSES])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_vector_taken_for_an_arity_has_room_for_it_and_nothing_in_it() {
        for arity in 0..=10 {
            let v = take(arity);
            assert!(v.is_empty(), "take({arity}) handed out {} values", v.len());
            assert!(
                v.capacity() >= arity,
                "take({arity}) has room for {}",
                v.capacity()
            );
        }
    }

    #[test]
    fn a_vector_given_back_full_does_not_come_out_of_take_full() {
        give(vec![Value::str("left over"), Value::Int(7)]);
        let v = take(2);
        assert!(v.is_empty(), "take handed out a vector holding {}", v.len());
    }

    /// ADR 0015 §2: a credential may not sit in a buffer the next call reads from.
    #[test]
    fn a_secret_handed_back_is_not_held_by_the_pool() {
        let payload = Arc::new(Value::str("hunter2"));
        give(vec![Value::Secret(Arc::clone(&payload))]);
        assert_eq!(
            Arc::strong_count(&payload),
            1,
            "a credential is still reachable after the call that carried it returned"
        );
    }

    /// The point of the list, stated as the assertion `link_reuse.rs`'s
    /// `a_warm_frame_push_allocates_nothing` makes for the control stack: once a buffer of a class
    /// has been handed back, the next call of that arity does not reach the allocator.
    #[test]
    fn a_warm_call_of_a_pooled_arity_reuses_the_buffer_it_gave_back() {
        drain_the_free_list();
        for arity in 1..=CLASSES {
            let mut first = take(arity);
            let address = first.as_ptr();
            first.push(Value::Int(1));
            first.clear();
            give(first);
            let second = take(arity);
            assert_eq!(
                second.as_ptr(),
                address,
                "arity {arity} allocated a second buffer instead of reusing the one given back"
            );
            give(second);
        }
    }

    /// An arity outside the four classes is the allocator's, and it must not come back out of a
    /// class it was never in.
    #[test]
    fn an_arity_the_list_does_not_serve_is_left_to_the_allocator() {
        drain_the_free_list();
        let wide = take(CLASSES + 1);
        assert_eq!(wide.capacity(), CLASSES + 1);
        give(wide);
        let _ = FREE.try_with(|free| {
            for (i, class) in free.borrow().iter().enumerate() {
                assert!(
                    class.is_empty(),
                    "a {}-argument buffer was kept in the arity-{} class",
                    CLASSES + 1,
                    i + 1
                );
            }
        });
    }

    /// The list is bounded, so its memory cost is a number rather than a hope.
    #[test]
    fn the_pools_upper_bound_is_stated_in_bytes() {
        drain_the_free_list();
        let width = size_of::<Value>();
        let buffers: usize = (1..=CLASSES).map(|arity| KEEP * arity * width).sum();
        println!(
            "the free list keeps at most {KEEP} buffers in each of {CLASSES} classes: \
             {buffers} bytes of argument storage per thread at {width}B per Value"
        );

        let deep: Vec<Vec<Value>> = (0..KEEP + 8).map(|_| take(1)).collect();
        for buffer in deep {
            give(buffer);
        }
        let kept = FREE.with(|free| free.borrow()[0].len());
        assert_eq!(
            kept, KEEP,
            "the arity-1 class kept {kept} buffers against a bound of {KEEP}"
        );
        drain_the_free_list();
    }
}

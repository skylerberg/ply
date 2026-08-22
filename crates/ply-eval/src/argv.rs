//! Where a call's argument vector comes from, and where it goes back to.
//!
//! The single largest line in a request's allocation profile. Measured on
//! `/health` over SimNet by
//! `cargo test -p ply-corpus --release --test r4_value_construction --
//! --nocapture`, fitted over the 20- and 200-request windows: **372.4 argument
//! vectors per request, 40.9% of the request's 911.5 allocations**. A vector is
//! filled by [`Frame::AppArgs`](crate::cont::Frame) and handed to the callee,
//! and the steady state was a `malloc`/`free` pair per application.
//!
//! That is [`crate::pool`]'s situation with a `Vec` in place of an `Rc` link,
//! and the same answer applies: the vector a callee has finished with goes on a
//! thread-local free list instead of back to the allocator. Arity 1–4 is 349.4
//! of the 372.4, so four size classes — 32, 64, 96 and 128 bytes — is the whole
//! of what the list holds.
//!
//! # What it moved, and what the ADR got wrong about why
//!
//! Taken with the same command on the same tree, with only [`take`] and
//! [`give`] swapped between the allocator and the list: **372.4 → 194.4
//! argument vectors per request, and 911.5 → 733.5 allocations per request —
//! 178.0 removed, 19.5% of the request.** ADR 0019 §1 predicted 341.4, on the
//! reasoning that every vector not retained as `Ctor.args` "are freed by
//! `enter_code`". **They are not, and that sentence is the error.** A builtin
//! callee does not reach [`Machine::enter_code`] at all — `enter_closure`'s
//! `ClosureKind::Builtin` arm goes to `Machine::call_builtin`, and
//! [`crate::builtins::call`] takes its `Vec<Value>` **by value** and consumes
//! it, so that buffer is freed by the allocator and never offered here.
//!
//! Measured rather than reasoned, as a paired experiment on one loop
//! (`r4_value_construction`'s micro-program, under
//! `a_warm_ply_call_takes_its_argument_vector_from_the_free_list`, which is the
//! re-armed `a_call_allocates_one_argument_vector_of_32_bytes_per_argument`):
//!
//! | one call added to the loop body | without the list | with it |
//! | --- | --- | --- |
//! | `r4_call1` — a 1-argument **Ply** call | +1.00 of 32 B per iteration | **+0.00** |
//! | `r4_str` — a 1-argument **builtin** call | +1.00 of 32 B per iteration | **+1.00 of 32 B** |
//!
//! So the 194.4 that remain are: at most 31.0 retained as `Ctor.args` (the
//! `ClosureKind::Ctor` arm of `enter_closure` is the only path that keeps a
//! buffer), 23.0 at arity 5–10 for which there is no class, and the remaining
//! **≥140.4 — 15.4% of the original request — freed somewhere other than
//! `enter_code`, of which the builtin path above is the identified bulk.**
//! Recovering those means changing what `builtins::call` takes, which is a
//! different change to a different signature and is not claimed here.
//!
//! # The class is read off the capacity, not off the arity
//!
//! [`give`] is reached from [`Machine::enter_code`] with whatever vector the
//! callee was handed, and that is not always one [`take`] built: the zero-arity
//! path hands over a `Vec::new()`. A capacity outside the four classes is
//! therefore an ordinary `drop`, and the class a vector goes back to is the
//! class it will be handed out for — so a recycled vector's capacity is exactly
//! its arity and never drifts upward through reuse.
//!
//! # What a pool here may not do
//!
//! - **Hold a `Value` alive.** A vector kept with its contents is a second
//!   owner of every argument in it: a `Cell` outlives the region that would
//!   have reclaimed it, `Arc::get_mut` in [`crate::value`]'s dismantler stops
//!   seeing a unique owner, and — the one that is not a performance question —
//!   a [`Value::Secret`] sits in a buffer the next call reads from. ADR 0015 §2
//!   makes that a correctness bound rather than a preference, and
//!   [`tests::a_secret_handed_back_is_not_held_by_the_pool`] is what checks it.
//!   [`give`] refuses a non-empty vector outright rather than emptying it: a
//!   caller that still holds an argument has not finished with it, and dropping
//!   its values here on its behalf would hide that rather than report it.
//! - **Hand out a vector that is not empty.** A callee pushes; a non-empty
//!   vector shifts every argument by whatever was left in it.
//! - **Cross a thread.** A `Value` is thread-confined (see [`crate::value`]'s
//!   note on `RcK`), so the list is thread-local and every access is
//!   `try_with`: a vector released during thread-local teardown must fall back
//!   to the allocator rather than abort a worker.
//!
//! Nothing observable moves either way — no Ply expression can read an address
//! — so no stored type's encoding and no rendered byte move with it, and
//! `ply_store::{FRONTEND_VERSION, RUNTIME_VERSION}` are untouched.
//! `--engine both` is what checks that claim rather than this paragraph.

use crate::value::Value;
use std::cell::RefCell;

/// Arities the free list serves, one class each.
///
/// Four because arity 1–4 is 349.4 of the 372.4 argument vectors a `/health`
/// request builds, and the fifth class would be worth 8.0. Re-exported as
/// `ply_eval::ARGUMENT_VECTOR_CLASSES` so that
/// `r4_value_construction::the_argument_vectors_the_free_list_does_not_take_are_the_ones_no_callee_gives_back`
/// splits its histogram at the number this file uses rather than at a copy of
/// it.
pub const CLASSES: usize = 4;

/// Vectors kept per class per thread.
///
/// [`crate::pool::KEEP`]'s bound and [`crate::pool::KEEP`]'s reason: the list
/// serves the steady state, not a deep recursion's peak, so a run that nested
/// a hundred thousand calls and returned gives that memory back. The whole list
/// is at most `KEEP * (32 + 64 + 96 + 128)` bytes of buffer per thread, which
/// [`tests::the_pools_upper_bound_is_stated_in_bytes`] states as a number.
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
///
/// The caller must have moved every argument out of it first. A vector still
/// holding one is not a candidate — see the module note — and is dropped here
/// exactly as it was before this list existed.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Empties every class, so a test that counts hits is not reading whatever
    /// the test before it left behind. Test order within a binary is not fixed.
    fn drain_the_free_list() {
        let _ = FREE.try_with(|free| {
            for class in free.borrow_mut().iter_mut() {
                class.clear();
            }
        });
    }

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

    /// ADR 0015 §2: a credential may not sit in a buffer the next call reads
    /// from. The count is taken on the `Arc` the `Secret` wraps, so this fails
    /// for a pool that keeps a vector without emptying it, whatever it does
    /// with the vector afterwards.
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
    /// `a_warm_frame_push_allocates_nothing` makes for the control stack: once a
    /// buffer of a class has been handed back, the next call of that arity does
    /// not reach the allocator. Read off the pointer rather than off a counting
    /// allocator, because this module is inside `ply-eval`'s lib test binary and
    /// a `#[global_allocator]` is a whole-binary decision.
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

    /// An arity outside the four classes is the allocator's, and it must not
    /// come back out of a class it was never in.
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
    /// A deep recursion that unwinds hands back one buffer per frame, and past
    /// the bound those are freed as they were before.
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

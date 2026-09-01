//! What a step costs the allocator, exactly.

use crate::counting::charge;
use ply_eval::cont::{Frame, Prompt, Segment};
use ply_eval::Stack;
use ply_span::Span;
use std::hint::black_box;
use std::rc::Rc;

fn allocations_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let (out, allocs, _) = charge(f);
    (out, allocs)
}

fn call_frame() -> Frame {
    Frame::Call {
        name: None,
        call_site: Span::DUMMY,
        memo: false,
        callee_window: 0,
        caller_window: 0,
    }
}

/// One allocation to push a frame — the link that holds it — and none at all to pop one.
#[test]
fn pushing_one_frame_costs_one_allocation_and_popping_costs_none() {
    let mut stack = Stack::new();
    // Warm the allocator so the first iteration is not measuring a fresh arena.
    for _ in 0..64 {
        stack = stack.push(call_frame());
    }

    const PUSHES: usize = 1_000;
    let (grown, allocs) = allocations_of(|| {
        let mut s = stack.clone();
        for _ in 0..PUSHES {
            s = s.push(call_frame());
        }
        s
    });
    let per_push = allocs as f64 / PUSHES as f64;

    // `into_next` rather than `next`, because that is what the machine's return transition uses: it
    // owns its stack, so the frame is moved out of its link rather than cloned.
    let (_, pop_allocs) = allocations_of(|| {
        let mut s = grown.clone();
        for _ in 0..PUSHES {
            match s.into_next() {
                ply_eval::Next::Frame(_, rest) => s = rest,
                other => {
                    black_box(&other);
                    return;
                }
            }
        }
        black_box(s);
    });
    let per_pop = pop_allocs as f64 / PUSHES as f64;

    println!(
        "Frame is {} bytes, Segment {} bytes; push = {per_push} allocations, pop = {per_pop}",
        std::mem::size_of::<Frame>(),
        std::mem::size_of::<Segment>(),
    );

    assert!(
        per_push <= 1.0,
        "a frame push cost {per_push} allocations, above the one link holding it"
    );
    assert_eq!(
        per_pop, 0.0,
        "a frame pop cost {per_pop} allocations, and popping an unshared link allocates nothing"
    );
}

/// Opening a prompt is the operation that genuinely needs a new segment, and a frame push must
/// never cost more than it: a `handle` is rare and a push happens on all but a handful of
/// transitions.
#[test]
fn pushing_a_frame_costs_no_more_than_opening_a_prompt() {
    let prompt = Rc::new(Prompt {
        clauses: Rc::new(Vec::new()),
        effects: Rc::new(Vec::new()),
        ret: None,
        clause_captures: Vec::new(),
        ret_captures: Rc::from(Vec::new()),
        module: 0,
        span: Span::DUMMY,
    });

    let mut stack = Stack::new();
    for _ in 0..64 {
        stack = stack.push(call_frame());
    }

    const N: usize = 1_000;
    let (_, prompts) = allocations_of(|| {
        let mut s = stack.clone();
        for _ in 0..N {
            s = s.push_prompt(prompt.clone(), 0);
        }
        s
    });
    let (_, frames) = allocations_of(|| {
        let mut s = stack.clone();
        for _ in 0..N {
            s = s.push(call_frame());
        }
        s
    });

    println!(
        "per operation: push_prompt = {}, push = {}",
        prompts as f64 / N as f64,
        frames as f64 / N as f64
    );
    assert!(prompts > 0);
    assert!(
        frames <= prompts,
        "pushing a frame allocated {frames} against {prompts} for opening a prompt, so the frequent operation is again the dearer one"
    );
}

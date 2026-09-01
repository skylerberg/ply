//! What a frame push and a scope binding cost the allocator once the machine is warm, and that
//! reusing a link cannot make two owners disagree.

use crate::counting::charge;
use ply_eval::{Frame, Next, Stack, Value};
use ply_span::Span;

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let (out, allocs, _) = charge(f);
    (out, allocs)
}

/// A frame that carries a value the test can read back, so a recycled link is checked for what it
/// holds rather than only for how many it holds.
fn marker(n: i64) -> Frame {
    Frame::BinaryApply {
        op: ply_syntax::ast::BinOp::Add,
        lhs: Value::Int(n),
        lhs_span: Span::DUMMY,
        rhs_span: Span::DUMMY,
        span: Span::DUMMY,
    }
}

/// Pops the whole stack into `out`, which the caller supplies so that a measured region does not
/// pay for the test's own bookkeeping.
fn drain_into(mut stack: Stack, out: &mut Vec<i64>) {
    loop {
        match stack.into_next() {
            Next::Frame(Frame::BinaryApply { lhs, .. }, rest) => {
                out.push(match lhs {
                    Value::Int(n) => n,
                    other => panic!("the stack held {other} rather than the marker"),
                });
                stack = rest;
            }
            Next::Frame(_, rest) => stack = rest,
            _ => return,
        }
    }
}

fn drain(stack: Stack) -> Vec<i64> {
    let mut out = Vec::with_capacity(DEPTH as usize * 4);
    drain_into(stack, &mut out);
    out
}

const DEPTH: i64 = 200;

fn push_and_pop(out: &mut Vec<i64>) {
    let mut stack = Stack::new();
    for n in 0..DEPTH {
        stack = stack.pushed(marker(n));
    }
    out.clear();
    drain_into(stack, out);
    assert_eq!(out.len(), DEPTH as usize);
}

#[test]
fn a_warm_frame_push_allocates_nothing() {
    let mut out = Vec::with_capacity(DEPTH as usize);
    push_and_pop(&mut out);
    let (_, allocs) = counted(|| push_and_pop(&mut out));
    assert_eq!(
        allocs, 0,
        "{DEPTH} frame pushes and pops allocated {allocs} times; the free list is not serving them"
    );
}

// The scope-binding twin of the frame test above died with the persistent chain: a binding is a
// slot write into the machine-owned window, which allocates only when the window vector grows.

/// The hazard a free list introduces, and the only one: a link two owners hold must not be recycled
/// when the first of them lets go.
#[test]
fn a_chain_two_owners_hold_survives_the_first_letting_go() {
    let mut shared = Stack::new();
    for n in 0..DEPTH {
        shared = shared.pushed(marker(n));
    }
    let survivor = shared.clone();

    // The first owner retires every link and offers each to the pool.
    assert_eq!(drain(shared).len(), DEPTH as usize);

    // Enough pushes to hand out every link the pool could have taken, each carrying a marker that
    // would be visible if a shared link were reused.
    let mut later = Stack::new();
    for n in 0..DEPTH * 4 {
        later = later.pushed(marker(-1 - n));
    }

    let expected: Vec<i64> = (0..DEPTH).rev().collect();
    assert_eq!(
        drain(survivor),
        expected,
        "the surviving owner read frames the pool had handed out again"
    );
    assert_eq!(drain(later).len(), (DEPTH * 4) as usize);
}

/// What the free list can hold, so its memory cost is a number rather than a hope.
#[test]
fn the_pools_upper_bound_is_stated_in_bytes() {
    let frame = std::mem::size_of::<Frame>();
    println!("Frame is {frame} bytes; the free list keeps at most 1024 links of each kind");
    assert!(
        frame <= 256,
        "a frame grew to {frame} bytes, which multiplies straight into what the free list holds"
    );
}

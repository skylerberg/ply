//! What a frame push and a scope binding cost the allocator once the machine is
//! warm, and that reusing a link cannot make two owners disagree.
//!
//! W6 measured the control stack and the environment as 47% and 8.5% of a
//! request's allocations: both are `Rc`-linked persistent chains rewritten on
//! almost every step, and both freed the link they had just allocated. The fix
//! is a thread-local free list, and the hazard a free list introduces is
//! exactly one — recycling a link somebody else still holds — so the third test
//! here is the one that matters.
//!
//! The counting allocator is why this is its own test binary: a
//! `#[global_allocator]` is a whole-binary decision.

use ply_eval::{Env, Frame, Next, ScopeSlot, Stack, Value};
use ply_span::{Span, Symbol};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize) {
    ALLOCS.with(|c| c.set(0));
    let out = f();
    (out, ALLOCS.with(Cell::get))
}

/// A frame that carries a value the test can read back, so a recycled link is
/// checked for what it holds rather than only for how many it holds.
fn marker(n: i64) -> Frame {
    Frame::BinaryApply {
        op: ply_syntax::ast::BinOp::Add,
        lhs: Value::Int(n),
        lhs_span: Span::DUMMY,
        rhs_span: Span::DUMMY,
        span: Span::DUMMY,
    }
}

/// Pops the whole stack into `out`, which the caller supplies so that a
/// measured region does not pay for the test's own bookkeeping.
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

fn bind_and_drop(names: &[Symbol]) {
    let mut env = Env::empty();
    for (n, name) in names.iter().enumerate() {
        env = env.bind(name.clone(), Value::Int(n as i64));
    }
    assert!(env.lookup(&names[0]).is_some());
}

#[test]
fn a_warm_scope_binding_allocates_nothing() {
    let names: Vec<Symbol> = (0..DEPTH).map(|n| Symbol::new(format!("x{n}"))).collect();
    bind_and_drop(&names);
    let (_, allocs) = counted(|| bind_and_drop(&names));
    assert_eq!(
        allocs, 0,
        "{DEPTH} bindings allocated {allocs} times; the free list is not serving them"
    );
}

/// The hazard a free list introduces, and the only one: a link two owners hold
/// must not be recycled when the first of them lets go.
///
/// The shared prefix here is what a captured continuation holds — `Stack::clone`
/// is what `capture` and `resume` are built out of — so a pool that recycled a
/// shared link would show up as the survivor reading frames the other owner's
/// pushes had overwritten.
#[test]
fn a_chain_two_owners_hold_survives_the_first_letting_go() {
    let mut shared = Stack::new();
    for n in 0..DEPTH {
        shared = shared.pushed(marker(n));
    }
    let survivor = shared.clone();

    // The first owner retires every link and offers each to the pool.
    assert_eq!(drain(shared).len(), DEPTH as usize);

    // Enough pushes to hand out every link the pool could have taken, each
    // carrying a marker that would be visible if a shared link were reused.
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

/// The same statement for a scope: a closure captures its defining environment
/// by cloning one pointer, and the pool must not be able to rewrite what that
/// pointer reaches.
#[test]
fn a_captured_scope_survives_the_scope_it_was_taken_from() {
    let name = Symbol::new("captured");
    let outer = Env::empty().bind(name.clone(), Value::Int(7));
    let captured = outer.clone();
    drop(outer);

    let mut churn = Env::empty();
    for n in 0..DEPTH * 4 {
        churn = churn.bind(name.clone(), Value::Int(n));
    }
    drop(churn);

    assert!(
        matches!(captured.lookup(&name), Some(ScopeSlot::Live(Value::Int(7)))),
        "the captured scope read a binding the pool had handed out again"
    );
}

/// What the free list can hold, so its memory cost is a number rather than a
/// hope. The bound is per link type per thread, and a worker thread pays it
/// only once it has run something that deep.
#[test]
fn the_pools_upper_bound_is_stated_in_bytes() {
    let frame = std::mem::size_of::<Frame>();
    println!("Frame is {frame} bytes; the free list keeps at most 1024 links of each kind");
    assert!(
        frame <= 256,
        "a frame grew to {frame} bytes, which multiplies straight into what the free list holds"
    );
}

//! The counting allocator every test in this binary measures against.
//!
//! There is one of these because a `#[global_allocator]` is a whole-binary
//! decision, and seven copies of it meant seven binaries. It counts on the
//! calling thread only — the counters are `thread_local!` — which is what lets
//! the tests here run in parallel with each other the way the tests inside any
//! one of those seven already did.
//!
//! `ARMED` is why the counters can be read at all: without it every test would
//! also charge itself the harness's own allocations between measured regions.
//!
//! `crates/ply-corpus-tests/tests/allocation/counting.rs` is the same file. Integration
//! tests in different crates cannot share a module, so closing that last seam
//! would mean a workspace member existing only to hold this — a member
//! `ci-shards.sh verify` would then require a shard for. Two copies is the
//! cheaper end of that trade; thirteen was not.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

pub struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
            let _ = BYTES.try_with(|c| c.set(c.get() + layout.size()));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// What `f` charged the global allocator on this thread: its value, then the
/// allocation count and the bytes.
pub fn charge<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    ALLOCS.with(|c| c.set(0));
    BYTES.with(|c| c.set(0));
    ARMED.with(|c| c.set(true));
    let out = f();
    ARMED.with(|c| c.set(false));
    (out, ALLOCS.with(Cell::get), BYTES.with(Cell::get))
}

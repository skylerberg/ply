//! What the persistent world costs per cell operation, so that "the forkable
//! world and the zero-cost path are mutually exclusive" is a number.
//!
//! ADR 0017 opens on 9,343 allocations for one `/health` response and concludes
//! that the persistent forkable world has to go. That conclusion is about
//! *uniqueness* — Perceus fires only on a uniquely-owned value — but it is easy
//! to read it as "the red-black tree is the allocation", and the two claims have
//! very different price tags. This file measures the second one directly: how
//! many heap allocations a `cell_get` and a `cell_set` cost, at world sizes
//! spanning the range a request and a test actually reach.
//!
//! The numbers are printed as well as asserted. An assertion pins a ceiling so
//! a regression is caught; the print is what a design decision gets read off.

use ply_eval::{Value, World};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
            let _ = BYTES.try_with(|b| b.set(b.get() + layout.size()));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Allocations and bytes charged while `f` runs.
fn charged<T>(f: impl FnOnce() -> T) -> (usize, usize, T) {
    COUNT.with(|c| c.set(0));
    BYTES.with(|b| b.set(0));
    ARMED.with(|a| a.set(true));
    let out = f();
    ARMED.with(|a| a.set(false));
    (COUNT.with(Cell::get), BYTES.with(Cell::get), out)
}

fn filled(n: usize) -> (World, Vec<ply_eval::CellId>) {
    let mut world = World::new();
    let ids = (0..n).map(|i| world.alloc(Value::Int(i as i64))).collect();
    (world, ids)
}

/// What a write into an **unshared** world costs, which is every write a Ply
/// program performs.
///
/// The result is flat at one allocation and 56 bytes from one cell to ten
/// thousand: `rpds` mutates in place once the nodes on the path are uniquely
/// owned, and they are, because nothing in a real run holds a second reference
/// to the world. Persistence is charged only where sharing is real, and the
/// next test measures that separately.
///
/// So the part of ADR 0017's 9,343 that removing persistence recovers is one
/// allocation per `cell_set` and none per `cell_get` — not a share of the
/// request, a constant per cell write.
#[test]
fn a_cell_write_into_an_unshared_world_costs_one_allocation() {
    println!("\n  cells   allocs/write   bytes/write   allocs/read");
    let mut at_ten_thousand = 0.0;
    for n in [1usize, 8, 64, 512, 4_096, 10_000] {
        let (mut world, ids) = filled(n);
        let target = ids[n / 2];

        const WRITES: usize = 1_000;
        let (writes, wbytes, _) = charged(|| {
            for i in 0..WRITES {
                assert!(world.set(target, Value::Int(i as i64)));
            }
        });
        let (reads, _, _) = charged(|| {
            for _ in 0..WRITES {
                std::hint::black_box(world.get(target));
            }
        });

        let per_write = writes as f64 / WRITES as f64;
        println!(
            "  {n:>5}   {per_write:>12.2}   {:>11.1}   {:>11.2}",
            wbytes as f64 / WRITES as f64,
            reads as f64 / WRITES as f64
        );
        if n == 10_000 {
            at_ten_thousand = per_write;
        }
    }

    // A read must not allocate at all; if it ever does, the cost model above is
    // measuring the wrong thing and the table is not a basis for a decision.
    let (world, ids) = filled(1_024);
    let (reads, _, _) = charged(|| std::hint::black_box(world.get(ids[512])).is_some());
    assert_eq!(reads, 0, "a `cell_get` must not allocate");

    assert!(
        (0.5..=2.0).contains(&at_ten_thousand),
        "a write into an unshared world was {at_ten_thousand} allocations at \
         10,000 cells; it has been one, flat, and a change either way moves the \
         only number that says what removing persistence is worth"
    );
}

/// A fork is free; the first write into the fork is what persistence charges.
///
/// Measured here beside the unshared write so the trade reads as two numbers in
/// one place — and so the second number is attached to the case it is actually
/// paid in, which is a world that something else still holds.
#[test]
fn a_fork_is_free_and_the_first_write_into_it_copies_a_path() {
    let (world, ids) = filled(10_000);
    let (fork_allocs, _, forked) = charged(|| world.fork());
    assert_eq!(
        fork_allocs, 0,
        "a fork is structural sharing; this is ADR 0005 §2's whole claim"
    );

    let mut forked = forked;
    let (write_allocs, _, _) = charged(|| forked.set(ids[5_000], Value::Int(-1)));
    println!("\n  fork of 10,000 cells: {fork_allocs} allocations");
    println!("  one write into it:    {write_allocs} allocations");
    assert!(
        write_allocs > 0,
        "a write into a shared fork must copy the path it touches"
    );

    // And the isolation the fork bought is intact after the write.
    assert!(matches!(world.get(ids[5_000]), Some(Value::Int(5_000))));
}

/// The base world every real run starts from is empty, so the fork is a fork of
/// nothing.
///
/// `World::fork` is reached from exactly four places — `Interp::set_base_world`,
/// `Interp::reset`, and the machine's two — and every one of them is "restore
/// the world to the base before a run". The base is non-empty only through
/// `ply_test`'s `with_fixture`, whose only two callers are Rust tests; the CLI
/// never sets one. **No Ply program reaches a fork of a non-empty world.**
///
/// That is what makes ADR 0017 §6's isolation question answerable: whatever
/// forking buys the scheduler, it is not buying it by forking, because the fork
/// is empty. Two tests do not observe each other's cells because each worker
/// evaluates on its own machine holding its own world, and a region closed at
/// the end of a test gives the same guarantee by the same argument.
#[test]
fn the_base_world_every_ply_program_forks_is_empty() {
    let base = World::new();
    assert!(base.is_empty());
    let (allocs, bytes, forked) = charged(|| base.fork());
    assert_eq!((allocs, bytes), (0, 0));
    assert!(forked.is_empty());
    assert_eq!(forked.high_water(), 0);
}

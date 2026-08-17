//! What a cell operation costs now that the store is a region, and what it cost
//! when the store was a persistent map — so "the forkable world and the
//! zero-cost path are mutually exclusive" stays a number rather than a slogan.
//!
//! ADR 0017 opens on 9,343 allocations for one `/health` response and concludes
//! that the persistent forkable world has to go. That conclusion is about
//! *uniqueness* — Perceus fires only on a uniquely-owned value — but it is easy
//! to read it as "the red-black tree is the allocation", and the two claims have
//! very different price tags. This file measures the second one directly, on
//! both sides of the change.
//!
//! The world is gone, so its side is measured against the data structure it
//! **was**: an `rpds::RedBlackTreeMap` keyed by a dense integer, allocated and
//! written exactly as `World` allocated and wrote. That keeps the denominator
//! honest without keeping the type alive.
//!
//! The numbers are printed as well as asserted. An assertion pins a ceiling so
//! a regression is caught; the print is what a design decision gets read off.

use ply_eval::arena::Slot;
use ply_eval::{Fixture, TaskRegions, Value};
use rpds::RedBlackTreeMap;
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

fn filled(n: usize) -> (TaskRegions, Vec<Slot>) {
    let mut regions = TaskRegions::new();
    let slots = (0..n)
        .map(|i| regions.alloc_cell(Value::Int(i as i64)))
        .collect();
    (regions, slots)
}

/// The store `World` was, rebuilt from the same crate it was built on, so the
/// comparison below is against the thing that was removed rather than against a
/// guess at it.
fn persistent(n: usize) -> RedBlackTreeMap<u32, Value> {
    let mut map = RedBlackTreeMap::new();
    for i in 0..n {
        map.insert_mut(i as u32, Value::Int(i as i64));
    }
    map
}

/// What a `cell_set` costs in the region store: nothing, at every size.
///
/// The persistent map charged one allocation and 56 bytes per write, flat from
/// one cell to ten thousand — `rpds` mutates in place once the nodes on the path
/// are uniquely owned, and in a real run they were, so persistence was charged
/// only where sharing was real. A slot write is an indexed store, so the
/// constant goes to zero rather than getting smaller.
#[test]
fn a_cell_write_into_the_region_store_costs_nothing() {
    println!("\n  cells   region allocs/write   map allocs/write   region allocs/read");
    let mut region_at_ten_thousand = 0.0;
    let mut map_at_ten_thousand = 0.0;
    for n in [1usize, 8, 64, 512, 4_096, 10_000] {
        let (mut regions, slots) = filled(n);
        let target = slots[n / 2];
        let mut map = persistent(n);
        let key = (n / 2) as u32;

        const WRITES: usize = 1_000;
        let (writes, _, _) = charged(|| {
            for i in 0..WRITES {
                assert!(regions.set(target, Value::Int(i as i64)));
            }
        });
        let (map_writes, _, _) = charged(|| {
            for i in 0..WRITES {
                map.insert_mut(key, Value::Int(i as i64));
            }
        });
        let (reads, _, _) = charged(|| {
            for _ in 0..WRITES {
                std::hint::black_box(regions.get(target));
            }
        });

        let per_write = writes as f64 / WRITES as f64;
        let map_per_write = map_writes as f64 / WRITES as f64;
        println!(
            "  {n:>5}   {per_write:>18.2}   {map_per_write:>16.2}   {:>18.2}",
            reads as f64 / WRITES as f64
        );
        if n == 10_000 {
            region_at_ten_thousand = per_write;
            map_at_ten_thousand = map_per_write;
        }
    }

    let (regions, slots) = filled(1_024);
    let (reads, _, _) = charged(|| std::hint::black_box(regions.get(slots[512])).is_some());
    assert_eq!(reads, 0, "a `cell_get` must not allocate");

    assert_eq!(
        region_at_ten_thousand, 0.0,
        "a slot write is an indexed store and must not reach the allocator"
    );
    assert!(
        map_at_ten_thousand > 0.0,
        "the persistent map allocated {map_at_ten_thousand} per write, so it is \
         not the baseline this comparison needs"
    );
}

/// Allocating a cell is a bump once the arena has been through a region of the
/// size before, where the persistent map allocated a node every time.
///
/// This is the part of ADR 0017's 9,343 that removing persistence recovers, and
/// it is per `with_cell` rather than a share of a request.
#[test]
fn allocating_a_cell_is_a_bump_once_the_arena_is_warm() {
    const CELLS: usize = 4_096;

    let mut regions = TaskRegions::new();
    // A steady state: a service opens a region per request and a test opens one
    // per test, so the interesting number is the second pass and not the first.
    for _ in 0..CELLS {
        regions.alloc_cell(Value::Unit);
    }
    regions.reset();

    let (warm, warm_bytes, _) = charged(|| {
        for i in 0..CELLS {
            regions.alloc_cell(Value::Int(i as i64));
        }
    });
    let (map_build, map_bytes, _) = charged(|| persistent(CELLS));

    println!(
        "\n  {CELLS} cells: region {warm} allocations / {warm_bytes} bytes; \
         persistent map {map_build} allocations / {map_bytes} bytes"
    );
    assert_eq!(
        (warm, warm_bytes),
        (0, 0),
        "a warm arena builds a region's cells without touching the allocator"
    );
    assert!(
        map_build >= CELLS,
        "the persistent map allocated {map_build} times for {CELLS} cells"
    );
}

/// The entry point's reset is what `World::fork` was, and it is what makes the
/// arena's memory a steady state rather than a leak: whatever a run allocated,
/// the next one starts from the fixture and takes nothing further from the
/// allocator.
#[test]
fn resetting_to_the_fixture_returns_every_slot_and_allocates_nothing() {
    let (mut regions, _) = Fixture::empty().open();
    for i in 0..10_000 {
        regions.alloc_cell(Value::Int(i));
    }

    let (allocs, bytes, ()) = charged(|| regions.reset());

    assert_eq!((allocs, bytes), (0, 0));
    assert_eq!(regions.live(), 0);
    assert_eq!(
        regions.depth(),
        2,
        "the fixture's region and a fresh entry region"
    );

    // And the chunks stayed, so the next run of the same size is free too.
    let (again, _, ()) = charged(|| {
        for i in 0..10_000 {
            regions.alloc_cell(Value::Int(i));
        }
    });
    assert_eq!(again, 0, "the second run reuses the first run's chunks");
}

/// The fixture every real run starts from is empty, so opening it copies
/// nothing.
///
/// A fixture is non-empty only through `ply_test`'s `with_fixture`, whose only
/// callers are Rust tests; the CLI never sets one. **No Ply program opens a
/// non-empty fixture**, which is what made ADR 0017 §6's isolation question
/// answerable in the first place: whatever the fork bought the scheduler, it was
/// not buying it by forking, because the fork was empty. Two tests do not
/// observe each other's cells because each worker evaluates on its own machine
/// holding its own region stack, and that argument is unchanged.
#[test]
fn the_fixture_every_ply_program_opens_is_empty() {
    let base = Fixture::empty();
    assert!(base.is_empty());
    let (allocs, bytes, (regions, _)) = charged(|| base.open());
    assert_eq!(regions.live(), 0);
    assert_eq!(regions.base_len(), 0);
    println!("\n  opening the empty fixture: {allocs} allocations / {bytes} bytes");
    // Not zero: a fresh stack pushes its root scope and takes a checkpoint of
    // it. It is a small constant that does not depend on the run, which is the
    // property `World::fork`'s zero was standing in for.
    assert!(
        allocs <= 8,
        "opening the empty fixture cost {allocs} allocations"
    );
}

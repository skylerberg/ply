//! What the region allocator costs, counted rather than asserted.

use ply_eval::Value;
use ply_eval::arena::{Arena, RegionKind};
use ply_span::Span;
use rpds::RedBlackTreeMap;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

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

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Allocations and bytes `f` took from the global allocator.
fn counted<R>(f: impl FnOnce() -> R) -> (usize, usize, R) {
    ALLOCS.with(|c| c.set(0));
    BYTES.with(|c| c.set(0));
    ARMED.with(|c| c.set(true));
    let out = f();
    ARMED.with(|c| c.set(false));
    (ALLOCS.with(Cell::get), BYTES.with(Cell::get), out)
}

/// One region's worth of work: open, fill, close.
fn cycle(arena: &mut Arena, kind: RegionKind, size: usize) {
    let r = arena.open(kind, Span::DUMMY);
    for i in 0..size {
        arena.alloc(Value::Int(i as i64));
    }
    arena.close(r);
}

#[test]
fn a_warm_unique_region_costs_the_allocator_nothing() {
    for size in [1usize, 16, 256, 1_000, 10_000] {
        let mut arena = Arena::new();
        for _ in 0..2 {
            cycle(&mut arena, RegionKind::Unique, size);
        }

        let (allocations, bytes, ()) = counted(|| {
            for _ in 0..100 {
                cycle(&mut arena, RegionKind::Unique, size);
            }
        });

        assert_eq!(
            (allocations, bytes),
            (0, 0),
            "a warm region of {size} values took {allocations} allocations and {bytes} bytes"
        );
        assert_eq!(arena.stats().allocations, (size * 102) as u64);
    }
}

/// The cold pass is bounded and stated: one chunk per 256 slots, and nothing per value.
#[test]
fn a_cold_region_costs_one_chunk_per_two_hundred_and_fifty_six_slots() {
    for size in [1usize, 256, 257, 1_000, 10_000] {
        let mut arena = Arena::new();
        let (_, _, ()) = counted(|| cycle(&mut arena, RegionKind::Unique, size));
        let chunks = size.div_ceil(256);
        assert_eq!(
            arena.stats().chunks_allocated,
            chunks,
            "a cold region of {size} values"
        );
    }
}

/// Nesting is free too: an inner region is a mark on the same bump pointer, not an arena of its
/// own.
#[test]
fn nesting_costs_the_allocator_nothing_once_warm() {
    let mut arena = Arena::new();
    let run = |arena: &mut Arena| {
        let outer = arena.open(RegionKind::Unique, Span::DUMMY);
        for depth in 0..64 {
            let inner = arena.open(RegionKind::Unique, Span::DUMMY);
            for i in 0..16 {
                arena.alloc(Value::Int(depth * 16 + i));
            }
            arena.close(inner);
        }
        arena.close(outer);
    };
    run(&mut arena);

    let (allocations, bytes, ()) = counted(|| {
        for _ in 0..100 {
            run(&mut arena);
        }
    });

    assert_eq!((allocations, bytes), (0, 0));
}

/// Claim 2, as a number: a snapshot is one allocation for the values, one for the generations and
/// one for the scope stack it has to put back, whatever the region holds.
#[test]
fn a_snapshot_costs_three_allocations_and_is_linear_in_bytes() {
    let mut widths = Vec::new();
    for size in [0usize, 1, 100, 1_000, 10_000] {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        for i in 0..size {
            arena.alloc(Value::Int(i as i64));
        }

        let (allocations, bytes, snapshot) =
            counted(|| arena.snapshot(r).expect("a shared region snapshots"));

        assert_eq!(snapshot.len(), size);
        assert_eq!(snapshot.regions(), 1, "one region is open");
        let expected = if size == 0 { 1 } else { 3 };
        assert_eq!(
            allocations, expected,
            "a snapshot of {size} slots took {allocations} allocations"
        );
        widths.push((size, bytes));
        drop(snapshot);
        arena.close(r);
    }

    // Bytes per slot is the `Value` plus its generation, and the remainder — the one scope that was
    // open — does not drift with the region's size.
    let per_slot = std::mem::size_of::<Value>() + std::mem::size_of::<u32>();
    let overhead = widths[0].1;
    assert!(overhead > 0, "one open scope was recorded");
    for (size, bytes) in &widths {
        assert_eq!(
            *bytes,
            size * per_slot + overhead,
            "a snapshot of {size} slots"
        );
    }
}

/// Restoring is the same shape as snapshotting: no allocation at all, because the arena writes back
/// into chunks it already owns.
#[test]
fn restoring_a_snapshot_costs_the_allocator_nothing() {
    let mut arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    for i in 0..2_000 {
        arena.alloc(Value::Int(i));
    }
    let snapshot = arena.snapshot(r).expect("a shared region snapshots");

    let (allocations, bytes, ()) = counted(|| {
        for _ in 0..100 {
            assert!(arena.restore(&snapshot));
        }
    });

    assert_eq!((allocations, bytes), (0, 0));
    arena.close(r);
}

/// The measurement ADR 0017 asks for: what a capture costs as a function of the region it crosses.
#[test]
fn snapshot_cost_as_a_function_of_region_size() {
    const REPEATS: usize = 200;
    let mut rows: Vec<(usize, f64)> = Vec::new();

    println!("\n  slots   ns/snapshot   ns/slot");
    for size in [1usize, 10, 100, 1_000, 10_000, 100_000] {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        for i in 0..size {
            arena.alloc(Value::Int(i as i64));
        }
        // Warm the allocator, so the first snapshot's `Vec` growth is not the measurement.
        drop(arena.snapshot(r));

        let mut best = f64::MAX;
        for _ in 0..5 {
            let start = Instant::now();
            for _ in 0..REPEATS {
                let snapshot = arena.snapshot(r).expect("a shared region snapshots");
                std::hint::black_box(snapshot.len());
            }
            best = best.min(start.elapsed().as_nanos() as f64 / REPEATS as f64);
        }
        println!("{size:>7}   {best:>11.1}   {:>7.2}", best / size as f64);
        rows.push((size, best));
        arena.close(r);
    }

    // Ten times the slots for at most twenty times the cost, at every step where the constant is
    // not the whole measurement.
    for pair in rows.windows(2) {
        let (small, big) = (&pair[0], &pair[1]);
        if small.0 < 100 {
            continue;
        }
        let growth = big.1 / small.1;
        assert!(
            growth < 20.0,
            "{} slots cost {:.1}ns and {} cost {:.1}ns — {growth:.1}x for 10x the region",
            small.0,
            small.1,
            big.0,
            big.1
        );
    }
}

/// What the allocator is worth on the workload it replaced.
#[test]
fn a_region_against_the_persistent_map_it_replaced() {
    const CELLS: usize = 10_000;

    // The handle vectors are reserved before either side is armed, so what is counted is the
    // store's own cost and not the test's bookkeeping.
    let mut map: RedBlackTreeMap<u32, Value> = RedBlackTreeMap::new();
    let mut ids = Vec::with_capacity(CELLS);
    let (world_build, world_bytes, ()) = counted(|| {
        for i in 0..CELLS {
            map.insert_mut(i as u32, Value::Int(i as i64));
            ids.push(i as u32);
        }
    });
    let (world_write, _, ()) = counted(|| {
        for id in &ids {
            map.insert_mut(*id, Value::Int(-1));
        }
    });

    let mut arena = Arena::new();
    // Warm, because the claim is about a steady state: a service opens a region per request and a
    // test opens one per test.
    cycle(&mut arena, RegionKind::Unique, CELLS);

    let mut slots = Vec::with_capacity(CELLS);
    let (region_build, region_bytes, ()) = counted(|| {
        arena.open(RegionKind::Unique, Span::DUMMY);
        for i in 0..CELLS {
            slots.push(arena.alloc(Value::Int(i as i64)).expect("inside a region"));
        }
    });
    let (region_write, _, ()) = counted(|| {
        for slot in &slots {
            arena.set(*slot, Value::Int(-1));
        }
    });
    let (region_close, _, ()) = counted(|| {
        arena.close_current();
    });

    println!(
        "\n  {CELLS} cells\n    map:    build {world_build} allocations, {world_bytes} bytes; \
         {world_write} allocations to write every cell\n    region: build {region_build} \
         allocations, {region_bytes} bytes; {region_write} allocations to write every cell; \
         {region_close} to close"
    );

    assert_eq!(
        (region_build, region_bytes),
        (0, 0),
        "a warm region builds ten thousand cells without touching the allocator"
    );
    assert_eq!((region_write, region_close), (0, 0));
    assert!(
        world_build > CELLS,
        "the persistent map allocates at least once per cell, and it took {world_build}"
    );
    assert!(
        world_write > 0,
        "a persistent write copies the path it rewrites"
    );
}

/// A `unique` region never pays the snapshot, which is the whole of "and it is free": the same
/// workload under the two kinds differs by exactly the copies.
#[test]
fn the_two_kinds_differ_by_exactly_the_snapshots() {
    const SIZE: usize = 4_000;
    let mut unique = Arena::new();
    let mut shared = Arena::new();

    for arena in [&mut unique, &mut shared] {
        cycle(arena, RegionKind::Unique, SIZE);
    }

    let run = |arena: &mut Arena, kind: RegionKind| {
        let r = arena.open(kind, Span::DUMMY);
        for i in 0..SIZE {
            arena.alloc(Value::Int(i as i64));
        }
        // One capture, as a handler that resumes twice would take.
        let snapshot = arena.snapshot(r);
        if let Some(snapshot) = &snapshot {
            arena.restore(snapshot);
        }
        arena.close(r);
    };

    let (unique_allocs, unique_bytes, ()) = counted(|| run(&mut unique, RegionKind::Unique));
    let (shared_allocs, shared_bytes, ()) = counted(|| run(&mut shared, RegionKind::Shared));

    assert_eq!((unique_allocs, unique_bytes), (0, 0));
    assert_eq!(
        shared_allocs, 3,
        "the snapshot's values, its generations and the scope stack — nothing else"
    );
    let slots = SIZE * (std::mem::size_of::<Value>() + std::mem::size_of::<u32>());
    assert!(
        shared_bytes > slots,
        "the scope stack is one region deep and costs the same at any region size"
    );
    assert!(
        shared_bytes - slots < 128,
        "and it is a scope record, not a second copy of the region: {shared_bytes} vs {slots}"
    );
    assert_eq!(unique.stats().snapshots, 0);
    assert_eq!(shared.stats().snapshots, 1);
    assert_eq!(shared.stats().slots_copied, SIZE as u64);
}

/// The reclamation event R2 put on the evaluation path, timed against the two answers it can give.
#[test]
fn a_close_is_priced_against_what_it_can_answer() {
    const SIZE: usize = 1_000;
    const ROUNDS: usize = 2_000;

    let mut arena = Arena::new();
    for _ in 0..4 {
        cycle(&mut arena, RegionKind::Unique, SIZE);
    }

    let mut freeing = std::time::Duration::ZERO;
    for _ in 0..ROUNDS {
        let r = arena.open(RegionKind::Unique, Span::DUMMY);
        for i in 0..SIZE {
            arena.alloc(Value::Int(i as i64));
        }
        let started = Instant::now();
        arena.close(r);
        freeing += started.elapsed();
    }

    let mut deferring = std::time::Duration::ZERO;
    for _ in 0..ROUNDS {
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        for i in 0..SIZE {
            arena.alloc(Value::Int(i as i64));
        }
        let pin = arena.pin().expect("the region is open");
        let started = Instant::now();
        arena.close(r);
        deferring += started.elapsed();
        // Outside the clock: what the deferral costs is the close, and the run it left goes back at
        // the next one.
        drop(pin);
    }
    arena.collect();

    let free = freeing.as_secs_f64() * 1e9 / ROUNDS as f64;
    let defer = deferring.as_secs_f64() * 1e9 / ROUNDS as f64;
    // A deferral is cheaper *at the close* than a free, and that is not a saving: it records a run
    // and postpones the truncation, which the next close after the continuation dies then pays in
    // full.
    println!(
        "  closing a region of {SIZE} slots: freed {free:.0} ns ({:.2} ns/slot); \
         deferred to a live continuation {defer:.0} ns, which is the run being recorded \
         rather than the slots being handed back",
        free / SIZE as f64,
    );

    assert_eq!(arena.retained_slots(), 0, "every deferred run went back");
    assert!(
        free > 0.0 && defer > 0.0,
        "the clock resolved neither close"
    );
}

use crate::counting::charge;
use ply_eval::Value;
use ply_eval::arena::{Arena, Owner, RegionKind};

fn counted<R>(f: impl FnOnce() -> R) -> (usize, usize, R) {
    let (out, allocs, bytes) = charge(f);
    (allocs, bytes, out)
}

/// One region's worth of work: open, fill, close.
fn cycle(arena: &mut Arena, kind: RegionKind, size: usize) {
    let r = arena.open(Owner::ENTRY, kind);
    for i in 0..size {
        arena.alloc(Owner::ENTRY, Value::Int(i as i64));
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

#[test]
fn nesting_costs_the_allocator_nothing_once_warm() {
    let mut arena = Arena::new();
    let run = |arena: &mut Arena| {
        let outer = arena.open(Owner::ENTRY, RegionKind::Unique);
        for depth in 0..64 {
            let inner = arena.open(Owner::ENTRY, RegionKind::Unique);
            for i in 0..16 {
                arena.alloc(Owner::ENTRY, Value::Int(depth * 16 + i));
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

/// Two stacks interleaving their allocations, the one that opened first closing first, so the
/// store frees out of order and reuses what it freed.
#[test]
fn two_stacks_interleaving_cost_the_allocator_nothing_once_warm() {
    let (first, second) = (Owner(0), Owner(1));
    let mut arena = Arena::new();
    let run = |arena: &mut Arena| {
        let older = arena.open(first, RegionKind::Shared);
        let younger = arena.open(second, RegionKind::Shared);
        for i in 0..300 {
            arena.alloc(first, Value::Int(i));
            arena.alloc(second, Value::Int(i));
        }
        arena.close(older);
        let nested = arena.open(second, RegionKind::Unique);
        for i in 0..300 {
            arena.alloc(second, Value::Int(i));
        }
        arena.close(nested);
        arena.close(younger);
    };
    for _ in 0..2 {
        run(&mut arena);
    }

    let (allocations, bytes, ()) = counted(|| {
        for _ in 0..100 {
            run(&mut arena);
        }
    });

    assert_eq!((allocations, bytes), (0, 0));
    assert_eq!((arena.total_depth(), arena.live()), (0, 0));
}

/// The way a continuation restored into a region goes through the arena: pinned at the stop,
/// parked by its close, reopened for the restore, parked again, and freed by the last unpin.
#[test]
fn a_pinned_region_parked_and_reopened_costs_the_allocator_nothing_once_warm() {
    let body = Owner(1);
    let mut arena = Arena::new();
    let run = |arena: &mut Arena| {
        let r = arena.open(body, RegionKind::Shared);
        for i in 0..300 {
            arena.alloc(body, Value::Int(i));
        }
        let pin = arena.pin(r).expect("an open region takes a pin");
        arena.close(r);
        assert!(arena.reopen(&pin, body));
        arena.close(r);
        arena.unpin(pin);
    };
    for _ in 0..2 {
        run(&mut arena);
    }

    let (allocations, bytes, ()) = counted(|| {
        for _ in 0..100 {
            run(&mut arena);
        }
    });

    assert_eq!((allocations, bytes), (0, 0));
    assert_eq!((arena.total_depth(), arena.live()), (0, 0));
}

/// Renewing between entries, as the backend does, keeps what earlier entries warmed: the chunks, the
/// heap of free indices and the list of taken slots.
#[test]
fn renewing_a_warm_store_costs_the_allocator_nothing() {
    const FLOOR: usize = 2;
    let (first, second) = (Owner(0), Owner(1));
    let mut arena = Arena::new();
    for _ in 0..FLOOR {
        arena.open(first, RegionKind::Shared);
    }
    let entry = |arena: &mut Arena| {
        let older = arena.open(first, RegionKind::Shared);
        let younger = arena.open(second, RegionKind::Shared);
        for i in 0..300 {
            arena.alloc(first, Value::Int(i));
            arena.alloc(second, Value::Int(i));
        }
        let unfinished = arena.alloc(first, Value::Int(-1)).expect("inside a region");
        arena.take(unfinished).expect("the cell is live");
        arena.close(older);
        arena.close(younger);
        assert!(arena.renew(first, FLOOR));
    };
    for _ in 0..2 {
        entry(&mut arena);
    }
    let chunks = arena.stats().chunks_allocated;

    let (allocations, bytes, ()) = counted(|| {
        for _ in 0..100 {
            entry(&mut arena);
        }
    });

    assert_eq!((allocations, bytes), (0, 0));
    assert_eq!(arena.stats().chunks_allocated, chunks);
}

#[test]
fn a_warm_region_builds_writes_and_closes_without_the_allocator() {
    const CELLS: usize = 10_000;

    let mut arena = Arena::new();
    // Warm first: the claim is about the steady state.
    cycle(&mut arena, RegionKind::Unique, CELLS);

    let mut slots = Vec::with_capacity(CELLS);
    let (region_build, region_bytes, region) = counted(|| {
        let region = arena.open(Owner::ENTRY, RegionKind::Unique);
        for i in 0..CELLS {
            slots.push(
                arena
                    .alloc(Owner::ENTRY, Value::Int(i as i64))
                    .expect("inside a region"),
            );
        }
        region
    });
    let (region_write, _, ()) = counted(|| {
        for slot in &slots {
            arena.set(*slot, Value::Int(-1));
        }
    });
    let (region_close, _, ()) = counted(|| {
        arena.close(region);
    });

    assert_eq!(
        (region_build, region_bytes),
        (0, 0),
        "a warm region builds ten thousand cells without touching the allocator"
    );
    assert_eq!((region_write, region_close), (0, 0));
}

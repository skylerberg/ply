use crate::counting::charge;
use ply_eval::Value;
use ply_eval::arena::{Arena, RegionKind};
use ply_span::Span;
use rpds::RedBlackTreeMap;

fn counted<R>(f: impl FnOnce() -> R) -> (usize, usize, R) {
    let (out, allocs, bytes) = charge(f);
    (allocs, bytes, out)
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

#[test]
fn a_region_against_the_persistent_map_it_replaced() {
    const CELLS: usize = 10_000;

    // Reserved before arming, so the count is the store's cost and not the test's bookkeeping.
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
    // Warm first: the claim is about the steady state.
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

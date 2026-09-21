use ply_eval::Value;
use ply_eval::arena::*;
use ply_span::Span;
use std::sync::Arc;

/// Behind an `Arc`, so `strong_count` shows whether the arena freed it.
fn payload(n: i64) -> (Arc<Vec<Value>>, Value) {
    let items = Arc::new(vec![Value::Int(n)]);
    (
        Arc::clone(&items),
        Value::Ctor {
            name: "Box".into(),
            args: items,
        },
    )
}

fn int_of(arena: &Arena, slot: Slot) -> i64 {
    match arena.get(slot) {
        Some(Value::Int(i)) => *i,
        other => panic!("expected an Int in {slot}, found {other:?}"),
    }
}

#[test]
fn allocation_is_a_bump_and_close_gives_the_slots_back() {
    let mut arena: Arena = Arena::new();
    let r = arena.open(RegionKind::Unique, Span::DUMMY);
    for i in 0..64 {
        arena.alloc(Value::Int(i));
    }
    assert_eq!(arena.live(), 64);
    assert_eq!(arena.extent(r), Some(64));

    arena.close(r);

    assert_eq!(arena.live(), 0);
    assert_eq!(arena.depth(), 0);
    assert_eq!(arena.extent(r), None);
}

#[test]
fn closing_a_region_drops_the_values_it_held() {
    let mut arena: Arena = Arena::new();
    let (arc, value) = payload(1);
    let r = arena.open(RegionKind::Unique, Span::DUMMY);
    arena.alloc(value);
    assert_eq!(
        Arc::strong_count(&arc),
        2,
        "the arena and this test hold it"
    );

    arena.close(r);

    assert_eq!(Arc::strong_count(&arc), 1, "the region's close freed it");
}

#[test]
fn a_second_region_of_the_same_size_allocates_nothing() {
    let mut arena: Arena = Arena::new();
    for _ in 0..4 {
        let r = arena.open(RegionKind::Unique, Span::DUMMY);
        for i in 0..1_000 {
            arena.alloc(Value::Int(i));
        }
        arena.close(r);
    }
    let after_warm = arena.stats().chunks_allocated;

    for _ in 0..1_000 {
        let r = arena.open(RegionKind::Unique, Span::DUMMY);
        for i in 0..1_000 {
            arena.alloc(Value::Int(i));
        }
        arena.close(r);
    }

    assert_eq!(
        arena.stats().chunks_allocated,
        after_warm,
        "a thousand more regions of a size the arena has already seen took no chunk"
    );
    assert_eq!(arena.stats().allocations, 1_004_000);
}

#[test]
fn a_slot_from_a_closed_region_reads_nothing_rather_than_the_value_after_it() {
    let mut arena: Arena = Arena::new();
    let first = arena.open(RegionKind::Unique, Span::DUMMY);
    let stale = arena.alloc(Value::Int(1)).expect("inside a region");
    arena.close(first);

    let second = arena.open(RegionKind::Unique, Span::DUMMY);
    let fresh = arena.alloc(Value::Int(2)).expect("inside a region");

    assert_eq!(
        stale.index(),
        fresh.index(),
        "the bump pointer reused the position, which is why the generation matters"
    );
    assert!(arena.get(stale).is_none());
    assert!(!arena.set(stale, Value::Int(99)));
    assert_eq!(int_of(&arena, fresh), 2);
    arena.close(second);
}

#[test]
fn allocating_outside_every_region_is_refused() {
    let mut arena: Arena = Arena::new();
    assert!(arena.alloc(Value::Int(1)).is_none());
    assert_eq!(arena.stats().allocations, 0);
}

#[test]
fn an_inner_region_reads_and_writes_an_outer_regions_values() {
    let mut arena: Arena = Arena::new();
    let outer = arena.open(RegionKind::Unique, Span::DUMMY);
    let a = arena.alloc(Value::Int(1)).expect("inside a region");

    let inner = arena.open(RegionKind::Unique, Span::DUMMY);
    let b = arena.alloc(Value::Int(2)).expect("inside a region");
    assert_eq!(int_of(&arena, a), 1);
    assert!(arena.set(a, Value::Int(10)));

    arena.close(inner);

    assert_eq!(int_of(&arena, a), 10, "the outer region kept its write");
    assert!(arena.get(b).is_none(), "the inner region's slot is gone");
    assert_eq!(arena.extent(outer), Some(1));
    arena.close(outer);
}

#[test]
fn closing_an_outer_region_closes_the_inner_regions_still_open_inside_it() {
    let mut arena: Arena = Arena::new();
    let outer = arena.open(RegionKind::Unique, Span::DUMMY);
    arena.alloc(Value::Int(1));
    let mid = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(2));
    let inner = arena.open(RegionKind::Unique, Span::DUMMY);
    let deep = arena.alloc(Value::Int(3)).expect("inside a region");

    arena.close(outer);

    assert_eq!(arena.depth(), 0);
    assert_eq!(arena.live(), 0);
    assert!(arena.get(deep).is_none());
    assert_eq!(arena.kind(mid), None);
    assert_eq!(arena.kind(inner), None);
}

#[test]
fn closing_a_region_twice_is_not_a_second_free() {
    let mut arena: Arena = Arena::new();
    let outer = arena.open(RegionKind::Unique, Span::DUMMY);
    let kept = arena.alloc(Value::Int(7)).expect("inside a region");
    let inner = arena.open(RegionKind::Unique, Span::DUMMY);
    arena.alloc(Value::Int(8));

    arena.close(inner);
    arena.close(inner);

    assert_eq!(int_of(&arena, kept), 7);
    assert_eq!(arena.depth(), 1);
    arena.close(outer);
}

#[test]
fn nesting_deeper_than_one_chunk_keeps_every_level_addressable() {
    let mut arena: Arena = Arena::new();
    let mut regions = Vec::new();
    let mut slots = Vec::new();
    for level in 0..32 {
        regions.push(arena.open(RegionKind::Unique, Span::DUMMY));
        for i in 0..40 {
            slots.push((
                arena
                    .alloc(Value::Int(level * 100 + i))
                    .expect("inside a region"),
                level * 100 + i,
            ));
        }
    }
    assert!(arena.live() > CHUNK * 4, "the test spans several chunks");
    for (slot, expected) in &slots {
        assert_eq!(int_of(&arena, *slot), *expected);
    }
    for region in regions.iter().rev() {
        arena.close(*region);
    }
    assert_eq!(arena.live(), 0);
    for (slot, _) in &slots {
        assert!(arena.get(*slot).is_none());
    }
}

#[test]
fn a_slots_generation_counts_frees_and_is_a_wrapping_u32() {
    let mut arena: Arena = Arena::new();
    let mut seen = Vec::new();
    for _ in 0..8 {
        let r = arena.open(RegionKind::Unique, Span::DUMMY);
        seen.push(
            arena
                .alloc(Value::Int(0))
                .expect("inside a region")
                .generation(),
        );
        arena.close(r);
    }
    assert_eq!(
        seen,
        (0..8).collect::<Vec<u32>>(),
        "one increment per close at the same index; `u32::MAX` closes later the sequence starts \
         over and slot @0.0 is live again"
    );
}

#[test]
fn slots_iterate_in_ascending_index_order() {
    let mut arena: Arena = Arena::new();
    let r = arena.open(RegionKind::Unique, Span::DUMMY);
    for i in 0..600 {
        arena.alloc(Value::Int(i));
    }
    let seen: Vec<u32> = arena.slots().map(|(slot, _)| slot.index()).collect();
    assert_eq!(seen, (0..600).collect::<Vec<u32>>());
    arena.close(r);
}

#[test]
fn the_default_kind_is_shared() {
    assert_eq!(RegionKind::default(), RegionKind::Shared);
    assert_eq!(RegionKind::parse("unique"), Some(RegionKind::Unique));
    assert_eq!(RegionKind::parse("shared"), Some(RegionKind::Shared));
    assert_eq!(RegionKind::parse("Unique"), None);
}

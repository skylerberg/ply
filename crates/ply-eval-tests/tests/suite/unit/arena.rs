use ply_eval::Value;
use ply_eval::arena::*;
use std::sync::Arc;

/// Two control stacks.
const A: Owner = Owner(0);
const B: Owner = Owner(1);

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
    let r = arena.open(A, RegionKind::Unique);
    for i in 0..64 {
        arena.alloc(A, Value::Int(i));
    }
    assert_eq!(arena.live(), 64);
    assert_eq!(arena.extent(r), Some(64));

    arena.close(r);

    assert_eq!(arena.live(), 0);
    assert_eq!(arena.depth(A), 0);
    assert_eq!(arena.extent(r), None);
}

#[test]
fn closing_a_region_drops_the_values_it_held() {
    let mut arena: Arena = Arena::new();
    let (arc, value) = payload(1);
    let r = arena.open(A, RegionKind::Unique);
    arena.alloc(A, value);
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
        let r = arena.open(A, RegionKind::Unique);
        for i in 0..1_000 {
            arena.alloc(A, Value::Int(i));
        }
        arena.close(r);
    }
    let after_warm = arena.stats().chunks_allocated;

    for _ in 0..1_000 {
        let r = arena.open(A, RegionKind::Unique);
        for i in 0..1_000 {
            arena.alloc(A, Value::Int(i));
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
    let first = arena.open(A, RegionKind::Unique);
    let stale = arena.alloc(A, Value::Int(1)).expect("inside a region");
    arena.close(first);

    let second = arena.open(A, RegionKind::Unique);
    let fresh = arena.alloc(A, Value::Int(2)).expect("inside a region");

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
    assert!(arena.alloc(A, Value::Int(1)).is_none());
    assert_eq!(arena.stats().allocations, 0);
}

#[test]
fn an_inner_region_reads_and_writes_an_outer_regions_values() {
    let mut arena: Arena = Arena::new();
    let outer = arena.open(A, RegionKind::Unique);
    let a = arena.alloc(A, Value::Int(1)).expect("inside a region");

    let inner = arena.open(A, RegionKind::Unique);
    let b = arena.alloc(A, Value::Int(2)).expect("inside a region");
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
    let outer = arena.open(A, RegionKind::Unique);
    arena.alloc(A, Value::Int(1));
    let mid = arena.open(A, RegionKind::Shared);
    arena.alloc(A, Value::Int(2));
    let inner = arena.open(A, RegionKind::Unique);
    let deep = arena.alloc(A, Value::Int(3)).expect("inside a region");

    arena.close(outer);

    assert_eq!(arena.depth(A), 0);
    assert_eq!(arena.live(), 0);
    assert!(arena.get(deep).is_none());
    assert_eq!(arena.kind(mid), None);
    assert_eq!(arena.kind(inner), None);
}

#[test]
fn closing_a_region_twice_is_not_a_second_free() {
    let mut arena: Arena = Arena::new();
    let outer = arena.open(A, RegionKind::Unique);
    let kept = arena.alloc(A, Value::Int(7)).expect("inside a region");
    let inner = arena.open(A, RegionKind::Unique);
    arena.alloc(A, Value::Int(8));

    arena.close(inner);
    arena.close(inner);

    assert_eq!(int_of(&arena, kept), 7);
    assert_eq!(arena.depth(A), 1);
    arena.close(outer);
}

#[test]
fn nesting_deeper_than_one_chunk_keeps_every_level_addressable() {
    let mut arena: Arena = Arena::new();
    let mut regions = Vec::new();
    let mut slots = Vec::new();
    for level in 0..32 {
        regions.push(arena.open(A, RegionKind::Unique));
        for i in 0..40 {
            slots.push((
                arena
                    .alloc(A, Value::Int(level * 100 + i))
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
        let r = arena.open(A, RegionKind::Unique);
        seen.push(
            arena
                .alloc(A, Value::Int(0))
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
    let r = arena.open(A, RegionKind::Unique);
    for i in 0..600 {
        arena.alloc(A, Value::Int(i));
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

#[test]
fn closing_the_older_of_two_stacks_regions_leaves_the_other_stacks_cells() {
    let mut arena: Arena = Arena::new();
    let older = arena.open(A, RegionKind::Shared);
    let a = arena.alloc(A, Value::Int(1)).expect("inside a region");
    let younger = arena.open(B, RegionKind::Shared);
    let b = arena.alloc(B, Value::Int(2)).expect("inside a region");
    let a_again = arena.alloc(A, Value::Int(3)).expect("inside a region");

    assert_eq!(arena.close(older), Reclaim::Freed(2));

    assert_eq!(int_of(&arena, b), 2, "the other stack's cell survives");
    assert!(arena.get(a).is_none() && arena.get(a_again).is_none());
    assert!(!arena.set(a, Value::Int(9)));
    assert_eq!(arena.kind(younger), Some(RegionKind::Shared));
    assert_eq!((arena.depth(A), arena.depth(B)), (0, 1));
    assert_eq!((arena.total_depth(), arena.live()), (1, 1));
    let live: Vec<Slot> = arena.slots().map(|(slot, _)| slot).collect();
    assert_eq!(live, vec![b]);

    // The positions go back lowest first, at a generation no stale slot carries.
    let reopened = arena.open(A, RegionKind::Shared);
    let first = arena.alloc(A, Value::Int(4)).expect("inside a region");
    let second = arena.alloc(A, Value::Int(5)).expect("inside a region");
    assert_eq!((first.index(), first.generation()), (0, 1));
    assert_eq!((second.index(), second.generation()), (2, 1));
    assert!(arena.get(a).is_none() && arena.get(a_again).is_none());
    assert_eq!(arena.close(younger), Reclaim::Freed(1));
    assert_eq!(arena.close(reopened), Reclaim::Freed(2));
    assert_eq!((arena.total_depth(), arena.live()), (0, 0));
}

#[test]
fn a_nested_close_stays_within_its_owner() {
    let mut arena: Arena = Arena::new();
    let a_outer = arena.open(A, RegionKind::Unique);
    arena.alloc(A, Value::Int(1));
    let b_outer = arena.open(B, RegionKind::Unique);
    let kept = arena.alloc(B, Value::Int(2)).expect("inside a region");
    let a_inner = arena.open(A, RegionKind::Unique);
    arena.alloc(A, Value::Int(3));
    let b_inner = arena.open(B, RegionKind::Shared);
    let also_kept = arena.alloc(B, Value::Int(4)).expect("inside a region");
    assert_eq!(arena.extent(a_outer), Some(2));

    assert_eq!(arena.close(a_outer), Reclaim::Freed(2));

    assert_eq!(
        arena.kind(a_inner),
        None,
        "the inner region of the same stack closed"
    );
    assert_eq!(arena.kind(b_outer), Some(RegionKind::Unique));
    assert_eq!(arena.kind(b_inner), Some(RegionKind::Shared));
    assert_eq!(arena.extent(b_outer), Some(2));
    assert_eq!(int_of(&arena, kept), 2);
    assert_eq!(int_of(&arena, also_kept), 4);
    assert_eq!(arena.close(a_outer), Reclaim::NotOpen);

    // The other stack's new cell goes into its own innermost region.
    let late = arena.alloc(B, Value::Int(5)).expect("inside a region");
    assert_eq!(arena.close(b_inner), Reclaim::Freed(2));
    assert!(arena.get(late).is_none());
    assert_eq!(int_of(&arena, kept), 2);
    assert!(
        arena.alloc(A, Value::Int(6)).is_none(),
        "the first stack holds no region"
    );
    assert_eq!(arena.close(b_outer), Reclaim::Freed(1));
    assert_eq!(arena.live(), 0);
}

#[test]
fn closing_above_a_depth_closes_one_owners_regions() {
    let mut arena: Arena = Arena::new();
    arena.open(A, RegionKind::Shared);
    let base = arena.alloc(A, Value::Int(1)).expect("inside a region");
    arena.open(B, RegionKind::Shared);
    let theirs = arena.alloc(B, Value::Int(2)).expect("inside a region");
    arena.open(A, RegionKind::Unique);
    let mid = arena.alloc(A, Value::Int(3)).expect("inside a region");
    arena.open(B, RegionKind::Unique);
    let deeper = arena.alloc(B, Value::Int(4)).expect("inside a region");
    arena.open(A, RegionKind::Unique);
    let top = arena.alloc(A, Value::Int(5)).expect("inside a region");

    arena.close_above(A, 1);

    assert_eq!(
        (arena.depth(A), arena.depth(B), arena.total_depth()),
        (1, 2, 3)
    );
    assert_eq!(int_of(&arena, base), 1);
    assert!(arena.get(mid).is_none() && arena.get(top).is_none());
    assert_eq!(int_of(&arena, theirs), 2);
    assert_eq!(int_of(&arena, deeper), 4);

    arena.close_above(B, 0);

    assert_eq!((arena.depth(A), arena.depth(B)), (1, 0));
    assert_eq!(int_of(&arena, base), 1);
    assert_eq!(arena.live(), 1);

    arena.close_all_but(A, 0);
    assert_eq!((arena.total_depth(), arena.live()), (0, 0));
}

/// One stack's nested opens and closes hand out the positions and generations a bump pointer
/// would, so the slots tests and diagnostics name do not depend on how the store frees.
#[test]
fn one_stack_gets_the_indices_and_generations_a_bump_pointer_gives() {
    let mut arena: Arena = Arena::new();
    let mut seen = Vec::new();
    let mut take = |arena: &mut Arena, n: usize| {
        for _ in 0..n {
            let slot = arena.alloc(A, Value::Unit).expect("inside a region");
            seen.push((slot.index(), slot.generation()));
        }
    };
    let outer = arena.open(A, RegionKind::Shared);
    take(&mut arena, 2);
    let inner = arena.open(A, RegionKind::Unique);
    take(&mut arena, 2);
    arena.close(inner);
    take(&mut arena, 1);
    arena.open(A, RegionKind::Unique);
    take(&mut arena, 2);
    arena.close(outer);
    let last = arena.open(A, RegionKind::Shared);
    take(&mut arena, 6);
    arena.close(last);

    assert_eq!(
        seen,
        vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (2, 1),
            (3, 1),
            (4, 0),
            (0, 1),
            (1, 1),
            (2, 2),
            (3, 2),
            (4, 1),
            (5, 0),
        ]
    );
    assert_eq!(arena.live(), 0);
}

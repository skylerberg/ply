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
fn a_unique_region_refuses_to_be_snapshotted() {
    let mut arena: Arena = Arena::new();
    let r = arena.open(RegionKind::Unique, Span::DUMMY);
    arena.alloc(Value::Int(1));
    assert!(arena.snapshot(r).is_none());
    assert_eq!(arena.stats().snapshots, 0);
    arena.close(r);
}

#[test]
fn a_restore_undoes_every_write_and_every_allocation_since_the_snapshot() {
    let mut arena: Arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    let c = arena.alloc(Value::Int(0)).expect("inside a region");

    let at_capture = arena.snapshot(r).expect("a shared region snapshots");

    assert!(arena.set(c, Value::Int(1)));
    let made_after = arena.alloc(Value::Int(111)).expect("inside a region");
    assert_eq!(int_of(&arena, c), 1);

    assert!(arena.restore(&at_capture));
    assert_eq!(int_of(&arena, c), 0, "the write is undone");
    assert!(
        arena.get(made_after).is_none(),
        "and a slot allocated since the snapshot is not readable through it"
    );

    arena.set(c, Value::Int(2));
    assert_eq!(int_of(&arena, c), 2);
    arena.close(r);
}

#[test]
fn allocations_either_side_of_a_restore_are_different_slots() {
    let mut arena: Arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(0));
    let at_capture = arena.snapshot(r).expect("a shared region snapshots");

    let first = arena.alloc(Value::Int(1)).expect("inside a region");
    arena.restore(&at_capture);
    let second = arena.alloc(Value::Int(2)).expect("inside a region");

    assert_eq!(first.index(), second.index());
    assert_ne!(first.generation(), second.generation());
    assert!(arena.get(first).is_none());
    assert_eq!(int_of(&arena, second), 2);
    arena.close(r);
}

#[test]
fn a_restore_keeps_the_slots_the_capture_was_holding() {
    let mut arena: Arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    let held: Vec<Slot> = (0..300)
        .map(|i| arena.alloc(Value::Int(i)).expect("inside a region"))
        .collect();
    let at_capture = arena.snapshot(r).expect("a shared region snapshots");

    for slot in &held {
        arena.set(*slot, Value::Int(-1));
    }
    arena.restore(&at_capture);

    for (i, slot) in held.iter().enumerate() {
        assert_eq!(int_of(&arena, *slot), i as i64);
    }
    arena.close(r);
}

#[test]
fn a_snapshot_covers_the_regions_nested_inside_the_one_it_names() {
    let mut arena: Arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    let a = arena.alloc(Value::Int(1)).expect("inside a region");
    let inner = arena.open(RegionKind::Unique, Span::DUMMY);
    let b = arena.alloc(Value::Int(2)).expect("inside a region");

    let at_capture = arena.snapshot(outer).expect("a shared region snapshots");
    assert_eq!(at_capture.len(), 2);

    arena.set(a, Value::Int(10));
    arena.set(b, Value::Int(20));
    arena.restore(&at_capture);

    assert_eq!(int_of(&arena, a), 1);
    assert_eq!(int_of(&arena, b), 2);
    arena.close(inner);
    arena.close(outer);
}

#[test]
fn a_snapshot_of_the_inner_region_leaves_the_enclosing_regions_writes_in_place() {
    let mut arena: Arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    let x = arena.alloc(Value::Int(0)).expect("inside a region");
    let inner = arena.open(RegionKind::Shared, Span::DUMMY);
    let y = arena.alloc(Value::Int(0)).expect("inside a region");

    let of_inner = arena.snapshot(inner).expect("a shared region snapshots");
    arena.set(x, Value::Int(1));
    arena.set(y, Value::Int(1));
    arena.restore(&of_inner);

    assert_eq!(int_of(&arena, y), 0, "the named region came back");
    assert_eq!(
        int_of(&arena, x),
        1,
        "and the enclosing region's write did not, which is why a capture may not use this"
    );

    // What a capture takes instead.
    arena.set(y, Value::Int(0));
    arena.set(x, Value::Int(0));
    let of_every = arena
        .snapshot_open()
        .expect("no open region is unique")
        .expect("two regions are open");
    assert_eq!(of_every.region(), outer, "rooted at the outermost");
    assert_eq!(of_every.regions(), 2);
    arena.set(x, Value::Int(1));
    arena.set(y, Value::Int(1));
    arena.restore(&of_every);
    assert_eq!(int_of(&arena, x), 0);
    assert_eq!(int_of(&arena, y), 0);
    arena.close(outer);
}

#[test]
fn covering_every_open_region_costs_the_whole_live_arena() {
    let mut arena: Arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    for i in 0..1_000 {
        arena.alloc(Value::Int(i));
    }
    let inner = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(-1));

    let of_inner = arena.snapshot(inner).expect("a shared region snapshots");
    let of_every = arena
        .snapshot_open()
        .expect("no open region is unique")
        .expect("two regions are open");

    assert_eq!(of_inner.len(), 1, "the region the capture is written in");
    assert_eq!(of_every.len(), 1_001, "the one that actually isolates it");
    arena.close(outer);
}

#[test]
fn a_capture_across_a_unique_region_is_refused_and_names_it() {
    let mut arena: Arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(1));
    let unique = arena.open(RegionKind::Unique, Span::DUMMY);
    arena.alloc(Value::Int(2));
    let inner = arena.open(RegionKind::Shared, Span::DUMMY);

    assert_eq!(arena.snapshot_open().err(), Some(unique));
    assert_eq!(arena.stats().snapshots, 0, "and nothing was copied");

    arena.close(inner);
    arena.close(unique);
    assert!(
        arena
            .snapshot_open()
            .expect("every open region is shared")
            .is_some()
    );
    arena.close(outer);
}

#[test]
fn a_capture_outside_every_region_has_nothing_to_snapshot() {
    let mut arena: Arena = Arena::new();
    assert!(arena.snapshot_open().expect("nothing is unique").is_none());
    assert_eq!(arena.stats().snapshots, 0);
}

#[test]
fn a_restore_brings_a_closed_regions_scope_back_with_its_slots() {
    let mut arena: Arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(1));
    let inner = arena.open(RegionKind::Shared, Span::DUMMY);
    let x = arena.alloc(Value::Int(7)).expect("inside a region");

    let at_capture = arena.snapshot(r).expect("a shared region snapshots");

    arena.set(x, Value::Int(8));
    arena.close(inner);
    assert!(arena.get(x).is_none(), "the close freed it");
    assert_eq!(arena.depth(), 1);

    assert!(arena.restore(&at_capture));

    assert_eq!(int_of(&arena, x), 7, "the slot is back");
    assert_eq!(
        arena.kind(inner),
        Some(RegionKind::Shared),
        "and so is the region that owns it, or nothing frees it again"
    );
    assert_eq!(arena.depth(), 2);
    assert_eq!(arena.current(), Some(inner));

    arena.close(inner);
    assert!(arena.get(x).is_none());
    arena.close(r);
    assert_eq!(arena.live(), 0);
}

#[test]
fn a_region_opened_after_the_snapshot_does_not_survive_the_restore() {
    let mut arena: Arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(1));

    let at_capture = arena.snapshot(r).expect("a shared region snapshots");

    arena.alloc(Value::Int(2));
    let q = arena.open(RegionKind::Shared, Span::DUMMY);
    let stranded = arena.alloc(Value::Int(3)).expect("inside a region");

    assert!(arena.restore(&at_capture));

    assert_eq!(arena.live(), 1);
    assert_eq!(arena.depth(), 1, "`q` did not exist at the snapshot");
    assert_eq!(arena.current(), Some(r));
    assert_eq!(arena.kind(q), None);
    assert_eq!(arena.extent(q), None, "and its extent is not a subtraction");
    assert!(arena.get(stranded).is_none());

    // The next allocation is reclaimed by the region it is actually in.
    let again = arena.alloc(Value::Int(4)).expect("inside a region");
    assert_eq!(arena.extent(r), Some(2));
    arena.close(r);
    assert_eq!(arena.live(), 0);
    assert!(arena.get(again).is_none());
}

/// The invariant `extent` and `snapshot` subtract under.
#[test]
fn no_sequence_of_restores_strands_a_regions_mark_above_the_bump_pointer() {
    let mut arena: Arena = Arena::new();
    let root = arena.open(RegionKind::Shared, Span::DUMMY);
    let mut snaps = Vec::new();
    for round in 0..6 {
        arena.alloc(Value::Int(round));
        let inner = arena.open(RegionKind::Shared, Span::DUMMY);
        arena.alloc(Value::Int(round * 10));
        snaps.push(arena.snapshot(root).expect("a shared region snapshots"));
        if round % 2 == 0 {
            arena.close(inner);
        }
    }
    for snap in snaps.iter().rev() {
        assert!(arena.restore(snap));
        assert!(arena.extent(root).unwrap() <= arena.live());
        assert!(arena.snapshot_open().is_ok());
        arena.alloc(Value::Int(-1));
    }
    arena.close(root);
    assert_eq!(arena.live(), 0);
    assert_eq!(arena.depth(), 0);
}

#[test]
fn a_snapshot_copies_the_regions_slots_and_no_others() {
    let mut arena: Arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    for i in 0..500 {
        arena.alloc(Value::Int(i));
    }
    let inner = arena.open(RegionKind::Shared, Span::DUMMY);
    for i in 0..40 {
        arena.alloc(Value::Int(i));
    }

    let of_inner = arena.snapshot(inner).expect("a shared region snapshots");
    let of_outer = arena.snapshot(outer).expect("a shared region snapshots");

    assert_eq!(of_inner.len(), 40, "the inner region's own slots");
    assert_eq!(of_outer.len(), 540, "the outer region's, nesting included");
    assert_eq!(arena.stats().slots_copied, 580);
    arena.close(outer);
}

#[test]
fn snapshot_cost_is_linear_in_the_regions_size() {
    for size in [0usize, 1, 100, 1_000, 10_000] {
        let mut arena: Arena = Arena::new();
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        for i in 0..size {
            arena.alloc(Value::Int(i as i64));
        }
        let before = arena.stats().slots_copied;
        let snap = arena.snapshot(r).expect("a shared region snapshots");
        assert_eq!(snap.len(), size);
        assert_eq!(arena.stats().slots_copied - before, size as u64);
        arena.close(r);
    }
}

#[test]
fn a_snapshot_shares_payloads_rather_than_deep_copying_them() {
    let mut arena: Arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    let mut kept = Vec::new();
    for i in 0..64 {
        let (arc, value) = payload(i);
        arena.alloc(value);
        kept.push(arc);
    }
    for arc in &kept {
        assert_eq!(Arc::strong_count(arc), 2);
    }

    let snap = arena.snapshot(r).expect("a shared region snapshots");

    for arc in &kept {
        assert_eq!(
            Arc::strong_count(arc),
            3,
            "the arena, the snapshot and this test — a deep copy would be more"
        );
    }
    drop(snap);
    arena.close(r);
    for arc in &kept {
        assert_eq!(Arc::strong_count(arc), 1);
    }
}

#[test]
fn a_snapshot_that_is_never_restored_costs_only_itself() {
    let mut arena: Arena = Arena::new();
    let (arc, value) = payload(1);
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(value);
    let snap = arena.snapshot(r).expect("a shared region snapshots");

    drop(snap);
    arena.close(r);

    assert_eq!(Arc::strong_count(&arc), 1);
    assert_eq!(arena.live(), 0);
    assert_eq!(arena.stats().restores, 0);
}

#[test]
fn restoring_into_a_closed_region_is_refused() {
    let mut arena: Arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(1));
    let snap = arena.snapshot(r).expect("a shared region snapshots");
    arena.close(r);

    assert!(!arena.restore(&snap));
    assert_eq!(arena.live(), 0);
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
    assert!(RegionKind::default().snapshots());
    assert!(!RegionKind::Unique.snapshots());
    assert_eq!(RegionKind::parse("unique"), Some(RegionKind::Unique));
    assert_eq!(RegionKind::parse("shared"), Some(RegionKind::Shared));
    assert_eq!(RegionKind::parse("Unique"), None);
}

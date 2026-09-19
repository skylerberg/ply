use ply_eval::Value;
use ply_eval::arena::{Arena, RegionKind, Slot};
use ply_span::Span;

fn int_at(arena: &Arena, slot: Slot) -> Option<i64> {
    match arena.get(slot) {
        Some(Value::Int(i)) => Some(*i),
        _ => None,
    }
}

/// [`Arena::snapshot`] covers one region and the regions nested inside it.
#[test]
fn only_the_open_form_of_snapshot_covers_an_enclosing_regions_writes() {
    let mut arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    let x = arena.alloc(Value::Int(0)).expect("inside a region");
    let inner = arena.open(RegionKind::Shared, Span::DUMMY);
    let y = arena.alloc(Value::Int(0)).expect("inside a region");

    let narrow = arena.snapshot(inner).expect("a shared region snapshots");
    assert_eq!(
        narrow.len(),
        1,
        "the narrow form holds the inner region's slot and nothing below it"
    );
    arena.set(x, Value::Int(1));
    arena.set(y, Value::Int(1));
    arena.restore(&narrow);
    assert_eq!(int_at(&arena, y), Some(0), "the inner region was restored");
    assert_eq!(
        int_at(&arena, x),
        Some(1),
        "and the enclosing region was not, which is the documented limit"
    );

    arena.set(x, Value::Int(0));
    let wide = arena
        .snapshot_open()
        .expect("no open region is unique")
        .expect("two regions are open");
    assert_eq!(wide.region(), outer, "rooted at the outermost open region");
    assert_eq!(wide.regions(), 2);
    arena.set(x, Value::Int(1));
    arena.set(y, Value::Int(1));
    arena.restore(&wide);
    assert_eq!(int_at(&arena, x), Some(0), "both regions came back");
    assert_eq!(int_at(&arena, y), Some(0));
    arena.close(outer);
}

#[test]
fn covering_every_open_region_costs_the_whole_live_arena_at_every_capture() {
    let mut arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    for i in 0..1_000 {
        arena.alloc(Value::Int(i));
    }
    let inner = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(-1));

    let narrow = arena.snapshot(inner).expect("a shared region snapshots");
    let correct = arena
        .snapshot_open()
        .expect("no open region is unique")
        .expect("two regions are open");

    assert_eq!(narrow.len(), 1);
    assert_eq!(
        correct.len(),
        1_001,
        "the snapshot that actually isolates the writes is the outermost one"
    );
    arena.close(outer);
}

#[test]
fn a_unique_region_open_at_a_checkpoint_is_reported_rather_than_skipped() {
    let mut arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(1));
    let unique = arena.open(RegionKind::Unique, Span::DUMMY);
    arena.alloc(Value::Int(2));

    assert_eq!(arena.snapshot_open().err(), Some(unique));
    assert_eq!(arena.stats().snapshots, 0, "and nothing was copied");
    arena.close(outer);
}

#[test]
fn a_restore_brings_a_closed_regions_scope_back_with_its_slots() {
    let mut arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(0)).expect("inside a region");
    let inner = arena.open(RegionKind::Unique, Span::DUMMY);
    let x = arena.alloc(Value::Int(7)).expect("inside a region");

    let at_capture = arena.snapshot(r).expect("a shared region snapshots");

    arena.set(x, Value::Int(8));
    arena.close(inner);
    assert_eq!(int_at(&arena, x), None, "the inner region's close freed it");
    assert_eq!(arena.depth(), 1);

    assert!(arena.restore(&at_capture));

    assert_eq!(int_at(&arena, x), Some(7), "the slot came back");
    assert_eq!(
        arena.depth(),
        2,
        "and so did the region that owns it, or nothing will free it again"
    );
    assert_eq!(arena.kind(inner), Some(RegionKind::Unique));
    assert_eq!(arena.current(), Some(inner));

    arena.close(inner);
    assert_eq!(int_at(&arena, x), None, "and the close frees it again");
    arena.close(r);
    assert_eq!(arena.live(), 0);
}

#[test]
fn a_region_opened_after_the_snapshot_does_not_survive_the_restore() {
    let mut arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(0)).expect("inside a region");

    let at_capture = arena.snapshot(r).expect("a shared region snapshots");

    arena.alloc(Value::Int(1)).expect("inside a region");
    let opened = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(2)).expect("inside a region");

    assert!(arena.restore(&at_capture));

    assert_eq!(arena.live(), 1, "the bump pointer went back to the capture");
    assert_eq!(arena.depth(), 1, "and so did the scope stack");
    assert_eq!(arena.current(), Some(r));
    assert_eq!(arena.kind(opened), None);

    let after = arena.alloc(Value::Int(9)).expect("inside a region");
    arena.close(r);
    assert_eq!(
        int_at(&arena, after),
        None,
        "an allocation made after the restore belongs to the region that is open"
    );
    assert_eq!(arena.live(), 0);
}

/// The precondition [`Arena::extent`] and [`Arena::snapshot`] subtract on.
#[test]
fn no_regions_mark_ever_sits_above_the_bump_pointer() {
    let mut arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(0)).expect("inside a region");
    let at_capture = arena.snapshot(r).expect("a shared region snapshots");
    arena.alloc(Value::Int(1)).expect("inside a region");
    let opened = arena.open(RegionKind::Shared, Span::DUMMY);

    assert_eq!(arena.extent(opened), Some(0));
    assert_eq!(arena.live(), 2);

    arena.restore(&at_capture);

    assert_eq!(arena.live(), 1);
    assert!(
        arena.kind(opened).is_none(),
        "`opened` is no longer a scope"
    );
    assert_eq!(arena.extent(r), Some(1));
    assert!(arena.extent(r).unwrap() <= arena.live());
    assert!(arena.snapshot_open().is_ok());
    arena.close(r);
}

#[test]
fn a_slots_generation_counts_frees_and_is_a_wrapping_u32() {
    let mut arena = Arena::new();
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

use ply_eval::arena::{Owner, RegionKind, Slot};
use ply_eval::task_regions::*;
use ply_eval::{Span, Value};

fn int_of(regions: &TaskRegions, slot: Slot) -> i64 {
    match regions.get(slot) {
        Some(Value::Int(i)) => *i,
        other => panic!("expected an Int in {slot}, found {other:?}"),
    }
}

#[test]
fn a_fresh_stack_can_allocate_because_its_entry_region_is_open() {
    let mut regions: TaskRegions = TaskRegions::new();
    let slot = regions.alloc_cell(Value::Int(1));
    assert_eq!(int_of(&regions, slot), 1);
    assert_eq!(
        regions.depth(Owner::ENTRY),
        2,
        "two scopes and no more: the fixture's and the entry point's"
    );
}

#[test]
fn a_reset_discards_what_the_entry_point_allocated() {
    let mut regions: TaskRegions = TaskRegions::new();
    let scratch = regions.alloc_cell(Value::Int(1));

    regions.reset();

    assert!(!regions.contains(scratch));
    assert_eq!(regions.live(), 0);
}

#[test]
fn a_reset_puts_the_fixture_back_and_keeps_its_slots_valid() {
    let fixture = Fixture::build(|r| {
        let a = r.alloc_cell(Value::Int(7));
        r.alloc_cell(Value::str("ada"));
        Value::Cell(a)
    });
    let (mut regions, handle) = fixture.open();
    let seeded = handle.as_cell(Span::DUMMY, "the handle").expect("a cell");

    assert_eq!(int_of(&regions, seeded), 7);
    assert!(regions.set(seeded, Value::Int(99)));
    regions.alloc_cell(Value::Int(-1));
    assert_eq!(int_of(&regions, seeded), 99);

    regions.reset();

    assert_eq!(
        int_of(&regions, seeded),
        7,
        "the entry point's write is gone"
    );
    assert_eq!(regions.live(), 2, "and so is what it allocated");
}

#[test]
fn two_stacks_opened_from_one_fixture_cannot_observe_each_other() {
    let fixture = Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(0))));
    let (mut a, handle) = fixture.open();
    let (mut b, _) = fixture.open();
    let c = handle.as_cell(Span::DUMMY, "the handle").expect("a cell");

    assert!(a.set(c, Value::Int(1)));
    assert!(b.set(c, Value::Int(2)));

    assert_eq!(int_of(&a, c), 1);
    assert_eq!(int_of(&b, c), 2);
    assert_eq!(fixture.len(), 1, "the fixture itself is untouched");
}

#[test]
fn sealing_makes_the_current_extent_the_thing_a_reset_goes_back_to() {
    let mut regions: TaskRegions = TaskRegions::new();
    let kept = regions.alloc_cell(Value::Int(5));
    regions.seal();
    let scratch = regions.alloc_cell(Value::Int(6));

    regions.reset();

    assert_eq!(int_of(&regions, kept), 5);
    assert!(!regions.contains(scratch));
    assert_eq!(regions.base_len(), 1);
}

#[test]
fn a_reset_closes_a_region_the_last_entry_point_abandoned() {
    let mut regions: TaskRegions = TaskRegions::new();
    regions.open(Owner::ENTRY, RegionKind::Unique);
    regions.alloc_cell(Value::Int(1));
    assert_eq!(regions.depth(Owner::ENTRY), 3);

    regions.reset();

    assert_eq!(
        regions.depth(Owner::ENTRY),
        2,
        "the fixture's region and a fresh entry region, and nothing the run left open"
    );
    assert_eq!(regions.live(), 0);
}

/// The backend's store, whose floor holds no cell, renews once its entry's regions are closed; a
/// fixture's cells are named by its handle, so a store holding them is never renewed.
#[test]
fn only_a_store_whose_floor_holds_no_cell_renews() {
    let mut regions: TaskRegions = TaskRegions::new();
    let task = Owner(1);
    regions.open(task, RegionKind::Shared);
    let earlier = regions.alloc(task, Value::Int(1)).expect("inside a region");
    regions.close_program_regions();

    assert!(regions.renew());

    regions.open(task, RegionKind::Shared);
    assert_eq!(regions.alloc(task, Value::Int(2)), Some(earlier));

    let fixture = Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(7))));
    let (mut seeded, handle) = fixture.open();
    let cell = handle.as_cell(Span::DUMMY, "the handle").expect("a cell");
    assert!(!seeded.renew());
    assert_eq!(int_of(&seeded, cell), 7);
}

#[test]
fn a_shared_region_reclaims_its_slots_at_its_close() {
    let mut regions: TaskRegions = TaskRegions::new();
    let id = regions.open(Owner::ENTRY, RegionKind::Shared);
    let cell = regions.alloc_cell(Value::Int(1));

    regions.close(id);

    assert!(!regions.contains(cell));
    assert_eq!(regions.live(), 0);
}

#[test]
fn a_unique_region_hands_its_slots_back_at_its_close() {
    let mut regions: TaskRegions = TaskRegions::new();
    let outer = regions.alloc_cell(Value::Int(1));
    let id = regions.open(Owner::ENTRY, RegionKind::Unique);
    let inner = regions.alloc_cell(Value::Int(2));

    regions.close(id);

    assert!(regions.contains(outer));
    assert!(!regions.contains(inner));
    assert_eq!(regions.live(), 1);
}

#[test]
fn an_empty_fixture_opens_an_empty_stack() {
    let (regions, handle) = Fixture::empty().open();
    assert_eq!(regions.live(), 0);
    assert_eq!(regions.base_len(), 0);
    assert!(matches!(handle, Value::Unit));
}

#[test]
fn the_program_regions_close_on_every_stack_down_to_the_fixture() {
    let fixture = Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(7))));
    let (mut regions, handle) = fixture.open();
    let seeded = handle.as_cell(Span::DUMMY, "the handle").expect("a cell");
    let task = Owner(3);
    regions.open(task, RegionKind::Shared);
    let theirs = regions.alloc(task, Value::Int(1)).expect("inside a region");
    regions.open(Owner::ENTRY, RegionKind::Unique);
    let mine = regions.alloc_cell(Value::Int(2));

    regions.close_regions_above(Owner::ENTRY, 0);
    assert_eq!(
        regions.depth(Owner::ENTRY),
        2,
        "no stack's jump reaches below the fixture's region and the entry's"
    );
    assert!(!regions.contains(mine));
    assert_eq!(
        int_of(&regions, theirs),
        1,
        "another stack's cell is not this one's to close"
    );

    regions.close_program_regions();

    assert!(!regions.contains(theirs));
    assert_eq!(int_of(&regions, seeded), 7);
    assert_eq!((regions.total_depth(), regions.live()), (2, 1));
}

/// A `handle` opened on the entry's own stack pins the fixture's region and the entry's, which no
/// close during a run reaches: giving the pins back leaves both open, and a reset then frees the
/// entry's region as it would an unpinned one.
#[test]
fn the_floor_regions_stay_open_through_their_pins_and_a_reset_still_frees_the_entry_region() {
    let fixture = Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(7))));
    let (mut regions, handle) = fixture.open();
    let seeded = handle.as_cell(Span::DUMMY, "the handle").expect("a cell");
    let floor: Vec<_> = regions.nesting(Owner::ENTRY).collect();
    let pins: Vec<_> = floor
        .iter()
        .map(|&region| regions.pin(region).expect("the floor is open"))
        .collect();
    let scratch = regions.alloc_cell(Value::Int(1));

    regions.close_program_regions();
    for pin in pins {
        assert_eq!(
            regions.unpin(pin),
            0,
            "an open region's unpin frees nothing"
        );
    }

    assert_eq!(regions.depth(Owner::ENTRY), 2);
    assert_eq!(int_of(&regions, scratch), 1);

    regions.reset();

    assert!(!regions.contains(scratch));
    assert_eq!(regions.kind(floor[0]), None, "the entry's region was freed");
    assert_eq!(int_of(&regions, seeded), 7);
    assert_eq!((regions.depth(Owner::ENTRY), regions.live()), (2, 1));
}

use ply_eval::Value;
use ply_eval::arena::RegionKind;
use ply_eval::arena::Slot;
use ply_eval::task_regions::*;
use ply_span::Span;

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
        regions.depth(),
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
    regions.open_region(RegionKind::Unique, Span::DUMMY);
    regions.alloc_cell(Value::Int(1));
    assert_eq!(regions.depth(), 3);

    regions.reset();

    assert_eq!(
        regions.depth(),
        2,
        "the fixture's region and a fresh entry region, and nothing the run left open"
    );
    assert_eq!(regions.live(), 0);
}

#[test]
fn a_shared_regions_close_keeps_the_slots_a_live_continuation_can_reach() {
    let mut regions: TaskRegions = TaskRegions::new();
    let id = regions.open_region(RegionKind::Shared, Span::DUMMY);
    let cell = regions.alloc_cell(Value::Int(1));
    let pin = regions.pin().expect("a program region is open");

    regions.close_region(id);

    assert!(
        regions.contains(cell),
        "a continuation resumed after the region closed still reads it"
    );
    drop(pin);
}

#[test]
fn a_shared_region_no_continuation_outlives_still_reclaims_at_its_close() {
    let mut regions: TaskRegions = TaskRegions::new();
    let id = regions.open_region(RegionKind::Shared, Span::DUMMY);
    let cell = regions.alloc_cell(Value::Int(1));
    drop(regions.pin().expect("a program region is open"));

    regions.close_region(id);

    assert!(!regions.contains(cell));
    assert_eq!(regions.live(), 0);
}

#[test]
fn a_unique_region_hands_its_slots_back_at_its_close() {
    let mut regions: TaskRegions = TaskRegions::new();
    let outer = regions.alloc_cell(Value::Int(1));
    let id = regions.open_region(RegionKind::Unique, Span::DUMMY);
    let inner = regions.alloc_cell(Value::Int(2));

    regions.close_region(id);

    assert!(regions.contains(outer));
    assert!(!regions.contains(inner));
    assert_eq!(regions.live(), 1);
}

/// Otherwise every `perform` in a program without `with_cell` would pay an `Rc` allocation for nothing.
#[test]
fn no_pin_is_taken_outside_every_program_region() {
    let mut regions: TaskRegions = TaskRegions::new();
    assert!(regions.pin().is_none());
    let id = regions.open_region(RegionKind::Shared, Span::DUMMY);
    assert!(regions.pin().is_some());
    regions.close_region(id);
    assert!(regions.pin().is_none());
}

#[test]
fn an_empty_fixture_opens_an_empty_stack() {
    let (regions, handle) = Fixture::empty().open();
    assert_eq!(regions.live(), 0);
    assert_eq!(regions.base_len(), 0);
    assert!(matches!(handle, Value::Unit));
}

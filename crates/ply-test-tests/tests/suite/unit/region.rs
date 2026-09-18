use ply_eval::arena::Slot;
use ply_eval::{TaskRegions, Value};
use ply_test::region::GroupRegion;

/// `cells` integer cells behind a list of handles: the only way a test reaches group state.
fn seeded(cells: usize) -> impl FnOnce(&mut TaskRegions) -> Value {
    move |regions| {
        Value::list(
            (0..cells)
                .map(|i| Value::Cell(regions.alloc_cell(Value::Int(i as i64))))
                .collect::<Vec<Value>>(),
        )
    }
}

fn cell(handle: &Value, at: usize) -> Slot {
    match handle {
        Value::List(items) => match items.get(at) {
            Some(Value::Cell(slot)) => *slot,
            other => panic!("expected a cell at {at}, found {other:?}"),
        },
        other => panic!("expected the handle list, found {other:?}"),
    }
}

fn int_of(regions: &TaskRegions, slot: Slot) -> i64 {
    match regions.get(slot) {
        Some(Value::Int(i)) => *i,
        other => panic!("expected an Int in {slot}, found {other:?}"),
    }
}

#[test]
fn an_empty_region_has_no_fixture_and_no_mark() {
    let region = GroupRegion::empty();
    assert!(region.is_empty());
    assert_eq!(region.mark(), 0);
    let (stack, _) = region.open();
    assert_eq!(stack.live(), 0);
}

#[test]
fn a_write_to_the_fixture_survives_the_test_that_made_it() {
    let mut region = GroupRegion::build(seeded(3));
    let (mut stack, handle) = region.open();
    let target = cell(&handle, 1);
    assert!(stack.set(target, Value::Int(42)));
    assert!(region.close(&stack));

    let (next, next_handle) = region.open();
    assert_eq!(int_of(&next, cell(&next_handle, 1)), 42);
    assert_eq!(region.mark(), 3, "a write allocates nothing");
}

#[test]
fn what_a_test_allocated_is_gone_when_the_region_closes() {
    let mut region = GroupRegion::build(seeded(2));

    let (mut first, _) = region.open();
    let private = first.alloc_cell(Value::str("first"));
    assert_eq!(private.index(), 2);
    assert!(region.close(&first));

    assert_eq!(region.mark(), 2);
    assert_eq!(region.fixture().len(), 2);

    let (mut second, _) = region.open();
    assert!(
        second.get(private).is_none(),
        "the next test must not be able to read the last one's cell"
    );
    assert_eq!(
        second.alloc_cell(Value::str("second")).index(),
        private.index(),
        "the region reopens at the mark, so the slots start again"
    );
}

/// Keeping the whole post-test stack would pass the two tests above and fail this one.
#[test]
fn a_test_that_writes_and_allocates_leaves_only_the_write() {
    let mut region = GroupRegion::build(seeded(4));
    let (mut stack, handle) = region.open();
    stack.alloc_cell(Value::str("scratch"));
    assert!(stack.set(cell(&handle, 0), Value::Int(-1)));
    stack.alloc_cell(Value::str("more scratch"));
    assert!(region.close(&stack));

    assert_eq!(region.fixture().len(), 4);
    assert_eq!(region.mark(), 4);
    let (next, next_handle) = region.open();
    assert_eq!(next.live(), 4, "the scratch cells did not survive");
    assert_eq!(int_of(&next, cell(&next_handle, 0)), -1);
    assert_eq!(int_of(&next, cell(&next_handle, 3)), 3);
}

#[test]
fn a_long_group_does_not_grow_the_region() {
    let mut region = GroupRegion::build(seeded(8));
    for i in 0..100 {
        let (mut stack, handle) = region.open();
        for _ in 0..16 {
            stack.alloc_cell(Value::Int(i));
        }
        assert!(stack.set(cell(&handle, 7), Value::Int(i)));
        assert!(region.close(&stack));
        assert_eq!(region.fixture().len(), 8, "round {i}");
        assert_eq!(region.mark(), 8, "round {i}");
    }
    let (last, handle) = region.open();
    assert_eq!(int_of(&last, cell(&handle, 7)), 99);
}

/// Shrinking to a stack below the mark would let the next test allocate inside the fixture.
#[test]
fn closing_over_a_stack_this_region_did_not_open_is_refused() {
    let mut region = GroupRegion::build(seeded(4));
    let stranger = GroupRegion::build(seeded(2));
    let (mut other, other_handle) = stranger.open();
    assert!(other.set(cell(&other_handle, 0), Value::Int(-1)));

    assert!(!region.close(&other));

    assert_eq!(region.mark(), 4);
    assert_eq!(region.fixture().len(), 4);
    let (mut next, handle) = region.open();
    assert_eq!(int_of(&next, cell(&handle, 0)), 0);
    assert_eq!(
        next.alloc_cell(Value::Int(9)).index(),
        4,
        "and the next test still allocates above the mark"
    );
}

#[test]
fn opening_twice_gives_two_stacks_that_cannot_see_each_other() {
    let region = GroupRegion::build(seeded(2));
    let (mut a, a_handle) = region.open();
    let (mut b, b_handle) = region.open();
    assert!(a.set(cell(&a_handle, 0), Value::Int(1)));
    assert!(b.set(cell(&b_handle, 0), Value::Int(2)));
    assert_eq!(int_of(&a, cell(&a_handle, 0)), 1);
    assert_eq!(int_of(&b, cell(&b_handle, 0)), 2);

    let (fresh, fresh_handle) = region.open();
    assert_eq!(int_of(&fresh, cell(&fresh_handle, 0)), 0);
}

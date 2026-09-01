//! The region a group of tests runs in, and the fixture that lives in it.

use ply_eval::{Arena, Fixture, TaskRegions, Value};

/// The live region a group's tests share: the fixture, and the mark that separates it from whatever
/// a test allocates on top.
#[derive(Clone, Debug, Default)]
pub struct GroupRegion {
    fixture: Fixture,
}

impl GroupRegion {
    /// No fixture.
    pub fn empty() -> GroupRegion {
        GroupRegion {
            fixture: Fixture::empty(),
        }
    }

    /// Runs `seed` once.
    pub fn build(seed: impl FnOnce(&mut TaskRegions) -> Value) -> GroupRegion {
        GroupRegion {
            fixture: Fixture::build(seed),
        }
    }

    /// The boundary.
    pub fn mark(&self) -> usize {
        self.fixture.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fixture.is_empty()
    }

    /// What a test runs against: a region stack seeded from the group's fixture, sealed at the
    /// mark, and the handle that reaches it.
    #[must_use = "opening a region builds a stack; dropping it discards the seed"]
    pub fn open(&self) -> (TaskRegions, Value) {
        self.fixture.open()
    }

    /// Closes the test's region: what it allocated is discarded and what it wrote to the fixture is
    /// kept.
    pub fn close(&mut self, after: &Arena) -> bool {
        let mark = self.fixture.len();
        if mark == 0 {
            return true;
        }
        if after.live() < mark {
            return false;
        }
        // Slots ascend by index and the fixture was seeded first, so the mark is a prefix.
        let kept: Vec<Value> = after.slots().take(mark).map(|(_, v)| v.clone()).collect();
        let handle = self.fixture.handle().clone();
        self.fixture = Fixture::build(move |regions| {
            for value in kept {
                regions.alloc_cell(value);
            }
            handle
        });
        true
    }

    /// The group's own state, for a caller that wants to look at it rather than run against it.
    pub fn fixture(&self) -> &Fixture {
        &self.fixture
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_eval::arena::Slot;

    /// A fixture of `cells` integer cells, reached through a list of handles — the shape the control-stack design
    /// The `Fixture` names and the only way a test can get at group state at all.
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

    /// The half that replaces the fork: a write a test makes to the fixture is there for the next
    /// test on this worker.
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

    /// The half that keeps tests from observing each other: what a test allocated is gone when it
    /// ends, and the next test allocates at the same slot rather than after it.
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

    /// Both halves at once, which is where an implementation that kept the whole post-test stack
    /// would pass the first two tests and fail this one.
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

    /// A hundred tests in a group leave a fixture the size of the fixture.
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

    /// A stack holding less than the mark cannot be one this region opened, and shrinking the
    /// fixture to it would leave the next test allocating inside the fixture's own range — where
    /// the following close would keep its cells as group state.
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

    /// Opening does not disturb the region, so two tests that ran against it out of order see the
    /// same thing.
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
}

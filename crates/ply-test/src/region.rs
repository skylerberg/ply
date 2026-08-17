//! The region a group of tests runs in, and the fixture that lives in it.
//!
//! ADR 0005 built a fixture once and forked it per test, which was free because
//! a `World` is persistent: `World::fork` was one pointer clone at any fixture
//! size. ADR 0017 §6 takes the fork away. A test now runs on a
//! [`TaskRegions`] — a bump arena whose scopes are the program's regions — and
//! the fixture is **built once per worker and mutated in place**.
//!
//! # What closes a test's region, and where
//!
//! Not here. Every entry point resets its stack to the fixture before it runs,
//! so the slots the previous test allocated go back to the bump pointer with
//! their generations bumped: a `Value::Cell` from the last test reads `None`
//! rather than this test's cell. That is the truncation ADR 0017 §1 promises,
//! it is the evaluator's, and it is why a corpus with no fixture pays this
//! module nothing at all — [`GroupRegion::open`] and [`GroupRegion::close`] are
//! both no-ops there, and the isolation is still total.
//!
//! What is left for this module is the half the evaluator cannot do: carrying a
//! test's writes to the *fixture* forward to the next test, since the reset
//! would otherwise undo them. [`GroupRegion::mark`] is the boundary — the
//! fixture's slot count. Every slot below it belongs to the group and outlives
//! the test; every slot at or above it belongs to the test and is discarded.
//!
//! # Per worker, not per group, and the difference is not cosmetic
//!
//! A `Value` holds `Rc` and cannot cross a thread, so a region stack belongs to
//! the thread that built it — ADR 0017 §5's "each task holds a region stack",
//! read at the granularity a test runner actually has. A group is executed by up
//! to `--jobs` workers, each of which builds its own. So a group costs *w*
//! builds and not one, and a write test *a* makes to the fixture is visible to
//! test *b* only when the two happened to land on one worker. Nothing may depend
//! on which: the colouring is what makes that safe, by putting no two tests that
//! could name one piece of fixture state in one group.
//!
//! Two properties an implementer must not swap:
//!
//! - **A test's writes to the fixture survive it on its own worker; its own
//!   cells do not survive at all.** That is in-place mutation, and it is only
//!   sound because of the disjointness the colouring guarantees.
//! - **A searched test never writes through.** A simulated test is replayed once
//!   per interleaving, so "the fixture as that test left it" names no single
//!   stack; each interleaving opens the region as the test found it, which is
//!   what makes a replay from one seed reproduce exactly.

use ply_eval::{Arena, Fixture, TaskRegions, Value};

/// The live region a group's tests share: the fixture, and the mark that
/// separates it from whatever a test allocates on top.
#[derive(Clone, Debug, Default)]
pub struct GroupRegion {
    fixture: Fixture,
}

impl GroupRegion {
    /// No fixture. Every test in the group opens an empty region, which is what
    /// a corpus with no fixture — every corpus today — gets.
    pub fn empty() -> GroupRegion {
        GroupRegion {
            fixture: Fixture::empty(),
        }
    }

    /// Runs `seed` once. The caller runs this per worker, on the thread that
    /// will use it: a region stack holds `Rc` and does not cross a thread.
    pub fn build(seed: impl FnOnce(&mut TaskRegions) -> Value) -> GroupRegion {
        GroupRegion {
            fixture: Fixture::build(seed),
        }
    }

    /// The boundary. A slot below it belongs to the group; one at or above it
    /// belongs to whichever test is running.
    pub fn mark(&self) -> usize {
        self.fixture.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fixture.is_empty()
    }

    /// What a test runs against: a region stack seeded from the group's
    /// fixture, sealed at the mark, and the handle that reaches it.
    ///
    /// Two engines under `--engine both` take one each. The two stacks hand out
    /// the *same* slots for the fixture and will hand out the same slots for
    /// whatever each allocates on top — the id collision `World`'s sibling forks
    /// had, for the same reason and with the same discipline: an engine holds
    /// exactly one stack and nothing carries a `Value` from one into the other.
    #[must_use = "opening a region builds a stack; dropping it discards the seed"]
    pub fn open(&self) -> (TaskRegions, Value) {
        self.fixture.open()
    }

    /// Closes the test's region: what it allocated is discarded and what it
    /// wrote to the fixture is kept.
    ///
    /// Free when there is no fixture, which is every corpus in this repository:
    /// there is nothing above the mark to keep, and the next entry point's reset
    /// is what discards the test's own slots. Otherwise it costs the fixture's
    /// size and not the test's.
    ///
    /// `false` when `after` is not a stack this region opened — it holds fewer
    /// live slots than the fixture it is supposed to be seeded from. The fixture
    /// is left alone rather than shrunk to it: the mark would then name a slot
    /// the fixture does not hold, the next test would allocate *inside* the
    /// fixture's own range, and the following close would keep its cells as
    /// group state. That is a silent isolation break, so it is refused the way
    /// [`Arena::set`] refuses a slot it does not hold.
    pub fn close(&mut self, after: &Arena) -> bool {
        let mark = self.fixture.len();
        if mark == 0 {
            return true;
        }
        if after.live() < mark {
            return false;
        }
        // Slots ascend by index and the fixture was seeded first, so the mark is
        // a prefix. Replaying it into a fresh stack reproduces exactly the slots
        // the seed handed out, which is what keeps the handle naming the same
        // cells.
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

    /// The group's own state, for a caller that wants to look at it rather than
    /// run against it.
    pub fn fixture(&self) -> &Fixture {
        &self.fixture
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_eval::arena::Slot;

    /// A fixture of `cells` integer cells, reached through a list of handles —
    /// the shape ADR 0005 §2's `Fixture` names and the only way a test can get
    /// at group state at all.
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
            Value::List(items) => match items[at] {
                Value::Cell(slot) => slot,
                ref other => panic!("expected a cell at {at}, found {other:?}"),
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

    /// The half that replaces the fork: a write a test makes to the fixture is
    /// there for the next test on this worker.
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

    /// The half that keeps tests from observing each other: what a test
    /// allocated is gone when it ends, and the next test allocates at the same
    /// slot rather than after it.
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

    /// Both halves at once, which is where an implementation that kept the whole
    /// post-test stack would pass the first two tests and fail this one.
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

    /// A hundred tests in a group leave a fixture the size of the fixture. An
    /// implementation that merely stopped handing out old slots would grow here,
    /// which is the shape of leak this milestone exists to remove.
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

    /// A stack holding less than the mark cannot be one this region opened, and
    /// shrinking the fixture to it would leave the next test allocating inside
    /// the fixture's own range — where the following close would keep its cells
    /// as group state. Refused, and the region is left as it was.
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

    /// Opening does not disturb the region, so two tests that ran against it out
    /// of order see the same thing. This is what makes a verdict independent of
    /// `--jobs`.
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

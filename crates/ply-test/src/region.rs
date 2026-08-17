//! The region a group of tests runs in, and the fixture that lives in it.
//!
//! ADR 0005 built a fixture once and forked it per test, which was free because
//! a `World` is persistent: `World::fork` is one pointer clone at any fixture
//! size. ADR 0017 §6 takes the fork away, so the fixture is **built once per
//! group and mutated in place** instead, and what keeps that safe is the
//! scheduler: a group's members have pairwise non-conflicting footprints, and
//! since ADR 0017 a region label conflicts like any other resource, so no two
//! tests in one group can name one piece of fixture state.
//!
//! The boundary between the two regions is a number. [`GroupRegion::mark`] is
//! the fixture's high-water mark: every cell below it belongs to the group and
//! outlives the test, every cell at or above it belongs to the test and is
//! discarded when the test ends. That is what "a test's allocations live in a
//! region closed when the test ends" means here, and it is why the next test in
//! the group cannot name anything this one allocated — it holds no id for it,
//! and the ids start again at the mark.
//!
//! Two properties an implementer must not swap:
//!
//! - **A test's writes to the fixture survive it; its own cells do not.** That
//!   is in-place mutation, and it is only sound because of the disjointness the
//!   colouring now guarantees.
//! - **A searched test never writes through.** A simulated test is replayed once
//!   per interleaving, so "the fixture as that test left it" names no single
//!   world; each interleaving opens the region as the test found it, which is
//!   what makes a replay from one seed reproduce exactly.

use ply_eval::World;

/// The live region a group's tests share: the fixture, and the mark that
/// separates it from whatever a test allocates on top.
#[derive(Clone, Debug)]
pub struct GroupRegion {
    fixture: World,
    mark: u32,
}

impl Default for GroupRegion {
    fn default() -> Self {
        GroupRegion::empty()
    }
}

impl GroupRegion {
    /// No fixture. Every test in the group opens an empty region, which is what
    /// a corpus with no fixture — every corpus today — gets.
    pub fn empty() -> GroupRegion {
        GroupRegion {
            fixture: World::new(),
            mark: 0,
        }
    }

    /// Runs `seed` once. The caller runs this per group, on the thread that
    /// will use it: a `World` holds `Rc` and does not cross a thread.
    pub fn build(seed: impl FnOnce() -> World) -> GroupRegion {
        let fixture = seed();
        let mark = fixture.high_water();
        GroupRegion { fixture, mark }
    }

    /// The boundary. A cell id below it belongs to the group; one at or above it
    /// belongs to whichever test is running.
    pub fn mark(&self) -> u32 {
        self.mark
    }

    pub fn is_empty(&self) -> bool {
        self.mark == 0
    }

    /// What a test runs against. Under the persistent world this is a fork, so
    /// it costs one pointer clone; under an arena it is the group's arena with
    /// a fresh one stacked on top. Neither copies the fixture.
    pub fn open(&self) -> World {
        self.fixture.fork()
    }

    /// Closes the test's region: everything it allocated is discarded, and the
    /// fixture keeps the writes the test made to it.
    ///
    /// The cheap case is the common one. A test that allocated nothing ends with
    /// a world that *is* the group region, so closing it is a pointer move. A
    /// test that allocated its own cells is rebuilt down to the mark, which
    /// costs the fixture's size and not the test's — and which is the price of
    /// the world being monotone, since it offers no way to drop a key. When
    /// ply-eval's arena lands this becomes a pointer reset at every size.
    pub fn close(&mut self, after: &World) {
        if after.high_water() == self.mark {
            self.fixture = after.fork();
            return;
        }
        // Ids are dense from zero and never removed, so re-allocating the cells
        // below the mark in ascending order reproduces exactly their ids — and
        // resets the allocator so the next test's region starts where this
        // one's did.
        let mut kept = World::new();
        for (id, value) in after.cells() {
            if id.0 >= self.mark {
                break;
            }
            kept.alloc(value.clone());
        }
        debug_assert_eq!(kept.high_water(), self.mark, "the fixture's ids are dense");
        self.fixture = kept;
    }

    /// The group's own state, for a caller that wants to look at it rather than
    /// run against it.
    pub fn fixture(&self) -> &World {
        &self.fixture
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_eval::{CellId, Value};

    fn seeded(cells: usize) -> World {
        let mut world = World::new();
        for i in 0..cells {
            world.alloc(Value::Int(i as i64));
        }
        world
    }

    fn int_of(world: &World, id: CellId) -> i64 {
        match world.get(id) {
            Some(Value::Int(i)) => *i,
            other => panic!("expected an Int in {id}, found {other:?}"),
        }
    }

    #[test]
    fn an_empty_region_has_no_fixture_and_no_mark() {
        let region = GroupRegion::empty();
        assert!(region.is_empty());
        assert_eq!(region.mark(), 0);
        assert!(region.open().is_empty());
    }

    /// The half that replaces the fork: a write a test makes to the fixture is
    /// there for the next test in the group.
    #[test]
    fn a_write_to_the_fixture_survives_the_test_that_made_it() {
        let mut region = GroupRegion::build(|| seeded(3));
        let mut world = region.open();
        assert!(world.set(CellId(1), Value::Int(42)));
        region.close(&world);

        assert_eq!(int_of(region.fixture(), CellId(1)), 42);
        assert_eq!(int_of(&region.open(), CellId(1)), 42);
        assert_eq!(region.mark(), 3, "a write allocates nothing");
    }

    /// The half that keeps tests from observing each other: what a test
    /// allocated is gone when it ends, and the next test allocates at the same
    /// ids rather than after them.
    #[test]
    fn what_a_test_allocated_is_gone_when_the_region_closes() {
        let mut region = GroupRegion::build(|| seeded(2));

        let mut first = region.open();
        let private = first.alloc(Value::str("first"));
        assert_eq!(private, CellId(2));
        region.close(&first);

        assert_eq!(region.mark(), 2);
        assert_eq!(region.fixture().len(), 2);
        assert!(!region.fixture().contains(private));

        let mut second = region.open();
        assert!(
            second.get(private).is_none(),
            "the next test must not be able to read the last one's cell"
        );
        assert_eq!(
            second.alloc(Value::str("second")),
            private,
            "the region reopens at the mark, so the ids start again"
        );
    }

    /// Both halves at once, which is where an implementation that kept the
    /// whole post-test world would pass the first two tests and fail this one.
    #[test]
    fn a_test_that_writes_and_allocates_leaves_only_the_write() {
        let mut region = GroupRegion::build(|| seeded(4));
        let mut world = region.open();
        world.alloc(Value::str("scratch"));
        assert!(world.set(CellId(0), Value::Int(-1)));
        world.alloc(Value::str("more scratch"));
        region.close(&world);

        assert_eq!(region.fixture().len(), 4);
        assert_eq!(region.mark(), 4);
        assert_eq!(int_of(region.fixture(), CellId(0)), -1);
        assert_eq!(int_of(region.fixture(), CellId(3)), 3);
    }

    /// A hundred tests in a group leave a fixture the size of the fixture. An
    /// implementation that merely stopped handing out old ids would grow here,
    /// which is the shape of leak this milestone exists to remove.
    #[test]
    fn a_long_group_does_not_grow_the_region() {
        let mut region = GroupRegion::build(|| seeded(8));
        for i in 0..100 {
            let mut world = region.open();
            for _ in 0..16 {
                world.alloc(Value::Int(i));
            }
            assert!(world.set(CellId(7), Value::Int(i)));
            region.close(&world);
            assert_eq!(region.fixture().len(), 8, "round {i}");
            assert_eq!(region.mark(), 8, "round {i}");
        }
        assert_eq!(int_of(region.fixture(), CellId(7)), 99);
    }

    /// Opening does not disturb the region, so two tests that ran against it
    /// out of order see the same thing. This is what makes a verdict independent
    /// of `--jobs`.
    #[test]
    fn opening_twice_gives_two_worlds_that_cannot_see_each_other() {
        let region = GroupRegion::build(|| seeded(2));
        let mut a = region.open();
        let mut b = region.open();
        assert!(a.set(CellId(0), Value::Int(1)));
        assert!(b.set(CellId(0), Value::Int(2)));
        assert_eq!(int_of(&a, CellId(0)), 1);
        assert_eq!(int_of(&b, CellId(0)), 2);
        assert_eq!(int_of(region.fixture(), CellId(0)), 0);
    }
}

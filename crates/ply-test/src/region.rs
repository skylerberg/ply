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

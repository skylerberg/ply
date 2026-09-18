//! The region a group of tests runs in, and the fixture that lives in it.

use ply_eval::{Arena, Fixture, TaskRegions, Value};

/// A group's shared fixture, and the mark separating it from what each test allocates on top.
#[derive(Clone, Debug, Default)]
pub struct GroupRegion {
    fixture: Fixture,
}

impl GroupRegion {
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

    pub fn mark(&self) -> usize {
        self.fixture.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fixture.is_empty()
    }

    /// A region stack seeded from the fixture and sealed at the mark, and the handle reaching it.
    #[must_use = "opening a region builds a stack; dropping it discards the seed"]
    pub fn open(&self) -> (TaskRegions, Value) {
        self.fixture.open()
    }

    /// Discards what the test allocated and keeps what it wrote to the fixture.
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

    pub fn fixture(&self) -> &Fixture {
        &self.fixture
    }
}

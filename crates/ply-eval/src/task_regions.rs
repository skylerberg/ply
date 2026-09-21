//! The region stack a task allocates in, and the fixture it starts from.

use crate::arena::{Arena, Pin, Reclaim, RegionId, RegionKind, Slot};
use crate::value::Value;
use ply_span::Span;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

/// Regions every stack holds from the moment it exists: the fixture's and the entry point's.
const FLOOR: usize = 2;

pub struct TaskRegions<V = Value> {
    arena: Arena<V>,
    root: RegionId,
    entry: RegionId,
    /// Shared with the [`Fixture`] it came from, so opening one copies no values.
    base: Rc<Vec<V>>,
    /// Lets a reset write the seed back through the same slots the handle names.
    base_slots: Vec<Slot>,
}

impl<V: Clone + Default> Default for TaskRegions<V> {
    fn default() -> TaskRegions<V> {
        TaskRegions::new()
    }
}

impl<V: Clone + Default> TaskRegions<V> {
    pub fn new() -> TaskRegions<V> {
        TaskRegions::from_values(Rc::new(Vec::new()))
    }

    fn from_values(base: Rc<Vec<V>>) -> TaskRegions<V> {
        let mut arena = Arena::new();
        // Both shared: a continuation captured across either may resume after its lexical close.
        let root = arena.open(RegionKind::Shared, Span::DUMMY);
        let base_slots = base
            .iter()
            .map(|value| {
                arena
                    .alloc(value.clone())
                    .expect("the root region is open, so an allocation cannot fail")
            })
            .collect();
        let entry = arena.open(RegionKind::Shared, Span::DUMMY);
        TaskRegions {
            arena,
            root,
            entry,
            base,
            base_slots,
        }
    }

    pub fn arena(&self) -> &Arena<V> {
        &self.arena
    }

    pub fn arena_mut(&mut self) -> &mut Arena<V> {
        &mut self.arena
    }

    /// Makes the current contents the fixture that [`TaskRegions::reset`] restores.
    pub fn seal(&mut self) {
        let base: Vec<V> = self.arena.slots().map(|(_, v)| v.clone()).collect();
        *self = TaskRegions::from_values(Rc::new(base));
    }

    pub fn reset(&mut self) {
        // Also closes regions abandoned by a handler that discarded its continuation.
        self.arena.close_final(self.entry);
        for (slot, value) in self.base_slots.iter().zip(self.base.iter()) {
            let restored = self.arena.set(*slot, value.clone());
            debug_assert!(restored, "the fixture's slots sit below every truncation");
        }
        self.entry = self.arena.open(RegionKind::Shared, Span::DUMMY);
        self.arena.clear_journal();
    }

    pub fn base_len(&self) -> usize {
        self.base.len()
    }

    /// The region the fixture lives in, which outlives every entry point.
    pub fn root(&self) -> RegionId {
        self.root
    }

    pub fn open_region(&mut self, kind: RegionKind, span: Span) -> RegionId {
        self.arena.open(kind, span)
    }

    pub fn close_region(&mut self, region: RegionId) -> Reclaim {
        self.arena.close(region)
    }

    /// A continuation's claim on every region open at this capture.
    pub fn pin(&mut self) -> Option<Pin> {
        if self.arena.depth() <= FLOOR {
            return None;
        }
        self.arena.pin()
    }

    pub fn close_program_regions(&mut self) {
        // Pins first: they are claims by control that will never run again.
        self.arena.abandon_pins();
        while self.arena.depth() > FLOOR {
            self.arena.close_current_final();
        }
    }

    /// Closes every region opened since the stack stood `depth` deep, for control that jumped
    /// back past their closes; a continuation captured across one still defers its slots.
    pub fn close_regions_above(&mut self, depth: usize) {
        while self.arena.depth() > depth.max(FLOOR) {
            self.arena.close_current();
        }
    }

    pub fn alloc_cell(&mut self, value: V) -> Slot {
        self.arena
            .alloc(value)
            .expect("a task's entry region is open for the whole of a run")
    }
}

impl<V: Clone + Default> Deref for TaskRegions<V> {
    type Target = Arena<V>;

    fn deref(&self) -> &Arena<V> {
        &self.arena
    }
}

impl<V: Clone + Default> DerefMut for TaskRegions<V> {
    fn deref_mut(&mut self) -> &mut Arena<V> {
        &mut self.arena
    }
}

impl<V: Clone + Default + fmt::Debug> fmt::Debug for TaskRegions<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.arena.slots().map(|(s, v)| (s.to_string(), v)))
            .finish()
    }
}

/// A seeded region stack and the value a test reaches it through.
#[derive(Clone, Debug)]
pub struct Fixture {
    values: Rc<Vec<Value>>,
    handle: Value,
}

impl Default for Fixture {
    fn default() -> Fixture {
        Fixture {
            values: Rc::new(Vec::new()),
            handle: Value::Unit,
        }
    }
}

impl Fixture {
    pub fn build(seed: impl FnOnce(&mut TaskRegions) -> Value) -> Fixture {
        let mut regions = TaskRegions::new();
        let handle = seed(&mut regions);
        Fixture::of(&regions, handle)
    }

    pub fn of(regions: &TaskRegions, handle: Value) -> Fixture {
        Fixture {
            values: Rc::new(regions.arena.slots().map(|(_, v)| v.clone()).collect()),
            handle,
        }
    }

    pub fn empty() -> Fixture {
        Fixture::default()
    }

    /// Sealed, so an entry point resets to the seed and not to nothing.
    #[must_use = "opening a fixture builds a region stack; dropping it discards the seed"]
    pub fn open(&self) -> (TaskRegions, Value) {
        (
            TaskRegions::from_values(Rc::clone(&self.values)),
            self.handle.clone(),
        )
    }

    pub fn handle(&self) -> &Value {
        &self.handle
    }

    /// The seeded cells in allocation order.
    pub fn values(&self) -> &[Value] {
        &self.values
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

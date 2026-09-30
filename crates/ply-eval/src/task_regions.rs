//! The cell arena an entry runs over: the fixture's region and the entry's, on the entry's own
//! stack, and every region the entry's stacks open over them.

use crate::arena::{Arena, Owner, RegionId, RegionKind, Slot};
use crate::value::Value;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

/// Regions the entry's own stack holds from the moment it exists: the fixture's and the entry's.
const FLOOR: usize = 2;

pub struct TaskRegions<V = Value> {
    arena: Arena<V>,
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
        arena.open(Owner::ENTRY, RegionKind::Shared);
        let base_slots = base
            .iter()
            .map(|value| {
                arena
                    .alloc(Owner::ENTRY, value.clone())
                    .expect("the root region is open, so an allocation cannot fail")
            })
            .collect();
        let entry = arena.open(Owner::ENTRY, RegionKind::Shared);
        TaskRegions {
            arena,
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
        self.close_program_regions();
        self.arena.close(self.entry);
        for (slot, value) in self.base_slots.iter().zip(self.base.iter()) {
            let restored = self.arena.set(*slot, value.clone());
            debug_assert!(restored, "the fixture's slots sit below every close");
        }
        self.entry = self.arena.open(Owner::ENTRY, RegionKind::Shared);
    }

    pub fn base_len(&self) -> usize {
        self.base.len()
    }

    /// Closes every region every stack opened, down to the fixture's and the entry's.
    pub fn close_program_regions(&mut self) {
        self.arena.close_all_but(Owner::ENTRY, FLOOR);
    }

    /// [`Arena::renew`] down to the fixture's region and the entry's, so refused while either
    /// holds a cell.
    pub fn renew(&mut self) -> bool {
        self.arena.renew(Owner::ENTRY, FLOOR)
    }

    /// Closes every region `owner` opened since it stood `depth` deep, for control that jumped
    /// back past their closes or a stack that will not run again.
    pub fn close_regions_above(&mut self, owner: Owner, depth: usize) {
        let floor = if owner == Owner::ENTRY { FLOOR } else { 0 };
        self.arena.close_above(owner, depth.max(floor));
    }

    /// A cell on the entry's own stack, in its innermost region.
    pub fn alloc_cell(&mut self, value: V) -> Slot {
        self.arena
            .alloc(Owner::ENTRY, value)
            .expect("the entry region is open for the whole of a run")
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

/// A seeded cell arena and the value a test reaches it through.
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
    #[must_use = "opening a fixture builds a cell arena; dropping it discards the seed"]
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

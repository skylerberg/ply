//! The region stack a task allocates in, and the fixture it starts from.

use crate::arena::{Arena, Pin, Reclaim, RegionId, RegionKind, Slot};
use crate::value::Value;
use ply_span::Span;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

/// Regions every stack holds from the moment it exists: the fixture's and the entry point's.
const FLOOR: usize = 2;

/// One task's region stack.
pub struct TaskRegions<V = Value> {
    arena: Arena<V>,
    /// Where the fixture lives.
    root: RegionId,
    /// Where an entry point allocates.
    entry: RegionId,
    /// The fixture as it was seeded, shared with the [`Fixture`] it came from so that opening one
    /// copies slots and not values.
    base: Rc<Vec<V>>,
    /// The slots holding `base`, so a reset can write the seed back through the very identities the
    /// handle names.
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
        // Both `shared`: a continuation may be captured across either and resumed after it, so
        // neither may hand its slots back at a lexical close.
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

    /// Makes everything the stack currently holds the fixture: what [`TaskRegions::reset`] goes
    /// back to.
    pub fn seal(&mut self) {
        let base: Vec<V> = self.arena.slots().map(|(_, v)| v.clone()).collect();
        *self = TaskRegions::from_values(Rc::new(base));
    }

    /// Back to the fixture, discarding everything the last entry point allocated.
    pub fn reset(&mut self) {
        // Closes every program region the run left open too — a handler that discarded its
        // continuation abandons them, and this is the only place left to reclaim them.
        self.arena.close_final(self.entry);
        for (slot, value) in self.base_slots.iter().zip(self.base.iter()) {
            let restored = self.arena.set(*slot, value.clone());
            debug_assert!(restored, "the fixture's slots sit below every truncation");
        }
        self.entry = self.arena.open(RegionKind::Shared, Span::DUMMY);
        self.arena.clear_journal();
    }

    /// Slots the fixture holds.
    pub fn base_len(&self) -> usize {
        self.base.len()
    }

    /// The region the fixture lives in, which outlives every entry point.
    pub fn root(&self) -> RegionId {
        self.root
    }

    /// Opens a program region of `kind`.
    pub fn open_region(&mut self, kind: RegionKind, span: Span) -> RegionId {
        self.arena.open(kind, span)
    }

    /// Closes a program region at its lexical end.
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

    /// Closes every region the program still has open, innermost first.
    pub fn close_program_regions(&mut self) {
        // Pins first: the run is over, so every claim a capture made is a claim by control that can
        // no longer run, and a region a dead claim covers would otherwise be retained until the
        // next reset.
        self.arena.abandon_pins();
        while self.arena.depth() > FLOOR {
            self.arena.close_current_final();
        }
    }

    /// Allocates in the innermost open region.
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
    /// Shared with every stack opened from it, so an open copies slots rather than values.
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
    /// Runs `seed` once against a fresh stack.
    pub fn build(seed: impl FnOnce(&mut TaskRegions) -> Value) -> Fixture {
        let mut regions = TaskRegions::new();
        let handle = seed(&mut regions);
        Fixture::of(&regions, handle)
    }

    /// The fixture a stack currently holds, with `handle` as its handle.
    pub fn of(regions: &TaskRegions, handle: Value) -> Fixture {
        Fixture {
            values: Rc::new(regions.arena.slots().map(|(_, v)| v.clone()).collect()),
            handle,
        }
    }

    pub fn empty() -> Fixture {
        Fixture::default()
    }

    /// A stack seeded exactly as the builder left it, sealed so that an entry point resets to the
    /// seed and not to nothing.
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

    /// The seeded cells in allocation order, for an engine that seeds its own arena rather than
    /// taking one.
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

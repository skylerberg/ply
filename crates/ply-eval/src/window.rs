//! The machine-owned slot stack.
//!
//! ADR 0034: the machine owns one stack of slots and an activation is a window into it; a frame
//! records a base index, not a scope, so carrying is free — and a value can be moved out of a
//! slot exactly because the array has one owner, the machine.
//!
//! Nothing here is addressed absolutely by anything a continuation can carry: frames and
//! segments record only *relative* quantities (window sizes and offsets from the top), so a
//! captured extent splices back at any height without rewriting a frame.

use crate::value::Value;

/// What one slot holds.
#[derive(Clone, Debug, Default)]
pub enum SlotVal {
    /// Reserved for a binder that has not run — an arm not taken, a binder further down the
    /// block, or a "binder" that is really a nullary constructor pattern. A read falls back to
    /// global resolution, which is what a name no local binding supplies means.
    #[default]
    Vacant,
    /// The last use moved the value out. A read is a defect in the liveness analysis, reported
    /// loudly rather than answered.
    Moved,
    Full(Value),
}

/// The slot stack and the current activation's base.
#[derive(Default)]
pub struct Windows {
    slots: Vec<SlotVal>,
    /// The current activation's first slot.
    pub base: usize,
}

impl Windows {
    pub fn new() -> Windows {
        Windows::default()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The current activation's window size, which every frame pushed now records as its
    /// caller-relative undo.
    pub fn window(&self) -> u32 {
        (self.slots.len() - self.base) as u32
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.base = 0;
    }

    /// Opens an activation of `size` slots on top, answering its base.
    pub fn enter(&mut self, size: u32) -> usize {
        let base = self.slots.len();
        self.slots
            .resize_with(base + size as usize, || SlotVal::Vacant);
        base
    }

    pub fn truncate(&mut self, to: usize) {
        self.slots.truncate(to);
    }

    /// The slot `at` of the current activation, read in place.
    pub fn read(&self, at: u32) -> &SlotVal {
        &self.slots[self.base + at as usize]
    }

    pub fn read_mut(&mut self, at: u32) -> &mut SlotVal {
        &mut self.slots[self.base + at as usize]
    }

    /// Moves the value out of slot `at`, leaving the slot [`SlotVal::Moved`]. A vacant slot stays
    /// vacant — there is nothing to mark as moved.
    pub fn take(&mut self, at: u32) -> SlotVal {
        let slot = &mut self.slots[self.base + at as usize];
        match slot {
            SlotVal::Full(_) => std::mem::replace(slot, SlotVal::Moved),
            SlotVal::Moved => SlotVal::Moved,
            SlotVal::Vacant => SlotVal::Vacant,
        }
    }

    pub fn write(&mut self, at: u32, value: Value) {
        self.slots[self.base + at as usize] = SlotVal::Full(value);
    }

    /// Cuts a captured extent's windows out: everything above `entry_top` leaves the stack
    /// whole, and `[floor, entry_top)` — the portion shared with the activation that pushed the
    /// captured prompt, which keeps running below — is cloned.
    pub fn cut(&mut self, floor: usize, entry_top: usize) -> Vec<SlotVal> {
        debug_assert!(floor <= entry_top && entry_top <= self.slots.len());
        let mut saved: Vec<SlotVal> = Vec::with_capacity(self.slots.len() - floor);
        saved.extend(self.slots[floor..entry_top].iter().cloned());
        saved.extend(self.slots.drain(entry_top..));
        saved
    }

    /// Restores a captured extent's windows on top of the stack. Cloned rather than moved,
    /// because a multi-shot continuation restores the same snapshot once per resumption.
    pub fn restore(&mut self, saved: &[SlotVal]) {
        self.slots.extend(saved.iter().cloned());
    }

    /// Takes the whole stack, for a control that becomes a task.
    pub fn drain_all(&mut self) -> Vec<SlotVal> {
        self.base = 0;
        std::mem::take(&mut self.slots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_move_leaves_the_slot_marked_and_a_vacant_slot_stays_vacant() {
        let mut w = Windows::new();
        w.enter(2);
        w.write(0, Value::Int(7));
        match w.take(0) {
            SlotVal::Full(Value::Int(7)) => {}
            other => panic!("expected the value, got {other:?}"),
        }
        assert!(matches!(w.read(0), SlotVal::Moved));
        assert!(matches!(w.take(1), SlotVal::Vacant));
        assert!(matches!(w.read(1), SlotVal::Vacant));
    }

    /// The overlap rule: the shared portion below the captured prompt's entry is cloned, and the
    /// extent above it is moved out — so the activation continuing below still reads its window,
    /// and the resumption reads the snapshot.
    #[test]
    fn a_cut_clones_the_shared_portion_and_moves_the_extent() {
        let mut w = Windows::new();
        w.enter(2);
        w.write(0, Value::Int(1));
        w.write(1, Value::Int(2));
        w.enter(1);
        w.write(2, Value::Int(3));
        let saved = w.cut(0, 2);
        assert_eq!(saved.len(), 3);
        assert_eq!(w.len(), 2, "the shared portion stays on the stack");
        w.restore(&saved);
        assert_eq!(w.len(), 5);
    }
}

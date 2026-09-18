//! The machine-owned slot stack; an activation is a window into it.
//! Frames record only relative offsets, so a captured extent splices back at any height.

use crate::value::Value;

#[derive(Clone, Debug, Default)]
pub enum SlotVal {
    /// A binder that has not run; a read falls back to global resolution.
    #[default]
    Vacant,
    /// Moved out by a last use; reading it is a liveness-analysis bug.
    Moved,
    Full(Value),
}

#[derive(Default)]
pub struct Windows {
    slots: Vec<SlotVal>,
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

    /// The current window size, which a pushed frame records as its caller-relative undo.
    pub fn window(&self) -> u32 {
        (self.slots.len() - self.base) as u32
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.base = 0;
    }

    pub fn enter(&mut self, size: u32) -> usize {
        let base = self.slots.len();
        self.slots
            .resize_with(base + size as usize, || SlotVal::Vacant);
        base
    }

    pub fn truncate(&mut self, to: usize) {
        self.slots.truncate(to);
    }

    pub fn read(&self, at: u32) -> &SlotVal {
        &self.slots[self.base + at as usize]
    }

    pub fn read_mut(&mut self, at: u32) -> &mut SlotVal {
        &mut self.slots[self.base + at as usize]
    }

    /// Moves the value out, leaving [`SlotVal::Moved`]; a vacant slot stays vacant.
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

    /// Cuts a captured extent out; `[floor, entry_top)` is cloned because the activation below
    /// keeps running on it.
    pub fn cut(&mut self, floor: usize, entry_top: usize) -> Vec<SlotVal> {
        debug_assert!(floor <= entry_top && entry_top <= self.slots.len());
        let mut saved: Vec<SlotVal> = Vec::with_capacity(self.slots.len() - floor);
        saved.extend(self.slots[floor..entry_top].iter().cloned());
        saved.extend(self.slots.drain(entry_top..));
        saved
    }

    /// Cloned rather than moved: a multi-shot continuation restores the same snapshot repeatedly.
    pub fn restore(&mut self, saved: &[SlotVal]) {
        self.slots.extend(saved.iter().cloned());
    }

    pub fn drain_all(&mut self) -> Vec<SlotVal> {
        self.base = 0;
        std::mem::take(&mut self.slots)
    }
}

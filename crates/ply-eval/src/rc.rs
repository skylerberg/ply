//! Reference counting for the values that outlive their region.

use crate::arena::Slot;
use crate::value::Value;
use ply_span::{Diagnostic, Span, Symbol, codes};
use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Own {
    /// The binding is read again later, so the read clones.
    #[default]
    Borrowed,
    /// The last use in the enclosing barrier: the read moves the value out of its slot.
    Owned,
}

/// Capture-against-carry counters.
pub mod census4 {
    use std::cell::Cell;
    thread_local! {
        pub static CARRIES: Cell<u64> = const { Cell::new(0) };
        pub static CAPTURES: Cell<u64> = const { Cell::new(0) };
        pub static CAPTURED_FRAMES: Cell<u64> = const { Cell::new(0) };
    }
    pub fn carry() {
        let _ = CARRIES.try_with(|c| c.set(c.get() + 1));
    }
    pub fn capture(frames: u64) {
        let _ = CAPTURES.try_with(|c| c.set(c.get() + 1));
        let _ = CAPTURED_FRAMES.try_with(|c| c.set(c.get() + frames));
    }
    pub fn read() -> (u64, u64, u64) {
        (
            CARRIES.try_with(|c| c.get()).unwrap_or(0),
            CAPTURES.try_with(|c| c.get()).unwrap_or(0),
            CAPTURED_FRAMES.try_with(|c| c.get()).unwrap_or(0),
        )
    }
    pub fn reset() {
        let _ = CARRIES.try_with(|c| c.set(0));
        let _ = CAPTURES.try_with(|c| c.set(0));
        let _ = CAPTURED_FRAMES.try_with(|c| c.set(0));
    }
}

/// A capture copies the windows it cuts, which is affordable only while carries outnumber it.
pub(crate) fn note_carry() {
    census4::carry();
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Stats {
    /// Reads of tracked bindings: the naive `dup` count.
    pub dup_sites: u64,
    /// Occurrences that still clone, because the binding is read again.
    pub dup_emitted: u64,
    /// Bindings introduced: the naive `drop` count.
    pub drop_sites: u64,
    /// Per-binding drops; always zero, since a scope ends with one window truncation.
    pub drop_emitted: u64,
    pub takes_attempted: u64,
    pub takes_moved: u64,
    /// Updates that answer a compound argument with one element changed.
    pub updates: u64,
    pub updates_in_place: u64,
    /// Slots copied by the updates that did not rewrite in place.
    pub elements_copied: u64,
    pub cycles: u64,
}

impl Stats {
    /// The fraction of the naive scheme's operations the pass removed.
    pub fn elided(&self) -> Option<f64> {
        let naive = self.dup_sites + self.drop_sites;
        if naive == 0 {
            return None;
        }
        let emitted = self.dup_emitted + self.drop_emitted;
        Some(1.0 - (emitted as f64 / naive as f64))
    }

    pub fn in_place(&self) -> Option<f64> {
        if self.updates == 0 {
            return None;
        }
        Some(self.updates_in_place as f64 / self.updates as f64)
    }
}

thread_local! {
    static COUNTERS: RefCell<Stats> = const { RefCell::new(Stats {
        dup_sites: 0,
        dup_emitted: 0,
        drop_sites: 0,
        drop_emitted: 0,
        takes_attempted: 0,
        takes_moved: 0,
        updates: 0,
        updates_in_place: 0,
        elements_copied: 0,
        cycles: 0,
    }) };
    static CYCLES: RefCell<Vec<Diagnostic>> = const { RefCell::new(Vec::new()) };
    /// Reported `(cell, site)` pairs, so one cycle warns once however often the write runs.
    static SEEN: RefCell<Vec<(Slot, Span)>> = const { RefCell::new(Vec::new()) };
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    static SITES: RefCell<FxHashMap<Span, SiteCount>> = RefCell::new(FxHashMap::default());
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct SiteCount {
    pub in_place: u64,
    pub copies: u64,
}

impl SiteCount {
    pub fn total(&self) -> u64 {
        self.in_place + self.copies
    }

    /// `None` when the site never ran, which is not zero.
    pub fn rate(&self) -> Option<f64> {
        if self.total() == 0 {
            return None;
        }
        Some(self.in_place as f64 / self.total() as f64)
    }
}

/// Arms or disarms per-site attribution of [`Stats::updates`], clearing the map either way.
pub fn record_sites(on: bool) {
    let _ = RECORDING.try_with(|c| c.set(on));
    let _ = SITES.try_with(|c| c.borrow_mut().clear());
}

pub fn sites() -> Vec<(Span, SiteCount)> {
    SITES
        .try_with(|c| c.borrow().iter().map(|(k, v)| (*k, *v)).collect())
        .unwrap_or_default()
}

fn bump(f: impl FnOnce(&mut Stats)) {
    let _ = COUNTERS.try_with(|c| f(&mut c.borrow_mut()));
}

/// What this thread has counted since the last [`reset`].
pub fn stats() -> Stats {
    COUNTERS.try_with(|c| *c.borrow()).unwrap_or_default()
}

pub fn reset() {
    let _ = COUNTERS.try_with(|c| *c.borrow_mut() = Stats::default());
    let _ = SITES.try_with(|c| c.borrow_mut().clear());
    let _ = CYCLES.try_with(|c| c.borrow_mut().clear());
    let _ = SEEN.try_with(|c| c.borrow_mut().clear());
}

pub fn note_update_of(in_place: bool, copied: usize, span: Span) {
    bump(|s| {
        s.updates += 1;
        s.updates_in_place += u64::from(in_place);
        s.elements_copied += copied as u64;
    });
    if RECORDING.try_with(Cell::get).unwrap_or(false) {
        let _ = SITES.try_with(|c| {
            let mut map = c.borrow_mut();
            let entry = map.entry(span).or_default();
            if in_place {
                entry.in_place += 1;
            } else {
                entry.copies += 1;
            }
        });
    }
}

pub fn take_cycles() -> Vec<Diagnostic> {
    // Slot positions restart at every entry point, so a later repeat is a new cycle.
    let _ = SEEN.try_with(|c| c.borrow_mut().clear());
    CYCLES
        .try_with(|c| std::mem::take(&mut *c.borrow_mut()))
        .unwrap_or_default()
}

pub(crate) fn cell_cycle(slot: Slot, value: &Value, span: Span) -> Option<Diagnostic> {
    if !value_reaches_cell(value, slot) {
        return None;
    }
    note_cell_cycle(slot, span)
}

/// Whether `v` reaches cell `slot`, within the walk's budget.
pub fn value_reaches_cell(v: &Value, slot: Slot) -> bool {
    let mut budget = CYCLE_WALK_BUDGET;
    reaches_cell(v, slot, 0, &mut budget)
}

/// Counts a found cell cycle and warns once per site.
pub fn note_cell_cycle(slot: Slot, span: Span) -> Option<Diagnostic> {
    bump(|s| s.cycles += 1);
    let seen = SEEN
        .try_with(|c| {
            let mut seen = c.borrow_mut();
            let known = seen.contains(&(slot, span));
            if !known {
                seen.push((slot, span));
            }
            known
        })
        .unwrap_or(false);
    let d = Diagnostic::warning(
        codes::REFERENCE_CYCLE,
        format!("cell {slot} is being made to contain itself"),
    )
    .primary(span, "this value reaches the cell it is stored in")
    .note("reference counting does not collect cycles, so this cell and everything it reaches stay allocated for the rest of the run")
    .note("break the cycle by storing the part that does not name the cell, or by clearing the cell before the run ends");
    if !seen {
        let _ = CYCLES.try_with(|c| c.borrow_mut().push(d.clone()));
    }
    Some(d)
}

const CYCLE_WALK_BUDGET: u32 = 256;

/// Depth- and node-bounded: the program chooses the shape, and the walk spends stack and time.
fn reaches_cell(v: &Value, slot: Slot, depth: usize, budget: &mut u32) -> bool {
    if depth >= crate::limit::MAX_VALUE_DEPTH || *budget == 0 {
        return false;
    }
    *budget -= 1;
    let next = depth + 1;
    match v {
        Value::Cell(other) => *other == slot,
        Value::List(xs) => xs.iter().any(|x| reaches_cell(x, slot, next, budget)),
        Value::Map(m) => m.iter().any(|(k, x)| {
            reaches_cell(k, slot, next, budget) || reaches_cell(x, slot, next, budget)
        }),
        Value::Record(fields) => fields.values().any(|x| reaches_cell(x, slot, next, budget)),
        Value::Ctor { args, .. } => args.iter().any(|x| reaches_cell(x, slot, next, budget)),
        Value::Secret(inner) => reaches_cell(inner, slot, next, budget),
        _ => false,
    }
}

/// The backward pass: bindings still read to the right, keyed per binding, never per field.
pub struct Live {
    later: Vec<Symbol>,
    /// Per barrier (lambda, clause, `simulate` body): every slot name it owns.
    ownable: Vec<Vec<Symbol>>,
}

impl Live {
    pub fn new(ownable: Vec<Symbol>) -> Live {
        Live {
            later: Vec::new(),
            ownable: vec![ownable],
        }
    }

    fn any_later(&self, name: &Symbol) -> bool {
        self.later.contains(name)
    }

    fn push(&mut self, name: &Symbol) {
        if !self.later.contains(name) {
            self.later.push(name.clone());
        }
    }

    /// Records a read and answers whether the value may be moved rather than cloned.
    pub fn use_of(&mut self, name: &Symbol) -> Own {
        let tracked = self.tracked(name);
        let last = !self.any_later(name);
        self.push(name);
        if tracked {
            bump(|s| {
                s.dup_sites += 1;
                s.dup_emitted += u64::from(!last);
            });
        }
        if last && tracked {
            Own::Owned
        } else {
            Own::Borrowed
        }
    }

    /// Only the current barrier's bindings are counted or moved.
    fn tracked(&self, name: &Symbol) -> bool {
        self.ownable
            .last()
            .is_some_and(|scope| scope.iter().any(|n| n == name))
    }

    pub fn is_live(&self, name: &Symbol) -> bool {
        self.any_later(name)
    }

    /// Crossing a binder, backwards: reads further left mean an outer binding of the same name.
    pub fn kill(&mut self, name: &Symbol) {
        self.later.retain(|u| u != name);
    }

    /// Enters a scope rebinding `binders`, returning the outer bindings' pending uses it hides.
    pub(crate) fn shadow(&mut self, binders: &[Symbol]) -> Vec<Symbol> {
        let held: Vec<Symbol> = self
            .later
            .iter()
            .filter(|u| binders.contains(u))
            .cloned()
            .collect();
        self.later.retain(|u| !binders.contains(u));
        held
    }

    pub(crate) fn snapshot(&self) -> Vec<Symbol> {
        self.later.clone()
    }

    pub(crate) fn restore(&mut self, uses: Vec<Symbol>) {
        self.later = uses;
    }

    pub(crate) fn union(&mut self, other: Vec<Symbol>) {
        for u in &other {
            self.push(u);
        }
    }

    /// Opens a barrier over `bound`, answering the live set to restore when it closes.
    pub(crate) fn open(&mut self, bound: Vec<Symbol>) -> Vec<Symbol> {
        self.ownable.push(bound);
        std::mem::take(&mut self.later)
    }

    /// Closes a barrier, replaying its free variables as reads and answering how each is taken.
    /// `movable` is false for captures taken at entry (handler clauses, `simulate`): they clone.
    pub(crate) fn close_with_owns(
        &mut self,
        outer: Vec<Symbol>,
        frees: &[Symbol],
        movable: bool,
    ) -> Vec<Own> {
        self.later = outer;
        self.ownable.pop();
        let mut owns = Vec::with_capacity(frees.len());
        for name in frees {
            let tracked = self.tracked(name);
            let moved = movable && tracked && !self.any_later(name);
            owns.push(if moved { Own::Owned } else { Own::Borrowed });
            if tracked {
                bump(|s| {
                    s.dup_sites += 1;
                    s.dup_emitted += u64::from(!moved);
                });
            }
            self.push(name);
        }
        owns
    }

    /// Counts the bindings a scope introduces, for the naive `drop` denominator.
    pub fn declare(&mut self, count: usize) {
        bump(|s| s.drop_sites += count as u64);
    }
}

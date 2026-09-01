//! Reference counting for the values that outlive their region.
//!
//! Perceus' operations, on the machine's slot calculus (ADR 0034): `dup` is a clone at a read
//! that is not the binding's last, `drop` is the window truncation at an activation's end, and a
//! *move* is a last use taking the value out of its slot. There is no fourth row: a pending frame
//! records a base index rather than holding a scope, so nothing owns "the scope" any more.

use crate::arena::Slot;
use crate::value::Value;
use ply_span::{Diagnostic, Span, Symbol, codes};
use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};

/// How a variable occurrence takes its value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Own {
    /// The binding is read again later, so the read clones — Perceus' `dup`.
    #[default]
    Borrowed,
    /// The last use of a binding of the enclosing barrier: the read moves the value out of its
    /// slot, leaving the slot empty.
    Owned,
    /// On a `Field` node whose base is a slot-resolved variable: the projection is the last use
    /// of this *field*, while other fields of the binding are still read later. The machine takes
    /// the field out of the record in place when the record is unshared — the fifth gate pair,
    /// which no release keyed by a name can reach.
    OwnedField,
}

/// Counters for the slot machine's capture-against-carry census. Diagnostics only.
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

/// A pending frame started waiting while a sub-expression runs — what the census counts against
/// captures. Under the slot machine a carry is a base index in the frame and costs nothing;
/// the census stays because the trade it guards is still the trade: a capture *copies* the
/// windows it cuts, and that is affordable only while carries outnumber captures.
pub(crate) fn note_carry() {
    census4::carry();
}

/// What the pass and the runtime counted.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Stats {
    /// Occurrences of a binding the pass tracks — Perceus' naive `dup` count, one per read.
    pub dup_sites: u64,
    /// Occurrences that still clone, because the binding is read again.
    pub dup_emitted: u64,
    /// Bindings introduced — the naive `drop` count, one per binding.
    pub drop_sites: u64,
    /// Per-binding drop operations the machine still runs. Zero since the slot rewrite: a scope's
    /// end is one window truncation, so no binding pays a drop of its own.
    pub drop_emitted: u64,
    /// Moves the machine attempted, at an `Owned` occurrence.
    pub takes_attempted: u64,
    /// Those that found a live value in the slot and moved it.
    pub takes_moved: u64,
    /// Updates of a compound value — an operation that answers the argument it was given with one
    /// element changed.
    pub updates: u64,
    /// Updates that rewrote the value rather than copying it.
    pub updates_in_place: u64,
    /// Elements copied by the updates that did not rewrite in place.
    ///
    /// The boolean above answers "did this append copy the whole list", which is the right question
    /// only while a copy is all-or-nothing. Under a chunked representation an append that cannot
    /// rewrite copies a path rather than an array, so the boolean would read `false` for something
    /// costing O(log n) and the rate would be uniformly bad while the program got faster. This
    /// counts what was actually copied, which is the question that survives the representation —
    /// ADR 0034's S5b.
    pub elements_copied: u64,
    /// Cycles reported by [`cell_cycle`].
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

    /// The fraction of updates that rewrote their argument in place.
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
    /// The `(cell, site)` pairs already reported, so that one cycle is one warning however many
    /// times the write runs.
    static SEEN: RefCell<Vec<(Slot, Span)>> = const { RefCell::new(Vec::new()) };
    /// Off unless [`record_sites`] armed it, and read once per update when it is off.
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    static SITES: RefCell<FxHashMap<Span, SiteCount>> = RefCell::new(FxHashMap::default());
}

/// What one `push` site did, over every time the corpus ran it.
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

/// Arms or disarms per-site attribution of [`Stats::updates`], clearing the map
/// in both directions so a measurement starts empty whatever ran before it.
pub fn record_sites(on: bool) {
    let _ = RECORDING.try_with(|c| c.set(on));
    let _ = SITES.try_with(|c| c.borrow_mut().clear());
}

/// What each `push` site has done since [`record_sites`] armed it.
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

/// Clears this thread's counters and the cycles it has reported.
pub fn reset() {
    let _ = COUNTERS.try_with(|c| *c.borrow_mut() = Stats::default());
    let _ = SITES.try_with(|c| c.borrow_mut().clear());
    let _ = CYCLES.try_with(|c| c.borrow_mut().clear());
    let _ = SEEN.try_with(|c| c.borrow_mut().clear());
}

pub(crate) fn note_take(moved: bool) {
    bump(|s| {
        s.takes_attempted += 1;
        s.takes_moved += u64::from(moved);
    });
}

pub(crate) fn note_update(in_place: bool, span: Span) {
    note_update_of(in_place, 0, span);
}

/// [`note_update`], with the number of elements the update had to copy.
pub(crate) fn note_update_of(in_place: bool, copied: usize, span: Span) {
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

/// Cycles reported so far, in the order they were built, and clears the list.
pub fn take_cycles() -> Vec<Diagnostic> {
    // The suppression list goes with them: a slot's position restarts at every entry point, so the
    // same `(cell, site)` pair in a later run is a second cycle rather than the first one again.
    let _ = SEEN.try_with(|c| c.borrow_mut().clear());
    CYCLES
        .try_with(|c| std::mem::take(&mut *c.borrow_mut()))
        .unwrap_or_default()
}

/// A cell that reaches itself, which nothing will ever reclaim.
pub(crate) fn cell_cycle(slot: Slot, value: &Value, span: Span) -> Option<Diagnostic> {
    let mut budget = CYCLE_WALK_BUDGET;
    if !reaches_cell(value, slot, 0, &mut budget) {
        return None;
    }
    bump(|s| s.cycles += 1);
    // One cycle, one warning.
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

/// What one `cell_set` may spend looking for a cycle.
const CYCLE_WALK_BUDGET: u32 = 256;

/// Depth-bounded exactly as `values_equal` is, and node-bounded on top of that: a value's shape is
/// the program's to choose, and this walk spends both the host's stack and the program's time.
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

/// One use the backward pass is still expecting to see, to the right of the point the walk has
/// reached: a whole read of a binding, or a read of one field of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Use {
    pub name: Symbol,
    /// `None` is a whole-value use; `Some(f)` a read of one field.
    pub field: Option<Symbol>,
}

impl Use {
    fn whole(name: &Symbol) -> Use {
        Use {
            name: name.clone(),
            field: None,
        }
    }
}

/// The backward pass: what is still live to the right of the point the walk has reached.
pub struct Live {
    later: Vec<Use>,
    /// One frame per barrier — a lambda, a handler clause, a `return` clause, a `simulate` body —
    /// holding every slot name of that barrier: captures, parameters and binders alike.
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
        self.later.iter().any(|u| &u.name == name)
    }

    fn push(&mut self, u: Use) {
        if !self.later.contains(&u) {
            self.later.push(u);
        }
    }

    /// Records a whole-value read and answers whether the value may be moved rather than cloned.
    pub fn use_of(&mut self, name: &Symbol) -> Own {
        let tracked = self.tracked(name);
        let last = !self.any_later(name);
        self.push(Use::whole(name));
        if tracked {
            bump(|s| {
                s.dup_sites += 1;
                s.dup_emitted += u64::from(!last);
            });
        }
        if last && tracked { Own::Owned } else { Own::Borrowed }
    }

    /// Records a read of one field, and answers what the projection may take: the whole value when
    /// nothing later reads the binding at all, the field alone when only *other* fields are read
    /// later, and a clone otherwise.
    pub fn use_field(&mut self, name: &Symbol, field: &Symbol) -> Own {
        let tracked = self.tracked(name);
        let whole_later = self
            .later
            .iter()
            .any(|u| &u.name == name && u.field.is_none());
        let field_later = self
            .later
            .iter()
            .any(|u| &u.name == name && u.field.as_ref() == Some(field));
        let any_later = self.any_later(name);
        let own = if !tracked {
            Own::Borrowed
        } else if !any_later {
            Own::Owned
        } else if !whole_later && !field_later {
            Own::OwnedField
        } else {
            Own::Borrowed
        };
        self.push(Use {
            name: name.clone(),
            field: Some(field.clone()),
        });
        if tracked {
            bump(|s| {
                s.dup_sites += 1;
                s.dup_emitted += u64::from(own == Own::Borrowed);
            });
        }
        own
    }

    /// Whether this name is a binding of the current barrier, which is the only thing the counts
    /// and the ownership answer are about.
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
        self.later.retain(|u| &u.name != name);
    }

    /// Enters a scope in which `binders` denote new bindings, answering the uses of an outer
    /// binding of one of those names that the rest of the activation still makes.
    pub(crate) fn shadow(&mut self, binders: &[Symbol]) -> Vec<Use> {
        let mut held = Vec::new();
        for u in &self.later {
            if binders.iter().any(|b| b == &u.name) && !held.contains(u) {
                held.push(u.clone());
            }
        }
        self.later.retain(|u| !binders.iter().any(|b| b == &u.name));
        held
    }

    pub(crate) fn snapshot(&self) -> Vec<Use> {
        self.later.clone()
    }

    pub(crate) fn restore(&mut self, uses: Vec<Use>) {
        self.later = uses;
    }

    /// Unions a branch's live set into the current one.
    pub(crate) fn union(&mut self, other: Vec<Use>) {
        for u in other {
            self.push(u);
        }
    }

    /// Opens a barrier over `bound`, answering the live set to restore with [`Live::close`].
    pub(crate) fn open(&mut self, bound: Vec<Symbol>) -> Vec<Use> {
        self.ownable.push(bound);
        std::mem::take(&mut self.later)
    }

    /// Closes a barrier, replaying its free variables as reads at the construct that captured
    /// them, and answering how the capture may take each: a clone while the enclosing activation
    /// still reads the name, a move when the capture is its last use.
    ///
    /// `movable` is false for a barrier whose capture runs *before* code lowered to its left in
    /// the walk — a handler clause or a `simulate` body, both captured at the construct's entry
    /// while the body between entry and any later read has yet to run. Those captures always
    /// clone.
    pub(crate) fn close_with_owns(
        &mut self,
        outer: Vec<Use>,
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
            self.push(Use::whole(name));
        }
        owns
    }

    /// Counts the bindings a scope introduces, for the naive `drop` denominator.
    pub fn declare(&mut self, count: usize) {
        bump(|s| s.drop_sites += count as u64);
    }
}

#[cfg(test)]
mod tests;

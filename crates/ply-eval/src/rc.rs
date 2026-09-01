//! Reference counting for the values that outlive their region — ADR 0017 §4.

use crate::arena::Slot;
use crate::env::Env;
use crate::value::Value;
use ply_span::{Diagnostic, Span, Symbol, codes};
use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// How a variable occurrence takes its value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Own {
    /// The binding is read again later, so the read clones — Perceus' `dup`.
    #[default]
    Borrowed,
    /// The last use of a binding introduced inside the enclosing barrier.
    Owned,
}

/// Whether ADR 0034 §11 S4's probe is armed, read once per process. Off by default: it is a probe
/// and not a landed change, and `Env::release` is O(scope depth) on the machine's hottest path, so
/// do not arm it in anything being timed.
pub fn probe_armed() -> bool {
    static ARMED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ARMED
        .get_or_init(|| std::env::var("PLY_ADR0034_PROBE").is_ok_and(|v| v == "1" || v == "params"))
}

/// Whether the probe's *carry-site* half is armed — the `App` and `Record` per-argument releases,
/// as against the statement-level parameter releases [`probe_armed`] gates.
pub fn probe_carries() -> bool {
    static ARMED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ARMED
        .get_or_init(|| std::env::var("PLY_ADR0034_PROBE").is_ok_and(|v| v == "1" || v == "carry"))
}

/// [`carry`], minus the bindings the sub-expression just started is the last reader of.
pub(crate) fn carry_released(env: &Env, remaining: bool, live: &[Symbol]) -> Env {
    if !remaining {
        return Env::empty();
    }
    // Empty means "hold what you hold today": lowering only asks for a narrowing where the
    // sub-expression appends, because the window costs a link per name kept.
    if !probe_carries() || live.is_empty() {
        return env.clone();
    }
    env.keep_only(live)
}

/// The scope a pending frame carries while the subexpression it is waiting for runs.
/// Counters for ADR 0034 §4's capture-against-carry census. Diagnostics only.
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

pub(crate) fn carry(env: &Env, remaining: bool) -> Env {
    census4::carry();
    if remaining { env.clone() } else { Env::empty() }
}

/// The bindings a statement's end kills, which the machine drops out of the scope its continuation
/// carries.
pub type Dead = Rc<[Symbol]>;

pub fn no_dead() -> Dead {
    Rc::from(Vec::new())
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
    /// Bindings named in a `dead` set, which is a `release` the machine runs.
    pub drop_emitted: u64,
    /// `take_unique` calls the machine attempted, at an `Owned` occurrence.
    pub takes_attempted: u64,
    /// Those that found the scope unshared and moved the value.
    pub takes_moved: u64,
    /// Updates of a compound value — an operation that answers the argument it was given with one
    /// element changed.
    pub updates: u64,
    /// Updates that rewrote the value rather than copying it.
    pub updates_in_place: u64,
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
    bump(|s| {
        s.updates += 1;
        s.updates_in_place += u64::from(in_place);
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

/// The backward pass: what is still live to the right of the point the walk has reached.
pub struct Live {
    later: Vec<Symbol>,
    /// One frame per barrier — a lambda, a handler clause, a `return` clause, a `simulate` body —
    /// holding every name bound anywhere inside it.
    ownable: Vec<Vec<Symbol>>,
    /// How many leading names of each `ownable` frame are that barrier's own
    /// **parameters**, which [`Live::barrier_params`] hands back.
    params: Vec<usize>,
}

impl Live {
    pub fn new(ownable: Vec<Symbol>) -> Live {
        Live {
            later: Vec::new(),
            ownable: vec![ownable],
            params: vec![0],
        }
    }

    /// The current barrier's parameters — ADR 0034 §11 S3 / ADR 0025 P2.
    pub fn barrier_params(&self) -> &[Symbol] {
        match (self.ownable.last(), self.params.last()) {
            (Some(scope), Some(&n)) => &scope[..n.min(scope.len())],
            _ => &[],
        }
    }

    /// Declares that the first `count` names of the current barrier's frame are
    /// its parameters. Called once, immediately after the frame is opened.
    pub fn params_are(&mut self, count: usize) {
        if let Some(last) = self.params.last_mut() {
            *last = count;
        }
    }

    /// Records a read and answers whether the value may be moved rather than cloned.
    pub fn use_of(&mut self, name: &Symbol) -> Own {
        let tracked = self.tracked(name);
        let last = !self.later.iter().any(|n| n == name);
        if last {
            self.later.push(name.clone());
        }
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

    /// Whether this name is a binding of the current barrier, which is the only thing the counts
    /// and the ownership answer are about.
    fn tracked(&self, name: &Symbol) -> bool {
        self.ownable
            .last()
            .is_some_and(|scope| scope.iter().any(|n| n == name))
    }

    /// Whether this name is a binding of the current barrier — [`Live::tracked`]
    /// in public form, for the ADR 0034 §11 S4 probe, which may only release a
    /// name whose last use this body can bound.
    pub fn is_ownable(&self, name: &Symbol) -> bool {
        self.tracked(name)
    }

    pub fn is_live(&self, name: &Symbol) -> bool {
        self.later.iter().any(|n| n == name)
    }

    /// Crossing a binder, backwards: reads further left mean an outer binding of the same name.
    pub fn kill(&mut self, name: &Symbol) {
        self.later.retain(|n| n != name);
    }

    /// Enters a scope in which `binders` denote new bindings, answering the ones the rest of the
    /// activation still reads.
    pub fn shadow(&mut self, binders: &[Symbol]) -> Vec<Symbol> {
        let mut held = Vec::new();
        for name in binders {
            if self.is_live(name) && !held.iter().any(|n| n == name) {
                held.push(name.clone());
            }
        }
        self.later.retain(|n| !binders.iter().any(|b| b == n));
        held
    }

    pub fn snapshot(&self) -> Vec<Symbol> {
        self.later.clone()
    }

    pub fn restore(&mut self, names: Vec<Symbol>) {
        self.later = names;
    }

    /// Unions a branch's live set into the current one.
    pub fn union(&mut self, other: Vec<Symbol>) {
        for name in other {
            if !self.is_live(&name) {
                self.later.push(name);
            }
        }
    }

    /// Opens a barrier over `bound`, answering the live set to restore with [`Live::close`].
    pub fn open(&mut self, bound: Vec<Symbol>) -> Vec<Symbol> {
        self.ownable.push(bound);
        self.params.push(0);
        std::mem::take(&mut self.later)
    }

    /// Closes a barrier.
    /// Closes a barrier, answering its **free variables** — the names still live inside it, which
    /// are exactly what a closure over it has to keep.
    pub fn close(&mut self, outer: Vec<Symbol>) -> Vec<Symbol> {
        let free = std::mem::replace(&mut self.later, outer);
        self.ownable.pop();
        self.params.pop();
        for name in &free {
            if !self.is_live(name) {
                self.later.push(name.clone());
            }
        }
        free
    }

    /// Counts the bindings a scope introduces, for the naive `drop` denominator.
    pub fn declare(&mut self, count: usize) {
        bump(|s| s.drop_sites += count as u64);
    }

    /// The names a statement's end kills, counted as the `drop`s the pass did emit.
    pub fn released(&mut self, dead: Vec<Symbol>) -> Dead {
        bump(|s| s.drop_emitted += dead.len() as u64);
        Rc::from(dead)
    }
}

#[cfg(test)]
mod tests;

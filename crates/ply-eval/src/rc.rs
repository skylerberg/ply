//! Reference counting for the values that outlive their region — ADR 0017 §4.
//!
//! The scheme is Perceus'. Every heap value carries a count; the *compiler*
//! decides where the count moves rather than the runtime discovering it; and an
//! operation on a value whose count is one rewrites that value in place instead
//! of copying it. The last of those is the whole point: an implementation that
//! counted correctly and never reached one would cost the counting and buy
//! nothing.
//!
//! ## What plays the part of each Perceus operation here
//!
//! Perceus is stated over a calculus whose variables are stack slots, so a last
//! use is a *move* out of a slot and a dead binding is an explicit `drop`. Ply's
//! machine has neither: a scope is a persistent `Rc` chain ([`crate::env::Env`])
//! that a closure, a continuation frame and the current evaluation all share by
//! pointer. So the operations are realized as:
//!
//! | Perceus | here |
//! | --- | --- |
//! | `dup x` at a non-last use | the [`Value`] clone `Var` already does |
//! | `drop x` when a binding dies | [`Env::release`], at the statement whose end kills it, applied to the *continuation's* scope only |
//! | a last use *moves* | [`Env::take_unique`], which empties the binding when this scope is provably its only owner |
//! | there is no fourth one | `carry`, because a *frame* holding a scope it will not read is an owner Perceus' calculus has no name for |
//!
//! [`Value`]: crate::Value
//! [`Env::release`]: crate::env::Env::release
//! [`Env::take_unique`]: crate::env::Env::take_unique
//!
//! That last row is the one that decided whether any of this was worth having.
//! With the three textbook operations in place and every pending frame still
//! carrying the caller's scope, **7.4%** of updates were in place across the
//! corpora on disk; not carrying a scope past the subexpressions that read it
//! took the same measurement to **75.3%**, with no change to the analysis at
//! all. `tests/reference_counting_cost.rs` is where both numbers are taken.
//!
//! ## Why the analysis cannot make a program mean something else
//!
//! It is stated plainly because it is the property that made this safe to build
//! against a shared, persistent environment:
//!
//! - **`take_unique` is guarded dynamically.** It empties a binding only when
//!   every link from the head of the chain down to that binding is uniquely
//!   owned, which means no closure, no captured continuation and no other scope
//!   can reach it — and the machine's `env` at a `Var` node is a by-value local
//!   it drops immediately after. A wrong `Owned` therefore costs a wasted walk,
//!   never a wrong answer. This is what makes multi-shot resumption safe: a
//!   resumed frame is *cloned* out of the continuation's shared segment, so its
//!   scope is shared, so nothing in it is ever taken.
//! - **`release` is functional.** It builds a new chain and never writes through
//!   a shared link, so a closure or a prompt that captured the old scope still
//!   sees every binding it captured. A wrong release can only be observed by the
//!   very continuation it was computed for, where it is [`codes::INTERNAL_ERROR`]
//!   naming the binding — loud, and never a silently different value.
//!
//! ## Cycles
//!
//! Not collected, per ADR 0017 §4. [`cell_cycle`] is the guard, and its
//! documentation is the argument that **no type-correct program reaches it
//! today** — an argument that belongs to the type system rather than to this
//! module, which is why it is a guard and a test rather than a deletion.

use crate::arena::Slot;
use crate::env::Env;
use crate::value::Value;
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::cell::RefCell;
use std::rc::Rc;

/// How a variable occurrence takes its value.
///
/// Only a `Var` node carries anything but [`Own::Borrowed`]. It is an
/// optimization hint and never a permission: see the module comment for why a
/// wrong `Owned` cannot change what a program means.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Own {
    /// The binding is read again later, so the read clones — Perceus' `dup`.
    #[default]
    Borrowed,
    /// The last use of a binding introduced inside the enclosing barrier. The
    /// machine may move the value out of the scope instead of cloning it.
    Owned,
}

/// The scope a pending frame carries while the subexpression it is waiting for
/// runs.
///
/// A frame needs a scope only for the subexpressions it has **not started**. It
/// is the same rule that makes `drop(env)` before a call the difference between
/// every argument being reused and none of them being: a frame that keeps a
/// scope it will not read holds every binding in that scope at two owners for
/// the whole of the last argument's evaluation, and the last argument is exactly
/// where the value an update could rewrite is produced. `push(acc, x)` nested in
/// a call reused nothing until this existed.
///
/// Correctness does not depend on the answer, only cost: the frames that take an
/// empty scope are the ones whose next step is to apply, build or perform, and
/// none of them looks at it.
pub(crate) fn carry(env: &Env, remaining: bool) -> Env {
    if remaining { env.clone() } else { Env::empty() }
}

/// The bindings a statement's end kills, which the machine drops out of the
/// scope its continuation carries.
pub type Dead = Rc<[Symbol]>;

pub fn no_dead() -> Dead {
    Rc::from(Vec::new())
}

/// What the pass and the runtime counted.
///
/// **Diagnostics only.** Nothing here enters a value, a hash, a cache key, a
/// footprint or a seeded choice, which is why a thread-local counter is an
/// honest place to keep it: two runs of one program answer identically whatever
/// these say.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Stats {
    /// Occurrences of a binding the pass tracks — Perceus' naive `dup` count,
    /// one per read.
    pub dup_sites: u64,
    /// Occurrences that still clone, because the binding is read again.
    pub dup_emitted: u64,
    /// Bindings introduced — the naive `drop` count, one per binding.
    pub drop_sites: u64,
    /// Bindings named in a `dead` set, which is a `release` the machine runs.
    /// The rest are elided: their scope's end frees them with no operation.
    pub drop_emitted: u64,
    /// `take_unique` calls the machine attempted, at an `Owned` occurrence.
    pub takes_attempted: u64,
    /// Those that found the scope unshared and moved the value.
    pub takes_moved: u64,
    /// Updates of a compound value — an operation that answers the argument it
    /// was given with one element changed.
    pub updates: u64,
    /// Updates that rewrote the value rather than copying it.
    pub updates_in_place: u64,
    /// Cycles reported by [`cell_cycle`].
    pub cycles: u64,
}

impl Stats {
    /// The fraction of the naive scheme's operations the pass removed.
    ///
    /// `None` when there were none to remove, which is not zero: a program with
    /// no bindings has elided nothing and a percentage would be a lie.
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
    /// The `(cell, site)` pairs already reported, so that one cycle is one
    /// warning however many times the write runs.
    static SEEN: RefCell<Vec<(Slot, Span)>> = const { RefCell::new(Vec::new()) };
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
    let _ = CYCLES.try_with(|c| c.borrow_mut().clear());
    let _ = SEEN.try_with(|c| c.borrow_mut().clear());
}

pub(crate) fn note_take(moved: bool) {
    bump(|s| {
        s.takes_attempted += 1;
        s.takes_moved += u64::from(moved);
    });
}

pub(crate) fn note_update(in_place: bool) {
    bump(|s| {
        s.updates += 1;
        s.updates_in_place += u64::from(in_place);
    });
}

/// Cycles reported so far, in the order they were built, and clears the list.
///
/// Evaluation order is deterministic, so this sequence is a function of the
/// program and its seed like every other artifact the run produces.
pub fn take_cycles() -> Vec<Diagnostic> {
    // The suppression list goes with them: a slot's position restarts at every
    // entry point, so the same `(cell, site)` pair in a later run is a second
    // cycle rather than the first one again.
    let _ = SEEN.try_with(|c| c.borrow_mut().clear());
    CYCLES
        .try_with(|c| std::mem::take(&mut *c.borrow_mut()))
        .unwrap_or_default()
}

/// A cell that reaches itself, which nothing will ever reclaim.
///
/// ADR 0017 §4 accepts that a cycle among escaped values leaks and asks that the
/// diagnostics say so where one is constructible. This is that place, and the
/// honest account of what it covers is short:
///
/// - A `Value` compound is an `Arc` and a [`Value::Cell`] is a **slot** in a
///   region rather than a pointer into it, so no cycle of `Arc`s is
///   reachable from a Ply program: a closure's scope is captured before the
///   binding that would name the closure exists, and a top-level function is
///   resolved by name against an empty scope.
/// - The one shape that would still leak is a cell whose contents reach the
///   cell — `cell_set(c, [c])` — and **no type-correct program writes it
///   today**. It needs a `Cell<T>` inside `T`: as a list, a map or a record that
///   is the infinite type the occurs check refuses, and as a declared variant it
///   is `REGION_ESCAPE` at the declaration, because a declared field's region
///   would be pinned by whichever cell reached it first.
///   `tests/reference_cycles.rs` pins both refusals, so the day either moves is
///   the day this stops being unreachable.
///
/// So it is a guard rather than a diagnostic anybody sees, and it stays because
/// its unreachability is a property of the *type system* rather than of this
/// module. If it does fire it is a warning and not an error: refusing the write
/// would change what a legal program means, which ADR 0017's governing property
/// forbids.
///
/// Bounded by nodes as well as by depth, because it runs on every `cell_set` and
/// a handler storing a long list would otherwise pay a walk of that list per
/// write. Past either bound the answer is "no cycle found", which under-reports
/// rather than charging the program for the check.
pub(crate) fn cell_cycle(slot: Slot, value: &Value, span: Span) -> Option<Diagnostic> {
    let mut budget = CYCLE_WALK_BUDGET;
    if !reaches_cell(value, slot, 0, &mut budget) {
        return None;
    }
    bump(|s| s.cycles += 1);
    // One cycle, one warning. `--engine both` runs the write on two evaluators
    // and a multi-shot handler runs it once per resumption; each of those is the
    // same cell closed at the same site, not a second thing to go and look at.
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
///
/// Small, because the walk is on the write path of every state handler and the
/// shape it looks for names a cell within a step or two of the value's root. A
/// cycle buried under hundreds of nodes goes unreported; a `cell_set` of a
/// ten-thousand-element list costs a bounded walk instead of a linear one.
const CYCLE_WALK_BUDGET: u32 = 256;

/// Depth-bounded exactly as `values_equal` is, and node-bounded on top of that:
/// a value's shape is the program's to choose, and this walk spends both the
/// host's stack and the program's time.
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

/// The backward pass: what is still live to the right of the point the walk has
/// reached.
///
/// Lowering runs it, because lowering is the one traversal of a body the machine
/// already pays for and a second one would cost a definition's shape twice. The
/// walk visits children in **reverse evaluation order**, so when it reaches an
/// occurrence, `later` holds exactly the names the rest of the activation still
/// reads.
pub struct Live {
    later: Vec<Symbol>,
    /// One frame per barrier — a lambda, a handler clause, a `return` clause, a
    /// `simulate` body — holding every name bound anywhere inside it.
    ///
    /// Only a name bound inside the *current* barrier may be owned. A free
    /// variable of a closure is reachable from the closure's captured scope for
    /// as long as the closure lives, which no analysis of this body can bound.
    ownable: Vec<Vec<Symbol>>,
}

impl Live {
    pub fn new(ownable: Vec<Symbol>) -> Live {
        Live {
            later: Vec::new(),
            ownable: vec![ownable],
        }
    }

    /// Records a read and answers whether the value may be moved rather than
    /// cloned.
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

    /// Whether this name is a binding of the current barrier, which is the only
    /// thing the counts and the ownership answer are about.
    fn tracked(&self, name: &Symbol) -> bool {
        self.ownable
            .last()
            .is_some_and(|scope| scope.iter().any(|n| n == name))
    }

    pub fn is_live(&self, name: &Symbol) -> bool {
        self.later.iter().any(|n| n == name)
    }

    /// Crossing a binder, backwards: reads further left mean an outer binding of
    /// the same name.
    ///
    /// Pair it with [`Live::shadow`] wherever the binder can shadow, and hand
    /// the answer back with [`Live::union`] at the crossing.
    pub fn kill(&mut self, name: &Symbol) {
        self.later.retain(|n| n != name);
    }

    /// Enters a scope in which `binders` denote new bindings, answering the ones
    /// the rest of the activation still reads.
    ///
    /// The live set is keyed by name, so a binder that shadows an outer binding
    /// of the same name would otherwise make one entry stand for two. Taking the
    /// name out here is what lets a read inside the scope be a last use of the
    /// *inner* binding; handing it back at the binder the walk crosses is what
    /// keeps the outer one live, and skipping that is not a missed optimization
    /// — a still-read binding lands in a `dead` set and the machine releases it
    /// out from under the read.
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

    /// Unions a branch's live set into the current one. Two arms of a `match`
    /// each get the live set the whole `match` was entered with, and what
    /// survives is the union — a name read in one arm only is a last use *in
    /// that arm*, which is exactly the per-branch drop Perceus inserts.
    pub fn union(&mut self, other: Vec<Symbol>) {
        for name in other {
            if !self.is_live(&name) {
                self.later.push(name);
            }
        }
    }

    /// Opens a barrier over `bound`, answering the live set to restore with
    /// [`Live::close`].
    ///
    /// The barrier's own live set starts empty: nothing to the right of the
    /// lambda is a read of anything inside its body.
    pub fn open(&mut self, bound: Vec<Symbol>) -> Vec<Symbol> {
        self.ownable.push(bound);
        std::mem::take(&mut self.later)
    }

    /// Closes a barrier. The names still live inside it are its free variables,
    /// and they become reads *at* the construct that captured them — never last
    /// ones, because the closure outlives this point by an amount no analysis
    /// here can bound.
    pub fn close(&mut self, outer: Vec<Symbol>) {
        let free = std::mem::replace(&mut self.later, outer);
        self.ownable.pop();
        for name in free {
            if !self.is_live(&name) {
                self.later.push(name);
            }
        }
    }

    /// Counts the bindings a scope introduces, for the naive `drop` denominator.
    pub fn declare(&mut self, count: usize) {
        bump(|s| s.drop_sites += count as u64);
    }

    /// The names a statement's end kills, counted as the `drop`s the pass did
    /// emit.
    pub fn released(&mut self, dead: Vec<Symbol>) -> Dead {
        bump(|s| s.drop_emitted += dead.len() as u64);
        Rc::from(dead)
    }
}

#[cfg(test)]
mod tests;

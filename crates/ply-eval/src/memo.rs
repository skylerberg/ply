//! What a nullary pure definition evaluated to, remembered for the run.
//!
//! A definition with no parameters and an empty published row is a constant:
//! nothing it can reach reads the world, so its value is a function of the
//! program alone. That is the same fact content addressing rests on, and it
//! makes re-evaluating one per call redundant work rather than an
//! optimization opportunity that has to be argued for.
//!
//! Both engines consult this, because a memo one engine keeps and the other
//! does not is a resource bound the two disagree on, and `--engine both` would
//! be reporting the disagreement as `E0503`.
//!
//! Three rules keep it honest. The published row is what is read — the
//! reviewable artifact, not the inferred body row — so a definition annotated
//! wider than it performs is left alone rather than quietly treated as pure.
//! The **first** completed evaluation wins: a value reached by resuming a
//! continuation captured inside the body is a resumption's value, not the
//! definition's. And the value has to be **world-independent**, which an empty
//! row does not imply: `with_cell` discharges its own atoms, so a definition
//! that allocates a cell and hands out something reaching it publishes `{}` and
//! is a constant by the row rule while its value is a key into *this run's*
//! world. Memoizing one hands the next test a cell its world never allocated —
//! `E0505` when the id is not there and a silently wrong read when it is, which
//! is the cross-test interference the whole isolation story rests on not
//! happening.

use crate::value::{ClosureKind, Value};
use ply_core::CheckOutput;
use ply_span::Symbol;
use rustc_hash::FxHashMap;

enum Slot {
    /// Not a constant, or the engine has no `CheckOutput` to tell from.
    Never,
    /// A constant whose value no call has produced yet.
    Pending,
    Known(Value),
}

/// What a caller should do with a nullary call it is about to enter.
pub(crate) enum Lookup {
    /// Enter it as any other call.
    Ignore,
    /// Enter it, and hand the value back to [`Memo::remember`].
    Remember,
    Known(Value),
}

#[derive(Default)]
pub(crate) struct Memo {
    slots: FxHashMap<Symbol, Slot>,
}

impl Memo {
    pub(crate) fn lookup(&mut self, check: Option<&CheckOutput>, name: &Symbol) -> Lookup {
        if let Some(slot) = self.slots.get(name) {
            return match slot {
                Slot::Never => Lookup::Ignore,
                Slot::Pending => Lookup::Remember,
                Slot::Known(value) => Lookup::Known(value.clone()),
            };
        }
        if !pure_by_published_row(check, name) {
            self.slots.insert(name.clone(), Slot::Never);
            return Lookup::Ignore;
        }
        self.slots.insert(name.clone(), Slot::Pending);
        Lookup::Remember
    }

    pub(crate) fn remember(&mut self, name: &Symbol, value: &Value) {
        if let Some(slot @ Slot::Pending) = self.slots.get_mut(name) {
            *slot = if world_independent(value, 0) {
                Slot::Known(value.clone())
            } else {
                Slot::Never
            };
        }
    }
}

/// Whether the value means the same thing in a world it was not produced in.
///
/// A [`Value::Cell`] and a [`Value::Task`] are keys into the run that made them,
/// so neither may be remembered; a closure is walked, because what it captured
/// is what it can hand out later. Everything this cannot decide — a captured
/// continuation, whose whole control stack would have to be walked, or a value
/// nested deeper than the budget — answers `false`, which costs a re-evaluation
/// and never a wrong read.
fn world_independent(value: &Value, depth: u32) -> bool {
    /// Deep enough for any value a constant plausibly builds, and finite so a
    /// cyclic value (`reference_cycles.rs`) terminates rather than recursing.
    const MAX_DEPTH: u32 = 64;

    if depth >= MAX_DEPTH {
        return false;
    }
    let deeper = depth + 1;
    match value {
        Value::Int(_)
        | Value::Bool(_)
        | Value::Float(_)
        | Value::Decimal(_)
        | Value::Str(_)
        | Value::Bytes(_)
        | Value::Unit => true,
        Value::Cell(_) | Value::Task(_) | Value::Continuation(_) => false,
        Value::List(items) => items.iter().all(|v| world_independent(v, deeper)),
        Value::Map(map) => map
            .iter()
            .all(|(k, v)| world_independent(k, deeper) && world_independent(v, deeper)),
        Value::Record(fields) => fields.values().all(|v| world_independent(v, deeper)),
        Value::Ctor { args, .. } => args.iter().all(|v| world_independent(v, deeper)),
        Value::Secret(inner) => world_independent(inner, deeper),
        Value::Closure(closure) => match &closure.kind {
            ClosureKind::Ctor { .. } | ClosureKind::Builtin(_) => true,
            ClosureKind::Fn { env, .. } | ClosureKind::Code { env, .. } => {
                env.values().all(|v| world_independent(v, deeper))
            }
        },
    }
}

/// Whether `name`'s *published* row claims it reads nothing of the world.
///
/// For the memo the caller has already established that the closure takes no
/// parameters; this is the rest of it. It is also the necessary — never
/// sufficient — purity gate `Machine::compiled_answer` reads before entering a
/// compiled body: see this module's note for what an empty row still permits,
/// and `compiled.rs` for why an allocation it still permits is unobservable
/// outside a `simulate` region.
///
/// `constraints` disqualifies a `where derivable(..)` definition. Evaluation is
/// type-erased, so such a definition is a constant too, but the exclusion costs
/// nothing and keeps the rule readable as a sentence about the signature.
pub(crate) fn pure_by_published_row(check: Option<&CheckOutput>, name: &Symbol) -> bool {
    check
        .and_then(|check| check.defs.get(name))
        .is_some_and(|def| def.footprint.is_empty() && def.constraints.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name() -> Symbol {
        Symbol::new("m.table")
    }

    #[test]
    fn without_a_check_output_nothing_is_a_constant() {
        let mut memo = Memo::default();
        assert!(matches!(memo.lookup(None, &name()), Lookup::Ignore));
        memo.remember(&name(), &Value::Int(1));
        assert!(matches!(memo.lookup(None, &name()), Lookup::Ignore));
    }

    /// The slot is reached without a `CheckOutput` by seeding it directly,
    /// because what is under test is the state machine and not the rule.
    #[test]
    fn a_pending_slot_is_filled_once_and_the_first_value_wins() {
        let mut memo = Memo::default();
        memo.slots.insert(name(), Slot::Pending);
        assert!(matches!(memo.lookup(None, &name()), Lookup::Remember));
        memo.remember(&name(), &Value::Int(7));
        memo.remember(&name(), &Value::Int(9));
        match memo.lookup(None, &name()) {
            Lookup::Known(value) => assert_eq!(value, Value::Int(7)),
            _ => panic!("a filled slot answers with its value"),
        }
    }

    #[test]
    fn a_slot_that_was_refused_is_never_filled_by_a_later_return() {
        let mut memo = Memo::default();
        memo.slots.insert(name(), Slot::Never);
        memo.remember(&name(), &Value::Int(7));
        assert!(matches!(memo.lookup(None, &name()), Lookup::Ignore));
    }
}

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
//! Two rules keep it honest. The published row is what is read — the
//! reviewable artifact, not the inferred body row — so a definition annotated
//! wider than it performs is left alone rather than quietly treated as pure.
//! And the **first** completed evaluation wins: a value reached by resuming a
//! continuation captured inside the body is a resumption's value, not the
//! definition's.

use crate::value::Value;
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
        if !is_constant(check, name) {
            self.slots.insert(name.clone(), Slot::Never);
            return Lookup::Ignore;
        }
        self.slots.insert(name.clone(), Slot::Pending);
        Lookup::Remember
    }

    pub(crate) fn remember(&mut self, name: &Symbol, value: &Value) {
        if let Some(slot @ Slot::Pending) = self.slots.get_mut(name) {
            *slot = Slot::Known(value.clone());
        }
    }
}

/// The caller has already established that the closure takes no parameters;
/// this is the rest of it.
///
/// `constraints` disqualifies a `where derivable(..)` definition. Evaluation is
/// type-erased, so such a definition is a constant too, but the exclusion costs
/// nothing and keeps the rule readable as a sentence about the signature.
fn is_constant(check: Option<&CheckOutput>, name: &Symbol) -> bool {
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

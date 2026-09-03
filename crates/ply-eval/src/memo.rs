//! What a nullary pure definition evaluated to, remembered for the run.

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
            *slot = if world_independent(value) {
                Slot::Known(value.clone())
            } else {
                Slot::Never
            };
        }
    }
}

/// Whether the value means the same thing in a world it was not produced in — what a
/// remembered constant must be, whichever engine produced it.
///
/// Walked with an explicit stack: a constant that holds a whole program's syntax tree is far
/// deeper than the Rust stack should be asked to recurse. A cycle can only run through a cell
/// (`reference_cycles.rs`), which the walk refuses without following, so it terminates.
pub fn world_independent(value: &Value) -> bool {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Int(_)
            | Value::Fixed(_)
            | Value::Bool(_)
            | Value::Float(_)
            | Value::Decimal(_)
            | Value::Str(_)
            | Value::Bytes(_)
            | Value::Unit => {}
            Value::Cell(_) | Value::Task(_) | Value::Continuation(_) => return false,
            Value::List(items) => pending.extend(items.iter()),
            Value::Map(map) => {
                for (k, v) in map.iter() {
                    pending.push(k);
                    pending.push(v);
                }
            }
            Value::Record(fields) => pending.extend(fields.values()),
            Value::Ctor { args, .. } => pending.extend(args.iter()),
            Value::Secret(inner) => pending.push(inner),
            Value::Closure(closure) => match &closure.kind {
                ClosureKind::Ctor { .. } | ClosureKind::Builtin(_) => {}
                ClosureKind::Native { captured, .. } => pending.extend(captured.iter()),
                ClosureKind::Code { captured, .. } => pending.extend(captured.iter()),
                ClosureKind::Fn { bindings, .. } => pending.extend(bindings.iter().map(|(_, v)| v)),
            },
        }
    }
    true
}

/// Whether `name`'s *published* row claims it reads nothing of the world.
pub fn pure_by_published_row(check: Option<&CheckOutput>, name: &Symbol) -> bool {
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

    /// The slot is reached without a `CheckOutput` by seeding it directly, because what is under
    /// test is the state machine and not the rule.
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

    fn nested(levels: usize, bottom: Value) -> Value {
        (0..levels).fold(bottom, |inner, _| Value::Ctor {
            name: Symbol::new("Node"),
            args: std::sync::Arc::new(vec![Value::list(vec![inner])]),
        })
    }

    /// A parsed program is a constant far deeper than any recursion budget; the walk must
    /// reach its bottom either way, since what is there decides the answer.
    #[test]
    fn a_constant_is_judged_by_its_leaves_however_deep_they_lie() {
        assert!(world_independent(&nested(2_000, Value::Unit)));
        let mut regions = crate::TaskRegions::new();
        let cell = Value::Cell(regions.alloc_cell(Value::Unit));
        assert!(!world_independent(&nested(2_000, cell)));
    }
}

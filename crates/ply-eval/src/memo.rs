//! What a nullary pure definition evaluated to, remembered for the run.

use crate::value::{ClosureKind, Value};
use ply_core::CheckOutput;
use ply_span::Symbol;

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
        let mut regions: crate::TaskRegions = crate::TaskRegions::new();
        let cell = Value::Cell(regions.alloc_cell(Value::Unit));
        assert!(!world_independent(&nested(2_000, cell)));
    }
}

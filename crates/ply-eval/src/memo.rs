//! What a nullary pure definition evaluated to, remembered for the run.

use crate::value::{ClosureKind, Value};

/// Whether the value means the same thing in a world it was not produced in.
pub fn world_independent(value: &Value) -> bool {
    // Explicit stack, since a constant may hold a whole syntax tree; any cycle ends at a cell.
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Int(_)
            | Value::Fixed(_)
            | Value::Char(_)
            | Value::Bool(_)
            | Value::Float(_)
            | Value::Decimal(_)
            | Value::Str(_)
            | Value::Bytes(_)
            | Value::Unit => {}
            Value::Cell(_) | Value::Task(_) | Value::Chan(_) => return false,
            Value::List(items) => pending.extend(items.iter()),
            Value::Array(items) => pending.extend(items.iter()),
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
                ClosureKind::Continuation { .. } => return false,
                ClosureKind::Synth { rule, .. } => pending.extend(rule.values()),
            },
        }
    }
    true
}

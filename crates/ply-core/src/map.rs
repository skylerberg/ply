//! The `Map` builtins.

use crate::value::Value;
use crate::{Diagnostic, Span};

pub(crate) fn new() -> Value {
    Value::empty_map()
}

pub(crate) fn len(m: &Value, span: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Int(m.as_map(span, "`Map::len`")?.len() as i64))
}

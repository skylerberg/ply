//! Ply values, as a lent effect hands them to the program.

use ply_core::{Symbol, Value as PlyValue};
use std::sync::Arc;

/// `Value::Record` holds an `Arc`, and its fields are not `Send`; every construction site says so.
#[allow(clippy::arc_with_non_send_sync)]
pub fn record(fields: Vec<(&str, PlyValue)>) -> PlyValue {
    PlyValue::Record(Arc::new(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    ))
}

pub fn option(value: Option<PlyValue>) -> PlyValue {
    match value {
        Some(value) => PlyValue::ctor("Some", vec![value]),
        None => PlyValue::ctor("None", Vec::new()),
    }
}

pub fn count(n: usize) -> PlyValue {
    PlyValue::Int(n as i64)
}

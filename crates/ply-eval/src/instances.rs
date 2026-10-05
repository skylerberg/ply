//! What a nominal type's module states its values go through, asked of the unit an entry runs:
//! the key two values are compared, ordered and hashed by, and what `show` writes of one. A type
//! is found by its constructor, so every reader of a value asks here and no caller passes it.

use crate::{Symbol, Value};
use std::cell::Cell;

/// The unit running on this thread, as a reader of its values asks it. `unit` is its own to read.
#[derive(Clone, Copy)]
pub struct Instances {
    pub unit: *mut (),
    /// Whether the type this constructor belongs to states a `key`.
    pub keyed: fn(*mut (), &Symbol) -> bool,
    /// What the type's `key` answers for this value; `None` where it states none, or the call
    /// failed, which the unit then holds as the entry's failure.
    pub key: fn(*mut (), &Value) -> Option<Value>,
    /// The text the type's `show` writes of this value; `None` as for `key`.
    pub shown: fn(*mut (), &Value) -> Option<Value>,
}

thread_local! {
    static RUNNING: Cell<Option<Instances>> = const { Cell::new(None) };
}

/// Sets what the entry beginning on this thread answers, and hands back what the one around it
/// did, which its end puts back.
pub fn swap(next: Option<Instances>) -> Option<Instances> {
    RUNNING.with(|r| r.replace(next))
}

/// The running unit, for a reader of words rather than values.
pub fn running() -> Option<Instances> {
    RUNNING.with(Cell::get)
}

pub(crate) fn keyed(ctor: &Symbol) -> bool {
    running().is_some_and(|i| (i.keyed)(i.unit, ctor))
}

/// `v` is a constructor's value.
pub(crate) fn key(v: &Value) -> Option<Value> {
    let i = running()?;
    (i.key)(i.unit, v)
}

/// Both keys, when the two are of a type that states one.
pub(crate) fn keys(a: &Value, b: &Value) -> Option<(Value, Value)> {
    let i = running()?;
    let x = (i.key)(i.unit, a)?;
    Some((x, (i.key)(i.unit, b)?))
}

pub(crate) fn shown(v: &Value) -> Option<Value> {
    let i = running()?;
    (i.shown)(i.unit, v)
}

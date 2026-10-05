//! What a nominal type's module states its values go through, asked of the unit an entry runs:
//! the key two values are compared, ordered and hashed by, what `show` writes of one, and the
//! arithmetic the operators mean at it. A type is found by its constructor, so every reader of a
//! value asks here and no caller passes it.

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
    /// What the type's `numeric` answers for this role over these values of it; `None` as for
    /// `key`.
    pub numeric: fn(*mut (), usize, &[Value]) -> Option<Value>,
    /// The witness a call passes for the type this constructor belongs to: [`STATED_WITNESS`]
    /// past its place among the unit's constructors. `None` where the type states no `numeric`.
    pub witness: fn(*mut (), &Symbol) -> Option<i64>,
    /// The type's `of_int` at this `Int`, for the type a witness names; `None` as for `key`.
    pub of_int: fn(*mut (), i64, i64) -> Option<Value>,
}

/// The functions a `numeric` names, in the order a unit lists them.
pub const ADD: usize = 0;
pub const SUB: usize = 1;
pub const MUL: usize = 2;
pub const NEG: usize = 3;
pub const OF_INT: usize = 4;

/// How many words each takes.
pub const NUMERIC_WORDS: [usize; 5] = [2, 2, 2, 1, 1];

/// The first witness of a type that states its own arithmetic: the builtin types' come before.
pub const STATED_WITNESS: i64 = 13;

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

/// `args[0]` is a constructor's value.
pub(crate) fn numeric(role: usize, args: &[Value]) -> Option<Value> {
    let i = running()?;
    (i.numeric)(i.unit, role, args)
}

pub(crate) fn witness(ctor: &Symbol) -> Option<i64> {
    let i = running()?;
    (i.witness)(i.unit, ctor)
}

pub(crate) fn of_int(witness: i64, n: i64) -> Option<Value> {
    let i = running()?;
    (i.of_int)(i.unit, witness, n)
}

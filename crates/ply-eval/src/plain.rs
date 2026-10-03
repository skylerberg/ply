//! A value as plain data: what crosses a thread, the store and the machine boundary, and what
//! `std.value.Value` holds on the Ply side. Nothing in Rust renders one; the CLI does.

use crate::limit::grow;
use crate::value::{Closure, ClosureKind, Fields, Synth};
use crate::{IntTy, Symbol, Value};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// What `std.value.render` shows before eliding the rest of a collection.
pub const SHOWN_ITEMS: usize = 32;
/// The nesting `std.value.render` shows before eliding what is inside.
pub const SHOWN_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Plain {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    #[serde(with = "decimal_bytes")]
    Decimal(Decimal),
    /// The pattern the width reads, with nothing above it: `-1i8` is `0xFF`.
    Fixed {
        ty: IntTy,
        bits: u128,
    },
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<Plain>),
    /// Sorted by name, as a record's fields are.
    Record(Vec<(String, Plain)>),
    Ctor(String, Vec<Plain>),
    /// In key order.
    Map(Vec<(Plain, Plain)>),
    Fn(Fun),
    Cell {
        index: u32,
        generation: u32,
    },
    Task(u64),
    /// A credential, whose payload is never copied out.
    Secret,
    /// What a bounded snapshot left out: `n` more items of a list or map, or with `0`, all
    /// that was nested deeper.
    Elided(u64),
    Char(char),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Fun {
    Named(String),
    Anonymous,
    Const {
        arity: usize,
        value: Box<Plain>,
    },
    Project {
        arity: usize,
        index: usize,
    },
    /// The first entry whose key equals the first argument, else `default`.
    Table {
        arity: usize,
        entries: Vec<(Plain, Plain)>,
        default: Box<Plain>,
    },
}

impl Plain {
    /// All of `v`.
    pub fn of(v: &Value) -> Plain {
        snapshot(v, None, 0)
    }

    /// As much of `v` as `std.value.render` shows, for a diagnostic to carry.
    pub fn shown(v: &Value) -> Plain {
        snapshot(v, Some((SHOWN_ITEMS, SHOWN_DEPTH)), 0)
    }

    /// The runtime value this names; `Err` says what cannot be one.
    pub fn into_value(self) -> Result<Value, &'static str> {
        Ok(match self {
            Plain::Unit => Value::Unit,
            Plain::Bool(b) => Value::Bool(b),
            Plain::Int(i) => Value::Int(i),
            Plain::Float(f) => Value::Float(f),
            Plain::Decimal(d) => Value::Decimal(d),
            Plain::Fixed { ty, bits } => Value::Fixed(crate::Fixed::new(ty, bits)),
            Plain::Char(c) => Value::Char(c),
            Plain::Str(s) => Value::str(s),
            Plain::Bytes(b) => Value::bytes(b),
            Plain::List(items) => Value::list(grow(|| values(items))?),
            Plain::Record(fields) => Value::Record(Arc::new(Fields::from_unsorted(
                fields
                    .into_iter()
                    .map(|(name, v)| Ok((Symbol::new(name), grow(|| v.into_value())?)))
                    .collect::<Result<_, &'static str>>()?,
            ))),
            Plain::Ctor(name, args) => Value::ctor(name, grow(|| values(args))?),
            Plain::Map(entries) => Value::map(grow(|| pairs(entries))?),
            Plain::Fn(Fun::Const { arity, value }) => {
                synth(arity, Synth::Const((*value).into_value()?))
            }
            Plain::Fn(Fun::Project { arity, index }) if index < arity => {
                synth(arity, Synth::Project(index))
            }
            Plain::Fn(Fun::Table {
                arity,
                entries,
                default,
            }) if arity > 0 => synth(
                arity,
                Synth::Table {
                    entries: pairs(entries)?,
                    default: (*default).into_value()?,
                },
            ),
            Plain::Fn(Fun::Project { .. } | Fun::Table { .. }) => {
                return Err("a generated function whose argument is not one of its own");
            }
            Plain::Fn(Fun::Named(_) | Fun::Anonymous) => {
                return Err("a function the program wrote, which only its own run can hold");
            }
            Plain::Cell { .. } => return Err("a cell, which only its own region can hold"),
            Plain::Task(_) => return Err("a task, which only its own run can hold"),
            Plain::Secret => return Err("a credential, whose payload never leaves the runtime"),
            Plain::Elided(_) => return Err("a value a diagnostic cut short"),
        })
    }

    /// What a reader with no renderer is told in place of the value.
    pub fn describe(&self) -> &'static str {
        match self {
            Plain::Unit => "`()`",
            Plain::Bool(_) => "a `Bool`",
            Plain::Int(_) => "an `Int`",
            Plain::Float(_) => "a `Float`",
            Plain::Decimal(_) => "a `Decimal`",
            Plain::Fixed { .. } => "a fixed-width integer",
            Plain::Char(_) => "a `Char`",
            Plain::Str(_) => "a `String`",
            Plain::Bytes(_) => "a `Bytes`",
            Plain::List(_) => "a `List`",
            Plain::Record(_) => "a record",
            Plain::Ctor(..) => "a variant",
            Plain::Map(_) => "a `Map`",
            Plain::Fn(_) => "a function",
            Plain::Cell { .. } => "a `Cell`",
            Plain::Task(_) => "a `Task`",
            Plain::Secret => "a `Secret`",
            Plain::Elided(_) => "a value",
        }
    }
}

fn values(items: Vec<Plain>) -> Result<Vec<Value>, &'static str> {
    items.into_iter().map(Plain::into_value).collect()
}

fn pairs(entries: Vec<(Plain, Plain)>) -> Result<Vec<(Value, Value)>, &'static str> {
    entries
        .into_iter()
        .map(|(k, v)| Ok((k.into_value()?, v.into_value()?)))
        .collect()
}

fn synth(arity: usize, rule: Synth) -> Value {
    Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Synth { arity, rule },
    }))
}

/// `bound` is the items and the depth kept; `None` keeps everything.
fn snapshot(v: &Value, bound: Option<(usize, usize)>, depth: usize) -> Plain {
    if bound.is_some_and(|(_, deepest)| depth > deepest) {
        return Plain::Elided(0);
    }
    let inner = |x: &Value| grow(|| snapshot(x, bound, depth + 1));
    let kept = |n: usize| bound.map_or(n, |(items, _)| n.min(items));
    match v {
        Value::Unit => Plain::Unit,
        Value::Bool(b) => Plain::Bool(*b),
        Value::Int(i) => Plain::Int(*i),
        Value::Float(f) => Plain::Float(*f),
        Value::Decimal(d) => Plain::Decimal(*d),
        Value::Fixed(f) => Plain::Fixed {
            ty: f.ty,
            bits: f.raw(),
        },
        Value::Char(c) => Plain::Char(*c),
        Value::Str(s) => Plain::Str(s.to_string()),
        Value::Bytes(b) => Plain::Bytes(b.to_vec()),
        Value::List(items) => {
            let mut out: Vec<Plain> = items.iter().take(kept(items.len())).map(inner).collect();
            if out.len() < items.len() {
                out.push(Plain::Elided((items.len() - out.len()) as u64));
            }
            Plain::List(out)
        }
        Value::Map(entries) => {
            let mut out: Vec<(Plain, Plain)> = entries
                .iter()
                .take(kept(entries.size()))
                .map(|(k, x)| (inner(k), inner(x)))
                .collect();
            if out.len() < entries.size() {
                let more = (entries.size() - out.len()) as u64;
                out.push((Plain::Elided(more), Plain::Elided(more)));
            }
            Plain::Map(out)
        }
        Value::Record(fields) => Plain::Record(
            fields
                .iter()
                .map(|(name, x)| (name.as_str().to_string(), inner(x)))
                .collect(),
        ),
        Value::Ctor { name, args } => {
            Plain::Ctor(name.as_str().to_string(), args.iter().map(inner).collect())
        }
        Value::Closure(c) => Plain::Fn(match &c.kind {
            ClosureKind::Synth { arity, rule } => match rule {
                Synth::Const(x) => Fun::Const {
                    arity: *arity,
                    value: Box::new(inner(x)),
                },
                Synth::Project(index) => Fun::Project {
                    arity: *arity,
                    index: *index,
                },
                Synth::Table { entries, default } => Fun::Table {
                    arity: *arity,
                    entries: entries.iter().map(|(k, x)| (inner(k), inner(x))).collect(),
                    default: Box::new(inner(default)),
                },
            },
            _ => match &c.name {
                Some(name) => Fun::Named(name.as_str().to_string()),
                None => Fun::Anonymous,
            },
        }),
        Value::Cell(slot) => Plain::Cell {
            index: slot.index(),
            generation: slot.generation(),
        },
        Value::Task(handle) => Plain::Task(handle.id().0),
        Value::Secret(_) => Plain::Secret,
    }
}

/// `Decimal`'s own sixteen bytes, since the workspace builds it without serde.
mod decimal_bytes {
    use rust_decimal::Decimal;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(d: &Decimal, s: S) -> Result<S::Ok, S::Error> {
        d.serialize().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Decimal, D::Error> {
        Ok(Decimal::deserialize(<[u8; 16]>::deserialize(d)?))
    }
}

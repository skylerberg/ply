//! A value as plain data: what a diagnostic carries to show, as `std.value.Value` holds it on the
//! Ply side. Nothing in Rust renders one; the CLI does.

use crate::limit::grow;
use crate::value::{ClosureKind, Synth};
use crate::{IntTy, Value};
use rust_decimal::Decimal;

/// What `std.value.render` shows before eliding the rest of a collection.
const SHOWN_ITEMS: usize = 32;
/// The nesting `std.value.render` shows before eliding what is inside.
const SHOWN_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub enum Plain {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
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
    /// What a `with_hold` binds; which one is the run's own.
    Hold,
    /// A credential, whose payload is never copied out.
    Secret,
    /// What a bounded snapshot left out: `n` more items of a list or map, or with `0`, all
    /// that was nested deeper.
    Elided(u64),
    Char(char),
    Array(Vec<Plain>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fun {
    Named(String),
    Anonymous,
    Const {
        arity: u8,
        value: Box<Plain>,
    },
    Project {
        arity: u8,
        index: u8,
    },
    /// The first entry whose key equals the first argument, else `default`.
    Table {
        arity: u8,
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
            Plain::Array(_) => "an `Array`",
            Plain::Record(_) => "a record",
            Plain::Ctor(..) => "a variant",
            Plain::Map(_) => "a `Map`",
            Plain::Fn(_) => "a function",
            Plain::Hold => "a `Hold`",
            Plain::Secret => "a `Secret`",
            Plain::Elided(_) => "a value",
        }
    }
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
        Value::Array(items) => {
            let mut out: Vec<Plain> = items.iter().take(kept(items.len())).map(inner).collect();
            if out.len() < items.len() {
                out.push(Plain::Elided((items.len() - out.len()) as u64));
            }
            Plain::Array(out)
        }
        Value::Map(entries) => {
            let mut out: Vec<(Plain, Plain)> = entries
                .iter()
                .take(kept(entries.len()))
                .map(|(k, x)| (inner(k), inner(x)))
                .collect();
            if out.len() < entries.len() {
                let more = (entries.len() - out.len()) as u64;
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
        Value::Closure(c) if matches!(c.kind, ClosureKind::Hold { .. }) => Plain::Hold,
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
        Value::Secret(_) => Plain::Secret,
    }
}

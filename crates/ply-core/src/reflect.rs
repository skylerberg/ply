//! A value as `std.value.Value` holds it: what a program reflects on.

use crate::limit::grow;
use crate::value::{ClosureKind, Synth};
use crate::{IntTy, Symbol, Value as PlyValue};
use std::sync::Arc;

fn value_ctor(name: &str, args: Vec<PlyValue>) -> PlyValue {
    PlyValue::ctor(Symbol::new(format!("std.value.{name}")), args)
}

#[allow(clippy::arc_with_non_send_sync)]
fn record(fields: Vec<(&str, PlyValue)>) -> PlyValue {
    PlyValue::Record(Arc::new(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    ))
}

fn byte(n: u8) -> PlyValue {
    PlyValue::Fixed(crate::Fixed::new(IntTy::U8, u128::from(n)))
}

/// All of `v`, a hold as `VHold` and a credential as `VSecret`.
#[allow(clippy::arc_with_non_send_sync)]
pub fn value_of(v: &PlyValue) -> PlyValue {
    let entry =
        |(k, x): (&PlyValue, &PlyValue)| record(vec![("key", value_of(k)), ("value", value_of(x))]);
    let items = |xs: &mut dyn Iterator<Item = &PlyValue>| {
        PlyValue::list(grow(|| xs.map(value_of).collect()))
    };
    match v {
        PlyValue::Unit => value_ctor("VUnit", vec![]),
        PlyValue::Bool(b) => value_ctor("VBool", vec![PlyValue::Bool(*b)]),
        PlyValue::Int(i) => value_ctor("VInt", vec![PlyValue::Int(*i)]),
        PlyValue::Float(f) => value_ctor("VFloat", vec![PlyValue::Float(*f)]),
        PlyValue::Decimal(d) => value_ctor("VDecimal", vec![PlyValue::Decimal(*d)]),
        PlyValue::Fixed(f) => value_ctor(
            "VFixed",
            vec![
                PlyValue::str(f.ty.name()),
                PlyValue::Fixed(crate::Fixed::new(IntTy::U128, f.raw())),
            ],
        ),
        PlyValue::Char(c) => value_ctor("VChar", vec![PlyValue::Char(*c)]),
        PlyValue::Str(s) => value_ctor("VStr", vec![PlyValue::str(s)]),
        PlyValue::Bytes(b) => value_ctor("VBytes", vec![PlyValue::bytes(b)]),
        PlyValue::List(xs) => value_ctor("VList", vec![items(&mut xs.iter())]),
        PlyValue::Array(xs) => value_ctor("VArray", vec![items(&mut xs.iter())]),
        PlyValue::Record(fields) => value_ctor(
            "VRecord",
            vec![PlyValue::list(grow(|| {
                fields
                    .iter()
                    .map(|(name, x)| {
                        record(vec![
                            ("name", PlyValue::str(name.as_str())),
                            ("value", value_of(x)),
                        ])
                    })
                    .collect()
            }))],
        ),
        PlyValue::Ctor { name, args } => value_ctor(
            "VCtor",
            vec![PlyValue::str(name.as_str()), items(&mut args.iter())],
        ),
        PlyValue::Map(m) => value_ctor(
            "VMap",
            vec![PlyValue::list(grow(|| m.iter().map(entry).collect()))],
        ),
        PlyValue::Closure(c) => match &c.kind {
            ClosureKind::Hold { .. } => value_ctor("VHold", vec![]),
            ClosureKind::Synth { arity, rule } => value_ctor(
                "VFn",
                vec![match rule {
                    Synth::Const(x) => value_ctor(
                        "FConst",
                        vec![record(vec![
                            ("arity", byte(*arity)),
                            ("value", grow(|| value_of(x))),
                        ])],
                    ),
                    Synth::Project(index) => value_ctor(
                        "FProject",
                        vec![record(vec![
                            ("arity", byte(*arity)),
                            ("index", byte(*index)),
                        ])],
                    ),
                    Synth::Table { entries, default } => value_ctor(
                        "FTable",
                        vec![record(vec![
                            ("arity", byte(*arity)),
                            (
                                "entries",
                                PlyValue::list(grow(|| {
                                    entries.iter().map(|(k, x)| entry((k, x))).collect()
                                })),
                            ),
                            ("default", grow(|| value_of(default))),
                        ])],
                    ),
                }],
            ),
            ClosureKind::Continuation { .. } => value_ctor(
                "VFn",
                vec![match &c.name {
                    Some(name) => value_ctor("FNamed", vec![PlyValue::str(name.as_str())]),
                    None => value_ctor("FAnonymous", vec![]),
                }],
            ),
        },
        PlyValue::Secret(_) => value_ctor("VSecret", vec![]),
    }
}

/// [`value_of`] of all of `v`, but for each value whose type states a `show`: that is `VShown` of
/// what the function writes, at whatever depth it sits.
pub fn shown_of(v: &PlyValue) -> PlyValue {
    let items = |xs: &mut dyn Iterator<Item = &PlyValue>| {
        PlyValue::list(grow(|| xs.map(shown_of).collect()))
    };
    match v {
        PlyValue::Ctor { name, args } => match crate::instances::shown(v) {
            Some(text) => value_ctor("VShown", vec![text]),
            None => value_ctor(
                "VCtor",
                vec![PlyValue::str(name.as_str()), items(&mut args.iter())],
            ),
        },
        PlyValue::List(xs) => value_ctor("VList", vec![items(&mut xs.iter())]),
        PlyValue::Array(xs) => value_ctor("VArray", vec![items(&mut xs.iter())]),
        PlyValue::Record(fields) => value_ctor(
            "VRecord",
            vec![PlyValue::list(grow(|| {
                fields
                    .iter()
                    .map(|(name, x)| {
                        record(vec![
                            ("name", PlyValue::str(name.as_str())),
                            ("value", shown_of(x)),
                        ])
                    })
                    .collect()
            }))],
        ),
        PlyValue::Map(entries) => value_ctor(
            "VMap",
            vec![PlyValue::list(grow(|| {
                entries
                    .iter()
                    .map(|(k, x)| record(vec![("key", shown_of(k)), ("value", shown_of(x))]))
                    .collect()
            }))],
        ),
        leaf => value_of(leaf),
    }
}

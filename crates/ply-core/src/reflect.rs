//! A value as `std.value.Value` holds it: what a program reflects on, and what a diagnostic's values
//! become before they are rendered.

use crate::limit::grow;
use crate::{Fun, IntTy, Plain, Symbol, Value as PlyValue};
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

#[allow(clippy::arc_with_non_send_sync)]
pub fn value_of(p: &Plain) -> PlyValue {
    let entry =
        |(k, v): &(Plain, Plain)| record(vec![("key", value_of(k)), ("value", value_of(v))]);
    let entries = |es: &[(Plain, Plain)]| PlyValue::list(es.iter().map(entry).collect());
    match p {
        Plain::Unit => value_ctor("VUnit", vec![]),
        Plain::Bool(b) => value_ctor("VBool", vec![PlyValue::Bool(*b)]),
        Plain::Int(i) => value_ctor("VInt", vec![PlyValue::Int(*i)]),
        Plain::Float(f) => value_ctor("VFloat", vec![PlyValue::Float(*f)]),
        Plain::Decimal(d) => value_ctor("VDecimal", vec![PlyValue::Decimal(*d)]),
        Plain::Fixed { ty, bits } => value_ctor(
            "VFixed",
            vec![
                PlyValue::str(ty.name()),
                PlyValue::Fixed(crate::Fixed::new(IntTy::U128, *bits)),
            ],
        ),
        Plain::Char(c) => value_ctor("VChar", vec![PlyValue::Char(*c)]),
        Plain::Str(s) => value_ctor("VStr", vec![PlyValue::str(s)]),
        Plain::Bytes(b) => value_ctor("VBytes", vec![PlyValue::bytes(b)]),
        Plain::List(items) => value_ctor(
            "VList",
            vec![PlyValue::list(grow(|| {
                items.iter().map(value_of).collect()
            }))],
        ),
        Plain::Array(items) => value_ctor(
            "VArray",
            vec![PlyValue::list(grow(|| {
                items.iter().map(value_of).collect()
            }))],
        ),
        Plain::Record(fields) => value_ctor(
            "VRecord",
            vec![PlyValue::list(grow(|| {
                fields
                    .iter()
                    .map(|(name, v)| {
                        record(vec![("name", PlyValue::str(name)), ("value", value_of(v))])
                    })
                    .collect()
            }))],
        ),
        Plain::Ctor(name, args) => value_ctor(
            "VCtor",
            vec![
                PlyValue::str(name),
                PlyValue::list(grow(|| args.iter().map(value_of).collect())),
            ],
        ),
        Plain::Map(es) => value_ctor("VMap", vec![grow(|| entries(es))]),
        Plain::Fn(f) => value_ctor(
            "VFn",
            vec![match f {
                Fun::Named(name) => value_ctor("FNamed", vec![PlyValue::str(name)]),
                Fun::Anonymous => value_ctor("FAnonymous", vec![]),
                Fun::Const { arity, value } => value_ctor(
                    "FConst",
                    vec![record(vec![
                        ("arity", byte(*arity)),
                        ("value", grow(|| value_of(value))),
                    ])],
                ),
                Fun::Project { arity, index } => value_ctor(
                    "FProject",
                    vec![record(vec![
                        ("arity", byte(*arity)),
                        ("index", byte(*index)),
                    ])],
                ),
                Fun::Table {
                    arity,
                    entries: es,
                    default,
                } => value_ctor(
                    "FTable",
                    vec![record(vec![
                        ("arity", byte(*arity)),
                        ("entries", grow(|| entries(es))),
                        ("default", grow(|| value_of(default))),
                    ])],
                ),
            }],
        ),
        Plain::Hold => value_ctor("VHold", vec![]),
        Plain::Task(id) => value_ctor("VTask", vec![PlyValue::Int(*id as i64)]),
        Plain::Chan(id) => value_ctor("VChan", vec![PlyValue::Int(*id as i64)]),
        Plain::Secret => value_ctor("VSecret", vec![]),
        Plain::Elided(n) => value_ctor("VElided", vec![PlyValue::Int(*n as i64)]),
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
        leaf => value_of(&Plain::of(leaf)),
    }
}

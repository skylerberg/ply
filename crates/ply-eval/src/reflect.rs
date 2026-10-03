//! A value as `std.value.Value` holds it, and back: what a program reflects on, what a lent effect
//! hands the CLI, and what a diagnostic's values become before they are rendered.

use crate::limit::grow;
use crate::{Diagnostic, Fun, IntTy, Plain, Span, Symbol, Value as PlyValue, codes};
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

fn count(n: usize) -> PlyValue {
    PlyValue::Int(n as i64)
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
                        ("arity", count(*arity)),
                        ("value", grow(|| value_of(value))),
                    ])],
                ),
                Fun::Project { arity, index } => value_ctor(
                    "FProject",
                    vec![record(vec![
                        ("arity", count(*arity)),
                        ("index", count(*index)),
                    ])],
                ),
                Fun::Table {
                    arity,
                    entries: es,
                    default,
                } => value_ctor(
                    "FTable",
                    vec![record(vec![
                        ("arity", count(*arity)),
                        ("entries", grow(|| entries(es))),
                        ("default", grow(|| value_of(default))),
                    ])],
                ),
            }],
        ),
        Plain::Cell { index, generation } => value_ctor(
            "VCell",
            vec![record(vec![
                ("index", PlyValue::Int(i64::from(*index))),
                ("generation", PlyValue::Int(i64::from(*generation))),
            ])],
        ),
        Plain::Task(id) => value_ctor("VTask", vec![PlyValue::Int(*id as i64)]),
        Plain::Chan(id) => value_ctor("VChan", vec![PlyValue::Int(*id as i64)]),
        Plain::Secret => value_ctor("VSecret", vec![]),
        Plain::Elided(n) => value_ctor("VElided", vec![PlyValue::Int(*n as i64)]),
    }
}

/// The plain value a `std.value.Value` names; anything else is Ply's fault, since the program's
/// types say it is one.
pub fn plain_of(v: &PlyValue, span: Span) -> Result<Plain, Diagnostic> {
    let bad = |why: &str| {
        Diagnostic::error(codes::INTERNAL_ERROR, format!("a `std.value.Value` {why}")).primary(
            span,
            "the program's types say this is a `std.value.Value`; this is Ply's fault",
        )
    };
    let PlyValue::Ctor { name, args } = v else {
        return Err(bad("is no constructor"));
    };
    let arg = |i: usize| args.get(i).ok_or_else(|| bad("is missing an argument"));
    let int = |x: &PlyValue| x.as_int(span, "a `std.value` count");
    let text = |x: &PlyValue| -> Result<String, Diagnostic> {
        match x {
            PlyValue::Str(s) => Ok(s.to_string()),
            _ => Err(bad("holds no text where text belongs")),
        }
    };
    let list = |x: &PlyValue| -> Result<Vec<PlyValue>, Diagnostic> {
        match x {
            PlyValue::List(items) => Ok(items.iter().cloned().collect()),
            _ => Err(bad("holds no list where a list belongs")),
        }
    };
    let field = |x: &PlyValue, name: &str| -> Result<PlyValue, Diagnostic> {
        match x {
            PlyValue::Record(fields) => fields
                .get(&Symbol::new(name))
                .cloned()
                .ok_or_else(|| bad(&format!("has no `{name}`"))),
            _ => Err(bad("holds no record where a record belongs")),
        }
    };
    let entries = |x: &PlyValue| -> Result<Vec<(Plain, Plain)>, Diagnostic> {
        list(x)?
            .iter()
            .map(|e| {
                Ok((
                    plain_of(&field(e, "key")?, span)?,
                    plain_of(&field(e, "value")?, span)?,
                ))
            })
            .collect()
    };
    let simple = name
        .as_str()
        .rsplit_once('.')
        .map_or(name.as_str(), |(_, s)| s);
    Ok(match simple {
        "VUnit" => Plain::Unit,
        "VBool" => Plain::Bool(matches!(arg(0)?, PlyValue::Bool(true))),
        "VInt" => Plain::Int(int(arg(0)?)?),
        "VFloat" => Plain::Float(arg(0)?.as_float(span, "a `std.value` float")?),
        "VDecimal" => Plain::Decimal(arg(0)?.as_decimal(span, "a `std.value` decimal")?),
        "VFixed" => {
            let ty = text(arg(0)?)?;
            let ty = IntTy::from_name(&ty)
                .ok_or_else(|| bad(&format!("names `{ty}`, which is no width")))?;
            let PlyValue::Fixed(bits) = arg(1)? else {
                return Err(bad("holds no `U128` pattern"));
            };
            Plain::Fixed {
                ty,
                bits: ty.normalize(bits.bits()) & mask(ty),
            }
        }
        "VChar" => {
            let PlyValue::Char(c) = arg(0)? else {
                return Err(bad("holds no `Char`"));
            };
            Plain::Char(*c)
        }
        "VStr" => Plain::Str(text(arg(0)?)?),
        "VBytes" => Plain::Bytes(arg(0)?.as_bytes(span, "a `std.value` bytes")?.to_vec()),
        "VList" => Plain::List(grow(|| {
            list(arg(0)?)?
                .iter()
                .map(|x| plain_of(x, span))
                .collect::<Result<_, Diagnostic>>()
        })?),
        "VArray" => Plain::Array(grow(|| {
            list(arg(0)?)?
                .iter()
                .map(|x| plain_of(x, span))
                .collect::<Result<_, Diagnostic>>()
        })?),
        "VRecord" => Plain::Record(grow(|| {
            list(arg(0)?)?
                .iter()
                .map(|f| {
                    Ok((
                        text(&field(f, "name")?)?,
                        plain_of(&field(f, "value")?, span)?,
                    ))
                })
                .collect::<Result<_, Diagnostic>>()
        })?),
        "VCtor" => Plain::Ctor(
            text(arg(0)?)?,
            grow(|| {
                list(arg(1)?)?
                    .iter()
                    .map(|x| plain_of(x, span))
                    .collect::<Result<_, Diagnostic>>()
            })?,
        ),
        "VMap" => Plain::Map(grow(|| entries(arg(0)?))?),
        "VFn" => {
            let PlyValue::Ctor {
                name: f,
                args: fargs,
            } = arg(0)?
            else {
                return Err(bad("holds no `Fun`"));
            };
            let at = |i: usize| fargs.get(i).ok_or_else(|| bad("is missing an argument"));
            let size = |x: &PlyValue, name: &str| -> Result<usize, Diagnostic> {
                usize::try_from(int(&field(x, name)?)?)
                    .map_err(|_| bad(&format!("has a negative `{name}`")))
            };
            Plain::Fn(
                match f.as_str().rsplit_once('.').map_or(f.as_str(), |(_, s)| s) {
                    "FNamed" => Fun::Named(text(at(0)?)?),
                    "FAnonymous" => Fun::Anonymous,
                    "FConst" => Fun::Const {
                        arity: size(at(0)?, "arity")?,
                        value: Box::new(grow(|| plain_of(&field(at(0)?, "value")?, span))?),
                    },
                    "FProject" => Fun::Project {
                        arity: size(at(0)?, "arity")?,
                        index: size(at(0)?, "index")?,
                    },
                    "FTable" => Fun::Table {
                        arity: size(at(0)?, "arity")?,
                        entries: grow(|| entries(&field(at(0)?, "entries")?))?,
                        default: Box::new(grow(|| plain_of(&field(at(0)?, "default")?, span))?),
                    },
                    other => return Err(bad(&format!("holds `{other}`, which is no `Fun`"))),
                },
            )
        }
        "VCell" => Plain::Cell {
            index: u32::try_from(int(&field(arg(0)?, "index")?)?)
                .map_err(|_| bad("names no cell"))?,
            generation: u32::try_from(int(&field(arg(0)?, "generation")?)?)
                .map_err(|_| bad("names no cell"))?,
        },
        "VTask" => Plain::Task(u64::try_from(int(arg(0)?)?).map_err(|_| bad("names no task"))?),
        "VChan" => Plain::Chan(u64::try_from(int(arg(0)?)?).map_err(|_| bad("names no channel"))?),
        "VSecret" => Plain::Secret,
        "VElided" => {
            Plain::Elided(u64::try_from(int(arg(0)?)?).map_err(|_| bad("elides a negative count"))?)
        }
        other => return Err(bad(&format!("is `{other}`, which is no `std.value.Value`"))),
    })
}

/// The bits a width reads, with nothing above it.
fn mask(ty: IntTy) -> u128 {
    u128::MAX >> (128 - ty.bits())
}

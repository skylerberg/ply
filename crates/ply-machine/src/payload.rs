//! Ply values, as a lent effect hands them to the program in `crates/ply-cli/ply`.

use ply_eval::limit::grow;
use ply_eval::{
    Diagnostic, Fun, IntTy, Plain, Severity, SourceMap, Span, Symbol, Value as PlyValue, codes,
};
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

/// A constructor of a type the program declares, by its program-wide name.
pub fn ctor(module: &str, name: &str, args: Vec<PlyValue>) -> PlyValue {
    PlyValue::ctor(Symbol::new(format!("{module}.{name}")), args)
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

pub fn strings<'a>(items: impl IntoIterator<Item = &'a str>) -> PlyValue {
    PlyValue::list(items.into_iter().map(PlyValue::str).collect())
}

/// A document this side wrote as `std.json.Json`: what a facility discloses about itself, which
/// the program places under the keys its own report gives it.
pub fn json(value: &serde_json::Value) -> PlyValue {
    match value {
        serde_json::Value::Null => ctor("std.json", "Null", Vec::new()),
        serde_json::Value::Bool(b) => ctor("std.json", "Bool", vec![PlyValue::Bool(*b)]),
        serde_json::Value::Number(n) => ctor("std.json", "Number", vec![number(n)]),
        serde_json::Value::String(s) => ctor("std.json", "Str", vec![PlyValue::str(s)]),
        serde_json::Value::Array(items) => ctor(
            "std.json",
            "Array",
            vec![PlyValue::list(items.iter().map(json).collect())],
        ),
        serde_json::Value::Object(fields) => ctor(
            "std.json",
            "Object",
            vec![PlyValue::map(
                fields
                    .iter()
                    .map(|(key, value)| (PlyValue::str(key), json(value))),
            )],
        ),
    }
}

/// A number written back through `Decimal`, which is what a `Json` number is. A magnitude it
/// cannot hold is no count or ratio this side ever writes.
fn number(n: &serde_json::Number) -> PlyValue {
    PlyValue::Decimal(n.to_string().parse().unwrap_or_default())
}

/// `compiler.resolve.Diag`, as `crates/ply-cli/ply/diagnostic.ply` renders it. A label carries
/// the source id its span names, which is the index of its file in `places`. A carried value is
/// said as what it is: only a crossing that hands the values over, [`raised_value`], keeps them.
pub fn diag_value(diagnostic: &Diagnostic) -> PlyValue {
    diag_record(diagnostic, &|text| diagnostic.described(text))
}

/// `diagnostic.Raised`: a runtime diagnostic beside the values its text names, which the
/// program renders.
pub fn raised_value(diagnostic: &Diagnostic) -> PlyValue {
    record(vec![
        ("diag", diag_record(diagnostic, &|text| text.to_string())),
        (
            "values",
            PlyValue::list(diagnostic.values.iter().map(plain_value).collect()),
        ),
    ])
}

fn diag_record(diagnostic: &Diagnostic, text: &dyn Fn(&str) -> String) -> PlyValue {
    record(vec![
        ("code", PlyValue::bytes(diagnostic.code.as_bytes())),
        ("notes", count(diagnostic.notes.len())),
        (
            "labels",
            PlyValue::list(
                diagnostic
                    .labels
                    .iter()
                    .map(|l| {
                        record(vec![
                            ("module", PlyValue::Int(l.span.source.0 as i64)),
                            ("start", PlyValue::Int(l.span.start as i64)),
                            ("end", PlyValue::Int(l.span.end as i64)),
                            ("primary", PlyValue::Bool(l.primary)),
                            ("text", PlyValue::bytes(text(&l.message).as_bytes())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("text", PlyValue::bytes(b"")),
        (
            "message",
            PlyValue::bytes(text(&diagnostic.message).as_bytes()),
        ),
        (
            "notes_text",
            PlyValue::list(
                diagnostic
                    .notes
                    .iter()
                    .map(|n| PlyValue::bytes(text(n).as_bytes()))
                    .collect(),
            ),
        ),
        (
            "severity",
            PlyValue::bytes(
                match diagnostic.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Note => "note",
                }
                .as_bytes(),
            ),
        ),
        (
            "fixes",
            PlyValue::list(
                diagnostic
                    .fixes
                    .iter()
                    .map(|f| {
                        record(vec![
                            ("title", PlyValue::bytes(f.title.as_bytes())),
                            (
                                "edits",
                                PlyValue::list(
                                    f.edits
                                        .iter()
                                        .map(|e| {
                                            record(vec![
                                                ("module", PlyValue::Int(e.span.source.0 as i64)),
                                                ("start", PlyValue::Int(e.span.start as i64)),
                                                ("end", PlyValue::Int(e.span.end as i64)),
                                                ("text", PlyValue::bytes(e.text.as_bytes())),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

pub fn diags_value(diagnostics: &[Diagnostic]) -> PlyValue {
    PlyValue::list(diagnostics.iter().map(diag_value).collect())
}

/// The modules a label can point into, in source-id order, which is what a label's index is.
pub fn places_value(sources: &SourceMap) -> PlyValue {
    PlyValue::list(
        sources
            .files()
            .iter()
            .map(|f| {
                record(vec![
                    ("path", PlyValue::str(f.path.display().to_string())),
                    ("text", PlyValue::bytes(f.text.as_bytes())),
                ])
            })
            .collect(),
    )
}

// --- Reading records the program hands the machine -------------------------------------------

pub fn field_of<'a>(
    value: &'a PlyValue,
    name: &str,
    span: Span,
) -> Result<&'a PlyValue, Diagnostic> {
    match value {
        PlyValue::Record(fields) => fields
            .iter()
            .find(|(k, _)| k.as_str() == name)
            .map(|(_, value)| value)
            .ok_or_else(|| missing(name, span)),
        _ => Err(missing(name, span)),
    }
}

/// An `Option` a caller built, read without knowing what is inside it.
pub fn option_of<'v>(
    value: &'v PlyValue,
    what: &str,
    span: Span,
) -> Result<Option<&'v PlyValue>, Diagnostic> {
    match value {
        PlyValue::Ctor { name, args } if name.as_str() == "Some" => {
            args.first().map(Some).ok_or_else(|| {
                Diagnostic::error(codes::INTERNAL_ERROR, format!("`Some` holds no {what}"))
                    .primary(span, "empty `Some`")
            })
        }
        PlyValue::Ctor { name, .. } if name.as_str() == "None" => Ok(None),
        other => Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("`{what}` is an option, and this is a {}", other.type_name()),
        )
        .primary(span, "an option")),
    }
}

pub fn opt_str_at(value: &PlyValue, name: &str, span: Span) -> Result<Option<String>, Diagnostic> {
    match field_of(value, name, span)? {
        PlyValue::Ctor { name, args } if name.as_str() == "Some" => Ok(args
            .first()
            .map(|v| v.as_str(span, "a value").map(str::to_string))
            .transpose()?),
        PlyValue::Ctor { name, .. } if name.as_str() == "None" => Ok(None),
        other => Err(shape(other, span)),
    }
}

pub fn opt_int_at(value: &PlyValue, name: &str, span: Span) -> Result<Option<i64>, Diagnostic> {
    match field_of(value, name, span)? {
        PlyValue::Ctor { name, args } if name.as_str() == "Some" => Ok(args
            .first()
            .map(|v| v.as_int(span, "a number"))
            .transpose()?),
        PlyValue::Ctor { name, .. } if name.as_str() == "None" => Ok(None),
        other => Err(shape(other, span)),
    }
}

pub fn str_list_at(value: &PlyValue, name: &str, span: Span) -> Result<Vec<String>, Diagnostic> {
    field_of(value, name, span)?
        .as_list(span, name)?
        .iter()
        .map(|v| v.as_str(span, "an entry").map(str::to_string))
        .collect::<Result<Vec<String>, Diagnostic>>()
}

pub fn missing(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the options record has no `{name}`"),
    )
    .primary(
        span,
        "the program and the machine agree on the record; this is Ply's fault",
    )
}

pub fn shape(value: &PlyValue, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "the machine read a {} where an Option was expected",
            value.type_name()
        ),
    )
    .primary(
        span,
        "the program and the machine agree on the record; this is Ply's fault",
    )
}

// --- `std.value.Value`: a value as the program holds it ---------------------------------------

fn value_ctor(name: &str, args: Vec<PlyValue>) -> PlyValue {
    ctor("std.value", name, args)
}

#[allow(clippy::arc_with_non_send_sync)]
pub fn plain_value(p: &Plain) -> PlyValue {
    let entry =
        |(k, v): &(Plain, Plain)| record(vec![("key", plain_value(k)), ("value", plain_value(v))]);
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
                PlyValue::Fixed(ply_eval::Fixed::new(IntTy::U128, *bits)),
            ],
        ),
        Plain::Str(s) => value_ctor("VStr", vec![PlyValue::str(s)]),
        Plain::Bytes(b) => value_ctor("VBytes", vec![PlyValue::bytes(b)]),
        Plain::List(items) => value_ctor(
            "VList",
            vec![PlyValue::list(grow(|| {
                items.iter().map(plain_value).collect()
            }))],
        ),
        Plain::Record(fields) => value_ctor(
            "VRecord",
            vec![PlyValue::list(grow(|| {
                fields
                    .iter()
                    .map(|(name, v)| {
                        record(vec![
                            ("name", PlyValue::str(name)),
                            ("value", plain_value(v)),
                        ])
                    })
                    .collect()
            }))],
        ),
        Plain::Ctor(name, args) => value_ctor(
            "VCtor",
            vec![
                PlyValue::str(name),
                PlyValue::list(grow(|| args.iter().map(plain_value).collect())),
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
                        ("value", grow(|| plain_value(value))),
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
                        ("default", grow(|| plain_value(default))),
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
        Plain::Secret => value_ctor("VSecret", vec![]),
        Plain::Elided(n) => value_ctor("VElided", vec![PlyValue::Int(*n as i64)]),
    }
}

/// The plain value a `std.value.Value` names; anything else is Ply's fault, since the program's
/// types say it is one.
pub fn value_plain(v: &PlyValue, span: Span) -> Result<Plain, Diagnostic> {
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
                    value_plain(&field(e, "key")?, span)?,
                    value_plain(&field(e, "value")?, span)?,
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
        "VStr" => Plain::Str(text(arg(0)?)?),
        "VBytes" => Plain::Bytes(arg(0)?.as_bytes(span, "a `std.value` bytes")?.to_vec()),
        "VList" => Plain::List(grow(|| {
            list(arg(0)?)?
                .iter()
                .map(|x| value_plain(x, span))
                .collect::<Result<_, Diagnostic>>()
        })?),
        "VRecord" => Plain::Record(grow(|| {
            list(arg(0)?)?
                .iter()
                .map(|f| {
                    Ok((
                        text(&field(f, "name")?)?,
                        value_plain(&field(f, "value")?, span)?,
                    ))
                })
                .collect::<Result<_, Diagnostic>>()
        })?),
        "VCtor" => Plain::Ctor(
            text(arg(0)?)?,
            grow(|| {
                list(arg(1)?)?
                    .iter()
                    .map(|x| value_plain(x, span))
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
                        value: Box::new(grow(|| value_plain(&field(at(0)?, "value")?, span))?),
                    },
                    "FProject" => Fun::Project {
                        arity: size(at(0)?, "arity")?,
                        index: size(at(0)?, "index")?,
                    },
                    "FTable" => Fun::Table {
                        arity: size(at(0)?, "arity")?,
                        entries: grow(|| entries(&field(at(0)?, "entries")?))?,
                        default: Box::new(grow(|| value_plain(&field(at(0)?, "default")?, span))?),
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

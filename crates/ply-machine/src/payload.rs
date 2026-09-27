//! Ply values, as a lent effect hands them to the program in `crates/ply-cli/ply`.

use ply_eval::Value as PlyValue;
use ply_span::{Diagnostic, Severity, SourceMap, Span, Symbol, codes};
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

/// A record whose field names are already symbols, in no particular order.
///
/// `Value::Record` holds an `Arc`, and its fields are not `Send`; every construction site says so.
#[allow(clippy::arc_with_non_send_sync)]
pub fn record_unsorted(fields: Vec<(Symbol, PlyValue)>) -> PlyValue {
    PlyValue::Record(Arc::new(ply_eval::Fields::from_unsorted(fields)))
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
/// the source id its span names, which is the index of its file in `places`.
pub fn diag_value(diagnostic: &Diagnostic) -> PlyValue {
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
                            ("text", PlyValue::bytes(l.message.as_bytes())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("text", PlyValue::bytes(b"")),
        ("message", PlyValue::bytes(diagnostic.message.as_bytes())),
        (
            "notes_text",
            PlyValue::list(
                diagnostic
                    .notes
                    .iter()
                    .map(|n| PlyValue::bytes(n.as_bytes()))
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

// --- `machine.Value`: a definition's argument or answer, as plain data ------------------------------

/// The runtime value as the program reads it: its machine module's `Value` constructors,
/// named as the calling program declares them (`module` is that module's name in the caller).
/// A closure, task, cell or secret cannot cross; trying to send one is the caller's error.
pub fn machine_value(v: &PlyValue, module: &str) -> Result<PlyValue, Diagnostic> {
    let c = |name: &str, args: Vec<PlyValue>| Ok(ctor(module, name, args));
    match v {
        PlyValue::Unit => c("VUnit", vec![]),
        PlyValue::Bool(b) => c("VBool", vec![PlyValue::Bool(*b)]),
        PlyValue::Int(i) => c("VInt", vec![PlyValue::Int(*i)]),
        PlyValue::Float(f) => c("VFloat", vec![PlyValue::Float(*f)]),
        PlyValue::Decimal(d) => c("VDecimal", vec![PlyValue::Decimal(*d)]),
        PlyValue::Fixed(f) => {
            let value = i64::try_from(f.value()).map_err(|_| {
                Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("a `{}` above `Int`'s range cannot cross", f.ty.name()),
                )
                .primary(Span::DUMMY, "the value does not fit an `Int`")
            })?;
            c(
                "VFixed",
                vec![PlyValue::str(f.ty.name()), PlyValue::Int(value)],
            )
        }
        PlyValue::Str(s) => c("VStr", vec![PlyValue::str(s.as_ref())]),
        PlyValue::Bytes(b) => c("VBytes", vec![PlyValue::bytes(b.as_ref())]),
        PlyValue::List(items) => c(
            "VList",
            vec![PlyValue::list(
                items
                    .iter()
                    .map(|x| machine_value(x, module))
                    .collect::<Result<_, _>>()?,
            )],
        ),
        PlyValue::Record(fields) => c(
            "VRecord",
            vec![PlyValue::list(
                fields
                    .iter()
                    .map(|(name, value)| {
                        Ok(record(vec![
                            ("name", PlyValue::str(name.as_str())),
                            ("value", machine_value(value, module)?),
                        ]))
                    })
                    .collect::<Result<_, Diagnostic>>()?,
            )],
        ),
        PlyValue::Ctor { name, args } => c(
            "VCtor",
            vec![
                PlyValue::str(name.as_str()),
                PlyValue::list(
                    args.iter()
                        .map(|x| machine_value(x, module))
                        .collect::<Result<_, _>>()?,
                ),
            ],
        ),
        PlyValue::Map(m) => c(
            "VMap",
            vec![PlyValue::list(
                m.iter()
                    .map(|(key, value)| {
                        Ok(record(vec![
                            ("key", machine_value(key, module)?),
                            ("value", machine_value(value, module)?),
                        ]))
                    })
                    .collect::<Result<_, Diagnostic>>()?,
            )],
        ),
        other => Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("a {other:?}'s value cannot cross the machine boundary"),
        )
        .primary(Span::DUMMY, "this is Ply's fault")),
    }
}

/// The runtime value a `machine.Value` names. The constructors are the program's own; anything
/// else is the caller's error.
pub fn value_of_adt(v: &PlyValue, span: Span, module: &str) -> Result<PlyValue, Diagnostic> {
    let bad = |why: &str| {
        Diagnostic::error(codes::RUNTIME_ERROR, format!("a call's argument {why}"))
            .primary(span, "not a `machine.Value`")
    };
    let PlyValue::Ctor { name, args } = v else {
        return Err(bad("is no constructor"));
    };
    let at = |i: usize| args.get(i);
    let prefixed = format!("{module}.");
    let simple = name
        .as_str()
        .strip_prefix(&prefixed)
        .unwrap_or(name.as_str());
    match simple {
        "VUnit" => Ok(PlyValue::Unit),
        "VBool" => Ok(at(0).cloned().unwrap_or_default()),
        "VInt" => Ok(at(0).cloned().unwrap_or_default()),
        "VFloat" => Ok(at(0).cloned().unwrap_or_default()),
        "VDecimal" => Ok(at(0).cloned().unwrap_or_default()),
        "VFixed" => {
            let Some(PlyValue::Str(ty)) = at(0) else {
                return Err(bad("'s fixed type is not text"));
            };
            let ty = ply_eval::IntTy::from_name(ty.as_ref())
                .ok_or_else(|| bad("'s fixed type is unknown"))?;
            let v = at(1)
                .ok_or_else(|| bad("'s fixed value is missing"))?
                .as_int(span, "a fixed-width integer")?;
            Ok(PlyValue::Fixed(
                ply_eval::Fixed::of(ty, i128::from(v))
                    .ok_or_else(|| bad("'s fixed value does not fit its type"))?,
            ))
        }
        "VStr" => Ok(at(0).cloned().unwrap_or_default()),
        "VBytes" => Ok(at(0).cloned().unwrap_or_default()),
        "VList" => {
            let Some(PlyValue::List(items)) = at(0) else {
                return Err(bad("'s list is not one"));
            };
            Ok(PlyValue::list(
                items
                    .iter()
                    .map(|x| value_of_adt(x, span, module))
                    .collect::<Result<_, _>>()?,
            ))
        }
        "VRecord" => {
            let Some(PlyValue::List(fields)) = at(0) else {
                return Err(bad("'s fields are not a list"));
            };
            let mut out = Vec::new();
            for field in fields.iter() {
                let PlyValue::Record(pair) = field else {
                    return Err(bad("'s field is not a record"));
                };
                let name = pair.get(&Symbol::new("name")).ok_or_else(|| bad(""))?;
                let value = pair.get(&Symbol::new("value")).ok_or_else(|| bad(""))?;
                let PlyValue::Str(text) = name else {
                    return Err(bad("'s field name is not text"));
                };
                out.push((
                    Symbol::new(text.as_ref()),
                    value_of_adt(value, span, module)?,
                ));
            }
            Ok(record_unsorted(out))
        }
        "VCtor" => {
            let Some(PlyValue::Str(name)) = at(0) else {
                return Err(bad("'s name is not text"));
            };
            let Some(PlyValue::List(items)) = at(1) else {
                return Err(bad("'s arguments are not a list"));
            };
            Ok(PlyValue::ctor(
                name.as_ref(),
                items
                    .iter()
                    .map(|x| value_of_adt(x, span, module))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        "VMap" => {
            let Some(PlyValue::List(entries)) = at(0) else {
                return Err(bad("'s entries are not a list"));
            };
            let mut out = Vec::new();
            for entry in entries.iter() {
                let PlyValue::Record(pair) = entry else {
                    return Err(bad("'s entry is not a record"));
                };
                let key = pair.get(&Symbol::new("key")).ok_or_else(|| bad(""))?;
                let value = pair.get(&Symbol::new("value")).ok_or_else(|| bad(""))?;
                out.push((
                    value_of_adt(key, span, module)?,
                    value_of_adt(value, span, module)?,
                ));
            }
            Ok(PlyValue::map(out))
        }
        other => Err(bad(&format!(
            "names `{other}`, which is not a `machine.Value`"
        ))),
    }
}

// --- The wire: what a `machine.call` crosses on ---------------------------------------------
// ply_eval::Value is not Send, so the crossing is a JSON-shaped encoding; the program's side
// holds the same value as `machine.Value` constructors, and these two turn one into the other.

pub fn value_to_wire(v: &PlyValue) -> serde_json::Value {
    match v {
        PlyValue::Int(i) => serde_json::json!({ "i": i }),
        PlyValue::Fixed(x) => serde_json::json!({
            "x": [x.ty.name(), i64::try_from(x.value()).ok()]
        }),
        PlyValue::Bool(b) => serde_json::json!({ "b": b }),
        PlyValue::Float(f) => serde_json::json!({ "f": f.to_string() }),
        PlyValue::Decimal(d) => serde_json::json!({ "d": d.to_string() }),
        PlyValue::Str(s) => serde_json::json!({ "s": s.as_ref() }),
        PlyValue::Bytes(b) => serde_json::json!({ "y": hex(b) }),
        PlyValue::Unit => serde_json::json!({ "u": 0 }),
        PlyValue::List(items) => {
            serde_json::json!({ "l": items.iter().map(value_to_wire).collect::<Vec<_>>() })
        }
        PlyValue::Map(m) => serde_json::json!({
            "m": m
                .iter()
                .map(|(k, v)| vec![value_to_wire(k), value_to_wire(v)])
                .collect::<Vec<_>>()
        }),
        PlyValue::Record(fields) => serde_json::json!({
            "r": fields
                .iter()
                .map(|(name, value)| (name.as_str().to_string(), value_to_wire(value)))
                .collect::<serde_json::Map<_, _>>()
        }),
        PlyValue::Ctor { name, args } => serde_json::json!({
            "c": [name.as_str(), serde_json::Value::Array(args.iter().map(value_to_wire).collect())]
        }),
        other => serde_json::json!({ "uncrossable": format!("{other:?}") }),
    }
}

pub fn value_from_wire(w: &serde_json::Value, span: Span) -> Result<PlyValue, Diagnostic> {
    let bad = |what: &str| {
        Diagnostic::error(codes::RUNTIME_ERROR, format!("a call's wire value {what}"))
            .primary(span, "not something `machine.Value` encodes")
    };
    let obj = w.as_object().ok_or_else(|| bad("is not an object"))?;
    if let Some(i) = obj.get("i") {
        return Ok(PlyValue::Int(
            i.as_i64().ok_or_else(|| bad("'s int is not one"))?,
        ));
    }
    if let Some(b) = obj.get("b") {
        return Ok(PlyValue::Bool(
            b.as_bool().ok_or_else(|| bad("'s bool is not one"))?,
        ));
    }
    if let Some(f) = obj.get("f") {
        let text = f.as_str().ok_or_else(|| bad("'s float is not text"))?;
        let f: f64 = text.parse().map_err(|_| bad("'s float does not parse"))?;
        return Ok(PlyValue::Float(f));
    }
    if let Some(d) = obj.get("d") {
        let text = d.as_str().ok_or_else(|| bad("'s decimal is not text"))?;
        return Ok(PlyValue::Decimal(
            text.parse().map_err(|_| bad("'s decimal does not parse"))?,
        ));
    }
    if let Some(x) = obj.get("x") {
        let pair = x.as_array().ok_or_else(|| bad("'s fixed is not a pair"))?;
        let ty = pair
            .first()
            .and_then(|t| t.as_str())
            .ok_or_else(|| bad("'s fixed type is not text"))?;
        let ty = ply_eval::IntTy::from_name(ty).ok_or_else(|| bad("'s fixed type is unknown"))?;
        let v = pair
            .get(1)
            .and_then(|v| v.as_i64())
            .ok_or_else(|| bad("'s fixed value does not fit an `Int`"))?;
        return Ok(PlyValue::Fixed(
            ply_eval::Fixed::of(ty, i128::from(v))
                .ok_or_else(|| bad("'s fixed value does not fit its type"))?,
        ));
    }
    if let Some(s) = obj.get("s") {
        return Ok(PlyValue::str(
            s.as_str().ok_or_else(|| bad("'s string is not one"))?,
        ));
    }
    if let Some(y) = obj.get("y") {
        let text = y.as_str().ok_or_else(|| bad("'s bytes are not hex text"))?;
        return Ok(PlyValue::bytes(
            unhex(text).ok_or_else(|| bad("'s bytes are not hex"))?,
        ));
    }
    if obj.contains_key("u") {
        return Ok(PlyValue::Unit);
    }
    if let Some(l) = obj.get("l") {
        let items = l.as_array().ok_or_else(|| bad("'s list is not one"))?;
        return Ok(PlyValue::list(
            items
                .iter()
                .map(|x| value_from_wire(x, span))
                .collect::<Result<_, _>>()?,
        ));
    }
    if let Some(r) = obj.get("r") {
        let fields = r.as_object().ok_or_else(|| bad("'s record is not one"))?;
        let mut out = Vec::new();
        for (name, value) in fields {
            out.push((Symbol::new(name.as_str()), value_from_wire(value, span)?));
        }
        return Ok(record_unsorted(out));
    }
    if let Some(c) = obj.get("c") {
        let pair = c.as_array().ok_or_else(|| bad("'s ctor is not a pair"))?;
        let name = pair
            .first()
            .and_then(|n| n.as_str())
            .ok_or_else(|| bad("'s ctor names nothing"))?;
        let args = pair
            .get(1)
            .and_then(|a| a.as_array())
            .ok_or_else(|| bad("'s ctor arguments are not a list"))?;
        return Ok(PlyValue::ctor(
            name,
            args.iter()
                .map(|x| value_from_wire(x, span))
                .collect::<Result<Vec<_>, _>>()?,
        ));
    }
    if let Some(m) = obj.get("m") {
        let entries = m.as_array().ok_or_else(|| bad("'s map is not a list"))?;
        let mut out = Vec::new();
        for entry in entries {
            let pair = entry
                .as_array()
                .ok_or_else(|| bad("'s entry is not a pair"))?;
            if pair.len() != 2 {
                return Err(bad("'s entry is not a pair"));
            }
            out.push((
                value_from_wire(&pair[0], span)?,
                value_from_wire(&pair[1], span)?,
            ));
        }
        return Ok(PlyValue::map(out));
    }
    Err(bad("names nothing"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    (0..bytes.len() / 2)
        .map(|i| {
            let hi = (bytes[2 * i] as char).to_digit(16)?;
            let lo = (bytes[2 * i + 1] as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

// The two hops the crossing takes: the program's `machine.Value` on the caller's thread, the
// wire on the way, the runtime value on the machine's.
pub fn adt_to_wire(
    v: &PlyValue,
    span: Span,
    module: &str,
) -> Result<serde_json::Value, Diagnostic> {
    Ok(value_to_wire(&value_of_adt(v, span, module)?))
}

pub fn wire_to_adt(
    w: &serde_json::Value,
    span: Span,
    module: &str,
) -> Result<PlyValue, Diagnostic> {
    machine_value(&value_from_wire(w, span)?, module)
}

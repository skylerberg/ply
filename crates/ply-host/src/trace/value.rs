//! Where a Ply value becomes something a sink can write.
//!
//! Every decode here runs on the **emit** path only: the handler asks
//! [`Sink::wants`] first, so a run with tracing off reaches none of it. That
//! ordering is the whole of §1.4's cost claim, and it is a property of the
//! caller rather than of this module — which is why the caller is one `match`
//! in one file.
//!
//! [`Sink::wants`]: super::Sink::wants

use super::sink::Field;
use super::{Level, Outcome};
use ply_eval::Value;
use ply_span::{Diagnostic, Span, Symbol, codes};

/// The module `std.trace` ships as. A constructor's identity in a `Value` is its
/// **program-wide** name, so `Info` is `std.trace.Info` and a decoder that
/// matched on the last segment would read another module's `Info` as this one's.
const MODULE: &str = super::MODULE;

/// The module `Json` ships in, for the one field that carries one.
const JSON_MODULE: &str = "std.json";

/// The simple name of a constructor a `Value` carries, when it is one of
/// `module`'s.
fn simple<'a>(name: &'a Symbol, module: &str) -> Option<&'a str> {
    name.as_str()
        .strip_prefix(module)
        .and_then(|rest| rest.strip_prefix('.'))
}

/// `Debug | Info | Warn | Error`.
///
/// Decoded **before** anything else, because it is what the level filter reads
/// and the filter runs before a name, a field or a clock is touched.
pub fn level(value: &Value, span: Span) -> Result<Level, Diagnostic> {
    match value {
        Value::Ctor { name, .. } => match simple(name, MODULE) {
            Some("Debug") => Ok(Level::Debug),
            Some("Info") => Ok(Level::Info),
            Some("Warn") => Ok(Level::Warn),
            Some("Error") => Ok(Level::Error),
            _ => Err(malformed(
                &format!("a `Level`, and `{name}` is not one"),
                span,
            )),
        },
        other => Err(malformed(
            &format!("a `Level`, and this is {}", other.type_name()),
            span,
        )),
    }
}

/// `Ok | Failed(String) | Abandoned`.
pub fn outcome(value: &Value, span: Span) -> Result<Outcome, Diagnostic> {
    match value {
        Value::Ctor { name, args } => match simple(name, MODULE) {
            Some("Ok") => Ok(Outcome::Ok),
            Some("Abandoned") => Ok(Outcome::Abandoned),
            Some("Failed") => match args.first() {
                Some(why) => Ok(Outcome::Failed(
                    why.as_str(span, "a `Failed` reason")?.to_string(),
                )),
                None => Err(malformed("a `Failed` with a reason", span)),
            },
            _ => Err(malformed(
                &format!("an `Outcome`, and `{name}` is not one"),
                span,
            )),
        },
        other => Err(malformed(
            &format!("an `Outcome`, and this is {}", other.type_name()),
            span,
        )),
    }
}

/// `{ id: Int, channel: String }` — the span a `trace.exit` names.
///
/// The channel is read but not trusted: the driver compares it against the
/// channel the span was opened on, because a `Span` is an ordinary record and a
/// program can build one.
pub fn span_id(value: &Value, span: Span) -> Result<i64, Diagnostic> {
    match value {
        Value::Record(fields) => match fields.get(&super::ID) {
            Some(id) => id.as_int(span, "a `Span`'s `id`"),
            None => Err(malformed("a `Span` with an `id` field", span)),
        },
        other => Err(malformed(
            &format!("a `Span`, and this is {}", other.type_name()),
            span,
        )),
    }
}

/// `Map<String, Field>`, in the map's own ascending key order.
///
/// That order is what makes a golden test over a trace line stable: two field
/// sets built in different orders are one `Map` and render identically.
pub fn fields(value: &Value, span: Span) -> Result<Vec<(String, Field)>, Diagnostic> {
    let entries = value.as_map(span, "a `Fields` map")?;
    let mut out = Vec::with_capacity(entries.size());
    for (key, field) in entries.iter() {
        out.push((
            key.as_str(span, "a field name")?.to_string(),
            decode_field(field, span)?,
        ));
    }
    Ok(out)
}

fn decode_field(value: &Value, span: Span) -> Result<Field, Diagnostic> {
    let Value::Ctor { name, args } = value else {
        return Err(malformed(
            &format!("a `Field`, and this is {}", value.type_name()),
            span,
        ));
    };
    let arg = |n: usize| -> Result<&Value, Diagnostic> {
        args.get(n)
            .ok_or_else(|| malformed(&format!("`{name}` with an argument"), span))
    };
    let Some(tag) = simple(name, MODULE) else {
        return Err(malformed(
            &format!("a `Field`, and `{name}` is not one of `std.trace`'s constructors"),
            span,
        ));
    };
    Ok(match tag {
        "FInt" => Field::Int(arg(0)?.as_int(span, "an `FInt`")?),
        "FBool" => Field::Bool(arg(0)?.as_bool(span, "an `FBool`")?),
        "FText" => Field::Text(arg(0)?.as_str(span, "an `FText`")?.to_string()),
        "FFloat" => Field::Float(arg(0)?.as_float(span, "an `FFloat`")?),
        "FDecimal" => Field::Decimal(arg(0)?.as_decimal(span, "an `FDecimal`")?),
        "FBytes" => Field::Bytes(arg(0)?.as_bytes(span, "an `FBytes`")?.to_vec()),
        "FJson" => {
            let mut out = String::new();
            write_json(&mut out, arg(0)?, span, 0)?;
            Field::Json(out)
        }
        other => {
            return Err(malformed(
                &format!("a `Field`, and `{other}` is not one"),
                span,
            ));
        }
    })
}

/// The bound on how deep a `json::Json` field may nest before the writer
/// refuses.
///
/// A recursive writer over a value a program built is host recursion with no
/// budget on it, and a deep enough value would abort the process from inside a
/// log line. The bound is generous by two orders of magnitude against any
/// document an operator reads.
const MAX_JSON_DEPTH: usize = 64;

/// `json::Json`, straight to its serialized form.
///
/// There is one JSON writer in this crate and it is this plus
/// [`super::sink::write_json`]; a second model of a `json::Json` in the trusted
/// computing base would be a second thing to keep in agreement with `std.json`.
fn write_json(out: &mut String, value: &Value, span: Span, depth: usize) -> Result<(), Diagnostic> {
    if depth >= MAX_JSON_DEPTH {
        return Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("a trace field holds a JSON value nested deeper than {MAX_JSON_DEPTH}"),
        )
        .primary(span, "this perform reached the trace sink")
        .note("the writer is recursive and a log line is not a reason to grow the host stack without a bound")
        .note("flatten the value, or attach the part of it the record is about"));
    }
    let Value::Ctor { name, args } = value else {
        return Err(malformed(
            &format!("a `json::Json`, and this is {}", value.type_name()),
            span,
        ));
    };
    match (simple(name, JSON_MODULE).unwrap_or(""), args.len()) {
        ("Null", 0) => out.push_str("null"),
        ("Bool", 1) => out.push_str(if args[0].as_bool(span, "a JSON boolean")? {
            "true"
        } else {
            "false"
        }),
        ("Number", 1) => out.push_str(&args[0].as_decimal(span, "a JSON number")?.to_string()),
        ("Str", 1) => super::sink::write_string(out, args[0].as_str(span, "a JSON string")?),
        ("Array", 1) => {
            out.push('[');
            for (i, item) in args[0].as_list(span, "a JSON array")?.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(out, item, span, depth + 1)?;
            }
            out.push(']');
        }
        ("Object", 1) => {
            out.push('{');
            for (i, (key, item)) in args[0].as_map(span, "a JSON object")?.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                super::sink::write_string(out, key.as_str(span, "a JSON object's key")?);
                out.push(':');
                write_json(out, item, span, depth + 1)?;
            }
            out.push('}');
        }
        (other, _) => {
            return Err(malformed(
                &format!("a `json::Json`, and `{other}` is not one of its constructors"),
                span,
            ));
        }
    }
    Ok(())
}

/// Inference checks a perform's shape, so reaching one of these means the
/// evaluator ran a module that was never checked. Ply's fault, and it says so.
#[cold]
fn malformed(wanted: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a `trace` operation was performed with a value that is not {wanted}"),
    )
    .primary(span, "this perform reached the trace sink")
    .note("inference checks a perform's argument types, so reaching this means the evaluator was handed a module that was never checked")
    .note("this is Ply's fault: report it with the program that produced it")
}

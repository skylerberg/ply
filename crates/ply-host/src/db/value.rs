//! Where a Ply value becomes a parameter and a row becomes a Ply value.

use super::scope::{Access, Isolation};
use super::stmt::{Answer, Row};
use super::types::{Datum, DbError, Json, Param};
use ply_eval::Value;
use ply_span::{Diagnostic, Span, Symbol, codes};
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The module `Json` ships in.
const JSON_MODULE: &str = "std.json";

/// A constructor of `module`, as the qualified symbol a `Value` carries.
fn ctor(module: &str, name: &str) -> Symbol {
    Symbol::new(format!("{module}.{name}"))
}

/// The simple name of a constructor a `Value` carries, when it is one of `module`'s.
fn simple<'a>(name: &'a Symbol, module: &str) -> Option<&'a str> {
    name.as_str()
        .strip_prefix(module)
        .and_then(|rest| rest.strip_prefix('.'))
}

/// `{ sql: String }` — the statement text, and nothing else in the record.
pub fn statement(value: &Value, span: Span) -> Result<String, Diagnostic> {
    match value {
        Value::Record(fields) => match fields.get(&Symbol::new("sql")) {
            Some(sql) => Ok(sql.as_str(span, "a statement's `sql`")?.to_string()),
            None => Err(malformed("a `Stmt` with no `sql` field", span)),
        },
        other => Err(malformed(
            &format!("a `Stmt`, and this is {}", other.type_name()),
            span,
        )),
    }
}

pub fn params(value: &Value, span: Span) -> Result<Vec<Param>, Diagnostic> {
    let items = value.as_list(span, "a list of parameters")?;
    items.iter().map(|item| param(item, span)).collect()
}

pub fn param(value: &Value, span: Span) -> Result<Param, Diagnostic> {
    let Value::Ctor { name, args } = value else {
        return Err(malformed(
            &format!("a `Param`, and this is {}", value.type_name()),
            span,
        ));
    };
    let arg = |n: usize| -> Result<&Value, Diagnostic> {
        args.get(n)
            .ok_or_else(|| malformed(&format!("`{name}` with an argument"), span))
    };
    let Some(tag) = simple(name, super::MODULE) else {
        return Err(malformed(
            &format!("a `Param`, and `{name}` is not one of `std.db`'s constructors"),
            span,
        ));
    };
    Ok(match tag {
        "PNull" => Param::Null,
        "PInt" => Param::Int(arg(0)?.as_int(span, "a `PInt`")?),
        "PBool" => Param::Bool(arg(0)?.as_bool(span, "a `PBool`")?),
        "PText" => Param::Text(arg(0)?.as_str(span, "a `PText`")?.to_string()),
        "PBytes" => Param::Bytes(arg(0)?.as_bytes(span, "a `PBytes`")?.to_vec()),
        "PFloat" => Param::Float(arg(0)?.as_float(span, "a `PFloat`")?),
        "PNumeric" => Param::Numeric(arg(0)?.as_decimal(span, "a `PNumeric`")?),
        "PJson" => Param::Json(json(arg(0)?, span)?),
        "PArray" => {
            let items = arg(0)?.as_list(span, "a `PArray`")?;
            Param::Array(
                items
                    .iter()
                    .map(|item| param(item, span))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        other => {
            return Err(malformed(
                &format!("a `Param`, and `{other}` is not one"),
                span,
            ));
        }
    })
}

pub fn json(value: &Value, span: Span) -> Result<Json, Diagnostic> {
    Ok(match value {
        Value::Ctor { name, args } => match (simple(name, JSON_MODULE).unwrap_or(""), args.len()) {
            ("Null", 0) => Json::Null,
            ("Bool", 1) => Json::Bool(args[0].as_bool(span, "a JSON boolean")?),
            ("Number", 1) => Json::Number(args[0].as_decimal(span, "a JSON number")?),
            ("Str", 1) => Json::Str(args[0].as_str(span, "a JSON string")?.to_string()),
            ("Array", 1) => {
                let items = args[0].as_list(span, "a JSON array")?;
                Json::Array(
                    items
                        .iter()
                        .map(|item| json(item, span))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            ("Object", 1) => {
                let entries = args[0].as_map(span, "a JSON object")?;
                let mut fields = Vec::new();
                for (key, value) in entries.iter() {
                    fields.push((
                        key.as_str(span, "a JSON object's key")?.to_string(),
                        json(value, span)?,
                    ));
                }
                Json::Object(fields)
            }
            (other, _) => {
                return Err(malformed(
                    &format!("a `json::Json`, and `{other}` is not one of its constructors"),
                    span,
                ));
            }
        },
        other => {
            return Err(malformed(
                &format!("a `json::Json`, and this is {}", other.type_name()),
                span,
            ));
        }
    })
}

/// `ReadCommitted | RepeatableRead | Serializable`, as the `SET TRANSACTION` text postgres wants.
pub fn isolation(value: &Value, span: Span) -> Result<Isolation, Diagnostic> {
    match value {
        Value::Ctor { name, .. } => {
            match simple(name, super::MODULE).and_then(Isolation::from_ctor) {
                Some(level) => Ok(level),
                None => Err(malformed(
                    &format!("an `Isolation`, and `{name}` is not one"),
                    span,
                )),
            }
        }
        other => Err(malformed(
            &format!("an `Isolation`, and this is {}", other.type_name()),
            span,
        )),
    }
}

pub fn access(value: &Value, span: Span) -> Result<Access, Diagnostic> {
    match value {
        Value::Ctor { name, .. } => match simple(name, super::MODULE).and_then(Access::from_ctor) {
            Some(access) => Ok(access),
            None => Err(malformed(
                &format!("an `Access`, and `{name}` is not one"),
                span,
            )),
        },
        other => Err(malformed(
            &format!("an `Access`, and this is {}", other.type_name()),
            span,
        )),
    }
}

// --- outward ----------------------------------------------------------------

pub fn answer(answer: &Answer) -> Value {
    match answer {
        Answer::Rows(rows) => Value::ctor(
            ctor(super::MODULE, "Rows"),
            vec![Value::list(rows.iter().map(row).collect())],
        ),
        Answer::Count(n) => Value::ctor(ctor(super::MODULE, "Count"), vec![Value::Int(*n)]),
        Answer::Failed(e) => Value::ctor(ctor(super::MODULE, "Failed"), vec![error(e)]),
    }
}

/// A `Map` from column name to value, which is what gives a row one canonical form: two rows built
/// in different column orders are one value, and a golden test over a result set is stable.
pub fn row(row: &Row) -> Value {
    Value::map(
        row.iter()
            .map(|(name, value)| (Value::str(name), datum(value))),
    )
}

pub fn datum(value: &Datum) -> Value {
    match value {
        Datum::Null => Value::ctor(ctor(super::MODULE, "CNull"), Vec::new()),
        Datum::Int(v) => Value::ctor(ctor(super::MODULE, "CInt"), vec![Value::Int(*v)]),
        Datum::Bool(v) => Value::ctor(ctor(super::MODULE, "CBool"), vec![Value::Bool(*v)]),
        Datum::Text(v) => Value::ctor(ctor(super::MODULE, "CText"), vec![Value::str(v)]),
        Datum::Bytes(v) => Value::ctor(ctor(super::MODULE, "CBytes"), vec![Value::bytes(v)]),
        Datum::Float(v) => Value::ctor(ctor(super::MODULE, "CFloat"), vec![Value::Float(*v)]),
        Datum::Numeric(v) => Value::ctor(ctor(super::MODULE, "CNumeric"), vec![Value::Decimal(*v)]),
        Datum::Json(v) => Value::ctor(ctor(super::MODULE, "CJson"), vec![json_value(v)]),
        Datum::Array(items) => Value::ctor(
            ctor(super::MODULE, "CArray"),
            vec![Value::list(items.iter().map(datum).collect())],
        ),
    }
}

pub fn json_value(value: &Json) -> Value {
    match value {
        Json::Null => Value::ctor(ctor(JSON_MODULE, "Null"), Vec::new()),
        Json::Bool(b) => Value::ctor(ctor(JSON_MODULE, "Bool"), vec![Value::Bool(*b)]),
        Json::Number(d) => Value::ctor(ctor(JSON_MODULE, "Number"), vec![Value::Decimal(*d)]),
        Json::Str(s) => Value::ctor(ctor(JSON_MODULE, "Str"), vec![Value::str(s)]),
        Json::Array(items) => Value::ctor(
            ctor(JSON_MODULE, "Array"),
            vec![Value::list(items.iter().map(json_value).collect())],
        ),
        Json::Object(fields) => Value::ctor(
            ctor(JSON_MODULE, "Object"),
            vec![Value::map(
                fields
                    .iter()
                    .map(|(key, value)| (Value::str(key), json_value(value))),
            )],
        ),
    }
}

pub fn error(e: &DbError) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert(Symbol::new("code"), Value::str(&e.code));
    fields.insert(Symbol::new("constraint"), Value::str(&e.constraint));
    fields.insert(Symbol::new("detail"), Value::str(&e.detail));
    Value::Record(Arc::new(fields))
}

/// A `Failed` answer built from a SQLSTATE the driver produced rather than the server: a pool that
/// could not reach anything, a connection that went away.
pub fn failed(code: &str, detail: impl Into<String>) -> Value {
    answer(&Answer::Failed(DbError::new(code, "", detail)))
}

/// The `Decimal` a `Number` carries, for a caller that wants it without the wrapper.
pub fn as_decimal(value: &Value) -> Option<Decimal> {
    match value {
        Value::Decimal(d) => Some(*d),
        _ => None,
    }
}

/// Inference checks a perform's shape, so reaching one of these means the evaluator ran a module
/// that was never checked.
#[cold]
fn malformed(wanted: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a `db` operation was performed with a value that is not {wanted}"),
    )
    .primary(span, "this perform reached the database driver")
    .note("inference checks a perform's argument types, so reaching this means the evaluator was handed a module that was never checked")
    .note("this is Ply's fault: report it with the program that produced it")
}

#[cfg(test)]
mod tests;

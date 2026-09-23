//! Ply values, as a lent effect hands them to the program in `crates/ply-cli/ply`.

use ply_eval::Value as PlyValue;
use ply_span::{Diagnostic, Severity, SourceMap, Symbol};
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

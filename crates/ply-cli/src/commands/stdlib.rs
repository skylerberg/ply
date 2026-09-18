//! `ply std` — the modules that ship with the compiler.

use super::common::{IND, diagnostics_json, emit_json, plural, print_diagnostics};
use crate::cli::StdArgs;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_span::{Diagnostic, SourceId, SourceMap, Span, Symbol, codes};
use ply_syntax::ast::ModuleName;
use serde_json::{Value, json};

/// The `--json` object's shape.
pub const SCHEMA_VERSION: u32 = 1;

pub struct Row {
    name: String,
    pub definitions: usize,
    tests: usize,
    bytes: usize,
}

pub fn execute(args: &StdArgs, style: Style) -> i32 {
    if args.digest {
        println!("{}", ply_std::digest_short());
        return EXIT_OK;
    }

    if let Some(wanted) = &args.show {
        return match ply_std::source(&ModuleName::from_dotted(wanted)) {
            Some(source) => {
                print!("{source}");
                EXIT_OK
            }
            None => {
                let diagnostic =
                    ply_std::unknown_module(&ModuleName::from_dotted(wanted), Span::DUMMY);
                report(std::slice::from_ref(&diagnostic), args.json, style)
            }
        };
    }

    let rows = match rows() {
        Ok(rows) => rows,
        Err(diagnostics) => return report(&diagnostics, args.json, style),
    };

    if args.json {
        emit_json(&json!({
            "command": "std",
            "schema_version": SCHEMA_VERSION,
            "ok": true,
            "exit_code": EXIT_OK,
            "modules": Value::Array(rows.iter().map(row_json).collect()),
            "definitions": rows.iter().map(|r| r.definitions).sum::<usize>(),
            "digest": ply_std::digest_short(),
            "diagnostics": Value::Array(Vec::new()),
        }));
        return EXIT_OK;
    }

    println!();
    for line in lines(&rows) {
        if line.is_empty() {
            println!();
        } else {
            println!("{IND}{line}");
        }
    }
    EXIT_OK
}

fn report(diagnostics: &[Diagnostic], json: bool, style: Style) -> i32 {
    let sources = SourceMap::new();
    if json {
        emit_json(&json!({
            "command": "std",
            "schema_version": SCHEMA_VERSION,
            "ok": false,
            "exit_code": EXIT_COMPILE_ERROR,
            "diagnostics": diagnostics_json(diagnostics, &sources),
        }));
    } else {
        print_diagnostics(diagnostics, &sources, style);
    }
    EXIT_COMPILE_ERROR
}

/// Counting definitions needs a front end, so this command goes through the same one every other
/// does — the port, asked once over the shipped modules together (ADR 0052 §1). A shipped module
/// the compiler's own front end refuses is Ply's fault: the user cannot have caused it and cannot
/// fix it.
pub fn rows() -> Result<Vec<Row>, Vec<Diagnostic>> {
    let sources: Vec<(String, String)> = ply_std::sources()
        .map(|(name, source)| (name.to_string(), source.to_string()))
        .collect();
    let ids: Vec<SourceId> = (0..sources.len()).map(|i| SourceId(i as u32)).collect();
    let front = ply_codegen::c::producer::checked_front(&sources, &ids)
        .map_err(|e| vec![shipped_refused(&format!("{e:#}"))])?;

    Ok(sources
        .iter()
        .map(|(name, source)| {
            let module = Symbol::new(name);
            // `ModuleInfo::items` names every declaration a reference could reach and, for a sum
            // type, its constructors beside it — and a constructor is reached through its type
            // rather than declared on its own.
            let items = front
                .check
                .modules
                .get(&module)
                .map_or(0, |m| m.items.len());
            let ctors = front
                .check
                .ctors
                .values()
                .filter(|c| c.module.as_symbol() == &module)
                .count();
            Row {
                name: name.clone(),
                definitions: items.saturating_sub(ctors),
                tests: front
                    .check
                    .tests
                    .iter()
                    .filter(|t| t.module.as_symbol() == &module)
                    .count(),
                bytes: source.len(),
            }
        })
        .collect())
}

fn shipped_refused(why: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the shipped modules do not compile: {why}"),
    )
    .note("this is a defect in the compiler's own sources, not in this program")
    .note("please report it with the version of `ply` that produced it")
}

const HEADERS: [&str; 4] = ["MODULE", "DEFINITIONS", "TESTS", "BYTES"];

fn cells(row: &Row) -> [String; 4] {
    [
        row.name.clone(),
        row.definitions.to_string(),
        row.tests.to_string(),
        row.bytes.to_string(),
    ]
}

/// Every line without the indent, so the shape is testable without a terminal.
pub fn lines(rows: &[Row]) -> Vec<String> {
    let definitions: usize = rows.iter().map(|r| r.definitions).sum();
    let mut lines = vec![
        format!(
            "{} {} · {definitions} {} · shipped with this compiler",
            rows.len(),
            plural(rows.len(), "module"),
            plural(definitions, "definition"),
        ),
        String::new(),
    ];

    if rows.is_empty() {
        lines.push("this build ships no stdlib module".to_string());
    } else {
        let cells: Vec<[String; 4]> = rows.iter().map(cells).collect();
        let mut widths = HEADERS.map(str::len);
        for row in &cells {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.chars().count());
            }
        }
        let line = |cells: &[String; 4]| {
            let mut out = String::new();
            for (i, (cell, width)) in cells.iter().zip(widths).enumerate() {
                if i + 1 == cells.len() {
                    out.push_str(cell);
                } else {
                    out.push_str(&format!("{cell:<width$}  "));
                }
            }
            out
        };
        lines.push(line(&HEADERS.map(str::to_string)));
        lines.extend(cells.iter().map(line));
    }

    lines.push(String::new());
    lines.push("`import std.<name>` to use one; `ply std --show <name>` prints its source".into());
    lines.push(String::new());
    lines.push(format!("digest: {}", ply_std::digest_short()));
    lines
}

fn row_json(row: &Row) -> Value {
    json!({
        "module": row.name,
        "definitions": row.definitions,
        "tests": row.tests,
        "bytes": row.bytes,
    })
}

//! `ply std` — the modules that ship with the compiler.

use super::common::{IND, diagnostics_json, emit_json, plural, print_diagnostics};
use crate::cli::StdArgs;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_span::{Diagnostic, SourceId, SourceMap, Span, codes};
use ply_syntax::ast::{Item, ModuleName};
use serde_json::{Value, json};

/// The `--json` object's shape.
pub const SCHEMA_VERSION: u32 = 1;

pub struct Row {
    name: String,
    definitions: usize,
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

/// Counting definitions needs a parse, and a shipped module that does not parse is Ply's fault: the
/// user cannot have caused it and cannot fix it.
fn rows() -> Result<Vec<Row>, Vec<Diagnostic>> {
    let mut out = Vec::new();
    let mut diagnostics = Vec::new();
    for (i, (name, source)) in ply_std::sources().enumerate() {
        let module = ModuleName::from_dotted(name);
        match ply_syntax::parse_module(SourceId(i as u32), module, source) {
            Ok(parsed) => out.push(Row {
                name: name.to_string(),
                definitions: parsed
                    .items
                    .iter()
                    .filter(|item| item.name().is_some())
                    .count(),
                tests: parsed
                    .items
                    .iter()
                    .filter(|item| matches!(item, Item::Test(_)))
                    .count(),
                bytes: source.len(),
            }),
            Err(_) => diagnostics.push(
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!("the shipped module `{name}` does not parse"),
                )
                .note("this is a defect in the compiler's own sources, not in this program")
                .note("please report it with the version of `ply` that produced it"),
            ),
        }
    }
    if diagnostics.is_empty() {
        Ok(out)
    } else {
        Err(diagnostics)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser;

    #[test]
    fn the_flags_parse_and_default_to_the_listing() {
        let args = match Cli::parse_from(["ply", "std"]).command {
            Command::Std(args) => args,
            other => panic!("expected `std`, got {other:?}"),
        };
        assert!(!args.json);
        assert!(!args.digest);
        assert_eq!(args.show, None);
    }

    /// `--digest` is the one-line form a CI check pins, so it may not also carry a table for a
    /// machine.
    #[test]
    fn digest_and_json_cannot_both_be_asked_for() {
        assert!(Cli::try_parse_from(["ply", "std", "--digest", "--json"]).is_err());
    }

    #[test]
    fn the_listing_names_every_shipped_module_and_ends_with_the_digest() {
        let rows = rows().expect("every shipped module parses");
        assert_eq!(rows.len(), ply_std::MODULES.len());
        let text = lines(&rows).join("\n");
        for (name, _) in ply_std::sources() {
            assert!(text.contains(name), "`{name}` is missing from:\n{text}");
        }
        assert!(
            lines(&rows).last().unwrap().starts_with("digest: b3:"),
            "{text}"
        );
        assert!(rows.iter().all(|r| r.definitions > 0), "a module is empty");
    }

    /// Two runs of one binary have to agree byte for byte, or pinning the digest in CI pins
    /// nothing.
    #[test]
    fn the_listing_is_stable_across_runs() {
        let once = lines(&rows().unwrap());
        let twice = lines(&rows().unwrap());
        assert_eq!(once, twice);
        assert_eq!(ply_std::digest_short(), ply_std::digest_short());
    }
}

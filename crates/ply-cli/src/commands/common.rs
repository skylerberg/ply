use crate::load::LoadError;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_span::{Diagnostic, Severity, SourceMap, Span};
use serde_json::{Value, json};

/// The gutter the specified output shape is indented by.
pub const IND: &str = "   ";

pub fn diagnostic_json(diagnostic: &Diagnostic, sources: &SourceMap) -> Value {
    serde_json::to_value(ply_span::render::to_json(diagnostic, sources))
        .unwrap_or_else(|e| json!({ "code": diagnostic.code, "message": diagnostic.message, "render_error": e.to_string() }))
}

pub fn diagnostics_json(diagnostics: &[Diagnostic], sources: &SourceMap) -> Value {
    Value::Array(diagnostics.iter().map(|d| diagnostic_json(d, sources)).collect())
}

pub fn location(sources: &SourceMap, span: Span) -> Option<String> {
    let file = sources.get(span.source)?;
    let (line, col) = file.line_col(span.start);
    Some(format!("{}:{line}:{col}", file.path.display()))
}

pub fn print_diagnostics(diagnostics: &[Diagnostic], sources: &SourceMap, style: Style) {
    let rendered = ply_span::render::all_to_terminal(diagnostics, sources);
    eprint!("{}", style.sanitize(&rendered));
}

/// Cache trouble and scheduling trouble are not the user's program misbehaving,
/// so they are one indented line each rather than a full report.
pub fn print_warnings(warnings: &[Diagnostic], style: Style) {
    for w in warnings {
        let label = match w.severity {
            Severity::Error => style.red("error"),
            Severity::Warning => style.yellow("warning"),
            Severity::Note => style.dim("note"),
        };
        println!("{IND}{label}: {}", w.message);
        for note in &w.notes {
            println!("{IND}  {} {note}", style.dim("="));
        }
    }
}

/// Every command fails the same way, so an agent can key off `command` and
/// `exit_code` without knowing which one it asked for.
pub fn report_load_error(command: &str, err: &LoadError, json: bool, style: Style) -> i32 {
    if json {
        emit_json(&json!({
            "command": command,
            "ok": false,
            "exit_code": EXIT_COMPILE_ERROR,
            "diagnostics": diagnostics_json(&err.diagnostics, &err.sources),
        }));
    } else {
        print_diagnostics(&err.diagnostics, &err.sources, style);
        let n = err.diagnostics.len();
        eprintln!("{IND}{} ({n} {})", style.red("compilation failed"), plural(n, "error"));
    }
    EXIT_COMPILE_ERROR
}

/// The one place a `--json` command writes to stdout, so "exactly one object and
/// nothing else" is checkable by reading this file.
pub fn emit_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        // Serialization of a tree we built ourselves cannot fail, but printing a
        // half-object would break the one guarantee `--json` makes.
        Err(e) => println!("{{\"ok\":false,\"error\":\"could not serialize the report: {e}\"}}"),
    }
}

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 { word.to_string() } else { format!("{word}s") }
}

pub fn exit_code(ok: bool) -> i32 {
    if ok { EXIT_OK } else { crate::EXIT_FAILED }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_span::codes;

    #[test]
    fn location_is_one_based_and_names_the_file() {
        let mut sources = SourceMap::new();
        let id = sources.add("src/ledger.ply", "fn f() = 1\nfn g() = 2\n");
        assert_eq!(location(&sources, Span::new(id, 11, 13)).unwrap(), "src/ledger.ply:2:1");
    }

    #[test]
    fn a_dummy_span_has_no_location_rather_than_a_made_up_one() {
        let sources = SourceMap::new();
        assert_eq!(location(&sources, Span::DUMMY), None);
    }

    #[test]
    fn diagnostic_json_carries_positions_not_raw_offsets() {
        let mut sources = SourceMap::new();
        let id = sources.add("t.ply", "fn f() = 1 + true\n");
        let d = Diagnostic::error(codes::TYPE_MISMATCH, "type mismatch")
            .primary(Span::new(id, 13, 17), "expected Int, found Bool");
        let v = diagnostic_json(&d, &sources);
        assert_eq!(v["code"], "E0201");
        assert_eq!(v["labels"][0]["start"]["line"], 1);
        assert_eq!(v["labels"][0]["snippet"], "true");
    }

    #[test]
    fn plurals_do_not_say_one_errors() {
        assert_eq!(plural(1, "error"), "error");
        assert_eq!(plural(0, "error"), "errors");
        assert_eq!(plural(2, "group"), "groups");
    }
}

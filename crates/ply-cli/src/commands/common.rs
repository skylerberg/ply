use crate::load::LoadError;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_machine::support::plural;
use ply_span::{Diagnostic, Severity, SourceMap, Span};
use serde_json::{Value, json};

pub const IND: &str = "   ";

pub fn diagnostic_json(diagnostic: &Diagnostic, sources: &SourceMap) -> Value {
    serde_json::to_value(ply_span::render::to_json(diagnostic, sources))
        .unwrap_or_else(|e| json!({ "code": diagnostic.code, "message": diagnostic.message, "render_error": e.to_string() }))
}

pub fn diagnostics_json(diagnostics: &[Diagnostic], sources: &SourceMap) -> Value {
    Value::Array(
        diagnostics
            .iter()
            .map(|d| diagnostic_json(d, sources))
            .collect(),
    )
}

pub fn location(sources: &SourceMap, span: Span) -> Option<String> {
    let file = sources.get(span.source)?;
    let (line, col) = file.line_col(span.start);
    Some(format!("{}:{line}:{col}", file.path.display()))
}

pub fn print_diagnostics(diagnostics: &[Diagnostic], sources: &SourceMap, style: Style) {
    eprint!(
        "{}",
        ply_span::render::all_to_terminal(diagnostics, sources, style.is_styled())
    );
}

/// Cache and scheduling trouble is one indented line each, not a full report.
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

/// One shape for every command, so an agent can key off `command` and `exit_code`.
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
        let n = err
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        eprintln!(
            "{IND}{} ({n} {})",
            style.red("compilation failed"),
            plural(n, "error")
        );
    }
    EXIT_COMPILE_ERROR
}

/// A registration that does not match the program is a start-up failure: nothing ran.
pub fn report_bind_error(
    command: &str,
    diagnostics: &[Diagnostic],
    sources: &SourceMap,
    json: bool,
    style: Style,
) -> i32 {
    if json {
        emit_json(&json!({
            "command": command,
            "ok": false,
            "exit_code": EXIT_COMPILE_ERROR,
            "binding": "host",
            "diagnostics": diagnostics_json(diagnostics, sources),
        }));
    } else {
        print_diagnostics(diagnostics, sources, style);
        let n = diagnostics.len();
        eprintln!(
            "{IND}{} ({n} {})",
            style.red("no host handler was bound"),
            plural(n, "error")
        );
    }
    EXIT_COMPILE_ERROR
}

/// The one place a `--json` command writes to stdout. Compact, because the answer is for a
/// machine, and because the commands the shipped program answers write it that way.
pub fn emit_json(value: &Value) {
    match serde_json::to_string(value) {
        Ok(text) => println!("{text}"),
        // A half-object would break the one guarantee `--json` makes.
        Err(e) => println!("{{\"ok\":false,\"error\":\"could not serialize the report: {e}\"}}"),
    }
}

pub fn phases_json(phases: &crate::driver::Phases) -> Value {
    let mut out = serde_json::Map::new();
    for (label, taken) in phases.labelled() {
        out.insert(label.replace(' ', "_"), json!(millis(taken)));
    }
    out.insert("total".to_string(), json!(millis(phases.total())));
    Value::Object(out)
}

/// Longest first, so what dominated a run is read first.
pub fn print_phases(phases: &crate::driver::Phases, style: Style) {
    println!();
    println!("{IND}{}", style.bold("front-end time"));
    let total = phases.total().as_secs_f64();
    let mut rows = phases.labelled();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    for (label, taken) in rows {
        let share = if total > 0.0 {
            taken.as_secs_f64() / total * 100.0
        } else {
            0.0
        };
        println!(
            "{IND}  {label:<11} {:>8.2}ms {}",
            millis(taken),
            style.dim(&format!("{share:>5.1}%"))
        );
    }
    println!("{IND}  {:<11} {:>8.2}ms", "total", millis(phases.total()));
}

pub fn millis(d: std::time::Duration) -> f64 {
    (d.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

pub fn exit_code(ok: bool) -> i32 {
    if ok { EXIT_OK } else { crate::EXIT_FAILED }
}

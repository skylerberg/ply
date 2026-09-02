use crate::load::LoadError;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_span::{Diagnostic, Severity, SourceMap, Span, codes};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The gutter the specified output shape is indented by.
pub const IND: &str = "   ";

/// `--backend`'s value as a spec, or the diagnostic that refuses it.
pub fn backend_spec(flag: Option<&String>) -> Result<Option<ply_eval::BackendSpec>, Diagnostic> {
    let Some(spec) = flag else {
        return Ok(None);
    };
    ply_eval::backend::parse(spec).map(Some).map_err(|message| {
        Diagnostic::error(codes::BACKEND_UNAVAILABLE, message).note(
            "a wrong backend is a self-test: it exists so that a green run with a backend \
             attached can be read as evidence",
        )
    })
}

/// The engine a run under `spec` selects against and records under, named before a provider
/// exists — selection is what decides whether building one is worth anything.
/// `a_commands_engine_is_the_one_the_run_records_under` pins that this agrees with what the
/// executor answers once the provider is built.
pub fn engine_of(spec: Option<&ply_eval::BackendSpec>) -> ply_test::Engine {
    let Some(spec) = spec else {
        return ply_test::Engine::Evaluator;
    };
    let (name, variant) = match spec.kind {
        ply_eval::BackendKind::Reference => ("reference", ""),
        ply_eval::BackendKind::Cranelift => ("cranelift", ply_codegen::backend::registry_width()),
    };
    ply_test::Engine::of_backend(name, variant, spec)
}

/// The run's backend, built once over a checked program, or the diagnostic that refuses it.
pub fn build_backend(
    spec: &ply_eval::BackendSpec,
    program: &ply_syntax::ast::Program,
    resolved: &ply_syntax::resolve::Resolved,
    check: &ply_core::CheckOutput,
) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
    match spec.kind {
        ply_eval::BackendKind::Reference => Ok(ply_eval::Fragment::over(program, resolved, check)),
        ply_eval::BackendKind::Cranelift => ply_codegen::Cranelift::over(program, resolved, check)
            .map(|unit| unit as &'static dyn ply_eval::Provider)
            .map_err(|error| {
                Diagnostic::error(
                    codes::BACKEND_UNAVAILABLE,
                    format!("the cranelift backend could not be built: {error:#}"),
                )
                .note(
                    "a backend that failed to build would decline every call, so the run is \
                     refused rather than reported green over a seam nothing reached",
                )
                .note("`--backend reference` needs no code generator and runs anywhere")
            }),
    }
}

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
    let rendered = ply_span::render::all_to_terminal(diagnostics, sources);
    eprint!("{}", style.sanitize(&rendered));
}

/// One unreadable file is found more than once — a lazy read consults it, and a later flush
/// re-reads it to merge — and each drain reports what it found.
pub fn once_each(warnings: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut seen: BTreeSet<(&'static str, String)> = BTreeSet::new();
    warnings
        .into_iter()
        .filter(|d| seen.insert((d.code, d.message.clone())))
        .collect()
}

/// Cache trouble and scheduling trouble are not the user's program misbehaving, so they are one
/// indented line each rather than a full report.
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

/// Every command fails the same way, so an agent can key off `command` and `exit_code` without
/// knowing which one it asked for.
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

/// A registration that does not match the program is the host author's bug, not the program's, and
/// it is a start-up failure: nothing ran, so the report is diagnostics and the binding that was
/// asked for.
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

/// Fill in the table and column counts of the `--db-schema` function, for the `database` block.
pub fn describe_schema(loaded: &crate::load::Loaded, hosts: &mut crate::hosts::Hosts) {
    let Some(name) = hosts.schema_function().map(str::to_string) else {
        return;
    };
    hosts.describe_schema(materialise_schema(loaded, &name));
}

/// Evaluate a resolved `--db-schema` function and read its size.
pub fn materialise_schema(
    loaded: &crate::load::Loaded,
    name: &str,
) -> Option<crate::db::schema::Shape> {
    let def = loaded
        .check
        .defs
        .values()
        .find(|d| d.name.as_str() == name)?;
    ply_eval::Machine::new(&loaded.program, &loaded.resolved, &loaded.check)
        .call(name, Vec::new(), def.span)
        .ok()
        .as_ref()
        .and_then(crate::db::schema::shape_of)
}

/// The one place a `--json` command writes to stdout, so "exactly one object and nothing else" is
/// checkable by reading this file.
pub fn emit_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        // Serialization of a tree we built ourselves cannot fail, but printing a half-object would
        // break the one guarantee `--json` makes.
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

/// Every phase, longest first, so what dominated a run is the first line read.
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

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        return word.to_string();
    }
    match word.strip_suffix('y') {
        Some(stem) if !stem.ends_with(['a', 'e', 'i', 'o', 'u']) => format!("{stem}ies"),
        _ => format!("{word}s"),
    }
}

/// The worker pool a run installs.
pub fn build_pool(
    jobs: Option<u32>,
    warnings: &mut Vec<Diagnostic>,
) -> (Option<rayon::ThreadPool>, usize) {
    let requested = jobs.unwrap_or(0) as usize;
    match rayon::ThreadPoolBuilder::new()
        .num_threads(requested)
        .build()
    {
        Ok(pool) => {
            let workers = pool.current_num_threads();
            (Some(pool), workers)
        }
        Err(e) => {
            warnings.push(
                Diagnostic::warning(
                    ply_span::codes::RUNTIME_ERROR,
                    format!("could not start {requested} worker threads: {e}"),
                )
                .note("the run continued on the default thread pool"),
            );
            (None, rayon::current_num_threads())
        }
    }
}

pub fn exit_code(ok: bool) -> i32 {
    if ok { EXIT_OK } else { crate::EXIT_FAILED }
}

/// What the reference-counting pass and the evaluator counted over one run.
pub fn counters_json(stats: &ply_eval::rc::Stats) -> Value {
    json!({
        "updates": stats.updates,
        "updates_in_place": stats.updates_in_place,
        "in_place": stats.in_place(),
        "takes_attempted": stats.takes_attempted,
        "takes_moved": stats.takes_moved,
        "dup_sites": stats.dup_sites,
        "dup_emitted": stats.dup_emitted,
        "drop_sites": stats.drop_sites,
        "drop_emitted": stats.drop_emitted,
        "elided": stats.elided(),
        "cycles": stats.cycles,
    })
}

/// The one-line human projection of [`counters_json`].
pub fn counters_line(stats: &ply_eval::rc::Stats) -> String {
    let pct = |v: Option<f64>| match v {
        Some(v) => format!("{:.1}%", v * 100.0),
        None => "n/a".to_string(),
    };
    format!(
        "counters    in place {} of {} ({}) · moved {} of {} · elided {}",
        stats.updates_in_place,
        stats.updates,
        pct(stats.in_place()),
        stats.takes_moved,
        stats.takes_attempted,
        pct(stats.elided()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_span::codes;

    #[test]
    fn location_is_one_based_and_names_the_file() {
        let mut sources = SourceMap::new();
        let id = sources.add("src/ledger.ply", "fn f() = 1\nfn g() = 2\n");
        assert_eq!(
            location(&sources, Span::new(id, 11, 13)).unwrap(),
            "src/ledger.ply:2:1"
        );
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
        assert_eq!(plural(1, "body"), "body");
        assert_eq!(plural(0, "body"), "bodies");
        assert_eq!(plural(2, "key"), "keys");
    }
}

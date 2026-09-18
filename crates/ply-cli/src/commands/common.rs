use crate::load::LoadError;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_span::{Diagnostic, Severity, SourceMap, Span, codes};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const IND: &str = "   ";

/// `--backend`'s value as a spec, or the diagnostic that refuses it.
pub fn backend_spec(flag: Option<&String>) -> Result<Option<ply_eval::BackendSpec>, Diagnostic> {
    let Some(spec) = flag else {
        return Ok(Some(ply_eval::BackendSpec {
            kind: ply_eval::BackendKind::C,
            ..ply_eval::BackendSpec::default()
        }));
    };
    ply_eval::backend::parse(spec).map(Some).map_err(|message| {
        Diagnostic::error(codes::BACKEND_UNAVAILABLE, message).note(
            "a wrong backend is a self-test: it exists so that a green run with a backend \
             attached can be read as evidence",
        )
    })
}

/// Fixes the emitted C tier's toolchain before anything compiles; not part of `Engine`'s variant,
/// because both profiles must answer identically.
pub fn select_profile(flag: &str) -> Result<(), Diagnostic> {
    let Some(profile) = ply_codegen::Profile::parse(flag) else {
        return Err(Diagnostic::error(
            codes::BACKEND_UNAVAILABLE,
            format!("`--profile {flag}` is not a profile"),
        )
        .note(
            "`development` compiles fast for code that runs slowly enough; `release` compiles \
             slowly for code that runs fast",
        )
        .note(
            "the two are required to answer identically, so this decides what a run costs and \
             not what it means",
        ));
    };
    ply_codegen::select_profile(profile);
    Ok(())
}

/// Named before a provider exists, since selection decides whether building one is worth it.
pub fn engine_of(spec: Option<&ply_eval::BackendSpec>) -> ply_test::Engine {
    let Some(spec) = spec else {
        return ply_test::Engine::Evaluator;
    };
    let (name, variant) = match spec.kind {
        ply_eval::BackendKind::C => ("c", ply_codegen::backend::registry_width()),
    };
    ply_test::Engine::of_backend(name, variant, spec)
}

pub(crate) use ply_codegen::emit_keys;

/// Runs `selection` on the compiled tier built from `loaded`'s module source texts.
pub fn run_on_tier(
    loaded: &crate::load::Loaded,
    selection: &ply_test::Selection,
    hosting: ply_test::Hosting<'_>,
    store: &mut ply_store::Store,
) -> ply_test::RunReport {
    ply_codegen::c::producer::ensure_default();
    let (program, resolved) = (&loaded.program, &loaded.resolved);
    let texts = module_texts(program, &loaded.sources);
    let unit = ply_codegen::Unit::over_front(program, resolved, &loaded.front, texts)
        .expect("this host has a C compiler");
    let spec = ply_eval::BackendSpec {
        kind: ply_eval::BackendKind::C,
        ..Default::default()
    };
    let executor = ply_test::InterpExecutor::new(program, resolved, &loaded.check)
        .with_backend(unit, spec)
        .with_search(ply_test::Search::of(selection))
        .with_hosts(hosting);
    ply_test::run_with(selection, &loaded.check, &loaded.hashes, store, &executor)
}

pub fn module_texts(
    program: &ply_syntax::ast::Program,
    sources: &SourceMap,
) -> std::collections::HashMap<String, String> {
    program
        .modules
        .iter()
        .filter_map(|m| {
            sources
                .get(m.source)
                .map(|f| (m.name.to_string(), f.text.to_string()))
        })
        .collect()
}

/// The unit over the whole program, its laws' and clauses' roots included.
pub fn prover_backend(
    flag: Option<&String>,
    loaded: &crate::load::Loaded,
) -> Result<Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>, Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    let Some(spec) = backend_spec(flag)? else {
        return Ok(None);
    };
    let provider = build_backend_over(
        &spec,
        &loaded.program,
        &loaded.resolved,
        &loaded.front,
        module_texts(&loaded.program, &loaded.sources),
    )?;
    Ok(Some((provider, spec)))
}

/// Every command that loaded a program uses this, so an invocation runs one front end.
pub fn build_backend_over(
    spec: &ply_eval::BackendSpec,
    program: &ply_syntax::ast::Program,
    resolved: &ply_syntax::resolve::Resolved,
    front: &ply_ty::Front,
    texts: std::collections::HashMap<String, String>,
) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    match spec.kind {
        ply_eval::BackendKind::C => ply_codegen::Unit::over_front(program, resolved, front, texts)
            .map(|unit| unit as &'static dyn ply_eval::Provider)
            .map_err(unbuilt),
    }
}

fn unbuilt(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(
        codes::BACKEND_UNAVAILABLE,
        format!("the C backend could not be built: {error:#}"),
    )
    .note(
        "a backend that failed to build would decline every call, so the run is refused rather \
         than reported green over a seam nothing reached",
    )
    .note("this tier shells out to `cc`; `PLY_CC` names another compiler")
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

/// A lazy read and a later flush can each report the same unreadable file.
pub fn once_each(warnings: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut seen: BTreeSet<(&'static str, String)> = BTreeSet::new();
    warnings
        .into_iter()
        .filter(|d| seen.insert((d.code, d.message.clone())))
        .collect()
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

pub fn describe_schema(
    hosts: &mut crate::hosts::Hosts,
    constant: &dyn Fn(&str) -> Result<ply_eval::Value, Diagnostic>,
) {
    let Some(name) = hosts.schema_function().map(str::to_string) else {
        return;
    };
    hosts.describe_schema(materialise_schema(&name, constant));
}

pub fn materialise_schema(
    name: &str,
    constant: &dyn Fn(&str) -> Result<ply_eval::Value, Diagnostic>,
) -> Option<crate::db::schema::Shape> {
    constant(name)
        .ok()
        .as_ref()
        .and_then(crate::db::schema::shape_of)
}

/// A pure nullary definition entered on `provider`'s unit: how a schema function is evaluated.
pub fn enter_constant(
    provider: Option<&'static dyn ply_eval::Provider>,
    name: &str,
) -> Result<ply_eval::Value, Diagnostic> {
    let spec = ply_eval::BackendSpec {
        kind: ply_eval::BackendKind::C,
        ..Default::default()
    };
    let name = ply_span::Symbol::new(name);
    let entered = match provider {
        Some(provider) => provider.attach(&spec).enter_whole(&name, &[], 10_000),
        None => ply_eval::Entered::Declined,
    };
    match entered {
        ply_eval::Entered::Answered(value) => Ok(value),
        ply_eval::Entered::Raised(raised) => Err(raised),
        ply_eval::Entered::Declined => Err(ply_eval::err_not_compiled(&name, Span::DUMMY)),
    }
}

/// The one place a `--json` command writes to stdout.
pub fn emit_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
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

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        return word.to_string();
    }
    match word.strip_suffix('y') {
        Some(stem) if !stem.ends_with(['a', 'e', 'i', 'o', 'u']) => format!("{stem}ies"),
        _ => format!("{word}s"),
    }
}

/// A compiled body honours `ply_eval::DEFAULT_MAX_CALLS` on the native stack, where unoptimised
/// frames can run to kilobytes. Reserved, not committed.
const WORKER_STACK: usize = 256 << 20;

pub fn build_pool(
    jobs: Option<u32>,
    warnings: &mut Vec<Diagnostic>,
) -> (Option<rayon::ThreadPool>, usize) {
    let requested = jobs.unwrap_or(0) as usize;
    match rayon::ThreadPoolBuilder::new()
        .num_threads(requested)
        .stack_size(WORKER_STACK)
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

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
        // Tier-only: the compiled tier is the default (and only) evaluator of the language.
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

/// Fixes the toolchain the emitted C tier compiles with, before anything compiles.
///
/// Not part of `Engine`'s variant, and the rule for that is `Provider::variant`'s own: what
/// belongs there is a knob that changes *which* definitions run natively. This one changes how the
/// same set is compiled, and the two profiles are required to answer identically -- which is what
/// `--audit-backend` checks and what a namespaced result cache would hide rather than prove.
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
        ply_eval::BackendKind::C => ("c", ply_codegen::backend::registry_width()),
    };
    ply_test::Engine::of_backend(name, variant, spec)
}

/// The run's backend, built once over a checked program, or the diagnostic that refuses it.
/// What each definition's emitted code is a function of: its own hash, which the hasher builds
/// over its text with every referent's hash spliced in -- so it moves when anything the emitter
/// would inline moves, which is what an inlining emitter's cache has to be keyed on.
///
/// A test's root is a definition here like any other, named the way `ply_codegen` names it.
/// `HashOutput::tests` is parallel to the program's tests walked module by module in load order,
/// which `driver::test_hashes_of` already relies on and says so.
pub(crate) fn emit_keys(
    program: &ply_syntax::ast::Program,
    hashes: &ply_hash::HashOutput,
) -> std::collections::HashMap<String, String> {
    use ply_syntax::ast::Item;
    let mut keys = std::collections::HashMap::new();
    let mut test_at = 0;
    let mut law_at = 0;
    for module in &program.modules {
        let mut ordinal = 0;
        let mut law_ordinal = 0;
        for item in &module.items {
            match item {
                Item::Fn(def) => {
                    let name = module.name.qualify(&def.name.name).to_string();
                    if let Some(h) = hashes.defs.get(&ply_span::Symbol::new(&name)) {
                        // A clause's root is keyed by its owner's hash, which covers the clause.
                        let (mut requires, mut ensures) = (0, 0);
                        for clause in &def.spec {
                            let (kind, k) = match clause.kind {
                                ply_syntax::ast::SpecKind::Requires => {
                                    requires += 1;
                                    ("requires", requires - 1)
                                }
                                ply_syntax::ast::SpecKind::Ensures => {
                                    ensures += 1;
                                    ("ensures", ensures - 1)
                                }
                            };
                            let root = module
                                .name
                                .qualify(&ply_codegen::clause_root_name(&def.name.name, kind, k))
                                .to_string();
                            keys.insert(root, format!("{}#{kind}#{k}", h.to_hex()));
                        }
                        keys.insert(name, h.to_hex());
                    }
                }
                Item::Law(law) => {
                    if let Some(h) = hashes.laws.get(law_at) {
                        for part in ["guard", "body"] {
                            if part == "guard" && law.guard.is_none() {
                                continue;
                            }
                            let root = module
                                .name
                                .qualify(&ply_codegen::law_root_name(law_ordinal, part))
                                .to_string();
                            keys.insert(root, format!("{}#{part}", h.to_hex()));
                        }
                    }
                    law_ordinal += 1;
                    law_at += 1;
                }
                Item::Test(_) => {
                    let name = module
                        .name
                        .qualify(&ply_codegen::test_root_name(ordinal))
                        .to_string();
                    if let Some(h) = hashes.tests.get(test_at) {
                        keys.insert(name, h.to_hex());
                    }
                    ordinal += 1;
                    test_at += 1;
                }
                _ => {}
            }
        }
    }
    keys
}

/// Runs `selection` on a compiled tier — the only evaluator under tier-only (ADR 0048) — built by
/// the whole Ply emitter from `loaded`'s module source texts, over the program the runner works on
/// (`to_run`). The reusable form of what the `test` command builds inline, for the callers that
/// reached for `ply_test::run` when the interpreter needed no backend and now decline every test
/// without one.
pub fn run_on_tier(
    loaded: &crate::load::Loaded,
    selection: &ply_test::Selection,
    hosting: ply_test::Hosting<'_>,
    store: &mut ply_store::Store,
) -> ply_test::RunReport {
    ply_codegen::c::producer::ensure_default();
    let (program, resolved) = loaded.to_run();
    let texts = module_texts(program, &loaded.sources);
    let unit = ply_codegen::Unit::over_with_texts(program, resolved, &loaded.check, texts)
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

/// The default tier attached to a machine over `loaded`'s program: what the commands build
/// inline, for a test that makes a machine of its own.
#[cfg(test)]
pub(crate) fn attach_tier(
    machine: &mut ply_eval::Machine<'_>,
    loaded: &crate::load::Loaded,
) -> Result<(), Diagnostic> {
    let Some(spec) = backend_spec(None)? else {
        return Ok(());
    };
    let texts = module_texts(&loaded.program, &loaded.sources);
    let provider = build_backend(
        &spec,
        &loaded.program,
        &loaded.resolved,
        &loaded.check,
        &loaded.hashes,
        texts,
    )?;
    machine.set_compiled(provider.attach(&spec));
    Ok(())
}

/// Each module's source text by name: what a second emitter reads the program from.
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

/// The backend `ply prove` and `ply review` attach for a program's propositions (ADR 0045
/// §"The facade"): the unit over the whole program, its laws' and clauses' roots included.
pub fn prover_backend(
    flag: Option<&String>,
    loaded: &crate::load::Loaded,
) -> Result<Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>, Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    let Some(spec) = backend_spec(flag)? else {
        return Ok(None);
    };
    let provider = build_backend(
        &spec,
        &loaded.program,
        &loaded.resolved,
        &loaded.check,
        &loaded.hashes,
        module_texts(&loaded.program, &loaded.sources),
    )?;
    Ok(Some((provider, spec)))
}

/// `PLY_C_EMITTER=ply:<dir>` makes the Ply emitter in `<dir>` the C tier's producer (ADR 0042).
/// The recipe loads that directory as a project of its own -- the front end, `emit.ply` and
/// the standard library they import -- and compiles it with the reference emitter; a worker
/// thread builds its own copy from the same recipe, since a loaded unit does not cross threads.
/// The self-hosted Ply emitter's directory, found by walking up from the working directory for
/// `spikes/ply-parser/emit.ply`. `None` when it is not on disk — a shipped binary, until the
/// bundle is embedded (ADR 0048).
fn find_emitter_dir() -> Option<std::path::PathBuf> {
    fn search(mut cur: std::path::PathBuf) -> Option<std::path::PathBuf> {
        loop {
            let candidate = cur.join("spikes/ply-parser");
            if candidate.join("emit.ply").is_file() {
                return Some(candidate);
            }
            if !cur.pop() {
                return None;
            }
        }
    }
    // The working directory first — a developer runs `ply` from the repository — then the
    // executable's own directory, since a `ply` invoked from elsewhere (a test's temp project) is
    // still `<repo>/target/**/ply`, under the `spikes/` it needs. A shipped binary finds neither
    // and falls through to the reference emitter until the bundle is embedded (ADR 0048).
    std::env::current_dir().ok().and_then(search).or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
            .and_then(search)
    })
}

pub(crate) fn install_producer_from_env() {
    if ply_codegen::c::producer::installed() {
        return;
    }
    // The self-hosted Ply emitter is the tier's producer, and under tier-only (ADR 0048) it is the
    // default: the Rust reference emitter is a fragment that declines `perform`/`handle`/`simulate`,
    // so the whole language runs only when the Ply emitter produces. `PLY_C_EMITTER` still overrides
    // — `ply:<dir>` for body-by-body, `ply-whole:<dir>` for the unit.
    let (dir, whole) = match std::env::var("PLY_C_EMITTER") {
        Ok(spec) => match (
            spec.strip_prefix("ply:").map(str::to_string),
            spec.strip_prefix("ply-whole:").map(str::to_string),
        ) {
            (Some(dir), _) => (std::path::PathBuf::from(dir), false),
            (_, Some(dir)) => (std::path::PathBuf::from(dir), true),
            _ => {
                eprintln!(
                    "PLY_C_EMITTER is `{spec}`; the producers are `ply:<dir>`, which answers \
                     bodies the reference accepted, and `ply-whole:<dir>`, which answers the unit"
                );
                return;
            }
        },
        // No override: the whole Ply emitter, found relative to the repository. A shipped binary
        // with no `spikes/ply-parser` on disk falls through to the reference emitter until the
        // bundle is embedded (ADR 0048's follow-up).
        Err(_) => match find_emitter_dir() {
            Some(dir) => (dir, true),
            None => return,
        },
    };
    ply_codegen::c::producer::set_whole(whole);
    let identity = {
        let mut modules = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "ply")
                    && let Ok(text) = std::fs::read_to_string(&p)
                {
                    modules.push((
                        p.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        text,
                    ));
                }
            }
        }
        ply_codegen::c::producer::digest_of(&modules)
    };
    ply_codegen::c::producer::install(
        std::sync::Arc::new(move || {
            // The directory's own `.ply` files and the standard library: not a project load, which
            // would sweep in fixtures and probes that sit beside the emitter on purpose.
            let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
                .map_err(|e| format!("{}: {e}", dir.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "ply"))
                .collect();
            files.sort();
            let mut sources = SourceMap::new();
            let mut inputs = Vec::new();
            for (module, text) in ply_std::sources() {
                let module = ply_syntax::ast::ModuleName::from_dotted(module);
                let id = sources.add(ply_std::pseudo_path(&module), text.to_string());
                inputs.push((id, module, text));
            }
            for path in &files {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| format!("{}: not a module name", path.display()))?;
                let text: &'static str = Box::leak(
                    std::fs::read_to_string(path)
                        .map_err(|e| format!("{}: {e}", path.display()))?
                        .into_boxed_str(),
                );
                let id = sources.add(path.clone(), text.to_string());
                inputs.push((id, ply_syntax::ast::ModuleName::from_dotted(stem), text));
            }
            let first = |ds: Vec<Diagnostic>| {
                ds.first()
                    .map(|d| d.message.clone())
                    .unwrap_or_else(|| "no diagnostic".to_string())
            };
            let mut ast = ply_syntax::parse_program(inputs).map_err(first)?;
            let expanded = ply_derive::expand_program(&mut ast);
            if !expanded.is_empty() {
                return Err(first(expanded));
            }
            let resolved = ply_syntax::resolve::resolve(&mut ast).map_err(first)?;
            let check = ply_core::check_program(&ast, &resolved).map_err(first)?;
            let program: &'static ply_syntax::ast::Program = Box::leak(Box::new(ast));
            let resolved = Box::leak(Box::new(resolved));
            let check = Box::leak(Box::new(check));
            let hashes = ply_hash::hash_program(program, resolved, check).map_err(first)?;
            let keys = emit_keys(program, &hashes);
            let source: &'static ply_codegen::Source = Box::leak(Box::new(
                ply_codegen::Source::keyed(program, resolved, check, keys),
            ));
            // The emitter is built from its bootstrap bundle when the directory holds one, and
            // by the reference emitter otherwise or when `PLY_C_BOOTSTRAP=off` asks for the
            // reference, which is how a bundle is refreshed.
            let bundle = dir.join("bootstrap");
            let from_bundle = ply_codegen::c::bundle::exists(&bundle)
                && std::env::var("PLY_C_BOOTSTRAP").as_deref() != Ok("off");
            let (native, _refused) = if from_bundle {
                ply_codegen::c::bundle::build(source, &bundle).map_err(|e| format!("{e:#}"))?
            } else {
                let names: Vec<String> = source.functions();
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                ply_codegen::c::build(source, &refs).map_err(|e| format!("{e:#}"))?
            };
            ply_codegen::c::producer::PlyProducer::new(native).map_err(|e| format!("{e:#}"))
        }),
        identity,
    );
}

/// The operations a binding would answer, as the producer keys them: a compiled `perform` of
/// one stays the machine's until the tier has a route to the host.
pub fn build_backend(
    spec: &ply_eval::BackendSpec,
    program: &ply_syntax::ast::Program,
    resolved: &ply_syntax::resolve::Resolved,
    check: &ply_core::CheckOutput,
    hashes: &ply_hash::HashOutput,
    texts: std::collections::HashMap<String, String>,
) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
    install_producer_from_env();
    match spec.kind {
        ply_eval::BackendKind::Reference => Ok(ply_eval::Fragment::over(program, resolved, check)),
        ply_eval::BackendKind::C => {
            ply_codegen::Unit::keyed(program, resolved, check, emit_keys(program, hashes), texts)
                .map(|unit| unit as &'static dyn ply_eval::Provider)
                .map_err(|error| {
                    Diagnostic::error(
                        codes::BACKEND_UNAVAILABLE,
                        format!("the C backend could not be built: {error:#}"),
                    )
                    .note(
                        "a backend that failed to build would decline every call, so the run is \
                     refused rather than reported green over a seam nothing reached",
                    )
                    .note("this tier shells out to `cc`; `PLY_CC` names another compiler")
                    .note("`--backend reference` needs no code generator and runs anywhere")
                })
        }
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
    // A `--db-schema` function is a pure const the tooling reads before the run; the pure applier
    // evaluates it without a compiled tier (ADR 0048), as `--config-schema` does.
    ply_eval::interp::Pure::new(&loaded.program, &loaded.resolved)
        .call(name, Vec::new(), def.span, 10_000)
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

/// A worker's stack. The machine's bound on nested calls is `ply_eval::DEFAULT_MAX_CALLS`, and a
/// compiled body honours the same count on the native stack -- where an unoptimising C compiler
/// gives every temporary a slot, so a frame can run to kilobytes. The stack has to hold the
/// budget's worth of the largest frames or the budget is not the bound that fires; a thread's
/// default two megabytes holds a few thousand. Reserved, not committed: the pages an idle worker
/// never touches cost nothing.
const WORKER_STACK: usize = 256 << 20;

/// The worker pool a run installs.
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

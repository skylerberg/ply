use super::common::{
    IND, counters_json, describe_schema, diagnostic_json, emit_json, enter_constant, location,
    plural, print_diagnostics, prover_backend, report_bind_error, report_load_error,
    select_profile,
};
use crate::cli::RunArgs;
use crate::hosts::Hosts;
use crate::load::{Loaded, load};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_DRAIN_INCOMPLETE, EXIT_FAILED, EXIT_OK};
use ply_eval::{Machine, Plan, Value as PlyValue};
use ply_host::process::{Executables, ProcessHost, Sink, Stream};
use ply_host::signal::{self, Shutdown};
use ply_span::{Diagnostic, SourceId, Span, codes};
use ply_ty::DefInfo;
use ply_ty::ty::Footprint;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn execute(args: &RunArgs, style: Style) -> i32 {
    // An artifact runs out of its own verified definitions, not a source tree.
    if args
        .path
        .extension()
        .is_some_and(|e| e == crate::artifact::EXTENSION)
    {
        return crate::artifact::run(args, style);
    }

    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("run", &err, args.json, style),
    };
    if let Some(err) = super::test::broken_promises(&loaded) {
        return report_load_error("run", &err, args.json, style);
    }

    let entry = match entry_point(&loaded) {
        Ok(entry) => entry,
        Err(diagnostic) => {
            if args.json {
                emit_json(&json!({
                    "command": "run",
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "root": loaded.root.display().to_string(),
                    "files": loaded.file_names(),
                    "diagnostics": [diagnostic_json(&diagnostic, &loaded.sources)],
                }));
            } else {
                print_diagnostics(std::slice::from_ref(&diagnostic), &loaded.sources, style);
            }
            return EXIT_COMPILE_ERROR;
        }
    };

    // Before anything evaluates; a hermetic run resolves nothing, so no registry can break it.
    let db = match args.db.resolve(args.host) {
        Ok(db) => db,
        Err(diagnostics) => {
            return report_bind_error("run", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    // What a host answer is checked against, and whether this run needs a database.
    let declared = loaded
        .check
        .defs
        .get(&entry.name)
        .map(|d| d.footprint.clone());
    // Before the configuration: its schema is entered on this unit.
    let backend = match select_profile(&args.profile)
        .and_then(|()| prover_backend(args.backend.as_ref(), &loaded))
    {
        Ok(backend) => backend,
        Err(diagnostic) => {
            return report_bind_error("run", &[diagnostic], &loaded.sources, args.json, style);
        }
    };
    let constant =
        |name: &str| enter_constant(backend.as_ref().map(|(provider, _)| *provider), name);
    let (configuration, config_warnings) =
        match crate::config::Configuration::open(&loaded.check, args.host, &args.config, &constant)
        {
            Ok(resolved) => resolved,
            Err(diagnostics) => {
                return report_bind_error("run", &diagnostics, &loaded.sources, args.json, style);
            }
        };
    // Before the binding, which decides whether `signal` is bound.
    let shutdown = args.host.then(|| Shutdown::new(args.shutdown.bounds()));
    if let Some(shutdown) = &shutdown
        && let Err(diagnostic) = signal::listen(shutdown)
    {
        return report_bind_error(
            "run",
            std::slice::from_ref(&diagnostic),
            &loaded.sources,
            args.json,
            style,
        );
    }
    let process = match args.host.then(|| process_host(args)).transpose() {
        Ok(process) => process,
        Err(diagnostic) => {
            return report_bind_error(
                "run",
                std::slice::from_ref(&diagnostic),
                &loaded.sources,
                args.json,
                style,
            );
        }
    };
    let mut hosts = match Hosts::open_stopping(
        &loaded.check,
        args.host,
        &args.tls,
        &args.fs.fs,
        db,
        configuration,
        &args.trace,
        declared.as_ref(),
        shutdown.clone(),
        process,
        Vec::new(),
    ) {
        Ok(hosts) => hosts,
        Err(diagnostics) => {
            return report_bind_error("run", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    describe_schema(&mut hosts, &constant);
    // An undeclared `--set` is a classic silent deploy failure, so it is always reported.
    if !args.json {
        print_diagnostics(&config_warnings, &loaded.sources, style);
    }
    let config_warnings =
        crate::commands::common::diagnostics_json(&config_warnings, &loaded.sources);

    if !args.json {
        print_binding(&hosts, style);
    }
    // `--drain-ms` should exceed the program's `body_timeout_ms + write_timeout_ms`, which the run
    // cannot see, so it is printed for comparison by eye.
    if let Some(shutdown) = &shutdown
        && !args.json
    {
        eprintln!(
            "{IND}{}",
            style.dim(&format!(
                "shutdown    signals {} · lead {}ms · drain {}ms · second signal exits 130/143",
                shutdown
                    .signals()
                    .iter()
                    .map(|s| s.name())
                    .collect::<Vec<_>>()
                    .join(" "),
                args.shutdown.drain_lead_ms,
                args.shutdown.drain_ms,
            ))
        );
    }

    let name = entry.name.clone();
    let module = entry.module.to_string();
    let span = entry.span;
    let plan = crate::simulation::run_plan(args.seed.as_ref());
    let backend = backend.map(|(provider, spec)| provider.attach(&spec));
    // The counters are process-wide and cumulative.
    ply_eval::rc::reset();
    ply_codegen::rt::set_step_budget(args.steps);
    ply_codegen::rt::set_time_budget(args.timeout);
    let answer = evaluate(
        &loaded,
        name.as_str(),
        span,
        &plan,
        &hosts,
        declared.as_ref(),
        backend,
    );

    let counters_value = counters_json(&ply_eval::rc::stats());

    // A cycle among escaped values is never collected, so only this run can report it.
    let mut config_warnings = config_warnings;
    let cycles = ply_eval::rc::take_cycles();
    if !cycles.is_empty() {
        if args.json {
            if let (Value::Array(items), Value::Array(more)) = (
                &mut config_warnings,
                crate::commands::common::diagnostics_json(&cycles, &loaded.sources),
            ) {
                items.extend(more);
            }
        } else {
            print_diagnostics(&cycles, &loaded.sources, style);
        }
    }

    // On the machine's own thread, before the process exits.
    let teardown = teardown(&hosts, shutdown.as_ref(), args.shutdown.drain_ms);
    let teardown_json = teardown_json(shutdown.as_ref(), teardown.as_ref(), &args.shutdown);
    if !args.json {
        for line in stop_lines(&hosts, shutdown.as_ref(), teardown.as_ref(), &answer) {
            eprintln!("{IND}{}", style.dim(&line));
        }
    }

    // The program chose its code and returned no value, so none is printed.
    if let Some(code) = hosts.requested_exit() {
        if args.json {
            emit_json(&json!({
                "command": "run",
                "ok": code == EXIT_OK,
                "exit_code": code,
                "root": loaded.root.display().to_string(),
                "files": loaded.file_names(),
                "entry": name,
                "module": module,
                "binding": hosts.label(),
                "counters": counters_value,
                "hosts": hosts.summary_json(),
                "value": Value::Null,
                "configuration": hosts.configuration().to_json(),
                "shutdown": teardown_json,
                "diagnostics": config_warnings,
            }));
        } else {
            print_handshakes(&hosts, style);
        }
        return code;
    }

    match answer {
        Ok(value) => {
            let rendered = value.to_string();
            if args.json {
                emit_json(&json!({
                    "command": "run",
                    "ok": true,
                    "exit_code": EXIT_OK,
                    "root": loaded.root.display().to_string(),
                    "files": loaded.file_names(),
                    "entry": name,
                    "module": module,
                    "binding": hosts.label(),
                    "counters": counters_value.clone(),
                    "hosts": hosts.summary_json(),
                    "value": rendered,
                    "configuration": hosts.configuration().to_json(),
                    "shutdown": teardown_json,
                    "diagnostics": config_warnings,
                }));
            } else {
                print_handshakes(&hosts, style);
                println!("{IND}{rendered}");
            }
            EXIT_OK
        }
        Err(diagnostic) => {
            // An expired drain is the configuration's fault: a warning, not attributed or bisected.
            let drained = ply_eval::is_drain_incomplete(&diagnostic);
            let code = if drained {
                EXIT_DRAIN_INCOMPLETE
            } else {
                EXIT_FAILED
            };
            let mut all = match config_warnings {
                Value::Array(items) => items,
                _ => Vec::new(),
            };
            all.push(diagnostic_json(&diagnostic, &loaded.sources));
            if args.json {
                emit_json(&json!({
                    "command": "run",
                    "ok": false,
                    "exit_code": code,
                    "root": loaded.root.display().to_string(),
                    "files": loaded.file_names(),
                    "entry": name,
                    "module": module,
                    "binding": hosts.label(),
                    "counters": counters_value.clone(),
                    "hosts": hosts.summary_json(),
                    "value": Value::Null,
                    "configuration": hosts.configuration().to_json(),
                    "shutdown": teardown_json,
                    "diagnostics": Value::Array(all),
                }));
            } else {
                print_diagnostics(std::slice::from_ref(&diagnostic), &loaded.sources, style);
                // An abandoned run raised nothing: the clock stopped it where it stood.
                let abandoned = diagnostic.code == codes::RUN_ABANDONED;
                if !drained
                    && !abandoned
                    && let Some(at) = diagnostic
                        .primary_span()
                        .and_then(|s| location(&loaded.sources, s))
                {
                    eprintln!("{IND}{} {at}", style.red("raised at"));
                }
            }
            code
        }
    }
}

/// `--json` promises stdout to the one object, so the program's own lines go to stderr instead.
/// Loaded up front so an `--exec` that cannot be started is `E0457` before anything runs.
pub(crate) fn process_host(args: &RunArgs) -> Result<ProcessHost, Diagnostic> {
    let out = if args.json { Stream::Err } else { Stream::Out };
    let executables = Executables::load(&args.exec.exec, Span::DUMMY)?;
    Ok(ProcessHost::new(args.argv.clone(), Sink::Real { out }).executing(executables))
}

/// Rolls back every open transaction, closes spans `Abandoned`, flushes the sink, closes the pool.
pub(crate) fn teardown(
    hosts: &Hosts,
    shutdown: Option<&Arc<Shutdown>>,
    drain_ms: u64,
) -> Option<ply_eval::ShutdownReport> {
    let budget = match shutdown.filter(|s| s.stopping()) {
        Some(stopping) => {
            let left = stopping.deadline_ms().max(0) as u64;
            left.max(TEARDOWN_FLOOR_MS)
        }
        None => drain_ms,
    };
    hosts.runtime().map(|rt| rt.shutdown(budget))
}

pub(crate) const TEARDOWN_FLOOR_MS: u64 = 1_000;

pub(crate) fn teardown_json(
    shutdown: Option<&Arc<Shutdown>>,
    teardown: Option<&ply_eval::ShutdownReport>,
    bounds: &crate::cli::ShutdownOptions,
) -> Value {
    json!({
        "requested": shutdown.is_some_and(|s| s.stopping()),
        "signal": shutdown.and_then(|s| s.signal()).map(|s| s.name().to_string()),
        "drain_ms": bounds.drain_ms,
        "drain_lead_ms": bounds.drain_lead_ms,
        "transactions_rolled_back": teardown.map_or(0, |t| t.transactions_rolled_back),
        "connections_closed": teardown.map_or(0, |t| t.connections_closed.len()),
        "spans_abandoned": teardown.map_or(0, |t| t.spans_abandoned),
        "problems": teardown.map_or_else(Vec::new, |t| t.problems.clone()),
    })
}

/// Nothing when nobody asked the service to stop.
pub(crate) fn stop_lines(
    hosts: &Hosts,
    shutdown: Option<&Arc<Shutdown>>,
    teardown: Option<&ply_eval::ShutdownReport>,
    answer: &Result<PlyValue, Diagnostic>,
) -> Vec<String> {
    let Some(shutdown) = shutdown.filter(|s| s.stopping()) else {
        return Vec::new();
    };
    let bounds = shutdown.bounds();
    let (listeners, connections, scopes) = shutdown.at_stop();
    let mut lines = vec![format!(
        "stopping    signal {} · lead {}ms · drain {}ms · {listeners} listener(s) closed · {connections} connection(s) in flight · {scopes} transaction(s) open",
        shutdown.signal().map_or("none", |s| s.name()),
        bounds.lead.as_millis(),
        bounds.drain.as_millis(),
    )];
    let expired = matches!(answer, Err(d) if ply_eval::is_drain_incomplete(d));
    lines.push(format!(
        "{}     {}ms since the signal",
        if expired { "abandoned" } else { "drained  " },
        shutdown.elapsed().unwrap_or_default().as_millis(),
    ));
    if let Some(teardown) = teardown {
        lines.push(format!(
            "teardown    {} transaction(s) rolled back, none committed · {} connection(s) closed rather than returned · {} span(s) abandoned · sink flushed",
            teardown.transactions_rolled_back,
            teardown.connections_closed.len(),
            teardown.spans_abandoned,
        ));
        for problem in &teardown.problems {
            lines.push(format!("warning[{}]: {problem}", codes::HOST_TEARDOWN));
        }
    }
    if let Some(counts) = hosts.trace_counts() {
        lines.push(format!(
            "trace       {} event(s) · {} span(s) · {} abandoned · {}",
            counts.events,
            counts.spans,
            counts.abandoned,
            if counts.flushed {
                "flushed"
            } else {
                "not flushed"
            },
        ));
    }
    lines
}

pub(crate) fn print_binding(hosts: &Hosts, style: Style) {
    if hosts.is_hermetic() {
        return;
    }
    let listing = hosts.listing();
    let disclosures = hosts.disclosures();
    println!(
        "{IND}{}",
        style.dim(&format!(
            "binding host · {} {} · {}",
            listing.rows.len(),
            plural(listing.rows.len(), "operation"),
            crate::hosts::digest_short(listing, &disclosures),
        ))
    );
    if disclosures.configuration.is_some() {
        println!(
            "{IND}{}",
            style.dim(&format!("config      {}", hosts.configuration().banner()))
        );
    }
    if let Some(observability) = &disclosures.observability {
        println!(
            "{IND}{}",
            style.dim(&format!("trace       {}", observability.banner()))
        );
    }
    if let Some(line) = crate::hosts::database_line(hosts) {
        println!("{IND}{}", style.dim(&line));
    }
}

/// Handshakes completed and refused, which a run only knows once it is over.
fn print_handshakes(hosts: &Hosts, style: Style) {
    if hosts.is_hermetic() {
        return;
    }
    for line in crate::hosts::handshake_lines(&hosts.handshakes()) {
        println!("{IND}{}", style.dim(&line));
    }
}

fn evaluate(
    loaded: &Loaded,
    name: &str,
    span: Span,
    plan: &Plan,
    hosts: &Hosts,
    declared: Option<&Footprint>,
    backend: Option<std::rc::Rc<dyn ply_eval::Compiled>>,
) -> Result<PlyValue, Diagnostic> {
    let mut machine = Machine::new(&loaded.front);
    if let Some(backend) = backend {
        machine.set_compiled(backend);
    }
    machine.set_host_binding(hosts.binding());
    if let Some(runtime) = hosts.runtime() {
        machine.set_host_runtime(runtime);
    }
    if let Some(declared) = declared {
        machine.set_declared_footprint(declared.clone());
    }
    // Exploration is a test-time activity; `ply run` takes the one interleaving its seed names.
    ply_test::sim::seed_run(&mut machine, &plan.seeds()[0], plan.steps);
    machine.call(name, Vec::new(), span)
}

pub fn entry_point(loaded: &Loaded) -> Result<&DefInfo, Diagnostic> {
    let mut candidates = loaded.entry_points();
    match candidates.len() {
        0 => Err(no_main(loaded)),
        1 => Ok(candidates.remove(0)),
        _ => Err(ambiguous_main(loaded, &candidates)),
    }
}

/// Every name already resolves, so a missing `main` is a missing entry point.
pub fn no_main(loaded: &Loaded) -> Diagnostic {
    let modules = loaded.modules();
    let mut diagnostic = Diagnostic::error(codes::UNKNOWN_NAME, "no `main` to run")
        .note("`ply test` runs the tests; `ply run` runs `main`");

    match modules.as_slice() {
        [only] => {
            diagnostic = diagnostic
                .primary(
                    end_of(loaded, only.info.source),
                    format!("`{}` declares no entry point", only.name),
                )
                .note(format!("add `fn main() -> Unit = ...` to `{}`", only.name));
        }
        several => {
            let names: Vec<&str> = several.iter().map(|m| m.name.as_str()).collect();
            diagnostic = diagnostic
                .note("add `fn main() -> Unit = ...` to one of the loaded modules")
                .note(format!("modules loaded: {}", names.join(", ")));
        }
    }
    diagnostic
}

/// The empty span at the end of a file, where the missing definition would go.
fn end_of(loaded: &Loaded, source: SourceId) -> Span {
    let end = loaded
        .sources
        .get(source)
        .map(|f| f.text.len() as u32)
        .unwrap_or(0);
    Span::new(source, end, end)
}

fn ambiguous_main(loaded: &Loaded, candidates: &[&DefInfo]) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::AMBIGUOUS_ENTRY_POINT,
        format!("{} modules declare `main`", candidates.len()),
    );

    for (i, def) in candidates.iter().enumerate() {
        let message = format!("`{}` declares `main` here", def.module);
        diagnostic = if i == 0 {
            diagnostic.primary(def.span, message)
        } else {
            diagnostic.secondary(def.span, message)
        };
    }

    for def in candidates {
        let path = loaded
            .check
            .modules
            .get(def.module.as_symbol())
            .map(|m| loaded.path_of(m.source).display().to_string())
            .unwrap_or_else(|| def.module.to_string());
        diagnostic = diagnostic.note(format!("run it with `ply run {path}`"));
    }

    diagnostic.note("a directory is a whole program, so `ply run` will not pick one for you")
}

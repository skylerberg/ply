use super::common::{
    IND, counted_engine, counters_json, describe_schema, diagnostic_json, emit_json, location,
    plural, print_diagnostics, report_bind_error, report_load_error,
};
use crate::cli::RunArgs;
use crate::hosts::Hosts;
use crate::load::{Loaded, load};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_DRAIN_INCOMPLETE, EXIT_FAILED, EXIT_OK};
use ply_core::DefInfo;
use ply_core::ty::Footprint;
use ply_eval::{Engine, EngineChoice, Interp, Machine, Plan, Value as PlyValue, compare_answers};
use ply_host::signal::{self, Shutdown};
use ply_span::{Diagnostic, SourceId, Span, codes};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn execute(args: &RunArgs, style: Style) -> i32 {
    // A built artifact is run out of its own verified definitions rather than
    // out of a source tree it may not be sitting next to.
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

    // After the entry point is known and before anything evaluates: a bad
    // registration is a start-up failure, and a hermetic run resolves nothing at
    // all, so the default path cannot be broken by a registry it never consults.
    // Before the binding, because a connection string that does not parse is
    // the run's configuration and has nothing to do with what the registry
    // resolves — and because a diagnostic about it must be raised by the one
    // component that has never held the password as text.
    let db = match args.db.resolve(args.host) {
        Ok(db) => db,
        Err(diagnostics) => {
            return report_bind_error("run", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    // The entry point's own row, which is what a host answer is checked against
    // and what decides whether this run needs a database at all.
    let declared = loaded
        .check
        .defs
        .get(&entry.name)
        .map(|d| d.footprint.clone());
    let (configuration, config_warnings) = match crate::config::Configuration::open(
        &loaded.program,
        &loaded.resolved,
        &loaded.check,
        args.host,
        &args.config,
    ) {
        Ok(resolved) => resolved,
        Err(diagnostics) => {
            return report_bind_error("run", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    // Before the binding, because whether `signal` is bound or withheld is
    // decided when the registry is built. `ply run --host` is the only command
    // that installs one: a suite whose verdicts could be ended by its own
    // ctrl-C is a suite whose verdicts depend on the terminal.
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
    let mut hosts = match Hosts::open_stopping(
        &loaded.check,
        args.host,
        &args.tls.tls,
        db,
        configuration,
        &args.trace,
        declared.as_ref(),
        shutdown.clone(),
    ) {
        Ok(hosts) => hosts,
        Err(diagnostics) => {
            return report_bind_error("run", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    describe_schema(&loaded, &mut hosts);
    // A `--set` the schema does not declare is the classic silent deploy
    // failure, so it is reported whichever projection the caller asked for.
    if !args.json {
        print_diagnostics(&config_warnings, &loaded.sources, style);
    }
    let config_warnings =
        crate::commands::common::diagnostics_json(&config_warnings, &loaded.sources);

    // Before anything evaluates, because a service that printed what it was
    // configured with only when it stopped would be a service nobody could read
    // that from. Every line is a fact the binding already holds; nothing here is
    // computed for the banner, and what is *not* here — the handshake counts —
    // is what a run can only know afterwards.
    if !args.json {
        print_binding(&hosts, style);
    }
    // ADR 0015 §4.5: `--drain-ms` should exceed the program's own
    // `body_timeout_ms + write_timeout_ms`, and the run cannot check that
    // because `http::Limits` is a Ply value it never sees — so the number is
    // printed where it can be compared by eye against the one in the program.
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
    let engine: EngineChoice = args.engine.into();
    let plan = crate::simulation::run_plan(args.seed.as_ref());
    // The counters are process-wide and cumulative, so they mean nothing unless
    // this run is the only thing they have seen. Reset immediately before, read
    // immediately after, and report only when a single engine ran — under
    // `--engine both` the sum would blend a machine with a tree-walker that does
    // no reference counting at all, and a blended figure is worse than none.
    ply_eval::rc::reset();
    let answer = evaluate(
        &loaded,
        engine,
        name.as_str(),
        span,
        &plan,
        &hosts,
        declared.as_ref(),
    );

    let counters_value = match engine {
        EngineChoice::Both => Value::Null,
        _ => counters_json(&ply_eval::rc::stats(), counted_engine(engine)),
    };

    // A cycle among escaped values is never collected (ADR 0017 §4), so the run
    // that built one is the only place a reader can be told it is there. It is
    // the program's own doing rather than the run's configuration, so it is
    // reported whatever the entry point answered and it changes no exit code.
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

    // After the entry point and before the process exits, on the machine's own
    // thread: roll every open transaction back, flush the sink, close the pool.
    // A signal handler never runs it — the handler sets a flag, and this is
    // where the flag is cashed.
    let teardown = teardown(&hosts, shutdown.as_ref(), args.shutdown.drain_ms);
    let teardown_json = teardown_json(shutdown.as_ref(), teardown.as_ref(), &args.shutdown);
    if !args.json {
        for line in stop_lines(&hosts, shutdown.as_ref(), teardown.as_ref(), &answer) {
            eprintln!("{IND}{}", style.dim(&line));
        }
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
            // A drain that expired is the run's configuration at fault rather
            // than the program's: it is a warning, it is attributed to no
            // definition and it is not bisected, and what carries the verdict is
            // the exit code. A deployment that sees `3` knows it lost requests
            // and one that sees `0` knows it did not, which is the whole product
            // of a graceful shutdown.
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
                if !drained
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

/// ADR 0015 §4.4's pinned order: every open transaction rolled back and never
/// committed, every open span closed `Abandoned`, the sink flushed, the pool
/// closed. On the machine's own thread, after the last entry point, and never
/// from a signal handler — the handler sets a flag and this is where the flag
/// is cashed.
///
/// The budget is what makes `--drain-ms` bound the *stop* rather than the drain
/// alone. A run that was signalled gets whatever is left of its deadline, floored
/// at [`TEARDOWN_FLOOR_MS`] so that a rollback on a healthy connection can still
/// complete after a drain that expired; a run that ended on its own gets the
/// whole of `--drain-ms`, because there is no deadline it is already past.
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

/// The least a teardown gets after a drain that already expired.
///
/// Not zero, because the step the teardown exists for is a `ROLLBACK` and one on
/// a connection that is not stuck takes a millisecond; refusing to wait at all
/// would discard connections a moment's patience would have handed back. Not
/// large, because the whole point is that a signal-to-exit time is `lead + drain
/// + this` and an operator can compute it.
pub(crate) const TEARDOWN_FLOOR_MS: u64 = 1_000;

/// The `shutdown` object of every `--json` run, source or artifact.
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

/// What a stopping service prints, and nothing when nobody asked it to stop.
///
/// Every number is a fact the run already held: the counts come from the socket
/// table, the scope table and the sink, and none of them is computed for the
/// banner.
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
    // What the sink saw, counted by the sink. A run whose log looks empty needs
    // to be able to tell "nothing happened" from "nothing was written".
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

/// What a `--host` run is about to reach for, before it reaches for it. Silent
/// when nothing is bound, which is every run that did not ask: a line on every
/// run would be noise, and the absence of one is not a claim.
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
    // Every number on this line is a fact the run already holds: the snapshot
    // counted by the source that won each key. Nothing is computed for it, and
    // a secret's value is absent while its key and its source are not — an
    // operator debugging "it used the wrong credential" needs the second.
    if disclosures.configuration.is_some() {
        println!(
            "{IND}{}",
            style.dim(&format!("config      {}", hosts.configuration().banner()))
        );
    }
    // Where this run's records go and which channels exist, which is the pair a
    // reader needs before deciding whether an empty log means "quiet" or "wrong
    // sink". Both are the bound registration's own answer.
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

/// The one thing on the banner a run can only know once it is over: how many
/// handshakes completed and how many were refused. Printed beside the value
/// rather than at start-up, where both numbers would be zero.
fn print_handshakes(hosts: &Hosts, style: Style) {
    if hosts.is_hermetic() {
        return;
    }
    for line in crate::hosts::handshake_lines(&hosts.handshakes()) {
        println!("{IND}{}", style.dim(&line));
    }
}

/// Under `both`, the authoritative engine's answer is what `main` produced and
/// the other engine's is only ever a reason to fail: a value the two disagree
/// about must never be printed as if it were the program's.
fn evaluate(
    loaded: &Loaded,
    engine: EngineChoice,
    name: &str,
    span: Span,
    plan: &Plan,
    hosts: &Hosts,
    declared: Option<&Footprint>,
) -> Result<PlyValue, Diagnostic> {
    let mut interp = Interp::new(&loaded.program, &loaded.resolved, &loaded.check);
    interp.set_host_binding(hosts.binding());
    let mut machine = Machine::new(&loaded.program, &loaded.resolved, &loaded.check);
    // One analysis for the program rather than one per engine: under `both` the
    // two would otherwise each run the whole-program region-kind pass, and the
    // answer is a property of the program neither of them owns.
    machine.share_region_kinds(interp.shared_region_kinds());
    machine.set_host_binding(hosts.binding());
    if let Some(runtime) = hosts.runtime() {
        machine.set_host_runtime(runtime);
    }
    if let Some(declared) = declared {
        machine.set_declared_footprint(declared.clone());
    }
    // `ply run` takes exactly one interleaving, the one its seed names:
    // exploration is a test-time activity, so there is nothing to search here.
    ply_test::sim::seed_run(&mut machine, &plan.seeds()[0], plan.steps);

    match engine {
        EngineChoice::Treewalk => interp.call(name, Vec::new(), span),
        EngineChoice::Machine => machine.call(name, Vec::new(), span),
        EngineChoice::Both => {
            let left = interp.call(name, Vec::new(), span);
            let right = machine.call(name, Vec::new(), span);
            // A refusal is not a disagreement: the tree-walker declined to
            // start, so the machine's answer is the only one there is.
            if matches!(&left, Err(d) if ply_eval::is_machine_only(d)) {
                return right;
            }
            match compare_answers(&interp, &machine, name, &left, &right) {
                Some(d) => Err(d.to_diagnostic(Engine::Treewalk, Engine::Machine, span)),
                None => left,
            }
        }
    }
}

/// A file argument names one module, so its `main` is the only candidate. A
/// directory may hold several, and picking one would make the answer depend on
/// which file sorted first — so it is refused and the candidates are listed.
pub fn entry_point(loaded: &Loaded) -> Result<&DefInfo, Diagnostic> {
    let mut candidates = loaded.entry_points();
    match candidates.len() {
        0 => Err(no_main(loaded)),
        1 => Ok(candidates.remove(0)),
        _ => Err(ambiguous_main(loaded, &candidates)),
    }
}

/// Inference already proved every name resolves, so a missing `main` is a
/// missing entry point rather than an unbound reference — say so before the
/// evaluator gets a chance to phrase it worse.
///
/// The error is an *absence*, so no code is it: one module has one place `main`
/// would go and several have none, which is why the second arm labels nothing
/// rather than aiming at whichever file sorted first. Reading the parsed program
/// is avoided for a second reason — gate 1 skips files, so an anchor taken from
/// it would move with the cache.
fn no_main(loaded: &Loaded) -> Diagnostic {
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

/// The empty span at the end of a file: where the missing definition would be
/// written, and the one position in the file that is not existing code.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn loaded(text: &str) -> (tempfile::TempDir, Loaded) {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "m.ply", text);
        let l = load(dir.path()).unwrap();
        (dir, l)
    }

    fn eval(l: &Loaded) -> Result<String, Diagnostic> {
        let entry = entry_point(l)?;
        let (name, span) = (entry.name.clone(), entry.span);
        Interp::new(&l.program, &l.resolved, &l.check)
            .call(name.as_str(), Vec::new(), span)
            .map(|v| v.to_string())
    }

    #[test]
    fn main_is_evaluated_and_its_value_rendered() {
        let (_dir, l) = loaded("fn main() -> Int = 20 + 22\n");
        assert_eq!(eval(&l).unwrap(), "42");
    }

    #[test]
    fn a_missing_main_points_at_the_program_rather_than_nowhere() {
        let (_dir, l) = loaded("fn other() -> Int = 1\n");
        let d = no_main(&l);
        assert_eq!(d.code, codes::UNKNOWN_NAME);
        assert!(!d.primary_span().unwrap().is_dummy());
        assert!(d.notes.iter().any(|n| n.contains("fn main")));
    }

    /// The span it used to carry was the first item's, which is a definition
    /// that has nothing to do with the error — a reader following it is sent to
    /// read `other`. It now names the position `main` would occupy, which
    /// overlaps no item.
    #[test]
    fn a_missing_main_never_points_at_an_unrelated_definition() {
        let text = "fn other() -> Int = 1\nfn another() -> Int = 2\n";
        let (_dir, l) = loaded(text);
        let d = no_main(&l);

        let span = d.primary_span().expect("one module has one place to point");
        assert_eq!(
            span.start, span.end,
            "the anchor is a position, not an extent"
        );
        assert_eq!(span.start as usize, text.len());

        let items: Vec<Span> = l.program.modules[0]
            .items
            .iter()
            .map(|i| i.span())
            .collect();
        assert!(
            items.iter().all(|i| span.start >= i.end),
            "the anchor landed inside `{}`",
            l.sources
                .snippet(*items.iter().find(|i| span.start < i.end).unwrap()),
        );
        assert!(!ply_span::render::to_terminal(&d, &l.sources).is_empty());
    }

    /// With several modules no file is the answer, so labelling one would be
    /// picking by load order — the same thing `ply run` refuses to do for two
    /// `main`s.
    #[test]
    fn a_missing_main_across_several_modules_labels_no_file_at_all() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.ply", "fn f() -> Int = 1\n");
        write(dir.path(), "b.ply", "fn g() -> Int = 2\n");

        let l = load(dir.path()).unwrap();
        let d = no_main(&l);
        assert!(d.labels.is_empty(), "labels: {:?}", d.labels);
        assert!(d.notes.iter().any(|n| n.contains("a, b")), "{:?}", d.notes);
        assert!(
            ply_span::render::to_terminal(&d, &l.sources).contains("E0101"),
            "an unlabelled diagnostic still has to render"
        );
    }

    #[test]
    fn a_raising_main_yields_a_diagnostic_not_a_panic() {
        let (_dir, l) = loaded("fn main() -> Unit = panic(\"nope\")\n");
        let err = eval(&l).unwrap_err();
        assert_eq!(err.code, codes::RUNTIME_ERROR);
        assert!(location(&l.sources, err.primary_span().unwrap()).is_some());
    }

    #[test]
    fn the_one_main_in_a_multi_module_program_is_the_entry_point() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "lib.ply", "pub fn answer() -> Int = 42\n");
        write(
            dir.path(),
            "app.ply",
            "import lib\nfn main() -> Int = lib::answer()\n",
        );

        let l = load(dir.path()).unwrap();
        assert_eq!(entry_point(&l).unwrap().name.as_str(), "app.main");
        assert_eq!(eval(&l).unwrap(), "42");
    }

    #[test]
    fn several_mains_are_refused_with_the_candidates_named() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "one.ply", "fn main() -> Int = 1\n");
        write(dir.path(), "two.ply", "fn main() -> Int = 2\n");

        let l = load(dir.path()).unwrap();
        let d = entry_point(&l).unwrap_err();
        assert_eq!(d.code, codes::AMBIGUOUS_ENTRY_POINT);
        assert!(d.message.contains("2 modules"));
        assert_eq!(d.labels.len(), 2);
        assert!(d.labels.iter().all(|l| !l.span.is_dummy()));
        assert!(d.notes.iter().any(|n| n.contains("one.ply")));
        assert!(d.notes.iter().any(|n| n.contains("two.ply")));
    }

    #[test]
    fn naming_the_file_resolves_the_ambiguity() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "one.ply", "fn main() -> Int = 1\n");
        write(dir.path(), "two.ply", "fn main() -> Int = 2\n");

        let l = load(&dir.path().join("two.ply")).unwrap();
        assert_eq!(entry_point(&l).unwrap().name.as_str(), "two.main");
        assert_eq!(eval(&l).unwrap(), "2");
    }

    #[test]
    fn a_missing_main_lists_the_modules_that_were_searched() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.ply", "fn f() -> Int = 1\n");
        write(dir.path(), "b.ply", "fn g() -> Int = 2\n");

        let l = load(dir.path()).unwrap();
        let d = entry_point(&l).unwrap_err();
        assert_eq!(d.code, codes::UNKNOWN_NAME);
        assert!(
            d.notes.iter().any(|n| n.contains("a, b")),
            "notes: {:?}",
            d.notes
        );
    }
}

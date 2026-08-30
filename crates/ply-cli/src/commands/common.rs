use crate::load::LoadError;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_span::{Diagnostic, Severity, SourceMap, Span};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The gutter the specified output shape is indented by.
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
    let rendered = ply_span::render::all_to_terminal(diagnostics, sources);
    eprint!("{}", style.sanitize(&rendered));
}

/// One unreadable file is found more than once — a lazy read consults it, and a
/// later flush re-reads it to merge — and each drain reports what it found. The
/// reader is told once. Deduplicated here rather than in the store so that human
/// output and `--json` cannot disagree about it.
pub fn once_each(warnings: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut seen: BTreeSet<(&'static str, String)> = BTreeSet::new();
    warnings
        .into_iter()
        .filter(|d| seen.insert((d.code, d.message.clone())))
        .collect()
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

/// A registration that does not match the program is the host author's bug, not
/// the program's, and it is a start-up failure: nothing ran, so the report is
/// diagnostics and the binding that was asked for. Exit `2`, like any other
/// failure to get as far as running.
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

/// Fill in the table and column counts of the `--db-schema` function, for the
/// `database` block.
///
/// The name was resolved at start-up and a failure to *evaluate* changes no
/// verdict here: the counts are a fact the block prints, and the check that the
/// live database agrees with the schema is the driver's at bind time. So a
/// schema function that raises leaves the counts absent — printed as absent
/// rather than as zero — instead of failing a run over a number.
pub fn describe_schema(loaded: &crate::load::Loaded, hosts: &mut crate::hosts::Hosts) {
    let Some(name) = hosts.schema_function().map(str::to_string) else {
        return;
    };
    hosts.describe_schema(materialise_schema(loaded, &name));
}

/// Evaluate a resolved `--db-schema` function and read its size.
///
/// Shared by `ply hosts`, which resolves the name itself, and by every command
/// that had [`Hosts::open`] resolve it — so there is one call that decides what
/// the block's numbers mean.
///
/// [`Hosts::open`]: crate::hosts::Hosts::open
pub fn materialise_schema(
    loaded: &crate::load::Loaded,
    name: &str,
) -> Option<crate::db::schema::Shape> {
    let def = loaded
        .check
        .defs
        .values()
        .find(|d| d.name.as_str() == name)?;
    ply_eval::Interp::new(&loaded.program, &loaded.resolved, &loaded.check)
        .call(name, Vec::new(), def.span)
        .ok()
        .as_ref()
        .and_then(crate::db::schema::shape_of)
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

/// The worker pool a run installs. A pool that cannot be built is a warning and
/// the default pool, never a failure: the number of threads changes the wall
/// clock and nothing else, which is what `--jobs 1` and `--jobs 16` agreeing
/// byte for byte means.
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


/// One engine's counters, and the cycle diagnostics that had to be rescued
/// from the reset that took them.
pub struct Counted<T> {
    pub answer: T,
    pub counters: ply_eval::rc::Stats,
    /// Which evaluator [`Counted::counters`] came from.
    pub engine: ply_eval::Engine,
    /// Cycles reported *before* this engine ran, drained so that the reset
    /// could not discard them. Empty except under `--engine both`.
    pub carried_cycles: Vec<Diagnostic>,
}

/// What the reference-counting pass and the evaluator counted over one run.
///
/// ADR 0020 §9 recorded *"No deterministic counter turned out to exist — `ply
/// run --json` reports no step, call or allocation count — so wall clock was
/// unavoidable"*. [`ply_eval::rc::Stats`] had counted all of this since ADR
/// 0017 §4 and was read by three test files and nothing else. This is its CLI
/// surface, so that a claim about reuse can be a count instead of a clock.
///
/// **Whose number each one is**, because they are not all the runtime's:
/// `dup_*` and `drop_*` are bumped by the *lowering pass* (`code.rs`'s
/// `use_of`, `declare` and `released`), which only the machine runs — so under
/// `--engine treewalk` they are zero and `elided` is `null`, and that is a true
/// report rather than a missing one. `updates*`, `takes_*` and `cycles` are
/// bumped by the evaluator as it runs.
///
/// `engine` names which evaluator produced them. It is not decoration: under
/// `--engine both` the program is evaluated twice and a pooled figure would be
/// two runs added together, so the counters are taken from the machine alone
/// and this says so.
///
/// **The scope of `updates`.** `builtins.rs:460` and `:472` are the only two
/// `rc::note_update` call sites in the tree, so `updates` counts `push` on a
/// `List` and nothing else. `map_insert` is the same kind of operation and is
/// not counted. See the correction on `Stats::updates`' own doc comment.
///
/// `in_place` and `elided` are `null` rather than `0.0` when their denominator
/// is zero, following `Stats::elided`'s reasoning: a program that updated
/// nothing has not failed to reuse anything, and a percentage would be a lie.
///
/// **`in_place` is `null` under `--engine treewalk` for a second reason**, and
/// it is the same one that makes `elided` null there: the number would not be
/// about the program. `interp.rs` calls nothing in `ply_eval::rc` — no `carry`,
/// no `Env::take_unique` — so it never moves a value out of a scope, and every
/// `push` whose list is read from a binding copies. The linear and the
/// quadratic program of `tests/refcount_counters.rs` read 0.995 and 0.0 on the
/// machine and **both read 0.0** on the tree-walker, so a reader comparing them
/// there learns nothing and a reader who did not notice the engine learns
/// something false. `updates` and `updates_in_place` stay: those are counts of
/// what happened, not a claim about reuse.
///
/// Whole-run on the `ply run` path, and checked rather than assumed:
/// `rc::COUNTERS` is thread-local, and Ply's own `spawn` is a cooperative task
/// in `sched.rs`'s `Vec<Task>` rather than an OS thread, so every Ply
/// expression a `ply run` evaluates runs on the machine's own thread. `ply
/// test` is a different matter and deliberately has no counter surface: its
/// workers are threads and a pooled figure there would be a function of the
/// scheduler.
pub fn counters_json(stats: &ply_eval::rc::Stats, engine: ply_eval::Engine) -> Value {
    json!({
        "engine": engine.as_str(),
        "updates": stats.updates,
        "updates_in_place": stats.updates_in_place,
        "in_place": engine_in_place(stats, engine),
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
///
/// Printed beside the handshakes because it answers the same kind of question:
/// what did this run actually do. `in_place` leads because it is the one an
/// author can act on: it is the share of container updates the machine
/// performed without copying.
pub fn counters_line(stats: &ply_eval::rc::Stats, engine: ply_eval::Engine) -> String {
    let pct = |v: Option<f64>| match v {
        Some(v) => format!("{:.1}%", v * 100.0),
        None => "n/a".to_string(),
    };
    format!(
        "counters    {} · in place {} of {} ({}) · moved {} of {} · elided {}",
        engine.as_str(),
        stats.updates_in_place,
        stats.updates,
        pct(engine_in_place(stats, engine)),
        stats.takes_moved,
        stats.takes_attempted,
        pct(stats.elided()),
    )
}

/// [`ply_eval::rc::Stats::in_place`] where it says something about the program,
/// and `None` where it says something about the evaluator. See
/// [`counters_json`].
fn engine_in_place(stats: &ply_eval::rc::Stats, engine: ply_eval::Engine) -> Option<f64> {
    match engine {
        ply_eval::Engine::Treewalk => None,
        ply_eval::Engine::Machine => stats.in_place(),
    }
}

/// Which evaluator's counters a single-engine run produced.
///
/// `EngineChoice::Both` has no answer here and is refused rather than guessed:
/// `run.rs` reports `null` for that case, because the two engines do not count
/// the same thing and their sum is not a figure about either.
pub fn counted_engine(choice: ply_eval::EngineChoice) -> ply_eval::Engine {
    match choice {
        ply_eval::EngineChoice::Treewalk => ply_eval::Engine::Treewalk,
        _ => ply_eval::Engine::Machine,
    }
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

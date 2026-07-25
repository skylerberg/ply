use super::common::{
    IND, diagnostic_json, emit_json, exit_code, location, millis, phases_json, plural,
    print_diagnostics, print_phases, print_warnings, report_load_error,
};
use crate::cli::TestArgs;
use crate::driver;
use crate::load::{Loaded, load, project_root};
use crate::style::Style;
use crate::EXIT_COMPILE_ERROR;
use ply_core::{CheckOutput, Footprint};
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceMap, Span, codes};
use ply_store::Store;
use ply_test::{Reason, RunReport, Selection, Status, TestResult};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn execute(args: &TestArgs, style: Style) -> i32 {
    let mut warnings = Vec::new();
    let mut cache = match Cache::open(&project_root(&args.path), args.no_cache) {
        Ok(cache) => cache,
        Err(diagnostic) => {
            if args.json {
                emit_json(&json!({
                    "command": "test",
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "diagnostics": [diagnostic_json(&diagnostic, &SourceMap::new())],
                }));
            } else {
                print_diagnostics(std::slice::from_ref(&diagnostic), &SourceMap::new(), style);
            }
            return EXIT_COMPILE_ERROR;
        }
    };
    warnings.append(&mut cache.warnings);

    let incremental = !args.no_incremental && !args.no_cache;
    let loaded = if incremental {
        driver::load_incremental(&args.path, &mut cache.store)
    } else {
        load(&args.path)
    };
    let mut loaded = match loaded {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("test", &err, args.json, style),
    };
    warnings.extend(cache.store.take_warnings());
    warnings.extend(loaded.frontend.warnings.iter().cloned());

    let mut hashes = loaded.hashes.clone();
    let selected = ply_test::select(&loaded.check, &hashes, &cache.store);
    let mut plan = Plan::new(selected, &loaded.check, args.filter.as_deref());

    // Evaluation needs an AST, and gate 1 may have skipped the file a selected
    // test lives in. Only those modules are re-parsed — with their imports, which
    // is everything the tests can reach — rather than the whole project: a run
    // that reparses everything the moment one test is selected has no incremental
    // front end at all, and one selected test is the normal case.
    //
    // The parse is also the only check on the cache that does not trust it. A
    // fingerprint that authorized a skip and then disagrees with a real parse is
    // a cache that lied, so the hashes are compared and the run says so.
    //
    // *Every* selected test's module is named, not only the ones missing an AST
    // now. The first load wrote its fingerprints back, so a file it parsed
    // because its bytes changed is a file the second load would skip — and the
    // second load is the one that has to produce the bodies.
    let mut needed: Vec<ply_syntax::ast::ModuleName> = Vec::new();
    for test in plan.selection.to_run.iter().filter_map(|&i| loaded.check.tests.get(i)) {
        if !needed.contains(&test.module) {
            needed.push(test.module.clone());
        }
    }
    if needed.iter().any(|m| !loaded.has_ast(m)) {
        let unparsed = needed;
        let reloaded = if incremental {
            driver::load_to_evaluate(&args.path, &mut cache.store, &unparsed)
        } else {
            load(&args.path)
        };
        match reloaded {
            Ok(full) => {
                if disagrees(&hashes, &full.hashes) {
                    warnings.push(cache_lied());
                }
                loaded = full;
                hashes = loaded.hashes.clone();
                let selected = ply_test::select(&loaded.check, &hashes, &cache.store);
                plan = Plan::new(selected, &loaded.check, args.filter.as_deref());
            }
            Err(err) => return report_load_error("test", &err, args.json, style),
        }
    }

    let (pool, workers) = build_pool(args.jobs, &mut warnings);
    let mut run = || {
        ply_test::run(
            &plan.selection,
            &loaded.program,
            &loaded.resolved,
            &loaded.check,
            &hashes,
            &mut cache.store,
        )
    };
    let report = match &pool {
        Some(pool) => pool.install(run),
        None => run(),
    };
    warnings.extend(report.warnings.iter().cloned());

    if args.json {
        emit_json(&report_json(&loaded, &hashes, &plan, &report, args, workers, &warnings));
    } else {
        print_human(&loaded, &hashes, &plan, &report, args, workers, &warnings, style);
    }
    exit_code(report.is_success())
}

// --- Selection under `--filter` ---------------------------------------------

/// A filtered run must report `selected 2 of 3`, not `of 47`: the denominator a
/// person checks against is the set they asked for. Regrouping rather than
/// pruning the existing groups matters too — dropping a test can merge two
/// groups that only conflicted through it.
pub struct Plan {
    pub selection: Selection,
    /// Test indices still in scope, ascending.
    pub visible: Vec<usize>,
    pub filtered_out: usize,
}

impl Plan {
    pub fn new(selection: Selection, check: &CheckOutput, filter: Option<&str>) -> Plan {
        let Some(needle) = filter else {
            let visible = (0..check.tests.len()).collect();
            return Plan { selection, visible, filtered_out: 0 };
        };

        // Matched against `<module>.<label>` rather than the label alone, so
        // `--filter store.` narrows to a module without a second flag, and a
        // label substring still matches because the key contains the label.
        let visible: Vec<usize> = check
            .tests
            .iter()
            .enumerate()
            .filter(|(_, t)| t.key.as_str().contains(needle))
            .map(|(i, _)| i)
            .collect();
        let keeps = |i: &usize| visible.binary_search(i).is_ok();

        let cached: Vec<_> =
            selection.cached.into_iter().filter(|(i, _)| keeps(i)).collect();
        let to_run: Vec<usize> = selection.to_run.into_iter().filter(keeps).collect();
        let footprints: Vec<(usize, Footprint)> =
            to_run.iter().map(|&i| (i, check.tests[i].footprint.clone())).collect();
        let groups = ply_test::group_by_conflict(&footprints);

        Plan {
            filtered_out: check.tests.len() - visible.len(),
            selection: Selection {
                total: visible.len(),
                cached,
                to_run,
                groups,
                // Indexed by test index, so it stays whole even when the plan is
                // narrowed; nothing reads its length.
                reasons: selection.reasons,
            },
            visible,
        }
    }

    fn group_footprint(&self, group: &[usize], check: &CheckOutput) -> Footprint {
        group
            .iter()
            .filter_map(|&i| check.tests.get(i))
            .fold(Footprint::empty(), |acc, t| acc.union(&t.footprint))
    }
}

// --- Cache ------------------------------------------------------------------

/// `--no-cache` is honoured by pointing the store at a scratch directory that is
/// deleted on the way out. Selection and recording then need no special case,
/// and — the part that matters — a bypassed run cannot leave a result behind
/// that a later run would trust.
struct Cache {
    store: Store,
    scratch: Option<PathBuf>,
    /// An unusable cache must never stop a run, but it must never pass
    /// unmentioned either — silently re-running everything looks like a bug in
    /// selection, which is the one thing this system asks to be trusted on.
    warnings: Vec<Diagnostic>,
}

impl Cache {
    fn open(root: &Path, bypass: bool) -> Result<Cache, Diagnostic> {
        if bypass {
            return Cache::scratch();
        }
        match Store::open(root) {
            Ok(store) => Ok(Cache { store, scratch: None, warnings: Vec::new() }),
            Err(e) => {
                let mut cache = Cache::scratch()?;
                cache.warnings.push(
                    Diagnostic::warning(
                        codes::RUNTIME_ERROR,
                        format!("could not open the cache under `{}`: {e:#}", root.display()),
                    )
                    .note("every test ran, and nothing this run proved was recorded")
                    .note("check the directory's permissions to get caching back"),
                );
                Ok(cache)
            }
        }
    }

    fn scratch() -> Result<Cache, Diagnostic> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("ply-{}-{nonce}", std::process::id()));

        std::fs::create_dir_all(&dir).map_err(|e| scratch_failed(&dir, &e.to_string()))?;
        match Store::open(&dir) {
            Ok(store) => Ok(Cache { store, scratch: Some(dir), warnings: Vec::new() }),
            Err(e) => Err(scratch_failed(&dir, &format!("{e:#}"))),
        }
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        if let Some(dir) = &self.scratch {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// The front-end cache authorized a skip and a real parse then disagreed with
/// it. Nothing the user wrote caused this, and the run recovered, but a cache
/// that can be wrong once must never be wrong silently.
/// Whether a real parse produced a different hash from the one the front-end
/// cache had promised. Only names present in both are compared: the second load
/// parses more than the first, so it legitimately knows more.
fn disagrees(cached: &HashOutput, parsed: &HashOutput) -> bool {
    cached
        .defs
        .iter()
        .any(|(name, hash)| parsed.defs.get(name).is_some_and(|fresh| fresh != hash))
        || cached.tests != parsed.tests
}

fn cache_lied() -> Diagnostic {
    Diagnostic::warning(
        codes::CACHE_CORRUPT,
        "the front-end cache disagreed with a fresh parse; this run ignored it",
    )
    .note("every file was parsed and every definition rechecked")
    .note("run `ply cache clear` if it happens again")
}

fn scratch_failed(dir: &Path, cause: &str) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("could not create a scratch cache at `{}`: {cause}", dir.display()),
    )
    .primary(Span::DUMMY, "the run needs somewhere to record results")
    .note("set TMPDIR to a writable directory, or drop `--no-cache`")
}

fn build_pool(
    jobs: Option<u32>,
    warnings: &mut Vec<Diagnostic>,
) -> (Option<rayon::ThreadPool>, usize) {
    let requested = jobs.unwrap_or(0) as usize;
    match rayon::ThreadPoolBuilder::new().num_threads(requested).build() {
        Ok(pool) => {
            let workers = pool.current_num_threads();
            (Some(pool), workers)
        }
        Err(e) => {
            warnings.push(
                Diagnostic::warning(
                    codes::RUNTIME_ERROR,
                    format!("could not start {requested} worker threads: {e}"),
                )
                .note("the run continued on the default thread pool"),
            );
            (None, rayon::current_num_threads())
        }
    }
}

// --- Human output -----------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn print_human(
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: &Plan,
    report: &RunReport,
    args: &TestArgs,
    workers: usize,
    warnings: &[Diagnostic],
    style: Style,
) {
    let selection = &plan.selection;

    println!(
        "{IND}{} {} of {} ({} cached)",
        style.bold("selected"),
        style.bold(&selection.to_run.len().to_string()),
        selection.total,
        selection.cached.len()
    );
    if !selection.to_run.is_empty() {
        println!(
            "{IND}{} {} · {workers} {}",
            selection.groups.len(),
            plural(selection.groups.len(), "group"),
            plural(workers, "worker")
        );
    }
    if args.no_cache {
        println!("{IND}{}", style.dim("--no-cache: results were neither read nor recorded"));
    }
    if plan.filtered_out > 0 {
        println!(
            "{IND}{}",
            style.dim(&format!(
                "--filter hid {} {}",
                plan.filtered_out,
                plural(plan.filtered_out, "test")
            ))
        );
    }

    if args.explain {
        print_explain(loaded, hashes, plan, style);
        print_phases(&loaded.frontend.phases, style);
    }

    if !report.results.is_empty() {
        println!();
        let names: Vec<String> = report
            .results
            .iter()
            .map(|r| display_name(&loaded.check, r.index, &r.name))
            .collect();
        let name_width = name_column(&names);
        for (result, name) in report.results.iter().zip(&names) {
            println!("{IND}{}", result_line(result, name, name_width, style));
        }
    }

    println!();
    print_summary(report, style);

    for failure in &report.failures {
        println!();
        println!("{IND}{}", style.bold(failure.key.as_str()));
        println!("{IND}  {}", failure.diagnostic.message);
        if let Some(at) =
            failure.diagnostic.primary_span().and_then(|s| location(&loaded.sources, s))
        {
            println!("{IND}    at {}", style.dim(&at));
        }
        for note in &failure.diagnostic.notes {
            println!("{IND}  {} {note}", style.dim("="));
        }
        if failure.suspects.is_empty() {
            println!("{IND}  {}", style.dim("suspects: none — nothing in this test's closure changed"));
        } else {
            let names: Vec<&str> = failure.suspects.iter().map(|s| s.as_str()).collect();
            println!("{IND}  {} {}", style.yellow("suspects:"), names.join(", "));
        }
    }

    if selection.total == 0 {
        println!();
        println!("{IND}{}", style.dim(no_tests_note(loaded, args)));
    }

    if !warnings.is_empty() {
        println!();
        print_warnings(warnings, style);
    }
}

fn no_tests_note(loaded: &Loaded, args: &TestArgs) -> &'static str {
    if loaded.check.tests.is_empty() {
        "no `test` items in this program"
    } else if args.filter.is_some() {
        "no test key contains that substring; nothing was verified"
    } else {
        "nothing to report"
    }
}

/// Two modules may label a test identically, so a single-module run reads the
/// label and anything larger reads `<module>.<label>` — the same key `--filter`
/// matches on.
fn display_name(check: &CheckOutput, index: usize, fallback: &str) -> String {
    match check.tests.get(index) {
        Some(test) if check.modules.len() > 1 => test.key.to_string(),
        Some(test) => test.name.clone(),
        None => fallback.to_string(),
    }
}

/// Long test names are the norm — they are sentences — so the duration column
/// follows the longest one rather than a guess that everything overruns.
fn name_column(names: &[String]) -> usize {
    names.iter().map(|n| n.chars().count()).max().unwrap_or(0).clamp(24, 64)
}

fn result_line(result: &TestResult, name: &str, name_width: usize, style: Style) -> String {
    let (mark, width): (String, usize) = if style.is_styled() {
        let mark = match result.status {
            Status::Passed => style.green("✓"),
            Status::Failed => style.red("✗"),
            Status::Panicked => style.yellow("!"),
        };
        (mark, 1)
    } else {
        let mark = match result.status {
            Status::Passed => "ok",
            Status::Failed => "FAIL",
            Status::Panicked => "PANIC",
        };
        (mark.to_string(), 5)
    };
    // `mark` may carry escapes, so the padding is computed rather than left to
    // `{:<width$}`, which counts bytes.
    let pad = " ".repeat(width.saturating_sub(display_width(&mark)));
    format!("{mark}{pad} {name:<name_width$} {:>8.1}ms", millis(result.duration))
}

fn print_summary(report: &RunReport, style: Style) {
    let failed = format!("{} failed", report.failed);
    let failed = if report.failed > 0 { style.red(&failed) } else { style.dim(&failed) };
    let passed = format!("{} passed", report.passed);
    let passed = if report.passed > 0 { style.green(&passed) } else { style.dim(&passed) };
    println!(
        "{IND}{failed}, {passed}, {} cached ({:.2}s)",
        report.cached,
        report.duration.as_secs_f64()
    );
}

fn print_explain(loaded: &Loaded, hashes: &HashOutput, plan: &Plan, style: Style) {
    let check = &loaded.check;
    println!();
    for file in &loaded.frontend.files {
        let state = if !file.parsed {
            style.green("skipped")
        } else if file.rechecked {
            style.yellow("checked")
        } else {
            style.dim("parsed")
        };
        println!(
            "{IND}{state:<9} {} {}",
            file.path.display(),
            style.dim(&file.refusal.describe())
        );
    }
    println!();
    for &index in &plan.visible {
        let Some(test) = check.tests.get(index) else { continue };
        let reason = plan.selection.reason(index).unwrap_or(Reason::Unhashed);
        let verb = if reason.runs() { style.bold("run ") } else { style.dim("skip") };
        let hash = hashes
            .tests
            .get(index)
            .map(|h| h.short())
            .unwrap_or_else(|| "-".repeat(12));
        println!(
            "{IND}{verb} {} {:<16} {}",
            style.dim(&hash),
            reason.as_str(),
            display_name(check, index, &test.name)
        );
    }

    let mut seen: Vec<Reason> = Vec::new();
    for &index in &plan.visible {
        if let Some(r) = plan.selection.reason(index)
            && !seen.contains(&r)
        {
            seen.push(r);
        }
    }
    if !seen.is_empty() {
        println!();
        println!("{IND}{}", style.dim("why"));
        for reason in seen {
            println!("{IND}  {:<16} {}", reason.as_str(), style.dim(why(reason)));
        }
    }

    if plan.selection.groups.is_empty() {
        return;
    }
    println!();
    println!("{IND}{}", style.dim("concurrency groups"));
    for (g, group) in plan.selection.groups.iter().enumerate() {
        let footprint = plan.group_footprint(group, check);
        println!(
            "{IND}  group {g} · {} {} · {}",
            group.len(),
            plural(group.len(), "test"),
            style.dim(&footprint.to_string())
        );
        for &index in group {
            if let Some(test) = check.tests.get(index) {
                println!("{IND}    {}", display_name(check, index, &test.name));
            }
        }
    }
}

pub fn why(reason: Reason) -> &'static str {
    match reason {
        Reason::New => "this hash has never gone green, so nothing is known about it",
        Reason::Nondet => "`test/nondet` always runs and is never cached",
        Reason::PreviousFailure => "the cache holds a failure, and a failure is never trusted",
        Reason::Cached => "this exact hash already passed; re-running cannot reveal anything new",
        Reason::Unhashed => "no hash was produced, so the cache cannot answer for it",
    }
}

// --- JSON output ------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn report_json(
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: &Plan,
    report: &RunReport,
    args: &TestArgs,
    workers: usize,
    warnings: &[Diagnostic],
) -> Value {
    let sources = &loaded.sources;
    let check = &loaded.check;
    let selection = &plan.selection;

    let tests: Vec<Value> = plan
        .visible
        .iter()
        .filter_map(|&index| {
            let test = check.tests.get(index)?;
            let reason = selection.reason(index).unwrap_or(Reason::Unhashed);
            Some(json!({
                "index": index,
                "key": test.key,
                "name": test.name,
                "module": test.module.as_str(),
                "hash": hashes.tests.get(index).map(|h| h.to_hex()),
                "nondet": test.nondet,
                "selected": reason.runs(),
                "reason": reason,
                "why": why(reason),
                "group": selection.group_of(index),
                "footprint": test.footprint.to_string(),
            }))
        })
        .collect();

    let groups: Vec<Value> = selection
        .groups
        .iter()
        .enumerate()
        .map(|(g, group)| {
            json!({
                "index": g,
                "tests": group,
                "footprint": plan.group_footprint(group, check).to_string(),
            })
        })
        .collect();

    let results: Vec<Value> = report
        .results
        .iter()
        .map(|r| {
            let test = check.tests.get(r.index);
            json!({
                "index": r.index,
                "key": test.map(|t| t.key.clone()),
                "name": test.map_or_else(|| r.name.clone(), |t| t.name.clone()),
                "module": test.map(|t| t.module.to_string()),
                "hash": r.hash.map(|h| h.to_hex()),
                "group": r.group,
                "status": r.status,
                "duration_ms": millis(r.duration),
                "diagnostic": r.failure.as_ref().map(|d| diagnostic_json(d, sources)),
            })
        })
        .collect();

    let failures: Vec<Value> = report
        .failures
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "key": f.key,
                "diagnostic": diagnostic_json(&f.diagnostic, sources),
                "location": f.diagnostic.primary_span().and_then(|s| location(sources, s)),
                "suspects": f.suspects,
            })
        })
        .collect();

    json!({
        "command": "test",
        "front_end": json!({
            "incremental": loaded.frontend.incremental,
            "parsed": loaded.frontend.parsed(),
            "skipped": loaded.frontend.skipped(),
            "cached": loaded.frontend.cached(),
            "rechecked": loaded.frontend.rechecked(),
            "phases": phases_json(&loaded.frontend.phases),
        }),
        "ok": report.is_success(),
        "exit_code": exit_code(report.is_success()),
        "root": loaded.root.display().to_string(),
        "files": loaded.file_names(),
        "modules": loaded.modules().iter().map(|m| json!({
            "name": m.name.as_str(),
            "file": m.path.display().to_string(),
        })).collect::<Vec<_>>(),
        "filter": args.filter,
        "no_cache": args.no_cache,
        "workers": workers,
        "selection": {
            "total": selection.total,
            "selected": selection.to_run.len(),
            "cached": selection.cached.len(),
            "filtered_out": plan.filtered_out,
            "groups": groups,
            "tests": tests,
        },
        "summary": {
            "passed": report.passed,
            "failed": report.failed,
            "cached": report.cached,
            "duration_ms": millis(report.duration),
        },
        "results": results,
        "failures": failures,
        "diagnostics": Value::Array(Vec::new()),
        "warnings": warnings.iter().map(|w| diagnostic_json(w, sources)).collect::<Vec<_>>(),
    })
}

fn display_width(s: &str) -> usize {
    crate::style::strip_ansi(s).chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_store::Outcome;
    use std::time::Duration;

    /// A test's footprint is what its handlers did *not* discharge, so the way
    /// to give one a residual atom is to grant it less than the code it calls
    /// may use. Both branches below are reachable per the signature; only the
    /// granted one runs.
    const SOURCE: &str = "\
effect db {
  read  all[t]() -> List<Int>
  write save[t](rows: List<Int>) -> Unit
}

fn peek(table: String) -> Int / {db.read[users], db.read[orders]} =
  if table == \"users\" { len(db.all[users]()) } else { len(db.all[orders]()) }

fn wipe(table: String) -> Unit / {db.write[users], db.read[orders]} =
  if table == \"users\" { db.save[users]([]) } else { assert_eq(len(db.all[orders]()), 0) }

test \"reads orders only\" {
  handle { assert_eq(peek(\"orders\"), 0) } with { db.all[orders]() -> [] }
}

test \"reads orders only again\" {
  handle { assert_eq(peek(\"orders\"), 0) } with { db.all[orders]() -> [] }
}

test \"writes users when asked\" {
  handle { wipe(\"orders\") } with { db.all[orders]() -> [] }
}

test \"pure arithmetic\" { assert_eq(1 + 1, 2) }
";

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, Loaded, HashOutput) {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in files {
            write(dir.path(), rel, text);
        }
        let loaded = load(dir.path()).unwrap();
        let hashes = loaded.hashes().unwrap();
        (dir, loaded, hashes)
    }

    fn fixture() -> (tempfile::TempDir, Loaded, HashOutput) {
        project(&[("m.ply", SOURCE)])
    }

    fn run(loaded: &Loaded, selection: &Selection, store: &mut Store) -> RunReport {
        ply_test::run(
            selection,
            &loaded.program,
            &loaded.resolved,
            &loaded.check,
            &loaded.hashes().unwrap(),
            store,
        )
    }

    fn plan_for(filter: Option<&str>) -> (tempfile::TempDir, Loaded, HashOutput, Plan) {
        let (dir, loaded, hashes) = fixture();
        let store = Store::open(dir.path()).unwrap();
        let selected = ply_test::select(&loaded.check, &hashes, &store);
        let plan = Plan::new(selected, &loaded.check, filter);
        (dir, loaded, hashes, plan)
    }

    fn args_for(filter: Option<&str>) -> TestArgs {
        TestArgs {
            path: PathBuf::from("."),
            json: true,
            explain: false,
            no_incremental: false,
            no_cache: false,
            filter: filter.map(str::to_string),
            jobs: None,
        }
    }

    #[test]
    fn a_cold_cache_selects_everything() {
        let (_dir, loaded, _h, plan) = plan_for(None);
        assert_eq!(plan.selection.total, 4);
        assert_eq!(plan.selection.to_run.len(), 4);
        assert!(plan.selection.cached.is_empty());
        assert_eq!(plan.visible, vec![0, 1, 2, 3]);
        assert_eq!(loaded.check.tests.len(), 4);
    }

    #[test]
    fn a_write_is_scheduled_apart_from_the_reads_of_the_same_resource() {
        let (_dir, loaded, _h, plan) = plan_for(None);
        let index_of =
            |name: &str| loaded.check.tests.iter().position(|t| t.name == name).unwrap();
        let group_of = |name: &str| plan.selection.group_of(index_of(name)).unwrap();

        assert_eq!(
            loaded.check.tests[index_of("reads orders only")].footprint.to_string(),
            "{m.db.read[users]}"
        );
        assert_eq!(
            loaded.check.tests[index_of("writes users when asked")].footprint.to_string(),
            "{m.db.write[users]}"
        );
        assert_eq!(group_of("reads orders only"), group_of("reads orders only again"));
        assert_ne!(group_of("reads orders only"), group_of("writes users when asked"));
        // A test that touches nothing conflicts with nothing, so it shares
        // whichever group it lands in rather than forcing a third.
        assert_eq!(plan.selection.groups.len(), 2);
    }

    #[test]
    fn the_filter_renarrows_the_denominator_and_regroups() {
        let (_dir, _l, _h, plan) = plan_for(Some("reads orders"));
        assert_eq!(plan.selection.total, 2);
        assert_eq!(plan.filtered_out, 2);
        assert_eq!(plan.visible.len(), 2);
        // Both survivors only read, so the writer's group is gone entirely.
        assert_eq!(plan.selection.groups.len(), 1);
        assert_eq!(plan.selection.groups[0].len(), 2);
    }

    #[test]
    fn a_filter_matching_nothing_selects_nothing_and_says_so() {
        let (_dir, loaded, _h, plan) = plan_for(Some("no such test"));
        assert_eq!(plan.selection.total, 0);
        assert!(plan.selection.to_run.is_empty());
        assert!(plan.selection.groups.is_empty());
        assert!(no_tests_note(&loaded, &args_for(Some("no such test"))).contains("substring"));
    }

    #[test]
    fn the_filter_matches_the_module_qualified_key() {
        let (dir, loaded, hashes) = project(&[
            ("alpha.ply", "test \"shared label\" { assert_eq(1, 1) }\n"),
            ("beta.ply", "test \"shared label\" { assert_eq(2, 2) }\n"),
        ]);
        let store = Store::open(dir.path()).unwrap();
        let keys: Vec<&str> = loaded.check.tests.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys, ["alpha.shared label", "beta.shared label"]);

        let select = |needle| {
            Plan::new(
                ply_test::select(&loaded.check, &hashes, &store),
                &loaded.check,
                Some(needle),
            )
        };
        assert_eq!(select("beta.").visible, vec![1]);
        assert_eq!(select("shared label").visible, vec![0, 1]);
    }

    #[test]
    fn a_warm_cache_selects_nothing_and_a_second_run_stays_empty() {
        let (dir, loaded, hashes) = fixture();
        let mut store = Store::open(dir.path()).unwrap();
        let selected = ply_test::select(&loaded.check, &hashes, &store);
        let report = run(&loaded, &selected, &mut store);
        assert_eq!(report.passed, 4);
        assert_eq!(report.failed, 0);

        let store = Store::open(dir.path()).unwrap();
        let again = ply_test::select(&loaded.check, &hashes, &store);
        assert!(again.to_run.is_empty());
        assert_eq!(again.cached.len(), 4);
        assert_eq!(Plan::new(again, &loaded.check, None).selection.total, 4);
    }

    #[test]
    fn tests_from_every_module_are_selected_and_run_together() {
        let (dir, loaded, hashes) = project(&[
            ("lib.ply", "pub fn one() -> Int = 1\ntest \"one is one\" { assert_eq(one(), 1) }\n"),
            (
                "app.ply",
                "import lib\n\
                 fn two() -> Int = lib::one() + lib::one()\n\
                 test \"two is two\" { assert_eq(two(), 2) }\n",
            ),
        ]);
        assert_eq!(loaded.module_count(), 2);

        let mut store = Store::open(dir.path()).unwrap();
        let selected = ply_test::select(&loaded.check, &hashes, &store);
        assert_eq!(selected.total, 2);
        let report = run(&loaded, &selected, &mut store);
        assert_eq!(report.passed, 2, "failures: {:?}", report.failures);
        assert!(report.is_success());
    }

    #[test]
    fn a_failing_test_names_its_suspects_by_program_wide_name() {
        let (dir, loaded, hashes) = project(&[
            ("lib.ply", "pub fn one() -> Int = 2\n"),
            (
                "app.ply",
                "import lib\ntest \"one is one\" { assert_eq(lib::one(), 1) }\n",
            ),
        ]);
        let mut store = Store::open(dir.path()).unwrap();
        let selected = ply_test::select(&loaded.check, &hashes, &store);
        let report = run(&loaded, &selected, &mut store);

        assert_eq!(report.failed, 1);
        assert_eq!(report.failures[0].suspects, vec![ply_span::Symbol::new("lib.one")]);
    }

    #[test]
    fn no_cache_never_touches_the_real_store() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "m.ply", SOURCE);
        let loaded = load(dir.path()).unwrap();
        let hashes = loaded.hashes().unwrap();

        let scratch_dir = {
            let mut cache = Cache::open(dir.path(), true).unwrap();
            let scratch = cache.scratch.clone().expect("bypass must use a scratch store");
            let selected = ply_test::select(&loaded.check, &hashes, &cache.store);
            assert_eq!(selected.to_run.len(), 4);
            run(&loaded, &selected, &mut cache.store);
            assert!(!cache.store.is_empty());
            scratch
        };

        assert!(!scratch_dir.exists(), "the scratch cache outlived the run");
        assert_eq!(Store::open(dir.path()).unwrap().len(), 0);
    }

    #[test]
    fn an_unopenable_cache_still_runs_but_says_it_gave_up_on_caching() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the cache directory needs to go: `create_dir_all` cannot
        // succeed, so `Store::open` has to fail.
        std::fs::write(dir.path().join(ply_store::CACHE_DIR_NAME), "in the way").unwrap();

        let cache = Cache::open(dir.path(), false).unwrap();
        assert!(cache.scratch.is_some(), "the run must fall back rather than abort");
        assert_eq!(cache.warnings.len(), 1);
        assert!(cache.warnings[0].message.contains("could not open the cache"));
        assert!(cache.warnings[0].notes.iter().any(|n| n.contains("nothing this run proved")));
    }

    #[test]
    fn a_failure_is_never_cached_so_it_re_runs() {
        let (dir, loaded, hashes) =
            project(&[("m.ply", "fn f() -> Int = 1\ntest \"wrong\" { assert_eq(f(), 2) }\n")]);
        let mut store = Store::open(dir.path()).unwrap();

        let selected = ply_test::select(&loaded.check, &hashes, &store);
        let report = run(&loaded, &selected, &mut store);
        assert_eq!(report.failed, 1);
        assert_eq!(exit_code(report.is_success()), crate::EXIT_FAILED);
        assert_eq!(report.failures[0].diagnostic.code, codes::ASSERTION_FAILED);

        let store = Store::open(dir.path()).unwrap();
        let again = ply_test::select(&loaded.check, &hashes, &store);
        assert_eq!(again.to_run.len(), 1, "a red test must re-run");
    }

    #[test]
    fn the_json_report_is_one_object_with_selection_results_and_failures() {
        let (dir, loaded, hashes) = project(&[(
            "m.ply",
            "fn f() -> Int = 1\n\
             test \"good\" { assert_eq(f(), 1) }\n\
             test \"bad\" { assert_eq(f(), 2) }\n",
        )]);
        let mut store = Store::open(dir.path()).unwrap();
        let plan =
            Plan::new(ply_test::select(&loaded.check, &hashes, &store), &loaded.check, None);
        let report = run(&loaded, &plan.selection, &mut store);

        let v = report_json(&loaded, &hashes, &plan, &report, &args_for(None), 4, &[]);

        assert_eq!(v["command"], "test");
        assert_eq!(v["ok"], false);
        assert_eq!(v["exit_code"], 1);
        assert_eq!(v["selection"]["total"], 2);
        assert_eq!(v["selection"]["selected"], 2);
        assert_eq!(v["selection"]["tests"][0]["reason"], "new");
        assert_eq!(v["selection"]["tests"][0]["key"], "m.good");
        assert_eq!(v["selection"]["tests"][0]["module"], "m");
        assert_eq!(v["selection"]["tests"][0]["hash"].as_str().unwrap().len(), 64);
        assert_eq!(v["modules"][0]["name"], "m");
        assert_eq!(v["failures"].as_array().unwrap().len(), 1);
        assert_eq!(v["failures"][0]["diagnostic"]["code"], codes::ASSERTION_FAILED);
        assert_eq!(v["failures"][0]["diagnostic"]["labels"][0]["start"]["line"], 3);
        // `f` is in the failing test's closure and has never gone green.
        assert_eq!(v["failures"][0]["suspects"], json!(["m.f"]));
        assert!(v["failures"][0]["location"].as_str().unwrap().contains("m.ply:3:"));
        assert_eq!(v["summary"]["failed"], 1);
        assert_eq!(v["summary"]["passed"], 1);
    }

    #[test]
    fn two_failures_sharing_a_label_are_told_apart_by_their_key() {
        let (dir, loaded, hashes) = project(&[
            ("alpha.ply", "fn one() -> Int = 1\ntest \"it adds up\" { assert_eq(one(), 2) }\n"),
            ("beta.ply", "fn two() -> Int = 2\ntest \"it adds up\" { assert_eq(two(), 3) }\n"),
        ]);
        let mut store = Store::open(dir.path()).unwrap();
        let plan =
            Plan::new(ply_test::select(&loaded.check, &hashes, &store), &loaded.check, None);
        let report = run(&loaded, &plan.selection, &mut store);
        assert_eq!(report.failed, 2);

        let v = report_json(&loaded, &hashes, &plan, &report, &args_for(None), 4, &[]);
        let keys: Vec<&str> =
            v["failures"].as_array().unwrap().iter().map(|f| f["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["alpha.it adds up", "beta.it adds up"]);
        for f in v["failures"].as_array().unwrap() {
            assert_eq!(f["name"], "it adds up");
        }
    }

    #[test]
    fn every_reason_has_a_distinct_explanation() {
        let all = [
            Reason::New,
            Reason::Nondet,
            Reason::PreviousFailure,
            Reason::Cached,
            Reason::Unhashed,
        ];
        let mut seen: Vec<&str> = all.iter().map(|r| why(*r)).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), all.len());
    }

    #[test]
    fn a_nondet_test_always_runs_and_is_never_recorded() {
        let (dir, loaded, hashes) = project(&[(
            "m.ply",
            "nondet effect clock {\n  read now() -> Int\n}\n\
             test/nondet \"reads the clock\" { assert(clock.now() > 0) }\n",
        )]);
        let mut store = Store::open(dir.path()).unwrap();

        let selected = ply_test::select(&loaded.check, &hashes, &store);
        assert_eq!(selected.reason(0), Some(Reason::Nondet));
        run(&loaded, &selected, &mut store);

        let store = Store::open(dir.path()).unwrap();
        let again = ply_test::select(&loaded.check, &hashes, &store);
        assert_eq!(again.to_run, vec![0]);
    }

    #[test]
    fn a_label_is_shown_bare_in_one_module_and_qualified_across_several() {
        let (_dir, one, _h) = project(&[("m.ply", "test \"only\" { assert_eq(1, 1) }\n")]);
        assert_eq!(display_name(&one.check, 0, "fallback"), "only");

        let (_dir, two, _h) = project(&[
            ("alpha.ply", "test \"shared\" { assert_eq(1, 1) }\n"),
            ("beta.ply", "test \"shared\" { assert_eq(2, 2) }\n"),
        ]);
        assert_eq!(display_name(&two.check, 0, "fallback"), "alpha.shared");
        assert_eq!(display_name(&two.check, 1, "fallback"), "beta.shared");
        assert_eq!(display_name(&two.check, 9, "fallback"), "fallback");
    }

    #[test]
    fn marks_are_ascii_when_unstyled_and_glyphs_when_styled() {
        let result = TestResult {
            index: 0,
            name: "balance never goes negative".into(),
            hash: None,
            group: 0,
            duration: Duration::from_micros(2100),
            status: Status::Failed,
            failure: None,
        };
        let plain = result_line(&result, &result.name, 44, Style::plain());
        assert!(plain.starts_with("FAIL "));
        assert!(!plain.contains('\x1b'));
        assert!(plain.contains("balance never goes negative"));
        assert!(plain.trim_end().ends_with("2.1ms"));

        let styled = result_line(&result, &result.name, 44, Style::new(true));
        assert!(styled.contains('✗'));
        assert!(styled.contains('\x1b'));
    }

    #[test]
    fn the_pass_and_panic_marks_line_up_with_the_failure_mark() {
        let make = |status| TestResult {
            index: 0,
            name: "n".into(),
            hash: None,
            group: 0,
            duration: Duration::from_millis(1),
            status,
            failure: None,
        };
        let column =
            |status| result_line(&make(status), "n", 24, Style::plain()).find("n ").unwrap();
        assert_eq!(column(Status::Passed), column(Status::Failed));
        assert_eq!(column(Status::Passed), column(Status::Panicked));
    }

    #[test]
    fn cached_results_do_not_appear_as_run_results() {
        let (dir, loaded, hashes) = fixture();
        let mut store = Store::open(dir.path()).unwrap();
        let first = ply_test::select(&loaded.check, &hashes, &store);
        run(&loaded, &first, &mut store);

        let mut store = Store::open(dir.path()).unwrap();
        let second = ply_test::select(&loaded.check, &hashes, &store);
        let report = run(&loaded, &second, &mut store);
        assert!(report.results.is_empty());
        assert_eq!(report.cached, 4);
        assert!(report.is_success());
    }

    #[test]
    fn a_stored_failure_is_re_run_rather_than_believed() {
        let (dir, loaded, hashes) = fixture();
        let mut store = Store::open(dir.path()).unwrap();
        store.put(
            hashes.tests[0],
            Outcome::Fail { message: "from an older runtime".into(), diagnostic: None },
        );
        store.flush().unwrap();

        let selected = ply_test::select(&loaded.check, &hashes, &store);
        assert_eq!(selected.reason(0), Some(Reason::PreviousFailure));
        assert!(selected.to_run.contains(&0));
    }

    /// Two structurally identical tests in different modules have the same
    /// hash, so proving one proves the other — the corollary the ADR calls out
    /// as looking like a bug.
    #[test]
    fn identical_tests_in_two_modules_share_one_cache_entry() {
        let (dir, loaded, hashes) = project(&[
            ("alpha.ply", "test \"same\" { assert_eq(1 + 1, 2) }\n"),
            ("beta.ply", "test \"same\" { assert_eq(1 + 1, 2) }\n"),
        ]);
        assert_eq!(hashes.tests[0], hashes.tests[1]);

        let mut store = Store::open(dir.path()).unwrap();
        let selected = ply_test::select(&loaded.check, &hashes, &store);
        assert_eq!(selected.to_run.len(), 2);
        run(&loaded, &selected, &mut store);

        let store = Store::open(dir.path()).unwrap();
        let again = ply_test::select(&loaded.check, &hashes, &store);
        assert!(again.to_run.is_empty());
        assert_eq!(again.cached.len(), 2);
    }
}

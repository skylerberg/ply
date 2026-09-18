use super::common::{
    IND, backend_spec, build_backend_over, build_pool, describe_schema, diagnostic_json,
    diagnostics_json, emit_json, exit_code, location, millis, once_each, phases_json, plural,
    print_diagnostics, print_phases, print_warnings, report_bind_error, report_load_error,
    select_profile,
};
use crate::EXIT_COMPILE_ERROR;
use crate::cli::{TestArgs, When};
use crate::driver;
use crate::hosts::{self, Hosts, hosting};
use crate::load::{Loaded, load, project_root};
use crate::style::Style;
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceMap, Span, codes};
use ply_store::Store;
use ply_test::{
    Bisection, Failure, Isolation, Reason, Record, RunReport, Selection, Skipped, Status, Suspect,
    TestResult, Verdict,
};
use ply_ty::{CheckOutput, Footprint};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub fn execute(args: &TestArgs, style: Style) -> i32 {
    let warnings = Vec::new();
    // Before the store opens, so a refused `--backend` leaves no cache directory behind.
    let backend =
        match select_profile(&args.profile).and_then(|()| backend_spec(args.backend.as_ref())) {
            Ok(spec) => spec,
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
    let no_cache = cache_bypassed(args);
    let engine = super::common::engine_of(backend.as_ref());
    let mut cache = match Cache::open(&project_root(&args.path), no_cache) {
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
    let mut warm = crate::warm::Warm::default();
    if !args.watch {
        return iterate(
            args, style, &mut cache, &backend, &engine, &mut warm, warnings,
        );
    }
    watch(
        args, style, &mut cache, &backend, &engine, &mut warm, warnings,
    )
}

/// Re-run whenever the tree moves, holding what the last iteration built.
fn watch(
    args: &TestArgs,
    style: Style,
    cache: &mut Cache,
    backend: &Option<ply_eval::BackendSpec>,
    engine: &ply_test::Engine,
    warm: &mut crate::warm::Warm,
    warnings: Vec<Diagnostic>,
) -> i32 {
    let root = project_root(&args.path);
    iterate(args, style, cache, backend, engine, warm, warnings);
    loop {
        // Polling: the walk is owed anyway, so a watcher would add a dependency for no latency.
        std::thread::sleep(std::time::Duration::from_millis(120));
        if !warm.tree_moved(&root) {
            continue;
        }
        if !args.json {
            println!();
        }
        iterate(args, style, cache, backend, engine, warm, Vec::new());
    }
}

#[allow(clippy::too_many_arguments)]
fn iterate(
    args: &TestArgs,
    style: Style,
    cache: &mut Cache,
    backend: &Option<ply_eval::BackendSpec>,
    engine: &ply_test::Engine,
    warm: &mut crate::warm::Warm,
    mut warnings: Vec<Diagnostic>,
) -> i32 {
    let no_cache = cache_bypassed(args);
    warnings.append(&mut cache.warnings);
    let opened = cache.store.take_warnings();
    let migration = crate::migrate::notice(&cache.store, &opened);
    warnings.extend(opened);
    warnings.extend(migration);

    let incremental = !args.no_incremental && !no_cache;
    // The front end is a function of the sources, so an unmoved tree reuses it.
    let (held, reuse) = warm.take(&project_root(&args.path));
    let loaded = match held {
        Some(loaded) => Ok(loaded),
        None if incremental => driver::load_incremental(&args.path, &mut cache.store),
        None => load(&args.path),
    };
    let loaded = match loaded {
        Ok(mut loaded) => {
            if reuse == crate::warm::Reuse::Whole {
                // Nothing was re-derived, so this iteration reports no phase time.
                loaded.frontend.phases = driver::Phases::default();
            }
            loaded
        }
        Err(err) => return report_load_error("test", &err, args.json, style),
    };
    warnings.extend(cache.store.take_warnings());
    warnings.extend(loaded.frontend.warnings.iter().cloned());

    let hashes = loaded.hashes.clone();
    // Part of a simulated test's cache key, so decided before selection.
    let search = crate::simulation::plan(&args.simulation);
    let selected = ply_test::select(&loaded.check, &hashes, &cache.store, &search, engine);
    let plan = Plan::new(selected, &loaded.check, args.filter.as_deref(), args.std);

    if let Some(err) = broken_promises(&loaded) {
        return report_load_error("test", &err, args.json, style);
    }

    // Before anything runs, so no test touches a resource the program does not declare.
    let db = match args.db.resolve(args.host) {
        Ok(db) => db,
        Err(diagnostics) => {
            return report_bind_error("test", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    let reach = ply_ty::ty::Footprint::from_atoms(
        loaded
            .check
            .tests
            .iter()
            .flat_map(|t| t.footprint.atoms().cloned()),
    );
    // Before binding, so a missing required key fails before any host test runs.
    let (configuration, config_warnings) = match crate::config::Configuration::open(
        &loaded.program,
        &loaded.resolved,
        &loaded.check,
        args.host,
        &args.config,
    ) {
        Ok(resolved) => resolved,
        Err(diagnostics) => {
            return report_bind_error("test", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    warnings.extend(config_warnings);
    let mut hosts = match Hosts::open(
        &loaded.check,
        args.host,
        &args.tls.tls,
        &args.fs.fs,
        db,
        configuration,
        // `--trace` on this command names the definition trace, so records are discarded.
        &crate::trace::TraceOptions::silent(),
        Some(&reach),
    ) {
        Ok(hosts) => hosts,
        Err(diagnostics) => {
            return report_bind_error("test", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    describe_schema(&loaded, &mut hosts);
    let hosts = hosts;

    let (pool, workers) = build_pool(args.jobs, &mut warnings);
    let simulation =
        ply_test::Search::of(&plan.selection).measuring(args.simulation.measure_reduction);
    // A factory: a reactor belongs to its thread, and each worker builds its own machine.
    let runtime = hosts.runtime_factory();
    // One per run, shared by the workers; an empty selection builds nothing.
    let nothing_to_run = plan.selection.to_run.is_empty();
    // A backend answers only for the program it was built over, and the machine checks that.
    let (run_program, run_resolved) = (&loaded.program, &loaded.resolved);
    // The last iteration's unit, when every definition is unchanged.
    let held_unit = backend
        .as_ref()
        .filter(|_| !nothing_to_run)
        .and_then(|spec| warm.unit_for(spec, &hashes));
    let provider = match backend
        .as_ref()
        .filter(|_| !nothing_to_run)
        .filter(|_| held_unit.is_none())
        .map(|spec| {
            build_backend_over(
                spec,
                run_program,
                run_resolved,
                &loaded.front,
                super::common::module_texts(run_program, &loaded.sources),
            )
        }) {
        None => held_unit,
        Some(Ok(provider)) => {
            if let Some(spec) = backend.as_ref() {
                warm.keep_unit(spec, &hashes, provider);
            }
            Some(provider)
        }
        Some(Err(diagnostic)) => {
            if args.json {
                emit_json(&json!({
                    "command": "test",
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "diagnostics": [diagnostic_json(&diagnostic, &loaded.sources)],
                }));
            } else {
                print_diagnostics(std::slice::from_ref(&diagnostic), &loaded.sources, style);
            }
            return EXIT_COMPILE_ERROR;
        }
    };
    let mut run = || {
        let mut executor = ply_test::InterpExecutor::new(run_program, run_resolved, &loaded.check)
            .with_search(simulation.clone())
            .with_hosts(hosting(&hosts, &runtime));
        if let (Some(provider), Some(spec)) = (provider, backend.clone()) {
            executor = executor.with_backend(provider, spec);
        }
        ply_test::run_with(
            &plan.selection,
            &loaded.check,
            &hashes,
            &mut cache.store,
            &executor,
        )
    };
    let mut report = match &pool {
        Some(pool) => pool.install(run),
        None => run(),
    };
    warnings.extend(report.warnings.iter().cloned());

    // After the run, since a pass recorded now is a valid baseline for another test's failure;
    // over the program that ran, since a run-only module's tests are absent from the checked one.
    warnings.extend(ply_test::diagnose_failures(
        &mut report,
        run_program,
        run_resolved,
        &loaded.front,
        &mut cache.store,
        &diagnosis_options(args),
    ));
    // Pass records are read lazily, so an unreadable baseline only surfaces here.
    warnings.extend(cache.store.take_warnings());
    let warnings = once_each(warnings);

    let view = HostView::of(&hosts, &plan, &loaded.check, &report);
    let backend_view = BackendView::of(backend.as_ref(), provider, &report, engine);
    let ok = report.is_success() && view.escapes.is_empty() && backend_view.escapes.is_empty();

    if args.json {
        emit_json(&report_json(
            &loaded,
            &hashes,
            &plan,
            &report,
            args,
            workers,
            &warnings,
            &view,
            &backend_view,
            ok,
        ));
    } else {
        print_human(
            &loaded,
            &hashes,
            &plan,
            &report,
            args,
            workers,
            &warnings,
            &view,
            &backend_view,
            style,
        );
    }
    // Only now: an iteration that returned early leaves no state behind.
    warm.keep(loaded);
    exit_code(ok)
}

pub struct BackendView {
    spec: Option<String>,
    /// Which backend answered, as `--backend` names it.
    name: &'static str,
    /// Definitions the backend had a body for.
    fragment: usize,
    compiled: Option<ply_eval::Compilation>,
    /// Summed over every worker.
    offers: ply_eval::Offers,
    /// Bodies entered natively and calls declined, summed over the tests.
    entries: u64,
    declines: u64,
    /// Tests that entered native code and whose passes were written to the result cache anyway.
    escapes: Vec<Diagnostic>,
}

impl BackendView {
    pub fn of(
        spec: Option<&ply_eval::BackendSpec>,
        provider: Option<&'static dyn ply_eval::Provider>,
        report: &RunReport,
        selected_under: &ply_test::Engine,
    ) -> BackendView {
        let Some(spec) = spec else {
            return BackendView {
                spec: None,
                name: "",
                fragment: 0,
                compiled: None,
                offers: ply_eval::Offers::default(),
                entries: 0,
                declines: 0,
                escapes: Vec::new(),
            };
        };
        let entries = report
            .results
            .iter()
            .filter_map(|r| r.backend)
            .map(|b| b.entries)
            .sum();
        let declines = report
            .results
            .iter()
            .filter_map(|r| r.backend)
            .map(|b| b.declines)
            .sum();
        let mut escapes = backend_escapes(report, selected_under);
        // An unbuilt backend declines every call, which would make a green run vacuous.
        let unbuilt = provider.map_or(0, ply_eval::Provider::unbuilt);
        if unbuilt > 0 {
            escapes.push(
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!(
                        "{unbuilt} worker(s) could not build the `{}` backend, and every call \
                         they were offered was declined",
                        provider.map_or("", ply_eval::Provider::name)
                    ),
                )
                .note(
                    "the backend was built once before the run started, so this cannot be a host \
                     that has no code generator",
                )
                .note(
                    "this is Ply's fault — a run that installs a backend and silently does not \
                     have one is green over a seam nothing reached",
                ),
            );
        }
        BackendView {
            spec: Some(spec.describe()),
            name: provider.map_or(spec.kind.as_str(), ply_eval::Provider::name),
            fragment: provider.map_or(0, ply_eval::Provider::len),
            compiled: provider.and_then(ply_eval::Provider::compilation),
            offers: provider.map_or_else(Default::default, ply_eval::Provider::offers),
            entries,
            declines,
            escapes,
        }
    }

    fn installed(&self) -> bool {
        self.spec.is_some()
    }
}

/// Tests that entered native code and whose passes were cached anyway.
pub fn backend_escapes(report: &RunReport, selected_under: &ply_test::Engine) -> Vec<Diagnostic> {
    if &report.engine == selected_under {
        return Vec::new();
    }
    // A run that selected nothing builds no backend and names the evaluator; it wrote nothing,
    // so it cannot have broken the invariant.
    if !report
        .results
        .iter()
        .any(|r| r.recorded.as_ref().is_some_and(Record::is_written))
    {
        return Vec::new();
    }
    vec![
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!(
                "this run selected against `{}` and recorded under `{}`",
                selected_under.label(),
                report.engine.label()
            ),
        )
        .note("a `Pass` is a claim about the engine that earned it, so the two must name the same one")
        .note(
            "the command names the engine before it builds a provider, because selection decides              whether building one is worth anything; the run names it from the provider it built",
        )
        .note("run `ply cache clear`: this run skipped what one engine proved and recorded it as another's")
        .note("this is Ply's fault — `common::engine_of` and `Executor::engine` disagree")
    ]
}

/// Shared by the human and `--json` projections so they cannot disagree.
pub struct HostView<'a> {
    hosts: &'a Hosts,
    pub counts: hosts::Counts,
    /// Tests the binding can reach whose passes were written to the result cache.
    pub escapes: Vec<Diagnostic>,
}

impl<'a> HostView<'a> {
    pub fn of(
        hosts: &'a Hosts,
        plan: &Plan,
        check: &CheckOutput,
        report: &RunReport,
    ) -> HostView<'a> {
        HostView {
            hosts,
            counts: counts(plan, check, hosts),
            escapes: cache_escapes(report, check, hosts),
        }
    }

    fn reaches(&self, check: &CheckOutput, index: usize) -> bool {
        check
            .tests
            .get(index)
            .is_some_and(|t| self.hosts.reaches(&t.footprint))
    }
}

/// How the corpus splits once the binding is taken into account.
fn counts(plan: &Plan, check: &CheckOutput, hosts: &Hosts) -> hosts::Counts {
    let parallelism = &plan.selection.parallelism;
    if hosts.is_hermetic() {
        return hosts::Counts {
            total: parallelism.total,
            isolated: parallelism.isolated,
            shared: parallelism.shared,
            host: 0,
        };
    }
    hosts::Counts::of(
        hosts,
        plan.visible
            .iter()
            .filter_map(|&index| Some((index, check.tests.get(index)?)))
            .map(|(index, test)| {
                let isolated = plan
                    .selection
                    .isolation_of(index)
                    .unwrap_or_else(|| Isolation::of(&test.footprint))
                    .is_isolated();
                (&test.footprint, isolated)
            }),
    )
}

/// A test the binding can reach always runs and is never written to the cache, in either direction.
fn cache_escapes(report: &RunReport, check: &CheckOutput, hosts: &Hosts) -> Vec<Diagnostic> {
    if hosts.is_hermetic() {
        return Vec::new();
    }
    report
        .results
        .iter()
        .filter(|r| {
            r.recorded.as_ref().is_some_and(Record::is_written)
                && check
                    .tests
                    .get(r.index)
                    .is_some_and(|t| hosts.reaches(&t.footprint))
        })
        .map(|r| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!(
                    "`{}` can reach the host binding, and its pass was written to the result cache",
                    r.name
                ),
            )
            .note("a run that reached the host proves nothing about the next one, so it is never cached")
            .note("run `ply cache clear`: an entry written here would be believed by a later hermetic run")
            .note("this is Ply's fault — the runner and the binding disagree about what this test can do")
        })
        .collect()
}

/// Only a deliberately corrupt backend bypasses the store: its green run must be evidence.
fn cache_bypassed(args: &TestArgs) -> bool {
    args.no_cache || backend_is_corrupt(args)
}

fn backend_is_corrupt(args: &TestArgs) -> bool {
    args.backend
        .as_deref()
        .and_then(|flag| ply_eval::backend::parse(flag).ok())
        .is_some_and(|spec| {
            spec.mutation != ply_eval::backend::Mutation::None || spec.target.is_some()
        })
}

/// `--bisect never` still goes through the diagnosis, so the artifact has one shape.
pub fn diagnosis_options(args: &TestArgs) -> ply_test::Options {
    ply_test::Options {
        bisect: match args.bisect {
            When::Auto => ply_test::Mode::Auto,
            When::Always => ply_test::Mode::Always,
            When::Never => ply_test::Mode::Never,
        },
        trace: match args.trace {
            When::Auto => ply_test::Tracing::Auto,
            When::Always => ply_test::Tracing::Always,
            When::Never => ply_test::Tracing::Never,
        },
        budget: ply_test::Budget::new(args.bisect_budget),
    }
}

/// Counts under `--filter` use the filtered set as their denominator.
pub struct Plan {
    pub selection: Selection,
    /// Test indices still in scope, ascending.
    pub visible: Vec<usize>,
    pub filtered_out: usize,
}

impl Plan {
    /// `std_tests` is `--std`.
    pub fn new(
        selection: Selection,
        check: &CheckOutput,
        filter: Option<&str>,
        std_tests: bool,
    ) -> Plan {
        let in_scope = |t: &ply_ty::TestInfo| std_tests || !ply_std::is_std(&t.module);
        // Against `<module>.<label>`, so `--filter store.` narrows to a module.
        let matches = |t: &ply_ty::TestInfo| filter.is_none_or(|n| t.key.as_str().contains(n));

        let scoped = check.tests.iter().filter(|t| in_scope(t)).count();
        let out_of_scope: BTreeSet<usize> = check
            .tests
            .iter()
            .enumerate()
            .filter(|(_, t)| !in_scope(t))
            .map(|(i, _)| i)
            .collect();
        let visible: Vec<usize> = check
            .tests
            .iter()
            .enumerate()
            .filter(|(_, t)| in_scope(t) && matches(t))
            .map(|(i, _)| i)
            .collect();
        if visible.len() == check.tests.len() {
            return Plan {
                selection,
                visible,
                filtered_out: 0,
            };
        }

        let keeps = |i: &usize| visible.binary_search(i).is_ok();

        let cached: Vec<_> = selection
            .cached
            .into_iter()
            .filter(|(i, _)| keeps(i))
            .collect();
        let to_run: Vec<usize> = selection.to_run.into_iter().filter(keeps).collect();
        let footprints: Vec<(usize, Footprint)> = to_run
            .iter()
            .map(|&i| (i, check.tests[i].footprint.clone()))
            .collect();
        let groups = ply_test::group_by_conflict(&footprints);
        // Over the visible tests, so every count shares one denominator.
        let parallelism = ply_test::parallelism(
            visible
                .iter()
                .filter_map(|&i| check.tests.get(i))
                .map(|t| &t.footprint),
            &footprints,
            &groups,
        );

        Plan {
            filtered_out: scoped - visible.len(),
            selection: Selection {
                total: visible.len(),
                cached,
                to_run,
                groups,
                // Indexed by test index, so they stay whole under a narrowed plan.
                reasons: selection.reasons,
                isolation: selection.isolation,
                parallelism,
                // A filter must not change the search, which is part of the cache key.
                plan: selection.plan,
                narrowed: selection.narrowed,
                out_of_scope,
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

/// `--no-cache` points the store at a scratch directory deleted on the way out.
pub struct Cache {
    pub store: Store,
    pub scratch: Option<PathBuf>,
    /// An unusable cache never stops a run, but is always reported.
    pub warnings: Vec<Diagnostic>,
}

impl Cache {
    pub fn open(root: &Path, bypass: bool) -> Result<Cache, Diagnostic> {
        if bypass {
            return Cache::scratch();
        }
        match Store::open(root) {
            Ok(store) => Ok(Cache {
                store,
                scratch: None,
                warnings: Vec::new(),
            }),
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
            Ok(store) => Ok(Cache {
                store,
                scratch: Some(dir),
                warnings: Vec::new(),
            }),
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

fn scratch_failed(dir: &Path, cause: &str) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!(
            "could not create a scratch cache at `{}`: {cause}",
            dir.display()
        ),
    )
    .primary(Span::DUMMY, "the run needs somewhere to record results")
    .note("set TMPDIR to a writable directory, or drop `--no-cache`")
}

#[allow(clippy::too_many_arguments)]
fn print_human(
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: &Plan,
    report: &RunReport,
    args: &TestArgs,
    workers: usize,
    warnings: &[Diagnostic],
    view: &HostView<'_>,
    backend: &BackendView,
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
    let counts = &view.counts;
    if counts.total > 0 {
        let shared = if counts.shared == 0 {
            String::new()
        } else {
            style.dim(&format!(
                " · {} {} can contend",
                counts.shared,
                plural(counts.shared, "test")
            ))
        };
        println!(
            "{IND}{} {} of {}{shared}",
            style.bold("isolated"),
            style.bold(&counts.isolated.to_string()),
            counts.total,
        );
    }
    // A socket lives outside every region, so a host-backed test is never isolated or cached.
    if !view.hosts.is_hermetic() {
        println!(
            "{IND}{} {} of {} · {}",
            style.bold("host"),
            style.bold(&counts.host.to_string()),
            counts.total,
            style.dim("not cached"),
        );
        let listing = view.hosts.listing();
        let disclosures = view.hosts.disclosures();
        println!(
            "{IND}{}",
            style.dim(&format!(
                "binding host · {} {} · {}",
                listing.rows.len(),
                plural(listing.rows.len(), "operation"),
                crate::hosts::digest_short(listing, &disclosures),
            ))
        );
        for line in crate::hosts::handshake_lines(&view.hosts.handshakes()) {
            println!("{IND}{}", style.dim(&line));
        }
        if let Some(line) = crate::hosts::database_line(view.hosts) {
            println!("{IND}{}", style.dim(&line));
        }
    }
    if let Some(line) = report.simulation.line() {
        println!("{IND}{}", style.bold(&line));
    }
    if let Some(corruption) = &backend.spec {
        let offers = backend.offers;
        println!(
            "{IND}{} {} · {} of {} offers entered · {} declined · {} in the fragment",
            style.bold("backend"),
            style.bold(backend.name),
            backend.entries,
            offers.offered,
            backend.declines,
            backend.fragment,
        );
        // Apart from the entry counts: analysis is paid once, code generation per worker.
        if let Some(c) = backend.compiled {
            println!(
                "{IND}{}",
                style.dim(&format!(
                    "compiled {} unit(s) in {:.1}ms, after {:.1}ms deciding what to compile",
                    c.units,
                    c.codegen_nanos as f64 / 1e6,
                    c.analysis_nanos as f64 / 1e6,
                ))
            );
        }
        if corruption != "nothing" {
            println!(
                "{IND}{}",
                style.dim(&format!(
                    "wrong on purpose: {corruption} · {} {} changed · {} {} of the target",
                    offers.fired,
                    plural(offers.fired as usize, "answer"),
                    offers.offered_target,
                    plural(offers.offered_target as usize, "offer"),
                ))
            );
        }
    }
    if cache_bypassed(args) {
        let why = if args.no_cache {
            "--no-cache"
        } else {
            "--backend"
        };
        println!(
            "{IND}{}",
            style.dim(&format!("{why}: results were neither read nor recorded"))
        );
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
        print_explain(loaded, hashes, plan, view, style);
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
            if let Some(line) = simulation_line(result) {
                println!("{IND}    {}", style.dim(&line));
            }
        }
    }

    println!();
    print_summary(
        report,
        view.counts.host,
        view.hosts.is_live_database(),
        style,
    );

    if !backend.escapes.is_empty() {
        println!();
        print_warnings(&backend.escapes, style);
    }
    if !view.escapes.is_empty() {
        println!();
        print_warnings(&view.escapes, style);
    }

    for failure in &report.failures {
        println!();
        print_failure(failure, loaded, style);
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

/// The culprit before the diff: the culprit is the answer.
fn print_failure(failure: &Failure, loaded: &Loaded, style: Style) {
    for line in failure_lines(failure, loaded, style) {
        println!("{IND}{line}");
    }
}

pub fn failure_lines(failure: &Failure, loaded: &Loaded, style: Style) -> Vec<String> {
    let mut lines = vec![style.bold(failure.key.as_str())];

    let bisection = &failure.attribution.bisection;
    if bisection.is_conclusive() {
        for (i, group) in bisection.groups.iter().enumerate() {
            let names: Vec<&str> = group.iter().map(|n| n.as_str()).collect();
            let label = if i == 0 { "culprit:" } else { "        " };
            let at = group
                .iter()
                .find_map(|n| loaded.check.defs.get(n))
                .and_then(|def| location(&loaded.sources, def.span))
                .map(|at| format!("   {}", style.dim(&at)))
                .unwrap_or_default();
            lines.push(format!("  {} {}{at}", style.red(label), names.join(" + ")));
        }
        lines.push(format!("    {}", style.dim(&bisection.reason)));
    } else if let Some(why) = no_culprit_reason(bisection) {
        lines.push(format!("  {} {why}", style.dim("no culprit:")));
    }

    lines.push(format!("  {}", failure.diagnostic.message));
    if let Some(at) = failure
        .diagnostic
        .primary_span()
        .and_then(|s| location(&loaded.sources, s))
    {
        lines.push(format!("    at {}", style.dim(&at)));
    }
    // A deadlock's cycle lives only in its secondary labels.
    for label in failure.diagnostic.labels.iter().filter(|l| !l.primary) {
        let at = location(&loaded.sources, label.span)
            .map(|at| format!("   {}", style.dim(&at)))
            .unwrap_or_default();
        lines.push(format!("    {}{at}", label.message));
    }
    for note in &failure.diagnostic.notes {
        lines.push(format!("  {} {note}", style.dim("=")));
    }
    lines.extend(seed_lines(failure, loaded, style));

    if let Some(slice) = &failure.attribution.slice
        && slice.traced
        && !slice.stack.is_empty()
    {
        let path: Vec<&str> = slice.path().iter().map(|n| n.as_str()).collect();
        lines.push(format!("  {} {}", style.dim("ran:"), path.join(" → ")));
        if !slice.reproduced {
            lines.push(format!(
                "    {}",
                style.yellow(
                    "the replay did not reproduce this failure; treat the path as evidence \
                     about a different execution"
                )
            ));
        }
    }

    let rest: Vec<String> = failure
        .attribution
        .suspects
        .iter()
        .filter(|s| !s.culprit)
        .map(describe_suspect)
        .collect();
    if !rest.is_empty() {
        lines.push(format!(
            "  {} {}",
            style.yellow("suspects:"),
            rest.join(", ")
        ));
    } else if failure.suspects.is_empty() {
        lines.push(format!(
            "  {}",
            style.dim("suspects: none — nothing in this test's closure changed")
        ));
    }
    lines
}

/// The repro, which is a seed rather than a stack trace.
fn seed_lines(failure: &Failure, loaded: &Loaded, style: Style) -> Vec<String> {
    let Some(seed) = &failure.seed else {
        return Vec::new();
    };
    let mut lines = vec![format!("  {} {seed}", style.dim("seed:"))];
    if let Some(race) = &failure.race {
        for (i, site) in [&race.left, &race.right].into_iter().enumerate() {
            let label = if i == 0 { "race:" } else { "     " };
            let definition = site
                .definition
                .as_ref()
                .map_or_else(|| "-".to_string(), |d| d.to_string());
            let at = location(&loaded.sources, site.span)
                .map(|at| format!("   {}", style.dim(&at)))
                .unwrap_or_default();
            lines.push(format!(
                "  {} {}  {definition}   {}{at}",
                style.yellow(label),
                site.task,
                site.access
            ));
        }
    }
    if let Some(command) = failure.replay() {
        lines.push(format!("  {} {command}", style.dim("replay:")));
    }
    lines
}

/// Silent when no bisection was asked for.
fn no_culprit_reason(bisection: &Bisection) -> Option<&str> {
    match bisection.verdict {
        Verdict::NotAttempted(Skipped::NotRequested) => None,
        _ => Some(bisection.reason.as_str()),
    }
}

pub fn describe_suspect(suspect: &Suspect) -> String {
    let mut notes: Vec<&str> = Vec::new();
    if let Some(change) = suspect.change {
        notes.push(change.as_str());
    }
    match suspect.ran {
        Some(false) => notes.push("did not run"),
        Some(true) if suspect.depth.is_none() => notes.push("ran, then returned"),
        _ => {}
    }
    if notes.is_empty() {
        suspect.name.to_string()
    } else {
        format!("{} ({})", suspect.name, notes.join(", "))
    }
}

pub fn no_tests_note(loaded: &Loaded, args: &TestArgs) -> &'static str {
    if loaded.check.tests.is_empty() {
        "no `test` items in this program"
    } else if args.filter.is_some() {
        "no test key contains that substring; nothing was verified"
    } else {
        "nothing to report"
    }
}

/// The label in a single-module run, `<module>.<label>` otherwise.
pub fn display_name(check: &CheckOutput, index: usize, fallback: &str) -> String {
    match check.tests.get(index) {
        Some(test) if check.modules.len() > 1 => test.key.to_string(),
        Some(test) => test.name.clone(),
        None => fallback.to_string(),
    }
}

fn name_column(names: &[String]) -> usize {
    names
        .iter()
        .map(|n| n.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(24, 64)
}

pub fn result_line(result: &TestResult, name: &str, name_width: usize, style: Style) -> String {
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
    // `mark` may carry escapes, and `{:<width$}` counts bytes.
    let pad = " ".repeat(width.saturating_sub(display_width(&mark)));
    format!(
        "{mark}{pad} {name:<name_width$} {:>8.1}ms",
        millis(result.duration)
    )
}

/// What one test's search did, under its result line.
pub fn simulation_line(result: &TestResult) -> Option<String> {
    let exploration = result.simulation.as_ref()?;
    let mut parts = vec![format!(
        "{} {}",
        exploration.explored,
        plural(exploration.explored as usize, "interleaving")
    )];
    if exploration.exhaustive {
        parts.push("exhaustive".to_string());
    }
    if exploration.exhausted {
        parts.push("budget spent — not cached".to_string());
    }
    if let Some(naive) = exploration.naive {
        parts.push(format!("naive {naive}"));
        if let Some(reduction) = exploration.reduction() {
            // A bounded naive count makes the ratio a lower bound.
            let bound = if naive.bounded { ">= " } else { "" };
            parts.push(format!("{bound}{reduction:.0}× reduction"));
        }
    }
    Some(parts.join(" · "))
}

/// `host` sits beside `cached` so "0 cached" is not misread as selection working.
fn print_summary(report: &RunReport, host: usize, database: bool, style: Style) {
    let failed = format!("{} failed", report.failed);
    let failed = if report.failed > 0 {
        style.red(&failed)
    } else {
        style.dim(&failed)
    };
    let passed = format!("{} passed", report.passed);
    let passed = if report.passed > 0 {
        style.green(&passed)
    } else {
        style.dim(&passed)
    };
    let hosted = if host == 0 {
        String::new()
    } else {
        let against = if database {
            " against a real database"
        } else {
            ""
        };
        style.dim(&format!(", {host} host-backed{against} and not cached"))
    };
    println!(
        "{IND}{failed}, {passed}, {} cached{hosted} ({:.2}s)",
        report.cached,
        report.duration.as_secs_f64()
    );
}

fn print_explain(
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: &Plan,
    view: &HostView<'_>,
    style: Style,
) {
    let check = &loaded.check;
    println!();
    for &index in &plan.visible {
        let Some(test) = check.tests.get(index) else {
            continue;
        };
        let reason = plan.selection.reason(index).unwrap_or(Reason::Unhashed);
        let verb = if reason.runs() {
            style.bold("run ")
        } else {
            style.dim("skip")
        };
        let hash = hashes
            .tests
            .get(index)
            .map(|h| h.short())
            .unwrap_or_else(|| "-".repeat(12));
        let isolation = isolation_label(plan, view, check, index, &test.footprint);
        let shared = ply_test::shared_footprint(&test.footprint);
        let atoms = if isolation == Isolation::Region.as_str() {
            String::new()
        } else if ply_test::contends_only_over_regions(&test.footprint) {
            // Contention a rename would remove, as opposed to one that needs a database.
            format!(" {shared} (region labels)")
        } else {
            format!(" {shared}")
        };
        let engine = if ply_test::is_seeded(&test.footprint) {
            " · searched"
        } else {
            ""
        };
        println!(
            "{IND}{verb} {} {:<16} {:<40} {}",
            style.dim(&hash),
            reason.as_str(),
            display_name(check, index, &test.name),
            style.dim(&format!("isolation: {isolation}{atoms}{engine}"))
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

    print_explain_search(plan, check, style);

    if plan.selection.groups.is_empty() {
        return;
    }
    println!();
    println!("{IND}{}", style.dim("concurrency groups"));
    let parallelism = &plan.selection.parallelism;
    let counts = &view.counts;
    // Grouped like any other test, but a host-backed test is never free.
    let hosted = if counts.host == 0 {
        String::new()
    } else {
        format!(" · {} host-backed and never free", counts.host)
    };
    let regioned = if parallelism.region_contended == 0 {
        String::new()
    } else {
        format!(
            " · {} of them only over a region label",
            parallelism.region_contended
        )
    };
    println!(
        "{IND}  {}",
        style.dim(&format!(
            "{} of {} region-isolated and free · {} {} for the {} shared {}{regioned}{hosted}",
            counts.isolated,
            counts.total,
            parallelism.shared_groups,
            plural(parallelism.shared_groups, "group"),
            counts.shared,
            plural(counts.shared, "test"),
        ))
    );
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

/// What the seeded tests will search, and what each of them still owes.
fn print_explain_search(plan: &Plan, check: &CheckOutput, style: Style) {
    let seeded: Vec<usize> = plan
        .visible
        .iter()
        .copied()
        .filter(|&i| {
            check
                .tests
                .get(i)
                .is_some_and(|t| ply_test::is_seeded(&t.footprint))
        })
        .collect();
    if seeded.is_empty() {
        return;
    }
    let search = &plan.selection.plan;
    println!();
    println!("{IND}{}", style.dim("search"));
    println!(
        "{IND}  {}",
        style.dim(&format!(
            "{} · {} {} · budget {} · steps {}",
            search.mode.as_str(),
            search.roots.len(),
            plural(search.roots.len(), "seed"),
            search.budget,
            search.steps,
        ))
    );
    println!(
        "{IND}  {}",
        style.dim(&format!(
            "{} of {} {} keyed on this plan and never on their bare hash",
            seeded.len(),
            plan.visible.len(),
            plural(plan.visible.len(), "test"),
        ))
    );
    for &index in &seeded {
        let owed = plan.selection.plan_for(index);
        if owed.roots.len() == search.roots.len() {
            continue;
        }
        if let Some(test) = check.tests.get(index) {
            println!(
                "{IND}  {}",
                style.dim(&format!(
                    "{}: {} of {} {} still owed; the rest already passed on their own",
                    display_name(check, index, &test.name),
                    owed.roots.len(),
                    search.roots.len(),
                    plural(search.roots.len(), "seed"),
                ))
            );
        }
    }
}

/// `region`, `shared` or `host`.
pub fn isolation_label(
    plan: &Plan,
    view: &HostView<'_>,
    check: &CheckOutput,
    index: usize,
    footprint: &Footprint,
) -> &'static str {
    if view.reaches(check, index) {
        return "host";
    }
    plan.selection
        .isolation_of(index)
        .unwrap_or_else(|| Isolation::of(footprint))
        .as_str()
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

#[allow(clippy::too_many_arguments)]
pub fn report_json(
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: &Plan,
    report: &RunReport,
    args: &TestArgs,
    workers: usize,
    warnings: &[Diagnostic],
    view: &HostView<'_>,
    backend: &BackendView,
    ok: bool,
) -> Value {
    let sources = &loaded.sources;
    let check = &loaded.check;
    let selection = &plan.selection;
    let counts = &view.counts;

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
                "isolation": isolation_label(plan, view, check, index, &test.footprint),
                // Whether this test reaches the binding, and so always runs uncached.
                "host": view.reaches(check, index),
                "shared_atoms": ply_test::shared_footprint(&test.footprint)
                    .atoms()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>(),
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
                // Absent, never zeroed, when no region was reached.
                "simulation": r.simulation.as_ref().map(ply_test::report::exploration_json),
                "cached": r.recorded.as_ref().map(|record| record.is_written()),
            })
        })
        .collect();

    let failures: Vec<Value> = report
        .failures
        .iter()
        .map(|f| failure_json(f, loaded, hashes, report))
        .collect();

    json!({
        "command": "test",
        "schema_version": ply_test::report::SCHEMA_VERSION,
        "front_end": json!({
            "incremental": loaded.frontend.incremental,
            "phases": phases_json(&loaded.frontend.phases),
        }),
        "ok": ok,
        "exit_code": exit_code(ok),
        "root": loaded.root.display().to_string(),
        "files": loaded.file_names(),
        "modules": loaded.modules().iter().map(|m| json!({
            "name": m.name.as_str(),
            "file": m.path.display().to_string(),
        })).collect::<Vec<_>>(),
        "filter": args.filter,
        "no_cache": cache_bypassed(args),
        "binding": view.hosts.label(),
        "hosts": view.hosts.summary_json(),
        "workers": workers,
        "options": {
            "bisect": args.bisect.as_str(),
            "bisect_budget": args.bisect_budget,
            "trace": args.trace.as_str(),
            // The whole plan: every field is in a seeded test's cache key.
            "sim": {
                "mode": selection.plan.mode.as_str(),
                "seed": args.simulation.seed.as_ref().map(|s| s.to_string()),
                "seeds": selection.plan.roots.len(),
                "budget": selection.plan.budget,
                "steps": selection.plan.steps,
                "measure_reduction": args.simulation.measure_reduction,
            },
        },
        "simulation": {
            "simulated": report.simulation.simulated,
            "seeds": report.simulation.seeds,
            "interleavings": report.simulation.interleavings,
            "exhaustive": report.simulation.exhaustive,
            "exhausted": report.simulation.exhausted,
            "failed": report.simulation.failed,
        },
        "backend": backend.installed().then(|| json!({
            "spec": args.backend,
            "name": backend.name,
            "corruption": backend.spec,
            "fragment": backend.fragment,
            "offered": backend.offers.offered,
            "offered_target": backend.offers.offered_target,
            "fired": backend.offers.fired,
            "analysis_nanos": backend.compiled.map(|c| c.analysis_nanos),
            "codegen_nanos": backend.compiled.map(|c| c.codegen_nanos),
            "units": backend.compiled.map(|c| c.units),
            "entered": backend.entries,
            "declined": backend.declines,
            "converted_in": backend.offers.converted_in,
            "converted_out": backend.offers.converted_out,
        })),
        "selection": {
            "total": selection.total,
            "selected": selection.to_run.len(),
            "cached": selection.cached.len(),
            "filtered_out": plan.filtered_out,
            "groups": groups,
            // A host-backed test counts only under `host`, since it is not isolated.
            "isolated": counts.isolated,
            "shared": counts.shared,
            "host": counts.host,
            "parallelism": selection.parallelism,
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
        // Not a warning: every later run believes an escaped entry.
        "diagnostics": diagnostics_json(
            &view.escapes.iter().chain(&backend.escapes).cloned().collect::<Vec<_>>(),
            sources,
        ),
        "warnings": warnings.iter().map(|w| diagnostic_json(w, sources)).collect::<Vec<_>>(),
    })
}

pub fn failure_json(
    failure: &Failure,
    loaded: &Loaded,
    hashes: &HashOutput,
    report: &RunReport,
) -> Value {
    let sources = &loaded.sources;
    let check = &loaded.check;
    let index = check.tests.iter().position(|t| t.key == failure.key);
    let test = index.and_then(|i| check.tests.get(i));

    let mut value = ply_test::report::failure_json(failure);
    let Some(object) = value.as_object_mut() else {
        return value;
    };

    object.insert(
        "diagnostic".into(),
        diagnostic_json(&failure.diagnostic, sources),
    );
    object.insert(
        "module".into(),
        json!(test.map(|t| t.module.as_str().to_string())),
    );
    object.insert(
        "test_hash".into(),
        json!(index.and_then(|i| hashes.tests.get(i)).map(|h| h.to_hex())),
    );
    object.insert("nondet".into(), json!(test.map(|t| t.nondet)));
    object.insert(
        "status".into(),
        json!(index.and_then(|i| status_of(report, i)).map(status_str)),
    );
    object.insert(
        "location".into(),
        failure
            .diagnostic
            .primary_span()
            .filter(|s| *s != Span::DUMMY)
            .map_or(Value::Null, |s| location_json(sources, s)),
    );
    object.insert(
        "footprint".into(),
        json!({
            "declared": test.map(|t| atoms(&t.footprint)),
            // Null rather than empty when untraced: unwatched differs from performing nothing.
            "observed": failure
                .attribution
                .slice
                .as_ref()
                .filter(|s| s.traced)
                .map(|s| atoms(&s.observed)),
        }),
    );
    value
}

fn atoms(footprint: &Footprint) -> Vec<String> {
    footprint.atoms().map(|a| a.to_string()).collect()
}

fn status_of(report: &RunReport, index: usize) -> Option<Status> {
    report
        .results
        .iter()
        .find(|r| r.index == index)
        .map(|r| r.status)
}

fn status_str(status: Status) -> &'static str {
    match status {
        Status::Passed => "passed",
        Status::Failed => "failed",
        Status::Panicked => "panicked",
    }
}

/// Line and column rather than byte offsets, for editors.
fn location_json(sources: &SourceMap, span: Span) -> Value {
    let Some(file) = sources.get(span.source) else {
        return Value::Null;
    };
    let (line, column) = file.line_col(span.start);
    let (end_line, end_column) = file.line_col(span.end);
    json!({
        "file": file.path.display().to_string(),
        "line": line,
        "column": column,
        "end_line": end_line,
        "end_column": end_column,
    })
}

fn display_width(s: &str) -> usize {
    crate::style::strip_ansi(s).chars().count()
}

/// A `reuse fn` whose promise the cost checker cannot show stops the run, as under `ply check`.
pub(crate) fn broken_promises(loaded: &Loaded) -> Option<crate::load::LoadError> {
    if !loaded.promised {
        return None;
    }
    let diagnostics = crate::costs::promises(&loaded.program, &loaded.resolved);
    (!diagnostics.is_empty()).then(|| crate::load::LoadError {
        sources: loaded.sources.clone(),
        diagnostics,
    })
}

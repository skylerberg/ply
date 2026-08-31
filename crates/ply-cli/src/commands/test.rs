use super::common::{
    IND, build_pool, describe_schema, diagnostic_json, diagnostics_json, emit_json, exit_code,
    location, millis, once_each, phases_json, plural, print_diagnostics, print_phases,
    print_warnings, report_bind_error, report_load_error,
};
use crate::EXIT_COMPILE_ERROR;
use crate::cli::{TestArgs, When};
use crate::driver;
use crate::hosts::{self, Hosts, hosting};
use crate::load::{Loaded, load, project_root};
use crate::style::Style;
use ply_core::{CheckOutput, Footprint};
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceMap, Span, codes};
use ply_store::Store;
use ply_test::{
    Bisection, Failure, Isolation, Reason, Record, RunReport, Selection, Skipped, Status, Suspect,
    TestResult, Verdict,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub fn execute(args: &TestArgs, style: Style) -> i32 {
    let mut warnings = Vec::new();
    let engine: ply_eval::EngineChoice = args.engine.into();
    // Before the store is opened, because a misspelled `--backend` must not
    // leave a cache directory behind for a run that is about to refuse.
    let backend = match backend_spec(args) {
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
    warnings.append(&mut cache.warnings);
    let opened = cache.store.take_warnings();
    let migration = crate::migrate::notice(&cache.store, &opened);
    warnings.extend(opened);
    warnings.extend(migration);

    let incremental = !args.no_incremental && !no_cache;
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
    // Half of what a simulated test is cached under, so it is decided before
    // selection and never after it.
    let search = crate::simulation::plan(&args.simulation);
    let selected = ply_test::select(&loaded.check, &hashes, &cache.store, &search);
    let mut plan = Plan::new(selected, &loaded.check, args.filter.as_deref(), args.std);

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
    for test in plan
        .selection
        .to_run
        .iter()
        .filter_map(|&i| loaded.check.tests.get(i))
    {
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
                let selected = ply_test::select(&loaded.check, &hashes, &cache.store, &search);
                plan = Plan::new(selected, &loaded.check, args.filter.as_deref(), args.std);
            }
            Err(err) => return report_load_error("test", &err, args.json, style),
        }
    }

    // Before anything runs: a registration the program does not declare is the
    // host author's bug, and a run that started anyway would touch a resource
    // nobody could name. Hermetic resolves nothing, so a stale registry cannot
    // stop a run that was never going to reach it.
    let db = match args.db.resolve(args.host) {
        Ok(db) => db,
        Err(diagnostics) => {
            return report_bind_error("test", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    // Every row this run can enter, which is the union of the tests'. A suite
    // whose tests all install the twin discharges every `db` atom in Ply and
    // needs no database, and refusing it for want of one would make the twin
    // unusable — which is the milestone's whole point.
    let reach = ply_core::ty::Footprint::from_atoms(
        loaded
            .check
            .tests
            .iter()
            .flat_map(|t| t.footprint.atoms().cloned()),
    );
    // Before the binding, because a required key nothing supplies is the run's
    // configuration and has nothing to do with what the registry resolves — and
    // because a suite that discovers it is misconfigured after its first host
    // test has already run that test against the wrong thing.
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
        db,
        configuration,
        // `ply test` discards, always: a suite asserts on its records through
        // `std.trace`'s twin, and `--trace` on this command already names M5's
        // definition trace.
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
    // A factory rather than a handle: a reactor belongs to the thread its
    // machine runs on, and the runner builds a machine per worker.
    let runtime = hosts.runtime_factory();
    // One per run, not one per worker: a backend may not borrow the program
    // (`Machine`'s `compiled` field says why), so building one costs a copy of
    // the AST and the workers share it. Built here, after the reload above may
    // have replaced `loaded`, so the address `Compiled::describes` compares
    // against is the program the run actually evaluates.
    let provider = match backend.as_ref().map(|spec| build_backend(spec, &loaded)) {
        None => None,
        Some(Ok(provider)) => Some(provider),
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
        let mut executor =
            ply_test::InterpExecutor::new(&loaded.program, &loaded.resolved, &loaded.check)
                .with_engine(engine)
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

    // After the run, and against the store as the run left it: a pass this run
    // recorded is a legitimate baseline for a *different* test's failure.
    warnings.extend(ply_test::diagnose_failures(
        &mut report,
        &loaded.program,
        &loaded.resolved,
        &loaded.check,
        &hashes,
        &mut cache.store,
        &diagnosis_options(args),
    ));
    // Not redundant with the drains above: the pass records are read lazily, on
    // the first question a failure asks, so an unreadable baseline only warns
    // here. Silently, the artifact would say `never_passed` about a test that
    // passed yesterday.
    warnings.extend(cache.store.take_warnings());
    let warnings = once_each(warnings);

    let view = HostView::of(&hosts, &plan, &loaded.check, &report);
    let backend_view = BackendView::of(backend.as_ref(), provider, &report);
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
    exit_code(ok)
}

/// What a compiled backend contributed to this run.
///
/// `None` everywhere when no backend was installed, which is every run that did
/// not ask. Never a zeroed value: "no backend ran" and "a backend ran and
/// entered nothing" are different claims, and the second is the null result ADR
/// 0018 §0.5 records R4 reporting a 0.998x speedup over.
struct BackendView {
    /// What was asked for, rendered.
    spec: Option<String>,
    /// Which backend answered — `reference` or `cranelift`. Read off the
    /// provider that was actually installed rather than off the flag, so a
    /// report cannot name a backend the run did not build.
    name: &'static str,
    /// Definitions the backend had a body for.
    fragment: usize,
    /// What the provider spent compiling, if it compiles anything. `None` for
    /// one that does not — see `ply_eval::Provider::compilation`.
    compiled: Option<ply_eval::Compilation>,
    /// Calls offered, offers naming a targeted definition, and answers a
    /// mutation changed — over every worker.
    offers: ply_eval::Offers,
    /// Bodies entered natively and calls declined, summed over the tests.
    entries: u64,
    declines: u64,
    /// Tests that entered native code and whose passes were written to the
    /// result cache anyway. Empty in a correct run; see [`backend_escapes`].
    escapes: Vec<Diagnostic>,
}

impl BackendView {
    fn of(
        spec: Option<&ply_eval::BackendSpec>,
        provider: Option<&'static dyn ply_eval::Provider>,
        report: &RunReport,
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
        let mut escapes = backend_escapes(report);
        // A worker whose backend failed to build declines every call, so the
        // run would be green over a seam nothing reached. Reported beside the
        // cache escapes because it is the same class of defect — a rule about
        // this run that only the run can see — and it fails the run for the
        // same reason.
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

/// A test that entered native code and whose pass was written to the result
/// cache — the run caching a claim about a third execution strategy.
///
/// ADR 0026 §4.6's **stage two**, and the reason it exists beside the one-line
/// clause in [`cache_bypassed`]: that clause covers a backend that arrives by
/// the flag, and this one covers a backend that arrives by any route, because it
/// reads what the machine *did* rather than what the arguments asked for. It is
/// `cache_escapes` one field over — the same shape, the same `INTERNAL_ERROR`,
/// and the same reason: the failure mode is silent and outlives the run that
/// caused it.
///
/// The rule is the one on `Machine::set_compiled`: *"A run with a backend
/// attached is a third execution strategy, and a cached `Pass` is a claim about
/// the authoritative engine."*
fn backend_escapes(report: &RunReport) -> Vec<Diagnostic> {
    report
        .results
        .iter()
        .filter(|r| {
            r.recorded.as_ref().is_some_and(Record::is_written)
                && r.backend.is_some_and(|b| b.entries > 0)
        })
        .map(|r| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!(
                    "`{}` entered compiled code, and its pass was written to the result cache",
                    r.name
                ),
            )
            .note("a backend is a third execution strategy; a cached `Pass` is a claim about the authoritative engine")
            .note("run `ply cache clear`: an entry written here would be believed by a later run with no backend")
            .note("this is Ply's fault — the runner and the backend disagree about what this run may record")
        })
        .collect()
}

/// What the binding contributed to this run, threaded through both projections
/// so the human summary and `--json` cannot disagree about it.
struct HostView<'a> {
    hosts: &'a Hosts,
    counts: hosts::Counts,
    /// Tests the binding can reach whose passes were written to the result
    /// cache. Empty in a correct run; see [`cache_escapes`].
    escapes: Vec<Diagnostic>,
}

impl<'a> HostView<'a> {
    fn of(hosts: &'a Hosts, plan: &Plan, check: &CheckOutput, report: &RunReport) -> HostView<'a> {
        HostView {
            hosts,
            counts: counts(plan, check, hosts),
            escapes: cache_escapes(report, check, hosts),
        }
    }

    /// Whether this test can reach a bound host handler. A footprint is an upper
    /// bound on what is performed, so this names every test that could and no
    /// test that could not.
    fn reaches(&self, check: &CheckOutput, index: usize) -> bool {
        check
            .tests
            .get(index)
            .is_some_and(|t| self.hosts.reaches(&t.footprint))
    }
}

/// How the corpus splits once the binding is taken into account.
///
/// A hermetic run returns [`Parallelism`]'s own numbers untouched, which is why
/// nothing about a run that binds nothing moves: `reaches` is false for every
/// footprint, so the correction is the identity and the branch only avoids
/// recomputing a denominator the runner already published.
///
/// [`Parallelism`]: ply_test::Parallelism
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

/// A test the binding can reach always runs and is never written to the cache,
/// in either direction. The runner decides that; this checks it.
///
/// The check is here because the failure mode is silent and outlives the run
/// that caused it: a cached pass earned over a real socket is believed by every
/// later hermetic run, and nothing about that run would look wrong. One set
/// lookup per result is a cheap price for turning it into a failure.
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

/// A stored `Pass` is a claim about what the authoritative engine did, so a run
/// on any other engine may neither believe one nor leave one behind. Asking for
/// a non-default engine therefore implies `--no-cache` without saying so.
///
/// **`--backend` is the third strategy and is read here for the same reason**,
/// and it is read *separately* rather than through the engine. `--engine both`
/// already bypasses the cache, so a backend installed on that path would be
/// cache-safe by accident while a backend on the default `--engine machine`
/// path would not — ADR 0026 §4.6 names that interlock as a trap and refuses to
/// let enforcement rest on it. `a_backend_on_the_default_engine_bypasses_the_cache`
/// is what holds this clause: delete it and that test goes red while every
/// `--engine both` test stays green.
///
/// This is the *flag* half of the rule, and it covers only a backend that
/// arrives by this flag. [`backend_escapes`] is the half that survives one
/// arriving by any other route.
fn cache_bypassed(args: &TestArgs) -> bool {
    args.no_cache
        || args.backend.is_some()
        || ply_eval::EngineChoice::from(args.engine).bypasses_cache()
}

/// What `--backend` asked for, or the diagnostic that refuses it.
///
/// Two refusals, and the second is the interesting one. A spec that does not
/// parse is a typo. `--engine treewalk --backend ..` is a request the seam
/// cannot serve at all: the tree-walker has no compiled path, so the flag would
/// be accepted and do nothing — which is `CONTRIBUTING.md` §"The one rule"'s
/// defect shape, a mechanism named where a reader would look for it and
/// constructed nowhere.
fn backend_spec(args: &TestArgs) -> Result<Option<ply_eval::BackendSpec>, Diagnostic> {
    let Some(spec) = &args.backend else {
        return Ok(None);
    };
    if args.engine == crate::cli::EngineArg::Treewalk {
        return Err(Diagnostic::error(
            codes::BACKEND_UNAVAILABLE,
            "`--backend` needs an engine that can enter compiled code",
        )
        .note("the tree-walker has no compiled path, so a backend attached to it would be inert")
        .note("use `--engine machine` (the default) or `--engine both`"));
    }
    ply_eval::backend::parse(spec).map(Some).map_err(|message| {
        Diagnostic::error(codes::BACKEND_UNAVAILABLE, message).note(
            "a wrong backend is a self-test: it exists so that a green run with a backend \
                 attached can be read as evidence",
        )
    })
}

/// The run's backend, built once, or the diagnostic that refuses it.
///
/// One provider per run and not one per worker: a backend may not borrow the
/// program (`Machine`'s `compiled` field says why), so building one costs a copy
/// of the AST and the workers share it.
///
/// **The cranelift arm is fallible and the reference arm is not, and that
/// asymmetry is the point.** A code generator can fail for reasons a
/// tree-walker cannot — a host with no cranelift backend for its architecture,
/// or a fixpoint that cannot close — and the only place those can still be
/// *said* is before the run starts. A backend that failed to build and declined
/// every call would leave a green run over a seam nothing reached, which is
/// `CONTRIBUTING.md` §"The one rule"'s defect shape.
fn build_backend(
    spec: &ply_eval::BackendSpec,
    loaded: &Loaded,
) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
    match spec.kind {
        ply_eval::BackendKind::Reference => Ok(ply_eval::Fragment::over(
            &loaded.program,
            &loaded.resolved,
            &loaded.check,
        )),
        ply_eval::BackendKind::Cranelift => {
            ply_codegen::Cranelift::over(&loaded.program, &loaded.resolved, &loaded.check)
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
                })
        }
    }
}

/// `--bisect never` goes *through* the diagnosis rather than around it, so that
/// the artifact has one shape: a consumer branches on `verdict` and never on
/// whether a field is present.
fn diagnosis_options(args: &TestArgs) -> ply_test::Options {
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
    /// `std_tests` is `--std`. A shipped module's tests are not a project's:
    /// without this rule a project's test count changes with a compiler upgrade,
    /// for tests the project did not write and cannot fix. They are checked by
    /// the compiler's own suite instead.
    pub fn new(
        selection: Selection,
        check: &CheckOutput,
        filter: Option<&str>,
        std_tests: bool,
    ) -> Plan {
        // Two separate questions. Scope decides which tests are this run's at
        // all; the filter narrows within it. Only the second is reported as
        // `filtered out`, because a shipped test was never in the denominator.
        let in_scope = |t: &ply_core::TestInfo| std_tests || !ply_std::is_std(&t.module);
        // Matched against `<module>.<label>` rather than the label alone, so
        // `--filter store.` narrows to a module without a second flag, and a
        // label substring still matches because the key contains the label.
        let matches = |t: &ply_core::TestInfo| filter.is_none_or(|n| t.key.as_str().contains(n));

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
        // Counted over the visible tests, because `selected 2 of 3` and
        // `isolated 1 of 3` have to share a denominator a person can check.
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
                // Indexed by test index, so they stay whole even when the plan
                // is narrowed; nothing reads their length.
                reasons: selection.reasons,
                isolation: selection.isolation,
                parallelism,
                // A filter hides tests; it does not change what the visible ones
                // search, and a search that changed with `--filter` would key
                // the cache on which tests happened to be asked for.
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
        format!(
            "could not create a scratch cache at `{}`: {cause}",
            dir.display()
        ),
    )
    .primary(Span::DUMMY, "the run needs somewhere to record results")
    .note("set TMPDIR to a writable directory, or drop `--no-cache`")
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
    // A socket lives outside every region, so a host-backed test is not isolated
    // and is never cached. Both facts are printed rather than left to be inferred
    // from a smaller `isolated` count than the run had yesterday.
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
    // `--engine both` is the differential oracle, and a green run under it reads
    // as "two engines agreed about every test". They cannot agree about a test
    // only one of them can run — a `resume` clause is E0504 on the tree-walker
    // and a searched test is replayed per interleaving on the machine alone — so
    // the coverage is printed beside the verdict rather than inferred from it.
    if let Some(audit) = &report.audit
        && let Some(line) = audit.line()
    {
        println!("{IND}{}", style.bold(&line));
        if audit.unaudited > 0 {
            println!(
                "{IND}{}",
                style.dim("the other engine refused those; no disagreement was possible")
            );
        }
    }
    // What the backend was asked and what it did with it, printed whether or not
    // anything went wrong. A run that installed a backend and entered nothing is
    // a null result, and the number that says so has to be in front of the
    // reader rather than in `--json`.
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
        // Printed apart from the entry counts because it is what the backend
        // cost rather than what it did, and because the two halves scale
        // differently: the analysis is paid once and the code generation is
        // paid per worker.
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
            "--no-cache".to_string()
        } else if args.backend.is_some() {
            "--backend".to_string()
        } else {
            format!("--engine {}", args.engine.as_str())
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

/// The culprit before the diff, because the culprit is the answer and the diff
/// is the evidence. A reader who already knows which definition broke does not
/// have to work backwards from an expected/actual pair to find out.
fn print_failure(failure: &Failure, loaded: &Loaded, style: Style) {
    for line in failure_lines(failure, loaded, style) {
        println!("{IND}{line}");
    }
}

fn failure_lines(failure: &Failure, loaded: &Loaded, style: Style) -> Vec<String> {
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
    // A deadlock says which task waits on which in its secondary labels and
    // nowhere else, so dropping them here leaves the terminal reader with a
    // count where the JSON has the cycle.
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

/// The repro, which under M7 is a seed rather than a stack trace.
///
/// The replay command is printed rather than described, because the claim being
/// made is that reproducing a concurrency failure is one command with one
/// argument, and a reader who has to assemble it does not get to check that.
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

/// Silent when nobody asked for a bisection: a run that was told not to look has
/// nothing to apologize for, while every other verdict names something the
/// reader can act on.
fn no_culprit_reason(bisection: &Bisection) -> Option<&str> {
    match bisection.verdict {
        Verdict::NotAttempted(Skipped::NotRequested) => None,
        _ => Some(bisection.reason.as_str()),
    }
}

/// A bare name is a list to read. `derived` and `did not run` are the two
/// annotations that take a name *off* that list, which is the whole point of
/// ranking them.
fn describe_suspect(suspect: &Suspect) -> String {
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
    names
        .iter()
        .map(|n| n.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(24, 64)
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
    format!(
        "{mark}{pad} {name:<name_width$} {:>8.1}ms",
        millis(result.duration)
    )
}

/// What one test's search did, under its result line. Silent for a test that
/// reached no `simulate` region, so a corpus with none reads exactly as it does
/// today.
///
/// `exhaustive` leads because it is the headline: it means every interleaving
/// ran, which is a proof rather than a sample. `naive` and the reduction appear
/// only under `--measure-reduction`, because a number that was not measured is a
/// slogan.
fn simulation_line(result: &TestResult) -> Option<String> {
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
            // A ratio over a naive count that spent its budget is a lower bound
            // too, and printing it bare claims a number nobody observed.
            let bound = if naive.bounded { ">= " } else { "" };
            parts.push(format!("{bound}{reduction:.0}× reduction"));
        }
    }
    Some(parts.join(" · "))
}

/// `host` is beside `cached` rather than only in the header, because the last
/// line a person reads is where "0 cached" would otherwise look like selection
/// working rather than a run that proved nothing it may keep.
///
/// `database` is beside it for the sharper version of the same argument: a
/// green suite that reached postgres and a green suite that reached the twin
/// are different claims, and the second one is the one a reader assumes.
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
            // A contention a rename would remove is worth distinguishing from
            // one that needs a database.
            format!(" {shared} (region labels)")
        } else {
            format!(" {shared}")
        };
        // A test whose row carries `sim.read` reaches a `simulate` region, and
        // the tree-walker cannot run one. Under `--engine both` it is therefore
        // run once, on the machine, and the audit covers strictly less of the
        // corpus than it did — which is worth saying rather than inferring.
        let engine = if ply_test::is_seeded(&test.footprint) {
            " · machine-only"
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
    // A host-backed test is grouped by footprint conflict like any other — a
    // host atom contends exactly as an in-memory one does — but it is never
    // *free*, so it is named apart from the count that claims it is.
    let hosted = if counts.host == 0 {
        String::new()
    } else {
        format!(" · {} host-backed and never free", counts.host)
    };
    // ADR 0008 §6 again, for the population ADR 0017 §6 moves: a contention a
    // rename would remove reads differently from one that needs a database, and
    // a report that did not separate them would leave the cost of losing the
    // fork looking like ordinary shared state.
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
///
/// Silent when nothing in the corpus reads a seed, so a project with no
/// `simulate` region sees exactly what it saw before.
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
///
/// `host` wins over both: region isolation is *inapplicable* to a computation
/// that reaches a socket rather than merely unavailable to it, and reporting
/// such a test as `region` is the over-claim ADR 0008 §6 exists to prevent.
fn isolation_label(
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
                // Whether this test's footprint meets the binding, and therefore
                // whether it always runs and is never cached. False throughout a
                // hermetic run.
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
                // Absent, never zeroed, on a test that reached no region: a
                // consumer cannot tell an explored count of zero from a test
                // that never simulated anything.
                "simulation": r.simulation.as_ref().map(ply_test::report::exploration_json),
                "cached": r.recorded.as_ref().map(|record| record.is_written()),
                // Absent outside `--engine both`, where there is no oracle whose
                // coverage this could describe.
                "audited": r.audited,
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
            "parsed": loaded.frontend.parsed(),
            "skipped": loaded.frontend.skipped(),
            "cached": loaded.frontend.cached(),
            "rechecked": loaded.frontend.rechecked(),
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
        // What this run could reach outside itself, and which trusted computing
        // base it was reached with. A green artifact that does not say this is a
        // green artifact whose meaning depends on a flag it did not record.
        "binding": view.hosts.label(),
        "hosts": view.hosts.summary_json(),
        "workers": workers,
        "options": {
            "bisect": args.bisect.as_str(),
            "bisect_budget": args.bisect_budget,
            "trace": args.trace.as_str(),
            "engine": args.engine.as_str(),
            // The whole plan, because every field of it is in a seeded test's
            // cache key and a consumer comparing two runs needs to see which
            // one searched more.
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
        // What the differential oracle actually covered. Absent, never zeroed,
        // when no oracle ran: a consumer cannot tell "compared nothing" from
        // "there was nothing to compare with".
        "audit": report.audit,
        // What a compiled backend was asked and what it did with it. Absent,
        // never zeroed, when none was installed — for the same reason `audit`
        // is: a consumer cannot tell "entered nothing" from "there was nothing
        // to enter with". Added within v4 and not a bump; nothing changed
        // meaning and nothing left.
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
        })),
        "selection": {
            "total": selection.total,
            "selected": selection.to_run.len(),
            "cached": selection.cached.len(),
            "filtered_out": plan.filtered_out,
            "groups": groups,
            // Corrected for the binding: a host-backed test is counted under
            // `host` and under neither of the other two, because it is not
            // isolated and saying otherwise over-claims the number M6
            // published. Identical to `parallelism` in a hermetic run.
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
        // Not a warning: an entry that escaped here is believed by every later
        // run, so it fails this one.
        "diagnostics": diagnostics_json(
            &view.escapes.iter().chain(&backend.escapes).cloned().collect::<Vec<_>>(),
            sources,
        ),
        "warnings": warnings.iter().map(|w| diagnostic_json(w, sources)).collect::<Vec<_>>(),
    })
}

/// The failure artifact, built on `ply-test`'s projection rather than beside it.
///
/// Everything an agent branches on — the verdict, the ranked suspects, the
/// causal slice — has exactly one implementation, and this adds only what the
/// runner does not hold: a rendered diagnostic, a file position, the test's hash
/// and its declared footprint.
fn failure_json(
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
            .map_or(Value::Null, |s| location_json(sources, s)),
    );
    object.insert(
        "footprint".into(),
        json!({
            "declared": test.map(|t| atoms(&t.footprint)),
            // Null rather than empty when nothing was traced: "performed no
            // atom" and "was never watched" are different findings, and a
            // consumer that reads one as the other looks in the wrong place.
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

/// Positions rather than byte offsets, because the consumer of this field opens
/// an editor with it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use ply_eval::Plan as SimPlan;
    use ply_span::Symbol;
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
            ply_eval::EngineChoice::Both,
            ply_test::Search::of(selection),
            ply_test::Hosting::hermetic(),
        )
    }

    fn plan_for(filter: Option<&str>) -> (tempfile::TempDir, Loaded, HashOutput, Plan) {
        let (dir, loaded, hashes) = fixture();
        let store = Store::open(dir.path()).unwrap();
        let selected = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
        let plan = Plan::new(selected, &loaded.check, filter, false);
        (dir, loaded, hashes, plan)
    }

    /// The artifact as `execute` builds it: the binding is opened the same way,
    /// so a test can never assert about a shape the real command does not
    /// produce.
    fn json_report(
        loaded: &Loaded,
        hashes: &HashOutput,
        plan: &Plan,
        report: &RunReport,
        args: &TestArgs,
        workers: usize,
    ) -> Value {
        let hosts = Hosts::open(
            &loaded.check,
            args.host,
            &[],
            None,
            crate::config::Configuration::default(),
            &crate::trace::TraceOptions::silent(),
            None,
        )
        .expect("the fixture binds");
        let view = HostView::of(&hosts, plan, &loaded.check, report);
        let backend = BackendView::of(None, None, report);
        let ok = report.is_success() && view.escapes.is_empty();
        report_json(
            loaded,
            hashes,
            plan,
            report,
            args,
            workers,
            &[],
            &view,
            &backend,
            ok,
        )
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
            bisect: When::Auto,
            bisect_budget: 64,
            trace: When::Auto,
            backend: None,
            host: false,
            tls: crate::cli::TlsOptions::default(),
            db: crate::db::DbOptions::default(),
            config: crate::config::ConfigOptions::default(),
            std: false,
            engine: crate::cli::EngineArg::default(),
            simulation: crate::cli::SimOptions {
                seed: None,
                sim: crate::cli::SimArg::default(),
                seeds: None,
                sim_budget: None,
                sim_steps: None,
                measure_reduction: false,
            },
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
        let index_of = |name: &str| {
            loaded
                .check
                .tests
                .iter()
                .position(|t| t.name == name)
                .unwrap()
        };
        let group_of = |name: &str| plan.selection.group_of(index_of(name)).unwrap();

        assert_eq!(
            loaded.check.tests[index_of("reads orders only")]
                .footprint
                .to_string(),
            "{m.db.read[users]}"
        );
        assert_eq!(
            loaded.check.tests[index_of("writes users when asked")]
                .footprint
                .to_string(),
            "{m.db.write[users]}"
        );
        assert_eq!(
            group_of("reads orders only"),
            group_of("reads orders only again")
        );
        assert_ne!(
            group_of("reads orders only"),
            group_of("writes users when asked")
        );
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
        // `isolated n of m` has to answer for the same m as `selected n of m`.
        let parallelism = &plan.selection.parallelism;
        assert_eq!(parallelism.total, 2);
        assert_eq!(parallelism.isolated, 0);
        assert_eq!(parallelism.shared_groups, 1);
        assert!(parallelism.holds(), "{parallelism:?}");
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
                ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default()),
                &loaded.check,
                Some(needle),
                false,
            )
        };
        assert_eq!(select("beta.").visible, vec![1]);
        assert_eq!(select("shared label").visible, vec![0, 1]);
    }

    #[test]
    fn a_warm_cache_selects_nothing_and_a_second_run_stays_empty() {
        let (dir, loaded, hashes) = fixture();
        let mut store = Store::open(dir.path()).unwrap();
        let selected = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
        let report = run(&loaded, &selected, &mut store);
        assert_eq!(report.passed, 4);
        assert_eq!(report.failed, 0);

        let store = Store::open(dir.path()).unwrap();
        let again = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
        assert!(again.to_run.is_empty());
        assert_eq!(again.cached.len(), 4);
        assert_eq!(
            Plan::new(again, &loaded.check, None, false).selection.total,
            4
        );
    }

    #[test]
    fn tests_from_every_module_are_selected_and_run_together() {
        let (dir, loaded, hashes) = project(&[
            (
                "lib.ply",
                "pub fn one() -> Int = 1\ntest \"one is one\" { assert_eq(one(), 1) }\n",
            ),
            (
                "app.ply",
                "import lib\n\
                 fn two() -> Int = lib::one() + lib::one()\n\
                 test \"two is two\" { assert_eq(two(), 2) }\n",
            ),
        ]);
        assert_eq!(loaded.module_count(), 2);

        let mut store = Store::open(dir.path()).unwrap();
        let selected = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
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
        let selected = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
        let report = run(&loaded, &selected, &mut store);

        assert_eq!(report.failed, 1);
        assert_eq!(
            report.failures[0].suspects,
            vec![ply_span::Symbol::new("lib.one")]
        );
    }

    #[test]
    fn no_cache_never_touches_the_real_store() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "m.ply", SOURCE);
        let loaded = load(dir.path()).unwrap();
        let hashes = loaded.hashes().unwrap();

        let scratch_dir = {
            let mut cache = Cache::open(dir.path(), true).unwrap();
            let scratch = cache
                .scratch
                .clone()
                .expect("bypass must use a scratch store");
            let selected =
                ply_test::select(&loaded.check, &hashes, &cache.store, &SimPlan::default());
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
        assert!(
            cache.scratch.is_some(),
            "the run must fall back rather than abort"
        );
        assert_eq!(cache.warnings.len(), 1);
        assert!(
            cache.warnings[0]
                .message
                .contains("could not open the cache")
        );
        assert!(
            cache.warnings[0]
                .notes
                .iter()
                .any(|n| n.contains("nothing this run proved"))
        );
    }

    #[test]
    fn a_failure_is_never_cached_so_it_re_runs() {
        let (dir, loaded, hashes) = project(&[(
            "m.ply",
            "fn f() -> Int = 1\ntest \"wrong\" { assert_eq(f(), 2) }\n",
        )]);
        let mut store = Store::open(dir.path()).unwrap();

        let selected = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
        let report = run(&loaded, &selected, &mut store);
        assert_eq!(report.failed, 1);
        assert_eq!(exit_code(report.is_success()), crate::EXIT_FAILED);
        assert_eq!(report.failures[0].diagnostic.code, codes::ASSERTION_FAILED);

        let store = Store::open(dir.path()).unwrap();
        let again = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
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
        let plan = Plan::new(
            ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default()),
            &loaded.check,
            None,
            false,
        );
        let report = run(&loaded, &plan.selection, &mut store);

        let v = json_report(&loaded, &hashes, &plan, &report, &args_for(None), 4);

        assert_eq!(v["command"], "test");
        assert_eq!(v["ok"], false);
        assert_eq!(v["exit_code"], 1);
        assert_eq!(v["selection"]["total"], 2);
        assert_eq!(v["selection"]["selected"], 2);
        assert_eq!(v["selection"]["tests"][0]["reason"], "new");
        assert_eq!(v["selection"]["tests"][0]["key"], "m.good");
        assert_eq!(v["selection"]["tests"][0]["module"], "m");
        assert_eq!(
            v["selection"]["tests"][0]["hash"].as_str().unwrap().len(),
            64
        );
        assert_eq!(v["modules"][0]["name"], "m");
        assert_eq!(v["failures"].as_array().unwrap().len(), 1);
        assert_eq!(
            v["failures"][0]["diagnostic"]["code"],
            codes::ASSERTION_FAILED
        );
        assert_eq!(
            v["failures"][0]["diagnostic"]["labels"][0]["start"]["line"],
            3
        );
        // `f` is in the failing test's closure and has never gone green.
        // Since v2: an object per suspect, ranked, not a bare name.
        assert_eq!(v["failures"][0]["suspects"][0]["name"], "m.f");
        assert_eq!(v["failures"][0]["suspects"][0]["culprit"], false);
        let at = &v["failures"][0]["location"];
        assert!(at["file"].as_str().unwrap().ends_with("m.ply"));
        assert_eq!(at["line"], 3);
        assert_eq!(v["summary"]["failed"], 1);
        assert_eq!(v["summary"]["passed"], 1);
    }

    #[test]
    fn the_failure_artifact_carries_the_v4_shape_an_agent_branches_on() {
        let (dir, loaded, hashes) = project(&[(
            "ledger.ply",
            "fn balance() -> Int = 0 - 5\n\
             test \"balance never goes negative\" { assert_eq(balance(), 0) }\n",
        )]);
        let mut store = Store::open(dir.path()).unwrap();
        let plan = Plan::new(
            ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default()),
            &loaded.check,
            None,
            false,
        );
        let report = run(&loaded, &plan.selection, &mut store);

        let v = json_report(&loaded, &hashes, &plan, &report, &args_for(None), 1);
        assert_eq!(v["schema_version"], 4);

        let f = &v["failures"][0];
        assert_eq!(f["key"], "ledger.balance never goes negative");
        assert_eq!(f["name"], "balance never goes negative");
        assert_eq!(f["module"], "ledger");
        assert_eq!(f["nondet"], false);
        assert_eq!(f["status"], "failed");
        assert_eq!(f["test_hash"].as_str().unwrap().len(), 64);
        assert_eq!(f["diagnostic"]["code"], codes::ASSERTION_FAILED);
        assert!(
            f["location"]["file"]
                .as_str()
                .unwrap()
                .ends_with("ledger.ply")
        );

        // Present even where the evidence behind them is missing: a field that
        // vanishes and a field that says "not known" are different answers, and
        // a consumer branches on the difference.
        assert!(f["culprit"]["verdict"].is_string());
        assert!(f["culprit"]["confidence"].is_string());
        assert!(f["culprit"]["definitions"].is_array());
        assert!(f["culprit"]["groups"].is_array());
        assert!(f["culprit"]["reason"].is_string());
        assert_eq!(f["culprit"]["search"]["evaluated"], 0);
        assert!(
            f["assertion"].is_null(),
            "the evaluator carries no payload yet"
        );
        // v3. Null rather than absent on a failure no simulation produced: a
        // consumer branches on the difference, and a default seed would replay
        // a different run.
        assert!(f["seed"].is_null());
        assert!(f["replay"].is_null());
        assert!(f["race"].is_null());
        assert!(f["causal_slice"].is_null(), "nothing traced this run");
        assert_eq!(f["footprint"]["declared"], json!([]));
        assert!(
            f["footprint"]["observed"].is_null(),
            "an untraced run must not claim an empty observed footprint"
        );
    }

    /// Two runs over one failure have to produce the same bytes, or yesterday's
    /// artifact cannot be diffed against today's.
    #[test]
    fn the_artifact_is_byte_identical_across_two_runs_over_one_failure() {
        let (dir, loaded, hashes) = project(&[(
            "m.ply",
            "fn a() -> Int = 1\nfn b() -> Int = 2\n\
             test \"wrong\" { assert_eq(a() + b(), 4) }\n",
        )]);
        let render = || {
            let mut store = Store::open(dir.path()).unwrap();
            let plan = Plan::new(
                ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default()),
                &loaded.check,
                None,
                false,
            );
            let report = run(&loaded, &plan.selection, &mut store);
            serde_json::to_string(
                &json_report(&loaded, &hashes, &plan, &report, &args_for(None), 1)["failures"],
            )
            .unwrap()
        };
        assert_eq!(render(), render());
    }

    fn failing(source: &str) -> (tempfile::TempDir, Loaded, HashOutput, RunReport) {
        let (dir, loaded, hashes) = project(&[("m.ply", source)]);
        let mut store = Store::open(dir.path()).unwrap();
        let selected = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
        let report = run(&loaded, &selected, &mut store);
        (dir, loaded, hashes, report)
    }

    const ONE_FAILURE: &str = "fn f() -> Int = 1\ntest \"wrong\" { assert_eq(f(), 2) }\n";

    #[test]
    fn bisect_never_reports_that_nothing_was_attempted_and_evaluates_nothing() {
        let (_dir, loaded, hashes, mut report) = failing(ONE_FAILURE);
        let mut args = args_for(None);
        args.bisect = When::Never;
        ply_test::diagnose_failures(
            &mut report,
            &loaded.program,
            &loaded.resolved,
            &loaded.check,
            &hashes,
            &mut Store::open(_dir.path()).unwrap(),
            &diagnosis_options(&args),
        );

        let bisection = &report.failures[0].attribution.bisection;
        assert_eq!(
            bisection.verdict,
            Verdict::NotAttempted(Skipped::NotRequested)
        );
        assert_eq!(bisection.search.evaluated, 0);

        let plan = Plan::new(
            ply_test::select(
                &loaded.check,
                &hashes,
                &Store::open(_dir.path()).unwrap(),
                &SimPlan::default(),
            ),
            &loaded.check,
            None,
            false,
        );
        let v = json_report(&loaded, &hashes, &plan, &report, &args, 1);
        assert_eq!(v["failures"][0]["culprit"]["verdict"], "not_attempted");
        assert_eq!(v["failures"][0]["culprit"]["skipped"], "not_requested");
        assert_eq!(v["options"]["bisect"], "never");
    }

    /// The one ordering claim the human form makes.
    #[test]
    fn the_culprit_line_comes_above_the_assertion_and_is_absent_when_there_is_none() {
        let (_dir, loaded, _h, mut report) = failing(ONE_FAILURE);

        let rendered =
            |report: &RunReport| failure_lines(&report.failures[0], &loaded, Style::plain());

        let quiet = rendered(&report);
        assert!(
            !quiet.iter().any(|l| l.contains("culprit")),
            "an unrequested bisection has nothing to apologize for: {quiet:?}"
        );
        assert!(quiet.iter().any(|l| l.contains("assertion failed")));

        let slice = report.failures[0].attribution.slice.clone();
        report.failures[0].attribution.resolve(
            Bisection {
                verdict: Verdict::Sole,
                confidence: ply_test::Confidence::Minimal,
                groups: vec![vec![Symbol::new("m.f")]],
                reason: "one definition changed".into(),
                search: ply_test::SearchStats::default(),
            },
            slice,
        );
        let loud = rendered(&report);
        let culprit = loud
            .iter()
            .position(|l| l.contains("culprit: m.f"))
            .expect("a conclusive bisection must name its culprit");
        let assertion = loud
            .iter()
            .position(|l| l.contains("assertion failed"))
            .expect("the diff is still the evidence");
        assert!(
            culprit < assertion,
            "the culprit is the answer and must precede the evidence: {loud:?}"
        );
        assert!(loud.iter().any(|l| l.contains("one definition changed")));

        let v = failure_json(&report.failures[0], &loaded, &_h, &report);
        assert_eq!(v["culprit"]["verdict"], "sole");
        assert_eq!(v["culprit"]["definitions"], json!(["m.f"]));
        assert_eq!(v["suspects"][0]["name"], "m.f");
        assert_eq!(v["suspects"][0]["culprit"], true);
    }

    /// E0414's whole value is the cycle, and the cycle lives in the secondary
    /// labels. A terminal reader who gets only the count has to open the JSON to
    /// learn which task waits on which, which is the artifact being reported
    /// twice rather than once.
    #[test]
    fn a_deadlock_names_every_blocked_task_and_what_it_waits_on() {
        const DEADLOCK: &str = "\
type Slot =
  | Empty
  | Peer(Task<Int>)

test \"stuck\" {
  simulate {
    with_cell[slot](Empty) { peer -> {
      let first = task.spawn(|| {
        clock.sleep(1);
        match cell_get(peer) {
          Peer(other) -> task.join(other),
          Empty -> 0,
        }
      });
      let second = task.spawn(|| task.join(first));
      cell_set(peer, Peer(second));
      task.join(first)
    } }
  }
}
";
        let (_dir, loaded, _hashes, report) = failing(DEADLOCK);
        assert_eq!(report.failures[0].diagnostic.code, codes::DEADLOCK);

        let lines = failure_lines(&report.failures[0], &loaded, Style::plain());
        for waiting in [
            "@0 waits here for @1 to finish",
            "@1 waits here for @2 to finish",
            "@2 waits here for @1 to finish",
        ] {
            let line = lines
                .iter()
                .find(|l| l.contains(waiting))
                .unwrap_or_else(|| panic!("`{waiting}` is missing from {lines:?}"));
            assert!(
                line.contains("m.ply:"),
                "a wait without a location is not actionable: {line}"
            );
        }
    }

    #[test]
    fn a_suspect_reads_as_a_reason_to_skip_it_rather_than_a_bare_name() {
        let plain = Suspect::new(Symbol::new("m.f"), None);
        assert_eq!(describe_suspect(&plain), "m.f");

        let mut derived = Suspect::new(Symbol::new("m.post"), None);
        derived.change = Some(ply_test::ChangeKind::Derived);
        derived.ran = Some(false);
        assert_eq!(describe_suspect(&derived), "m.post (derived, did not run)");

        let mut returned = Suspect::new(Symbol::new("m.setup"), None);
        returned.change = Some(ply_test::ChangeKind::Edited);
        returned.ran = Some(true);
        assert_eq!(
            describe_suspect(&returned),
            "m.setup (edited, ran, then returned)"
        );
    }

    #[test]
    fn two_failures_sharing_a_label_are_told_apart_by_their_key() {
        let (dir, loaded, hashes) = project(&[
            (
                "alpha.ply",
                "fn one() -> Int = 1\ntest \"it adds up\" { assert_eq(one(), 2) }\n",
            ),
            (
                "beta.ply",
                "fn two() -> Int = 2\ntest \"it adds up\" { assert_eq(two(), 3) }\n",
            ),
        ]);
        let mut store = Store::open(dir.path()).unwrap();
        let plan = Plan::new(
            ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default()),
            &loaded.check,
            None,
            false,
        );
        let report = run(&loaded, &plan.selection, &mut store);
        assert_eq!(report.failed, 2);

        let v = json_report(&loaded, &hashes, &plan, &report, &args_for(None), 4);
        let keys: Vec<&str> = v["failures"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["key"].as_str().unwrap())
            .collect();
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
            "nondet effect wall {\n  read now() -> Int\n}\n\
             test/nondet \"reads the clock\" { assert(wall.now() > 0) }\n",
        )]);
        let mut store = Store::open(dir.path()).unwrap();

        let selected = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
        assert_eq!(selected.reason(0), Some(Reason::Nondet));
        run(&loaded, &selected, &mut store);

        let store = Store::open(dir.path()).unwrap();
        let again = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
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
            simulation: None,
            recorded: None,
            audited: None,
            backend: None,
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

    /// A naive search that spent its budget bounds the ratio as well as the
    /// count, and the ratio is the number the milestone is claimed on.
    #[test]
    fn a_reduction_over_a_bounded_naive_count_is_reported_as_a_bound() {
        let line = |naive: ply_eval::Naive| {
            simulation_line(&TestResult {
                index: 0,
                name: "n".into(),
                hash: None,
                group: 0,
                duration: Duration::from_millis(1),
                status: Status::Passed,
                failure: None,
                simulation: Some(ply_eval::Exploration {
                    explored: 1,
                    exhaustive: true,
                    naive: Some(naive),
                    ..Default::default()
                }),
                recorded: None,
                audited: None,
                backend: None,
            })
            .expect("a simulated test has a line")
        };

        let exact = line(ply_eval::Naive {
            explored: 720,
            bounded: false,
        });
        assert!(exact.contains("naive 720"), "{exact}");
        assert!(exact.contains("720× reduction"), "{exact}");
        assert!(!exact.contains(">="), "{exact}");

        let bounded = line(ply_eval::Naive {
            explored: 4096,
            bounded: true,
        });
        assert!(bounded.contains("naive >= 4096"), "{bounded}");
        assert!(bounded.contains(">= 4096× reduction"), "{bounded}");
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
            simulation: None,
            recorded: None,
            audited: None,
            backend: None,
        };
        let column = |status| {
            result_line(&make(status), "n", 24, Style::plain())
                .find("n ")
                .unwrap()
        };
        assert_eq!(column(Status::Passed), column(Status::Failed));
        assert_eq!(column(Status::Passed), column(Status::Panicked));
    }

    #[test]
    fn cached_results_do_not_appear_as_run_results() {
        let (dir, loaded, hashes) = fixture();
        let mut store = Store::open(dir.path()).unwrap();
        let first = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
        run(&loaded, &first, &mut store);

        let mut store = Store::open(dir.path()).unwrap();
        let second = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
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
            Outcome::Fail {
                message: "from an older runtime".into(),
                diagnostic: None,
            },
        );
        store.flush().unwrap();

        let selected = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
        assert_eq!(selected.reason(0), Some(Reason::PreviousFailure));
        assert!(selected.to_run.contains(&0));
    }

    // --- The binding's effect on what a run reports -------------------------

    /// A binding over the fixture's `db.all[users]`, which is the residual atom
    /// of the two reading tests and of neither of the others. A registration
    /// names the effect as its declaration writes it, so `db` rather than the
    /// program-wide `m.db`.
    fn bound(loaded: &Loaded) -> Hosts {
        use crate::hosts::fixture::{deterministic, named, op, registry};
        Hosts::bind(
            registry(vec![deterministic(op(
                "db",
                "all",
                named("users"),
                ply_eval::host::Linearity::AtMostOnce,
                false,
                "ply_host::postgres::read",
            ))]),
            &loaded.check,
            true,
        )
        .expect("the fixture binds")
    }

    /// A report over exactly these results. `RunReport` has no `Default`, and
    /// giving it one would let a real caller forget a field.
    fn report_over(results: Vec<TestResult>) -> RunReport {
        RunReport {
            passed: 0,
            failed: 0,
            cached: 0,
            failures: Vec::new(),
            duration: Duration::ZERO,
            parallelism: ply_test::Parallelism::default(),
            results,
            warnings: Vec::new(),
            simulation: ply_test::SimSummary::default(),
            audit: None,
        }
    }

    fn index_of(loaded: &Loaded, name: &str) -> usize {
        loaded
            .check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap()
    }

    /// The claim `--explain` publishes: a test that can reach a socket is not
    /// isolated, and saying `region` about it would over-claim exactly the
    /// number M6 introduced.
    #[test]
    fn a_host_backed_test_is_reported_as_host_rather_than_world() {
        let (_dir, loaded, _h, plan) = plan_for(None);
        let hosts = bound(&loaded);
        let view = HostView::of(&hosts, &plan, &loaded.check, &report_over(Vec::new()));
        let label = |name: &str| {
            let index = index_of(&loaded, name);
            let footprint = &loaded.check.tests[index].footprint;
            isolation_label(&plan, &view, &loaded.check, index, footprint)
        };

        assert_eq!(label("reads orders only"), "host");
        assert_eq!(label("reads orders only again"), "host");
        assert_eq!(label("writes users when asked"), "shared");
        assert_eq!(label("pure arithmetic"), "region");

        assert_eq!(view.counts.total, 4);
        assert_eq!(view.counts.host, 2);
        assert_eq!(view.counts.isolated, 1);
        assert_eq!(view.counts.shared, 1);
    }

    /// The same corpus with nothing bound: every label and every count is what
    /// it was before W1, which is what makes the hermetic default free.
    #[test]
    fn a_hermetic_run_reports_exactly_what_it_did_before() {
        let (_dir, loaded, _h, plan) = plan_for(None);
        let hosts = Hosts::open(
            &loaded.check,
            false,
            &[],
            None,
            crate::config::Configuration::default(),
            &crate::trace::TraceOptions::silent(),
            None,
        )
        .unwrap();
        let view = HostView::of(&hosts, &plan, &loaded.check, &report_over(Vec::new()));

        for (index, test) in loaded.check.tests.iter().enumerate() {
            let expected = plan
                .selection
                .isolation_of(index)
                .unwrap_or_else(|| Isolation::of(&test.footprint));
            assert_eq!(
                isolation_label(&plan, &view, &loaded.check, index, &test.footprint),
                expected.as_str()
            );
        }
        let parallelism = &plan.selection.parallelism;
        assert_eq!(view.counts.host, 0);
        assert_eq!(view.counts.isolated, parallelism.isolated);
        assert_eq!(view.counts.shared, parallelism.shared);
        assert_eq!(view.counts.total, parallelism.total);
        assert!(view.escapes.is_empty());
    }

    /// The failure mode this milestone is built around is a green result over
    /// unexplored space, and a cached pass earned over a real socket is exactly
    /// that: every later hermetic run believes it, and nothing about those runs
    /// looks wrong. So it fails the run that produced it rather than warning.
    #[test]
    fn a_cached_pass_over_the_host_fails_the_run_that_wrote_it() {
        let (_dir, loaded, hashes, plan) = plan_for(None);
        let hosts = bound(&loaded);
        let index = index_of(&loaded, "reads orders only");

        let recorded = |index: usize| {
            report_over(vec![TestResult {
                index,
                name: loaded.check.tests[index].name.clone(),
                hash: hashes.tests.get(index).copied(),
                group: 0,
                duration: Duration::from_millis(1),
                status: Status::Passed,
                failure: None,
                simulation: None,
                recorded: Some(Record::Under(vec![hashes.tests[index]])),
                audited: None,
                backend: None,
            }])
        };

        let escaped = recorded(index);
        let view = HostView::of(&hosts, &plan, &loaded.check, &escaped);
        assert_eq!(view.escapes.len(), 1, "{:?}", view.escapes);
        assert_eq!(view.escapes[0].code, codes::INTERNAL_ERROR);
        assert!(view.escapes[0].message.contains("reads orders only"));
        assert!(
            view.escapes[0]
                .notes
                .iter()
                .any(|n| n.contains("ply cache clear"))
        );

        // A test the binding cannot reach is cached exactly as it always was:
        // `--host` is not a `--no-cache`, and a build that made it one would
        // teach people not to run it.
        let ordinary = recorded(index_of(&loaded, "pure arithmetic"));
        let view = HostView::of(&hosts, &plan, &loaded.check, &ordinary);
        assert!(view.escapes.is_empty());

        // And hermetically the check costs nothing, because nothing is reachable.
        let hermetic = Hosts::open(
            &loaded.check,
            false,
            &[],
            None,
            crate::config::Configuration::default(),
            &crate::trace::TraceOptions::silent(),
            None,
        )
        .unwrap();
        let view = HostView::of(&hermetic, &plan, &loaded.check, &escaped);
        assert!(view.escapes.is_empty());
    }

    /// The same corpus behind the postgres driver's own registration path, with
    /// a database configured — which is the run W4 introduces and the one whose
    /// cached pass would be believed by every later hermetic run.
    ///
    /// A `db` handler is not a special case of any of this and must not become
    /// one: the check keys on "the binding reaches this footprint", so a driver
    /// added to the trusted computing base inherits it. That is what this test
    /// pins.
    #[test]
    fn a_database_backed_test_is_host_backed_never_cached_and_says_which_database() {
        use crate::hosts::fixture::{deterministic, named, op, registry};
        let (_dir, loaded, hashes, plan) = plan_for(None);
        let config = crate::db::DbOptions {
            url: Some("postgres://ply:hunter2@127.0.0.1:5433/desk".to_string()),
            ..crate::db::DbOptions::default()
        }
        .resolve_with(true, &|_| None)
        .expect("the fixture URL parses");
        let hosts = Hosts::bind_with(
            registry(vec![deterministic(op(
                "db",
                "all",
                named("users"),
                ply_eval::host::Linearity::AtMostOnce,
                true,
                "ply_host::db::query",
            ))]),
            &loaded.check,
            true,
            config,
        )
        .expect("the fixture binds");

        assert!(hosts.is_live_database());
        let line = crate::hosts::database_line(&hosts).expect("a live database is reported");
        assert!(
            line.contains("postgres://ply:****@127.0.0.1:5433/desk"),
            "{line}"
        );
        assert!(!line.contains("hunter2"), "{line}");

        let index = index_of(&loaded, "reads orders only");
        let view = HostView::of(&hosts, &plan, &loaded.check, &report_over(Vec::new()));
        assert_eq!(view.counts.host, 2);
        assert_eq!(
            isolation_label(
                &plan,
                &view,
                &loaded.check,
                index,
                &loaded.check.tests[index].footprint
            ),
            "host"
        );

        let recorded = report_over(vec![TestResult {
            index,
            name: loaded.check.tests[index].name.clone(),
            hash: hashes.tests.get(index).copied(),
            group: 0,
            duration: Duration::from_millis(1),
            status: Status::Passed,
            failure: None,
            simulation: None,
            recorded: Some(Record::Under(vec![hashes.tests[index]])),
            audited: None,
            backend: None,
        }]);
        let view = HostView::of(&hosts, &plan, &loaded.check, &recorded);
        assert_eq!(
            view.escapes.len(),
            1,
            "a pass over a real database was kept"
        );
        assert_eq!(view.escapes[0].code, codes::INTERNAL_ERROR);
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
        let selected = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
        assert_eq!(selected.to_run.len(), 2);
        run(&loaded, &selected, &mut store);

        let store = Store::open(dir.path()).unwrap();
        let again = ply_test::select(&loaded.check, &hashes, &store, &SimPlan::default());
        assert!(again.to_run.is_empty());
        assert_eq!(again.cached.len(), 2);
    }
}

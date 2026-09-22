//! What `ply test` loads, selects, binds and runs, as the program in `crates/ply-cli/ply/tests.ply`
//! performs it.
//!
//! The front end, the result cache, the selection it answers, the conflict grouping, the compiled
//! backend, the host binding, the worker pool, the per-test unwind catching and the bisection stay
//! here: a front end is not a value a program can hold, a Rust unwind is not a Ply value, and a
//! reader of the store's on-disk format written in Ply would be a second implementation of it.
//! What is said about all of it, in both forms, and the code the run exits with are the program's.

use crate::cli::{TestArgs, When};
use crate::commands::common::{
    backend_spec, build_backend_over, build_pool, describe_schema, enter_constant, module_texts,
    once_each, select_profile,
};
use crate::hosts::{self, Hosts, Lent, hosting};
use crate::load::{Loaded, load, project_root};
use crate::payload::{count, diag_value, diags_value, json, option, places_value, record, strings};
use ply_eval::Value as PlyValue;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_span::{Diagnostic, SourceMap, Span, Symbol, codes};
use ply_store::Store;
use ply_test::{
    Isolation, Parallelism, Record, RunReport, Selection, Skipped, Status, Suspect, TestResult,
    Verdict,
};
use ply_ty::{CheckOutput, Footprint, HashOutput};
use serde_json::{Value, json as jsonlit};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

/// The effect `crates/ply-cli/ply/tests.ply` declares. It is lent to that one entry and nowhere
/// else: no other command runs a corpus.
const EFFECT: &str = "tester";

const OPERATIONS: [(&str, &str); 3] = [
    ("loaded", "ply_cli::test::loaded"),
    ("bound", "ply_cli::test::bound"),
    ("ran", "ply_cli::test::ran"),
];

/// A compiled body honours its call bound on the native stack, where unoptimised frames run to
/// kilobytes. Reserved, not committed.
const RUN_STACK: usize = 256 << 20;

/// The load, the store and the binding are opened on a thread of their own, so the flags the
/// machine reads are all this side keeps.
pub fn lent(args: &TestArgs) -> Vec<Lent> {
    let site: Arc<dyn HostHandler> = Arc::new(Site {
        args: args.clone(),
        machine: Mutex::new(None),
    });
    OPERATIONS
        .into_iter()
        .map(|(op, path)| (registration(op, path), Arc::clone(&site)))
        .collect()
}

fn registration(op: &str, path: &'static str) -> HostOp {
    HostOp {
        effect: Symbol::new(EFFECT),
        op: Symbol::new(op),
        resource: HostResource::Any,
        // A tree, a clock and a cache are not functions of program state.
        determinism: Determinism::Nondeterministic,
        // One report per entry, and `--watch` enters the program again rather than replaying.
        linearity: Linearity::Repeatable,
        // The answer is in hand when the operation returns: this thread waits for the one the
        // corpus runs on rather than being handed a token to poll.
        blocking: false,
        secrets: false,
        path,
    }
}

struct Site {
    args: TestArgs,
    /// Started by the first operation and joined by the last.
    machine: Mutex<Option<Machine>>,
}

impl HostHandler for Site {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match req.op.op.as_str() {
            "loaded" => self.loaded()?,
            "bound" => self.bound()?,
            "ran" => self.ran()?,
            other => return Err(unasked(other, span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

impl Site {
    fn held(&self) -> std::sync::MutexGuard<'_, Option<Machine>> {
        self.machine.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn loaded(&self) -> Result<PlyValue, Diagnostic> {
        let mut held = self.held();
        if held.is_none() {
            *held = Some(Machine::start(self.args.clone())?);
        }
        let machine = held.as_ref().ok_or_else(|| unstarted("loaded"))?;
        match machine.step()? {
            Step::Loaded(found) => Ok(answered((*found).map(found_value))),
            _ => Err(out_of_step("loaded")),
        }
    }

    fn bound(&self) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("bound"))?;
        machine.ask(Go::Bind)?;
        match machine.step()? {
            Step::Bound(refused) => Ok(answered(match *refused {
                Some(refused) => Err(refused),
                None => Ok(PlyValue::Unit),
            })),
            _ => Err(out_of_step("bound")),
        }
    }

    fn ran(&self) -> Result<PlyValue, Diagnostic> {
        let mut held = self.held();
        let step = {
            let machine = held.as_ref().ok_or_else(|| unstarted("ran"))?;
            machine.ask(Go::Run)?;
            machine.step()?
        };
        // The report is written: the thread it was measured on is joined here.
        held.take();
        match step {
            Step::Ran(over) => Ok(ran_value(&over)),
            _ => Err(out_of_step("ran")),
        }
    }
}

/// `Ok(v)` or `Err(Refusal)`, as the program reads an operation's answer.
fn answered(answer: Result<PlyValue, Refused>) -> PlyValue {
    match answer {
        Ok(value) => PlyValue::ctor("Ok", vec![value]),
        Err(refused) => PlyValue::ctor("Err", vec![refusal_value(&refused)]),
    }
}

// --- The thread the corpus runs on --------------------------------------------

enum Go {
    Bind,
    Run,
}

enum Step {
    Loaded(Box<Result<Found, Refused>>),
    Bound(Box<Option<Refused>>),
    Ran(Box<Over>),
}

/// The thread this run's machine lives on. The `ply` program performing these operations is itself
/// inside an entry; two entries do not nest on one thread, and a bisection and a mutation each
/// evaluate a program of their own. The load, the store, the binding, the pool and the diagnosis
/// all happen here, and only what a report is written from crosses back.
struct Machine {
    go: Option<mpsc::Sender<Go>>,
    steps: mpsc::Receiver<Step>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Machine {
    fn start(args: TestArgs) -> Result<Machine, Diagnostic> {
        let (go, asked) = mpsc::channel();
        let (told, steps) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .stack_size(RUN_STACK)
            .spawn(move || serve(&args, &told, &asked))
            .map_err(|e| unspawned(&e))?;
        Ok(Machine {
            go: Some(go),
            steps,
            thread: Some(thread),
        })
    }

    fn ask(&self, go: Go) -> Result<(), Diagnostic> {
        match &self.go {
            Some(sender) => sender.send(go).map_err(|_| unanswered()),
            None => Err(unanswered()),
        }
    }

    fn step(&self) -> Result<Step, Diagnostic> {
        self.steps.recv().map_err(|_| unanswered())
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        // Dropping the sender ends whichever wait the thread is parked on, so a program that
        // stopped short of running leaves nothing behind and no scratch directory.
        self.go.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(args: &TestArgs, told: &mpsc::Sender<Step>, asked: &mpsc::Receiver<Go>) {
    let mut cache = match Cache::open(&project_root(&args.path), args.no_cache) {
        Ok(cache) => cache,
        Err(diagnostic) => {
            let _ = told.send(Step::Loaded(Box::new(Err(Refused {
                diagnostics: vec![diagnostic],
                sources: SourceMap::new(),
            }))));
            return;
        }
    };
    let mut warnings = cache.warnings.clone();
    let opened = cache.store.take_warnings();
    warnings.extend(crate::migrate::notice(&cache.store, &opened));
    warnings.extend(opened);

    let loaded = if args.no_cache {
        load(&args.path)
    } else {
        crate::driver::load_incremental(&args.path, &mut cache.store)
    };
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(err) => {
            let _ = told.send(Step::Loaded(Box::new(Err(Refused {
                diagnostics: err.diagnostics,
                sources: err.sources,
            }))));
            return;
        }
    };
    warnings.extend(cache.store.take_warnings());
    warnings.extend(loaded.frontend.warnings.iter().cloned());

    // Part of a simulated test's cache key, so decided before selection.
    let search = crate::simulation::plan(&args.simulation);
    let engine = ply_test::Engine::Evaluator;
    let hashes = loaded.hashes.clone();
    let selected = ply_test::select(&loaded.check, &hashes, &cache.store, &search, &engine);
    let plan = Plan::new(selected, &loaded.check, args.filter.as_deref(), args.std);

    if let Some(err) = crate::costs::broken_promises(&loaded) {
        let _ = told.send(Step::Loaded(Box::new(Err(Refused {
            diagnostics: err.diagnostics,
            sources: err.sources,
        }))));
        return;
    }

    let _ = told.send(Step::Loaded(Box::new(Ok(found(
        args, &loaded, &hashes, &plan, warnings,
    )))));
    let Ok(Go::Bind) = asked.recv() else {
        return;
    };
    bind(
        args, &mut cache, loaded, hashes, plan, &search, engine, told, asked,
    );
}

#[allow(clippy::too_many_arguments)]
fn bind(
    args: &TestArgs,
    cache: &mut Cache,
    loaded: Loaded,
    hashes: HashOutput,
    plan: Plan,
    search: &ply_eval::Plan,
    engine: ply_test::Engine,
    told: &mpsc::Sender<Step>,
    asked: &mpsc::Receiver<Go>,
) {
    let refuse = |diagnostics: Vec<Diagnostic>| {
        let _ = told.send(Step::Bound(Box::new(Some(Refused {
            diagnostics,
            sources: loaded.sources.clone(),
        }))));
    };
    let backend =
        match select_profile(&args.profile).and_then(|()| backend_spec(args.backend.as_ref())) {
            Ok(spec) => spec,
            Err(diagnostic) => return refuse(vec![diagnostic]),
        };
    // Before anything runs, so no test touches a resource the program does not declare.
    let db = match args.db.resolve(args.host) {
        Ok(db) => db,
        Err(diagnostics) => return refuse(diagnostics),
    };
    let reach = Footprint::from_atoms(
        loaded
            .check
            .tests
            .iter()
            .flat_map(|t| t.footprint.atoms().cloned()),
    );
    // One per run, shared by the workers; an empty selection builds nothing a schema does not need.
    let nothing_to_run = plan.selection.to_run.is_empty();
    let schema_named =
        args.config.schema.is_some() || db.as_ref().is_some_and(|c| c.schema.is_some());
    let wanted = backend.as_ref().filter(|_| !nothing_to_run || schema_named);
    let unit = match wanted.map(|spec| {
        build_backend_over(
            spec,
            &loaded.front,
            module_texts(&loaded.check, &loaded.sources),
        )
    }) {
        None => None,
        Some(Ok(provider)) => Some(provider),
        Some(Err(diagnostic)) => return refuse(vec![diagnostic]),
    };
    let constant = |name: &str| enter_constant(unit, name);
    // Before binding, so a missing required key fails before any host test runs.
    let (configuration, config_warnings) =
        match crate::config::Configuration::open(&loaded.check, args.host, &args.config, &constant)
        {
            Ok(resolved) => resolved,
            Err(diagnostics) => return refuse(diagnostics),
        };
    let mut hosts = match Hosts::open(
        &loaded.check,
        args.host,
        &args.tls,
        &args.fs.fs,
        db,
        configuration,
        // `--trace` on this command names the definition trace, so records are discarded.
        &crate::trace::TraceOptions::silent(),
        Some(&reach),
    ) {
        Ok(hosts) => hosts,
        Err(diagnostics) => return refuse(diagnostics),
    };
    describe_schema(&mut hosts, &constant);
    let _ = told.send(Step::Bound(Box::new(None)));
    let Ok(Go::Run) = asked.recv() else {
        return;
    };
    let over = execute(
        args,
        cache,
        &loaded,
        &hashes,
        &plan,
        search,
        &engine,
        &hosts,
        backend,
        unit.filter(|_| !nothing_to_run),
        config_warnings,
    );
    let _ = told.send(Step::Ran(Box::new(over)));
}

#[allow(clippy::too_many_arguments)]
fn execute(
    args: &TestArgs,
    cache: &mut Cache,
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: &Plan,
    search: &ply_eval::Plan,
    engine: &ply_test::Engine,
    hosts: &Hosts,
    backend: Option<ply_eval::BackendSpec>,
    provider: Option<&'static dyn ply_eval::Provider>,
    mut warnings: Vec<Diagnostic>,
) -> Over {
    let (pool, workers) = build_pool(args.jobs, &mut warnings);
    let simulation =
        ply_test::Search::of(&plan.selection).measuring(args.simulation.measure_reduction);
    // A factory: a reactor belongs to its thread, and each worker builds its own machine.
    let runtime = hosts.runtime_factory();
    // The budgets a test is given are set on the thread it is measured on and nowhere else: the
    // `ply` program performing this is inside a scope that zeroed the thread-local ones, and the
    // lookup prefers a thread-local to the process value. The diagnosis below evaluates hybrid
    // programs, so it is inside the same scope rather than beside it.
    let (mut report, mutants) = ply_codegen::rt::with_step_budget(args.steps, || {
        ply_codegen::rt::with_time_budget(args.timeout, || {
            let mut run = || {
                let mut executor = ply_test::InterpExecutor::new(&loaded.front)
                    .with_search(simulation.clone())
                    .with_hosts(hosting(hosts, &runtime));
                if let (Some(provider), Some(spec)) = (provider, backend.clone()) {
                    executor = executor.with_backend(provider, spec);
                }
                ply_test::run_with(
                    &plan.selection,
                    &loaded.check,
                    hashes,
                    &mut cache.store,
                    &executor,
                )
            };
            let mut report = match &pool {
                Some(pool) => pool.install(run),
                None => run(),
            };
            // After the run, since a pass recorded now is a valid baseline for another's failure.
            ply_test::diagnose_failures(
                &mut report,
                &loaded.texts(),
                &loaded.front,
                &mut cache.store,
                &diagnosis_options(args),
            );
            let escapes = hosts_escapes(&report, &loaded.check, hosts);
            let ok = report.is_success() && escapes.is_empty();
            // Only over a green program: a survivor of a red one says nothing.
            let mutants = match (&args.mutate, ok, &backend) {
                (Some(query), true, Some(spec)) => {
                    match crate::commands::mutate::targets(loaded, query) {
                        Ok(targets) => Some(Ok(crate::commands::mutate::run(
                            loaded,
                            hashes,
                            &targets,
                            args.mutate_budget,
                            search,
                            engine,
                            spec,
                            hosts,
                            &runtime,
                        ))),
                        Err(diagnostic) => Some(Err(diagnostic)),
                    }
                }
                _ => None,
            };
            (report, mutants)
        })
    });
    warnings.extend(report.warnings.iter().cloned());
    // Pass records are read lazily, so an unreadable baseline only surfaces here.
    warnings.extend(cache.store.take_warnings());

    let counts = counts(plan, &loaded.check, hosts);
    let mut escapes = backend_escapes(&report, engine);
    escapes.extend(hosts_escapes(&report, &loaded.check, hosts));
    if let Some(unbuilt) = unbuilt_backend(provider) {
        escapes.push(unbuilt);
    }
    let mutants = match mutants {
        Some(Ok(report)) => Some(mutants_view(&report, loaded)),
        Some(Err(diagnostic)) => {
            warnings.push(diagnostic);
            None
        }
        None => None,
    };
    Over {
        hermetic: hosts.is_hermetic(),
        label: hosts.label().to_string(),
        operations: hosts.listing().rows.len(),
        digest: hosts::digest_short(hosts.listing(), &hosts.disclosures()),
        database: hosts::database_line(hosts),
        live_database: hosts.is_live_database(),
        handshakes: hosts::handshake_lines(&hosts.handshakes()),
        hosts: hosts.summary_json(),
        reaches: plan
            .visible
            .iter()
            .copied()
            .filter(|&index| reaches(hosts, &loaded.check, index))
            .collect(),
        counts,
        workers,
        backend: backend
            .as_ref()
            .map(|spec| backend_view(spec, provider, &report, args)),
        failures: report
            .failures
            .iter()
            .map(|f| fault(f, loaded, hashes, &report))
            .collect(),
        results: report.results.iter().map(outcome).collect(),
        summary: report_summary(&report),
        simulation: report.simulation,
        escapes,
        warnings: once_each(warnings),
        mutants,
        coverage: args
            .coverage
            .then(|| crate::commands::mutate::coverage_json(loaded, hashes)),
    }
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

// --- Counts under `--filter` --------------------------------------------------

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
        let in_scope = |t: &ply_ty::TestInfo| std_tests || !crate::shipped::is_shipped(&t.module);
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
                store: store.with_upstream(ply_store::Upstream::from_env()),
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

    pub fn scratch() -> Result<Cache, Diagnostic> {
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

// --- What the binding makes of the corpus -------------------------------------

fn reaches(hosts: &Hosts, check: &CheckOutput, index: usize) -> bool {
    check
        .tests
        .get(index)
        .is_some_and(|t| hosts.reaches(&t.footprint))
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
pub fn hosts_escapes(report: &RunReport, check: &CheckOutput, hosts: &Hosts) -> Vec<Diagnostic> {
    if hosts.is_hermetic() {
        return Vec::new();
    }
    report
        .results
        .iter()
        .filter(|r| {
            r.recorded.as_ref().is_some_and(Record::is_written) && reaches(hosts, check, r.index)
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
        .note("the command names the engine before it builds a provider, because selection decides whether building one is worth anything")
        .note("run `ply cache clear`: this run skipped what one engine proved and recorded it as another's")
        .note("this is Ply's fault — the command and `Executor::engine` disagree")
    ]
}

/// An unbuilt backend declines every call, which would make a green run vacuous.
fn unbuilt_backend(provider: Option<&'static dyn ply_eval::Provider>) -> Option<Diagnostic> {
    let unbuilt = provider.map_or(0, ply_eval::Provider::unbuilt);
    if unbuilt == 0 {
        return None;
    }
    Some(
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!(
                "{unbuilt} worker(s) could not build the `{}` backend, and every call they were \
                 offered was declined",
                provider.map_or("", ply_eval::Provider::name)
            ),
        )
        .note(
            "the backend was built once before the run started, so this cannot be a host that has \
             no code generator",
        )
        .note(
            "this is Ply's fault — a run that installs a backend and silently does not have one is \
             green over a seam nothing reached",
        ),
    )
}

// --- What crosses back --------------------------------------------------------

struct Refused {
    diagnostics: Vec<Diagnostic>,
    sources: SourceMap,
}

struct CaseView {
    index: usize,
    key: String,
    name: String,
    module: String,
    nondet: bool,
    hash: Option<String>,
    footprint: String,
    shared: String,
    atoms: Vec<String>,
    region_only: bool,
    seeded: bool,
    isolated: bool,
    reason: &'static str,
    owed: usize,
    group: Option<usize>,
}

struct Found {
    root: String,
    files: Vec<String>,
    sources: SourceMap,
    modules: Vec<(String, String)>,
    incremental: bool,
    phases: crate::driver::Phases,
    declared: usize,
    cases: Vec<CaseView>,
    total: usize,
    selected: usize,
    cached: usize,
    filtered_out: usize,
    groups: Vec<(Vec<usize>, String)>,
    parallelism: Parallelism,
    plan: (String, usize, String, String),
    warnings: Vec<Diagnostic>,
    options: Value,
}

struct Over {
    hermetic: bool,
    label: String,
    operations: usize,
    digest: String,
    database: Option<String>,
    live_database: bool,
    handshakes: Vec<String>,
    hosts: Value,
    reaches: Vec<usize>,
    counts: hosts::Counts,
    workers: usize,
    backend: Option<BackendView>,
    results: Vec<OutcomeView>,
    failures: Vec<FaultView>,
    summary: SummaryView,
    simulation: ply_test::SimSummary,
    escapes: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
    mutants: Option<MutantsView>,
    coverage: Option<Value>,
}

struct BackendView {
    name: String,
    spec: Option<String>,
    fragment: usize,
    offered: u64,
    entered: u64,
    declined: u64,
    converted_in: u64,
    converted_out: u64,
    units: Option<usize>,
    analysis_nanos: Option<u64>,
    codegen_nanos: Option<u64>,
}

struct OutcomeView {
    index: usize,
    name: String,
    hash: Option<String>,
    group: usize,
    duration_us: u128,
    status: &'static str,
    diagnostic: Option<Diagnostic>,
    search: Option<SearchView>,
    cached: Option<bool>,
}

struct SearchView {
    explored: u64,
    exhaustive: bool,
    exhausted: bool,
    naive: Option<(u64, bool, String)>,
    reduction_tenths: Option<i64>,
    steps: u64,
    virtual_time_ns: u64,
    failing_seed: Option<String>,
}

struct SuspectView {
    name: String,
    change: Option<String>,
    ran: Option<bool>,
    depth: Option<usize>,
    culprit: bool,
}

struct FaultView {
    key: String,
    diagnostic: Diagnostic,
    conclusive: bool,
    requested: bool,
    reason: String,
    culprits: Vec<(Vec<String>, Option<Span>)>,
    slice: Option<(bool, bool, Vec<String>)>,
    suspects: Vec<SuspectView>,
    unchanged: bool,
    seed: Option<String>,
    race: Option<(SiteView, SiteView)>,
    replay: Option<String>,
    artifact: Value,
    module: Option<String>,
    test_hash: Option<String>,
    nondet: Option<bool>,
    status: Option<&'static str>,
    declared: Option<Vec<String>>,
    observed: Option<Vec<String>>,
}

struct SiteView {
    task: String,
    definition: Option<String>,
    access: String,
    span: Span,
}

struct MutantsView {
    killed: usize,
    survived: usize,
    skipped: usize,
    budget_spent: bool,
    unreached: Vec<String>,
    survivors: Vec<(String, String, String, Span)>,
    json: Value,
}

struct SummaryView {
    passed: usize,
    failed: usize,
    abandoned: usize,
    cached: usize,
    duration_us: u128,
}

fn report_summary(report: &RunReport) -> SummaryView {
    SummaryView {
        passed: report.passed,
        failed: report.failed,
        abandoned: report.abandoned,
        cached: report.cached,
        duration_us: report.duration.as_micros(),
    }
}

fn found(
    args: &TestArgs,
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: &Plan,
    warnings: Vec<Diagnostic>,
) -> Found {
    let check = &loaded.check;
    let selection = &plan.selection;
    Found {
        root: loaded.root.display().to_string(),
        files: loaded.file_names(),
        sources: loaded.sources.clone(),
        modules: loaded
            .modules()
            .iter()
            .map(|m| (m.name.to_string(), m.path.display().to_string()))
            .collect(),
        incremental: loaded.frontend.incremental,
        phases: loaded.frontend.phases,
        declared: check.tests.len(),
        cases: plan
            .visible
            .iter()
            .filter_map(|&index| {
                let test = check.tests.get(index)?;
                let reason = selection
                    .reason(index)
                    .unwrap_or(ply_test::Reason::Unhashed);
                Some(CaseView {
                    index,
                    key: test.key.to_string(),
                    name: test.name.clone(),
                    module: test.module.to_string(),
                    nondet: test.nondet,
                    hash: hashes.tests.get(index).map(|h| h.to_hex()),
                    footprint: test.footprint.to_string(),
                    shared: ply_test::shared_footprint(&test.footprint).to_string(),
                    atoms: ply_test::shared_footprint(&test.footprint)
                        .atoms()
                        .map(|a| a.to_string())
                        .collect(),
                    region_only: ply_test::contends_only_over_regions(&test.footprint),
                    seeded: ply_test::is_seeded(&test.footprint),
                    isolated: selection
                        .isolation_of(index)
                        .unwrap_or_else(|| Isolation::of(&test.footprint))
                        .is_isolated(),
                    reason: reason.as_str(),
                    owed: selection.plan_for(index).roots.len(),
                    group: selection.group_of(index),
                })
            })
            .collect(),
        total: selection.total,
        selected: selection.to_run.len(),
        cached: selection.cached.len(),
        filtered_out: plan.filtered_out,
        groups: selection
            .groups
            .iter()
            .map(|group| {
                (
                    group.clone(),
                    plan.group_footprint(group, check).to_string(),
                )
            })
            .collect(),
        parallelism: selection.parallelism,
        plan: (
            selection.plan.mode.as_str().to_string(),
            selection.plan.roots.len(),
            selection.plan.budget.to_string(),
            selection.plan.steps.to_string(),
        ),
        warnings,
        options: jsonlit!({
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
        }),
    }
}

fn backend_view(
    spec: &ply_eval::BackendSpec,
    provider: Option<&'static dyn ply_eval::Provider>,
    report: &RunReport,
    args: &TestArgs,
) -> BackendView {
    let offers = provider.map_or_else(Default::default, ply_eval::Provider::offers);
    let compiled = provider.and_then(ply_eval::Provider::compilation);
    BackendView {
        name: provider
            .map_or(spec.kind.as_str(), ply_eval::Provider::name)
            .to_string(),
        spec: args.backend.clone(),
        fragment: provider.map_or(0, ply_eval::Provider::len),
        offered: offers.offered,
        entered: report
            .results
            .iter()
            .filter_map(|r| r.backend)
            .map(|b| b.entries)
            .sum(),
        declined: report
            .results
            .iter()
            .filter_map(|r| r.backend)
            .map(|b| b.declines)
            .sum(),
        converted_in: offers.converted_in,
        converted_out: offers.converted_out,
        units: compiled.map(|c| c.units),
        analysis_nanos: compiled.map(|c| c.analysis_nanos),
        codegen_nanos: compiled.map(|c| c.codegen_nanos),
    }
}

fn status_str(status: Status) -> &'static str {
    match status {
        Status::Passed => "passed",
        Status::Failed => "failed",
        Status::Panicked => "panicked",
        Status::Abandoned => "abandoned",
    }
}

fn outcome(result: &TestResult) -> OutcomeView {
    OutcomeView {
        index: result.index,
        name: result.name.clone(),
        hash: result.hash.map(|h| h.to_hex()),
        group: result.group,
        duration_us: result.duration.as_micros(),
        status: status_str(result.status),
        diagnostic: result.failure.clone(),
        search: result.simulation.as_ref().map(|e| SearchView {
            explored: u64::from(e.explored),
            exhaustive: e.exhaustive,
            exhausted: e.exhausted,
            naive: e
                .naive
                .map(|naive| (u64::from(naive.explored), naive.bounded, naive.to_string())),
            // Tenths, so one division answers both the line and the document.
            reduction_tenths: e.reduction().map(|r| (r * 10.0).round() as i64),
            steps: u64::from(e.steps),
            virtual_time_ns: e.virtual_time,
            failing_seed: e.failure.as_ref().map(|s| s.to_string()),
        }),
        cached: result.recorded.as_ref().map(Record::is_written),
    }
}

fn suspect_view(suspect: &Suspect) -> SuspectView {
    SuspectView {
        name: suspect.name.to_string(),
        change: suspect.change.map(|c| c.as_str().to_string()),
        ran: suspect.ran,
        depth: suspect.depth,
        culprit: suspect.culprit,
    }
}

fn fault(
    failure: &ply_test::Failure,
    loaded: &Loaded,
    hashes: &HashOutput,
    report: &RunReport,
) -> FaultView {
    let check = &loaded.check;
    let index = check.tests.iter().position(|t| t.key == failure.key);
    let test = index.and_then(|i| check.tests.get(i));
    let bisection = &failure.attribution.bisection;
    FaultView {
        key: failure.key.as_str().to_string(),
        diagnostic: failure.diagnostic.clone(),
        conclusive: bisection.is_conclusive(),
        // Silent when no bisection was asked for.
        requested: !matches!(
            bisection.verdict,
            Verdict::NotAttempted(Skipped::NotRequested)
        ),
        reason: bisection.reason.clone(),
        culprits: bisection
            .groups
            .iter()
            .map(|group| {
                (
                    group.iter().map(|n| n.as_str().to_string()).collect(),
                    group
                        .iter()
                        .find_map(|n| check.defs.get(n))
                        .map(|def| def.span),
                )
            })
            .collect(),
        slice: failure.attribution.slice.as_ref().map(|slice| {
            (
                slice.traced,
                slice.reproduced,
                slice
                    .path()
                    .iter()
                    .map(|n| n.as_str().to_string())
                    .collect(),
            )
        }),
        suspects: failure
            .attribution
            .suspects
            .iter()
            .map(suspect_view)
            .collect(),
        unchanged: failure.suspects.is_empty(),
        seed: failure.seed.as_ref().map(|s| s.to_string()),
        race: failure
            .race
            .as_ref()
            .map(|race| (site_view(&race.left), site_view(&race.right))),
        replay: failure.replay(),
        artifact: ply_test::report::failure_json(failure),
        module: test.map(|t| t.module.as_str().to_string()),
        test_hash: index.and_then(|i| hashes.tests.get(i)).map(|h| h.to_hex()),
        nondet: test.map(|t| t.nondet),
        status: index.and_then(|i| {
            report
                .results
                .iter()
                .find(|r| r.index == i)
                .map(|r| status_str(r.status))
        }),
        declared: test.map(|t| atoms(&t.footprint)),
        // Null rather than empty when untraced: unwatched differs from performing nothing.
        observed: failure
            .attribution
            .slice
            .as_ref()
            .filter(|s| s.traced)
            .map(|s| atoms(&s.observed)),
    }
}

fn site_view(site: &ply_eval::RaceSite) -> SiteView {
    SiteView {
        task: site.task.to_string(),
        definition: site.definition.as_ref().map(|d| d.to_string()),
        access: site.access.to_string(),
        span: site.span,
    }
}

fn atoms(footprint: &Footprint) -> Vec<String> {
    ply_ty::Printer::new().atoms(&footprint.0)
}

fn mutants_view(report: &crate::commands::mutate::Report, loaded: &Loaded) -> MutantsView {
    MutantsView {
        killed: report.killed(),
        survived: report.survived(),
        skipped: report.skipped() + report.unresolved(),
        budget_spent: report.budget_spent,
        unreached: report.unreached.iter().map(|n| n.to_string()).collect(),
        survivors: report
            .judged
            .iter()
            .filter(|j| matches!(j.verdict, crate::commands::mutate::Verdict::Survived))
            .map(|j| {
                (
                    j.mutant.definition.as_str().to_string(),
                    j.mutant.from.clone(),
                    j.mutant.to.clone(),
                    j.mutant.span,
                )
            })
            .collect(),
        json: crate::commands::mutate::to_json(report, loaded),
    }
}

/// Line and column rather than byte offsets, for editors.
pub fn location_json(sources: &SourceMap, span: Span) -> Value {
    let Some(file) = sources.get(span.source) else {
        return Value::Null;
    };
    let (line, column) = file.line_col(span.start);
    let (end_line, end_column) = file.line_col(span.end);
    jsonlit!({
        "file": file.path.display().to_string(),
        "line": line,
        "column": column,
        "end_line": end_line,
        "end_column": end_column,
    })
}

// --- The same, as Ply values --------------------------------------------------

fn tally(n: u64) -> PlyValue {
    PlyValue::Int(n as i64)
}

fn micros(n: u128) -> PlyValue {
    PlyValue::Int(n as i64)
}

fn texts(items: &[String]) -> PlyValue {
    strings(items.iter().map(String::as_str))
}

fn opt_texts(items: Option<&[String]>) -> PlyValue {
    option(items.map(texts))
}

fn at_value(span: Span) -> PlyValue {
    record(vec![
        ("module", PlyValue::Int(i64::from(span.source.0))),
        ("start", PlyValue::Int(i64::from(span.start))),
        ("end", PlyValue::Int(i64::from(span.end))),
    ])
}

fn refusal_value(refused: &Refused) -> PlyValue {
    record(vec![
        ("diags", diags_value(&refused.diagnostics)),
        ("places", places_value(&refused.sources)),
    ])
}

fn case_value(case: &CaseView) -> PlyValue {
    record(vec![
        ("index", count(case.index)),
        ("key", PlyValue::str(&case.key)),
        ("name", PlyValue::str(&case.name)),
        ("module", PlyValue::str(&case.module)),
        ("nondet", PlyValue::Bool(case.nondet)),
        ("hash", option(case.hash.as_deref().map(PlyValue::str))),
        ("footprint", PlyValue::str(&case.footprint)),
        ("shared", PlyValue::str(&case.shared)),
        ("atoms", texts(&case.atoms)),
        ("region_only", PlyValue::Bool(case.region_only)),
        ("seeded", PlyValue::Bool(case.seeded)),
        ("isolated", PlyValue::Bool(case.isolated)),
        ("reason", PlyValue::str(case.reason)),
        ("owed", count(case.owed)),
        ("group", option(case.group.map(count))),
    ])
}

fn parallelism_value(p: &Parallelism) -> PlyValue {
    record(vec![
        ("total", count(p.total)),
        ("isolated", count(p.isolated)),
        ("shared", count(p.shared)),
        ("region_contended", count(p.region_contended)),
        ("scheduled", count(p.scheduled)),
        ("groups", count(p.groups)),
        ("shared_groups", count(p.shared_groups)),
    ])
}

fn counts_value(c: &hosts::Counts) -> PlyValue {
    record(vec![
        ("total", count(c.total)),
        ("isolated", count(c.isolated)),
        ("shared", count(c.shared)),
        ("host", count(c.host)),
    ])
}

fn found_value(found: Found) -> PlyValue {
    record(vec![
        ("root", PlyValue::str(&found.root)),
        ("files", texts(&found.files)),
        ("places", places_value(&found.sources)),
        (
            "modules",
            PlyValue::list(
                found
                    .modules
                    .iter()
                    .map(|(name, file)| {
                        record(vec![
                            ("name", PlyValue::str(name)),
                            ("file", PlyValue::str(file)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("incremental", PlyValue::Bool(found.incremental)),
        (
            "phases",
            record(vec![
                ("read_us", micros(found.phases.read.as_micros())),
                ("front_us", micros(found.phases.front.as_micros())),
                ("write_back_us", micros(found.phases.write_back.as_micros())),
            ]),
        ),
        ("declared", count(found.declared)),
        (
            "cases",
            PlyValue::list(found.cases.iter().map(case_value).collect()),
        ),
        ("total", count(found.total)),
        ("selected", count(found.selected)),
        ("cached", count(found.cached)),
        ("filtered_out", count(found.filtered_out)),
        (
            "groups",
            PlyValue::list(
                found
                    .groups
                    .iter()
                    .map(|(tests, footprint)| {
                        record(vec![
                            (
                                "tests",
                                PlyValue::list(tests.iter().map(|&i| count(i)).collect()),
                            ),
                            ("footprint", PlyValue::str(footprint)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("parallelism", parallelism_value(&found.parallelism)),
        (
            "plan",
            record(vec![
                ("mode", PlyValue::str(&found.plan.0)),
                ("seeds", count(found.plan.1)),
                ("budget", PlyValue::str(&found.plan.2)),
                ("steps", PlyValue::str(&found.plan.3)),
            ]),
        ),
        ("warnings", diags_value(&found.warnings)),
        ("options", json(&found.options)),
    ])
}

fn search_value(search: &SearchView) -> PlyValue {
    record(vec![
        ("explored", tally(search.explored)),
        ("exhaustive", PlyValue::Bool(search.exhaustive)),
        ("exhausted", PlyValue::Bool(search.exhausted)),
        (
            "naive",
            option(search.naive.as_ref().map(|(explored, bounded, rendered)| {
                record(vec![
                    ("explored", tally(*explored)),
                    ("bounded", PlyValue::Bool(*bounded)),
                    ("rendered", PlyValue::str(rendered)),
                ])
            })),
        ),
        (
            "reduction_tenths",
            option(search.reduction_tenths.map(PlyValue::Int)),
        ),
        ("steps", tally(search.steps)),
        ("virtual_time_ns", tally(search.virtual_time_ns)),
        (
            "failing_seed",
            option(search.failing_seed.as_deref().map(PlyValue::str)),
        ),
    ])
}

fn outcome_value(o: &OutcomeView) -> PlyValue {
    record(vec![
        ("index", count(o.index)),
        ("name", PlyValue::str(&o.name)),
        ("hash", option(o.hash.as_deref().map(PlyValue::str))),
        ("group", count(o.group)),
        ("duration_us", micros(o.duration_us)),
        ("status", PlyValue::str(o.status)),
        ("diagnostic", option(o.diagnostic.as_ref().map(diag_value))),
        ("search", option(o.search.as_ref().map(search_value))),
        ("cached", option(o.cached.map(PlyValue::Bool))),
    ])
}

fn site_value(site: &SiteView) -> PlyValue {
    record(vec![
        ("task", PlyValue::str(&site.task)),
        (
            "definition",
            option(site.definition.as_deref().map(PlyValue::str)),
        ),
        ("access", PlyValue::str(&site.access)),
        ("at", option(placed(site.span))),
    ])
}

/// A span nothing wrote places nothing.
fn placed(span: Span) -> Option<PlyValue> {
    (span != Span::DUMMY).then(|| at_value(span))
}

fn fault_value(f: &FaultView) -> PlyValue {
    record(vec![
        ("key", PlyValue::str(&f.key)),
        ("diagnostic", diag_value(&f.diagnostic)),
        (
            "bisect",
            record(vec![
                ("conclusive", PlyValue::Bool(f.conclusive)),
                ("requested", PlyValue::Bool(f.requested)),
                ("reason", PlyValue::str(&f.reason)),
                (
                    "culprits",
                    PlyValue::list(
                        f.culprits
                            .iter()
                            .map(|(names, span)| {
                                record(vec![
                                    ("names", texts(names)),
                                    ("at", option(span.and_then(placed))),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
        ),
        (
            "slice",
            option(f.slice.as_ref().map(|(traced, reproduced, path)| {
                record(vec![
                    ("traced", PlyValue::Bool(*traced)),
                    ("reproduced", PlyValue::Bool(*reproduced)),
                    ("path", texts(path)),
                ])
            })),
        ),
        (
            "suspects",
            PlyValue::list(
                f.suspects
                    .iter()
                    .map(|x| {
                        record(vec![
                            ("name", PlyValue::str(&x.name)),
                            ("change", option(x.change.as_deref().map(PlyValue::str))),
                            ("ran", option(x.ran.map(PlyValue::Bool))),
                            ("depth", option(x.depth.map(count))),
                            ("culprit", PlyValue::Bool(x.culprit)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("unchanged", PlyValue::Bool(f.unchanged)),
        ("seed", option(f.seed.as_deref().map(PlyValue::str))),
        (
            "race",
            option(f.race.as_ref().map(|(left, right)| {
                record(vec![
                    ("left", site_value(left)),
                    ("right", site_value(right)),
                ])
            })),
        ),
        ("replay", option(f.replay.as_deref().map(PlyValue::str))),
        ("artifact", json(&f.artifact)),
        ("module", option(f.module.as_deref().map(PlyValue::str))),
        (
            "test_hash",
            option(f.test_hash.as_deref().map(PlyValue::str)),
        ),
        ("nondet", option(f.nondet.map(PlyValue::Bool))),
        ("status", option(f.status.map(PlyValue::str))),
        ("declared", opt_texts(f.declared.as_deref())),
        ("observed", opt_texts(f.observed.as_deref())),
    ])
}

fn ran_value(over: &Over) -> PlyValue {
    record(vec![
        ("hermetic", PlyValue::Bool(over.hermetic)),
        ("label", PlyValue::str(&over.label)),
        ("operations", count(over.operations)),
        ("digest", PlyValue::str(&over.digest)),
        (
            "database",
            option(over.database.as_deref().map(PlyValue::str)),
        ),
        ("live_database", PlyValue::Bool(over.live_database)),
        ("handshakes", texts(&over.handshakes)),
        ("hosts", json(&over.hosts)),
        (
            "reaches",
            PlyValue::list(over.reaches.iter().map(|&i| count(i)).collect()),
        ),
        ("counts", counts_value(&over.counts)),
        ("workers", count(over.workers)),
        (
            "backend",
            option(over.backend.as_ref().map(|b| {
                record(vec![
                    ("name", PlyValue::str(&b.name)),
                    ("spec", option(b.spec.as_deref().map(PlyValue::str))),
                    ("fragment", count(b.fragment)),
                    ("offered", tally(b.offered)),
                    ("entered", tally(b.entered)),
                    ("declined", tally(b.declined)),
                    ("converted_in", tally(b.converted_in)),
                    ("converted_out", tally(b.converted_out)),
                    ("units", option(b.units.map(count))),
                    ("analysis_nanos", option(b.analysis_nanos.map(tally))),
                    ("codegen_nanos", option(b.codegen_nanos.map(tally))),
                ])
            })),
        ),
        (
            "results",
            PlyValue::list(over.results.iter().map(outcome_value).collect()),
        ),
        (
            "failures",
            PlyValue::list(over.failures.iter().map(fault_value).collect()),
        ),
        (
            "summary",
            record(vec![
                ("passed", count(over.summary.passed)),
                ("failed", count(over.summary.failed)),
                ("abandoned", count(over.summary.abandoned)),
                ("cached", count(over.summary.cached)),
                ("duration_us", micros(over.summary.duration_us)),
            ]),
        ),
        (
            "simulation",
            record(vec![
                ("simulated", count(over.simulation.simulated)),
                ("total", count(over.simulation.total)),
                ("seeds", count(over.simulation.seeds)),
                ("interleavings", tally(over.simulation.interleavings)),
                ("exhaustive", count(over.simulation.exhaustive)),
                ("exhausted", count(over.simulation.exhausted)),
                ("failed", count(over.simulation.failed)),
            ]),
        ),
        ("escapes", diags_value(&over.escapes)),
        ("warnings", diags_value(&over.warnings)),
        (
            "mutants",
            option(over.mutants.as_ref().map(|m| {
                record(vec![
                    ("killed", count(m.killed)),
                    ("survived", count(m.survived)),
                    ("skipped", count(m.skipped)),
                    ("budget_spent", PlyValue::Bool(m.budget_spent)),
                    ("unreached", texts(&m.unreached)),
                    (
                        "survivors",
                        PlyValue::list(
                            m.survivors
                                .iter()
                                .map(|(definition, from, to, span)| {
                                    record(vec![
                                        ("definition", PlyValue::str(definition)),
                                        ("from", PlyValue::str(from)),
                                        ("to", PlyValue::str(to)),
                                        ("at", option(placed(*span))),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    ("json", json(&m.json)),
                ])
            })),
        ),
        (
            "coverage",
            option(over.coverage.as_ref().map(|document| {
                let unreached: Vec<String> = document["unreached"]
                    .as_array()
                    .map(|xs| {
                        xs.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                record(vec![
                    (
                        "definitions",
                        count(document["definitions"].as_array().map_or(0, Vec::len)),
                    ),
                    ("unreached", texts(&unreached)),
                    ("json", json(document)),
                ])
            })),
        ),
    ])
}

// --- Small things -------------------------------------------------------------

#[cold]
fn unspawned(e: &std::io::Error) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("the run could not be started on a thread of its own: {e}"),
    )
    .primary(Span::DUMMY, "no test ran")
}

#[cold]
fn unanswered() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the thread this run lives on stopped without answering",
    )
    .note("the program and the thread it drives are written together; this is Ply's fault")
}

#[cold]
fn unstarted(op: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` was performed before the corpus was loaded"),
    )
    .note("the operations are performed in order; this is Ply's fault")
}

#[cold]
fn out_of_step(op: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` was answered with another step's answer"),
    )
    .note("the operations are performed in order; this is Ply's fault")
}

#[cold]
fn unasked(op: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` reached the binding, and `ply test` serves no such operation"),
    )
    .primary(span, "this perform reached `ply test`")
    .note("the effect and its handler are written together; this is Ply's fault")
}

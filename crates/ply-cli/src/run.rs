//! What `ply run` loads, binds, enters and tears down, as the program in `crates/ply-cli/ply`
//! performs it.
//!
//! The front end, the host binding and the entry itself stay here: a front end is not a value a
//! program can hold, a binding holding a connection pool belongs to the thread that drives it,
//! and an entry does not nest on a thread. Which entry runs, what is disclosed and when, what
//! both forms of the report say and the code the run exits with are the program's, in
//! `crates/ply-cli/ply/run.ply`.

use crate::artifact::{self, Artifact};
use crate::cli::RunArgs;
use crate::commands::common::{describe_schema, enter_constant, prover_backend, select_profile};
use crate::config::Configuration;
use crate::hosts::{Hosts, Lent};
use crate::load::Loaded;
use crate::payload::{count, diags_value, json, option, record, strings};
use ply_eval::Value as PlyValue;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_host::process::{Executables, ProcessHost, Sink, Stream};
use ply_host::signal::{self, Shutdown};
use ply_span::{Diagnostic, SourceMap, Span, Symbol, codes};
use ply_ty::{CheckOutput, Front, ModuleName};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

/// The effect `crates/ply-cli/ply/run.ply` declares. It is lent to that one entry and nowhere
/// else: this is the only command that enters a program of somebody else's.
const EFFECT: &str = "runner";

const OPERATIONS: [(&str, &str); 3] = [
    ("loaded", "ply_cli::run::loaded"),
    ("bound", "ply_cli::run::bound"),
    ("entered", "ply_cli::run::entered"),
];

/// A compiled body honours its call bound on the native stack, where unoptimised frames run to
/// kilobytes. Reserved, not committed.
const RUN_STACK: usize = 256 << 20;

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

/// `--json` promises stdout to the one object, so the program's own lines go to stderr instead.
/// Loaded up front so an `--exec` that cannot be started is `E0457` before anything runs.
fn process_host(args: &RunArgs) -> Result<ProcessHost, Diagnostic> {
    let out = if args.json { Stream::Err } else { Stream::Out };
    let executables = Executables::load(&args.exec.exec, Span::DUMMY)?;
    Ok(ProcessHost::new(args.argv.clone(), Sink::Real { out }).executing(executables))
}

/// The load and the artifact are read on the machine's own thread, so the flags it reads are all
/// this side keeps.
pub fn lent(args: &RunArgs) -> Vec<Lent> {
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
        // A tree, a clock, a signal and a database are not functions of program state.
        determinism: Determinism::Nondeterministic,
        // Each step is performed once, in order; nothing here is replayed.
        linearity: Linearity::Repeatable,
        // The answer is in hand when the operation returns: this thread waits for the one the run
        // lives on rather than being handed a token to poll.
        blocking: false,
        secrets: false,
        path,
    }
}

struct Site {
    args: RunArgs,
    /// Started by the first operation and joined by the last.
    machine: Mutex<Option<Machine>>,
}

impl HostHandler for Site {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match (req.op.op.as_str(), req.args) {
            ("loaded", _) => self.loaded()?,
            ("bound", [entry]) => self.bound(entry.as_str(span, "an entry point's name")?)?,
            ("entered", _) => self.entered()?,
            (other, _) => return Err(unasked(other, span)),
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

    fn bound(&self, entry: &str) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("bound"))?;
        machine.ask(Go::Bind(entry.to_string()))?;
        match machine.step()? {
            Step::Bound(disclosed) => Ok(answered((*disclosed).map(disclosed_value))),
            _ => Err(out_of_step("bound")),
        }
    }

    fn entered(&self) -> Result<PlyValue, Diagnostic> {
        let mut held = self.held();
        let step = {
            let machine = held.as_ref().ok_or_else(|| unstarted("entered"))?;
            machine.ask(Go::Enter)?;
            machine.step()?
        };
        // The run is over: the thread it lived on is joined here rather than at the process exit.
        held.take();
        match step {
            Step::Ended(outcome) => Ok(outcome_value(&outcome)),
            _ => Err(out_of_step("entered")),
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

// --- The thread the run lives on ----------------------------------------------

/// What the program asks the machine for next.
enum Go {
    Bind(String),
    Enter,
}

/// What the machine answers with, in the order the operations ask for it. Each answer is boxed:
/// they cross a channel once and are read once, and no step is bigger than another.
enum Step {
    Loaded(Box<Result<Found, Refused>>),
    Bound(Box<Result<Disclosed, Refused>>),
    Ended(Box<Outcome>),
}

/// The thread this run's machine lives on. The `ply` program performing these operations is
/// itself inside an entry, two entries do not nest on one thread, and the binding the entry runs
/// under holds `Rc`s and a connection pool; so the load, the binding, the entry and the teardown
/// all happen here, and only what a report is written from crosses back.
struct Machine {
    go: Option<mpsc::Sender<Go>>,
    steps: mpsc::Receiver<Step>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Machine {
    fn start(args: RunArgs) -> Result<Machine, Diagnostic> {
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
        // stopped short of entering leaves nothing running.
        self.go.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(args: &RunArgs, told: &mpsc::Sender<Step>, asked: &mpsc::Receiver<Go>) {
    let target = match Target::open(args) {
        Ok(target) => target,
        Err(refused) => {
            let _ = told.send(Step::Loaded(Box::new(Err(refused))));
            return;
        }
    };
    let _ = told.send(Step::Loaded(Box::new(Ok(target.found()))));
    let Ok(Go::Bind(entry)) = asked.recv() else {
        return;
    };
    bind(args, &target, &entry, told, asked);
}

// --- What this run runs -------------------------------------------------------

/// A program the front end loaded, or an artifact opened from a file.
enum Target {
    Project(Box<Loaded>),
    Deployed(Box<Deployment>),
}

struct Deployment {
    path: String,
    artifact: Artifact,
    opened: artifact::Opened,
    digest: String,
    /// Whether the embedded unit is one this runtime can enter; a stale one is left aside.
    unit: bool,
    warnings: Vec<Diagnostic>,
}

impl Target {
    /// An artifact runs out of its own verified definitions, not a source tree.
    fn open(args: &RunArgs) -> Result<Target, Refused> {
        if args
            .path
            .extension()
            .is_some_and(|e| e == artifact::EXTENSION)
        {
            return deployment(args).map(|d| Target::Deployed(Box::new(d)));
        }
        let loaded = crate::load::load(&args.path).map_err(|err| Refused {
            diagnostics: err.diagnostics,
            sources: err.sources,
            artifact: None,
        })?;
        match crate::costs::broken_promises(&loaded) {
            Some(err) => Err(Refused {
                diagnostics: err.diagnostics,
                sources: err.sources,
                artifact: None,
            }),
            None => Ok(Target::Project(Box::new(loaded))),
        }
    }

    fn front(&self) -> &Front {
        match self {
            Target::Project(loaded) => &loaded.front,
            Target::Deployed(d) => &d.opened.front,
        }
    }

    fn check(&self) -> &CheckOutput {
        &self.front().check
    }

    /// A closure's positions are in text printed at build time, which no reader wrote.
    fn sources(&self) -> SourceMap {
        match self {
            Target::Project(loaded) => loaded.sources.clone(),
            Target::Deployed(_) => SourceMap::new(),
        }
    }

    fn found(&self) -> Found {
        match self {
            Target::Project(loaded) => Found::Project(Program {
                root: loaded.root.display().to_string(),
                files: loaded.file_names(),
                sources: loaded.sources.clone(),
                mains: mains_of(loaded),
                modules: modules_of(loaded),
            }),
            Target::Deployed(d) => Found::Deployed(Deployed {
                path: d.path.clone(),
                digest: d.digest.clone(),
                entry: d.opened.entry.as_str().to_string(),
                definitions: d.artifact.bodies.len(),
                unit: d.unit,
                warnings: d.warnings.clone(),
            }),
        }
    }

    /// The unit this run evaluates on: an artifact's own as built, else one over the sources.
    fn tier(
        &self,
        args: &RunArgs,
    ) -> Result<Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>, Diagnostic> {
        select_profile(&args.profile)?;
        match self {
            Target::Project(loaded) => prover_backend(args.backend.as_ref(), loaded),
            Target::Deployed(d) => {
                let unit = if d.unit {
                    d.artifact.unit.as_ref()
                } else {
                    None
                };
                artifact::tier(&d.opened, args.backend.as_ref(), unit)
            }
        }
    }
}

fn deployment(args: &RunArgs) -> Result<Deployment, Refused> {
    let about = |diagnostics: Vec<Diagnostic>| Refused {
        diagnostics,
        sources: SourceMap::new(),
        artifact: Some(args.path.display().to_string()),
    };
    let (container, mut warnings) = artifact::read(&args.path).map_err(|d| about(vec![d]))?;
    let opened = artifact::open(&container, &args.path).map_err(about)?;
    let unit = artifact::servable(&container);
    if container.has_unit() && !unit {
        warnings.push(artifact::stale_unit());
    }
    Ok(Deployment {
        path: args.path.display().to_string(),
        digest: container.digest_short(),
        artifact: container,
        opened,
        unit,
        warnings,
    })
}

/// Every definition named `main`, as `crates/ply-cli/ply/entry.ply` reads them. `ply build` with
/// no `--entry` asks the same question of the same list.
pub(crate) fn mains_value(loaded: &Loaded) -> PlyValue {
    named_values(&mains_of(loaded))
}

pub(crate) fn modules_value(loaded: &Loaded) -> PlyValue {
    placed_values(&modules_of(loaded))
}

/// Every non-shipped definition named `main`, which is what the program picks its entry from.
fn mains_of(loaded: &Loaded) -> Vec<Named> {
    loaded
        .entry_points()
        .into_iter()
        .map(|def| Named {
            name: def.name.as_str().to_string(),
            module: def.module.to_string(),
            path: file_of(loaded, &def.module),
            at: At::of(def.span),
        })
        .collect()
}

/// Every module the load read, with the empty position at the end of its file: where the entry
/// point it does not declare would be written.
fn modules_of(loaded: &Loaded) -> Vec<Placed> {
    loaded
        .modules()
        .into_iter()
        .map(|view| {
            let end = loaded
                .sources
                .get(view.info.source)
                .map_or(0, |f| f.text.len() as u32);
            Placed {
                name: view.name.to_string(),
                path: view.path.display().to_string(),
                at: At {
                    module: view.info.source.0,
                    start: end,
                    end,
                },
            }
        })
        .collect()
}

fn file_of(loaded: &Loaded, module: &ModuleName) -> String {
    loaded
        .check
        .modules
        .get(module.as_symbol())
        .map(|m| loaded.path_of(m.source).display().to_string())
        .unwrap_or_else(|| module.to_string())
}

// --- The binding, held while the entry runs -----------------------------------

fn bind(
    args: &RunArgs,
    target: &Target,
    entry: &str,
    told: &mpsc::Sender<Step>,
    asked: &mpsc::Receiver<Go>,
) {
    let refuse = |diagnostics: Vec<Diagnostic>| {
        let _ = told.send(Step::Bound(Box::new(Err(Refused {
            diagnostics,
            sources: target.sources(),
            artifact: None,
        }))));
    };
    // Before anything evaluates; a hermetic run resolves nothing, so no registry can break it.
    let db = match args.db.resolve(args.host) {
        Ok(db) => db,
        Err(diagnostics) => return refuse(diagnostics),
    };
    // What a host answer is checked against, and whether this run needs a database.
    let declared = target
        .check()
        .defs
        .get(&Symbol::new(entry))
        .map(|d| d.footprint.clone());
    // Before the configuration: its schema is entered on this unit.
    let tier = match target.tier(args) {
        Ok(tier) => tier,
        Err(diagnostic) => return refuse(vec![diagnostic]),
    };
    let constant = |name: &str| enter_constant(tier.as_ref().map(|(provider, _)| *provider), name);
    let (configuration, warnings) =
        match Configuration::open(target.check(), args.host, &args.config, &constant) {
            Ok(resolved) => resolved,
            Err(diagnostics) => return refuse(diagnostics),
        };
    // Before the binding, which decides whether `signal` is bound.
    let shutdown = args.host.then(|| Shutdown::new(args.shutdown.bounds()));
    if let Some(shutdown) = &shutdown
        && let Err(diagnostic) = signal::listen(shutdown)
    {
        return refuse(vec![diagnostic]);
    }
    let process = match args.host.then(|| process_host(args)).transpose() {
        Ok(process) => process,
        Err(diagnostic) => return refuse(vec![diagnostic]),
    };
    let mut hosts = match Hosts::open_stopping(
        target.check(),
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
        Err(diagnostics) => return refuse(diagnostics),
    };
    describe_schema(&mut hosts, &constant);
    let _ = told.send(Step::Bound(Box::new(Ok(disclosed(
        args,
        &hosts,
        shutdown.as_ref(),
        warnings,
    )))));
    let Ok(Go::Enter) = asked.recv() else {
        return;
    };
    let outcome = enter(
        args,
        target,
        entry,
        &hosts,
        declared.as_ref(),
        tier,
        shutdown.as_ref(),
    );
    let _ = told.send(Step::Ended(Box::new(outcome)));
}

fn disclosed(
    args: &RunArgs,
    hosts: &Hosts,
    shutdown: Option<&Arc<Shutdown>>,
    warnings: Vec<Diagnostic>,
) -> Disclosed {
    let listing = hosts.listing();
    let facilities = hosts.disclosures();
    Disclosed {
        hermetic: hosts.is_hermetic(),
        operations: listing.rows.len(),
        digest: crate::hosts::digest_short(listing, &facilities),
        config: facilities
            .configuration
            .as_ref()
            .map(|_| hosts.configuration().banner()),
        trace: facilities.observability.as_ref().map(|o| o.banner()),
        database: crate::hosts::database_line(hosts),
        signals: shutdown.map(|s| Signals {
            names: s
                .signals()
                .iter()
                .map(|sig| sig.name().to_string())
                .collect(),
            lead_ms: args.shutdown.drain_lead_ms,
            drain_ms: args.shutdown.drain_ms,
        }),
        warnings,
    }
}

fn enter(
    args: &RunArgs,
    target: &Target,
    entry: &str,
    hosts: &Hosts,
    declared: Option<&ply_ty::ty::Footprint>,
    tier: Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>,
    shutdown: Option<&Arc<Shutdown>>,
) -> Outcome {
    let span = target
        .check()
        .defs
        .get(&Symbol::new(entry))
        .map(|d| d.span)
        .unwrap_or(Span::DUMMY);
    let plan = crate::simulation::run_plan(args.seed.as_ref());
    let compiled = tier.map(|(provider, spec)| provider.attach(&spec));
    // The counters are per thread, and this is the thread the entry runs on.
    ply_eval::rc::reset();
    // The `ply` program performing this is inside a scope that zeroed the thread-local budgets,
    // and the lookup prefers a thread-local to the process value, so the entry's own bounds are
    // set here, on the thread it runs on, and nowhere else.
    let answer = ply_codegen::rt::with_step_budget(args.steps, || {
        ply_codegen::rt::with_time_budget(args.timeout, || {
            evaluate(
                target.front(),
                entry,
                span,
                &plan,
                hosts,
                declared,
                compiled,
            )
        })
    });
    let counters = ply_eval::rc::stats();
    // A cycle among escaped values is never collected, so only this run can report it.
    let cycles = ply_eval::rc::take_cycles();
    // On the machine's own thread, never from a signal handler.
    let report = teardown(hosts, shutdown, args.shutdown.drain_ms);
    let stopping = shutdown.filter(|s| s.stopping()).map(|s| {
        let (listeners, connections, scopes) = s.at_stop();
        Stopped {
            signal: s.signal().map(|sig| sig.name().to_string()),
            listeners,
            connections,
            scopes,
            elapsed_ms: s.elapsed().unwrap_or_default().as_millis() as u64,
        }
    });
    let ended = Outcome {
        exit: hosts.requested_exit(),
        value: None,
        raised: None,
        counters,
        cycles,
        stopping,
        teardown: Teardown {
            lead_ms: args.shutdown.drain_lead_ms,
            drain_ms: args.shutdown.drain_ms,
            transactions_rolled_back: report.as_ref().map_or(0, |r| r.transactions_rolled_back),
            connections_closed: report.as_ref().map_or(0, |r| r.connections_closed.len()),
            spans_abandoned: report.as_ref().map_or(0, |r| r.spans_abandoned),
            problems: report.map_or_else(Vec::new, |r| r.problems),
        },
        trace: hosts.trace_counts(),
        handshakes: if hosts.is_hermetic() {
            Vec::new()
        } else {
            crate::hosts::handshake_lines(&hosts.handshakes())
        },
        hosts: hosts.summary_json(),
        configuration: hosts.configuration().to_json(),
    };
    // The program chose its code and returned no value, so none is carried.
    if ended.exit.is_some() {
        return ended;
    }
    match answer {
        Ok(value) => Outcome {
            value: Some(value.to_string()),
            ..ended
        },
        Err(diagnostic) => Outcome {
            raised: Some(diagnostic),
            ..ended
        },
    }
}

fn evaluate(
    front: &Front,
    entry: &str,
    span: Span,
    plan: &ply_eval::Plan,
    hosts: &Hosts,
    declared: Option<&ply_ty::ty::Footprint>,
    compiled: Option<std::rc::Rc<dyn ply_eval::Compiled>>,
) -> Result<PlyValue, Diagnostic> {
    let mut machine = ply_eval::Machine::new(front);
    if let Some(compiled) = compiled {
        machine.set_compiled(compiled);
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
    machine.call(entry, Vec::new(), span)
}

// --- What crosses back --------------------------------------------------------

struct Refused {
    diagnostics: Vec<Diagnostic>,
    sources: SourceMap,
    artifact: Option<String>,
}

/// Where a definition is written, as a label points at it.
struct At {
    module: u32,
    start: u32,
    end: u32,
}

impl At {
    fn of(span: Span) -> At {
        At {
            module: span.source.0,
            start: span.start,
            end: span.end,
        }
    }
}

struct Named {
    name: String,
    module: String,
    path: String,
    at: At,
}

struct Placed {
    name: String,
    path: String,
    at: At,
}

struct Program {
    root: String,
    files: Vec<String>,
    sources: SourceMap,
    mains: Vec<Named>,
    modules: Vec<Placed>,
}

struct Deployed {
    path: String,
    digest: String,
    entry: String,
    definitions: usize,
    unit: bool,
    warnings: Vec<Diagnostic>,
}

enum Found {
    Project(Program),
    Deployed(Deployed),
}

struct Signals {
    names: Vec<String>,
    lead_ms: u64,
    drain_ms: u64,
}

struct Disclosed {
    hermetic: bool,
    operations: usize,
    digest: String,
    config: Option<String>,
    trace: Option<String>,
    database: Option<String>,
    signals: Option<Signals>,
    warnings: Vec<Diagnostic>,
}

struct Stopped {
    signal: Option<String>,
    listeners: usize,
    connections: usize,
    scopes: usize,
    elapsed_ms: u64,
}

struct Teardown {
    lead_ms: u64,
    drain_ms: u64,
    transactions_rolled_back: usize,
    connections_closed: usize,
    spans_abandoned: usize,
    problems: Vec<String>,
}

struct Outcome {
    exit: Option<i32>,
    value: Option<String>,
    raised: Option<Diagnostic>,
    counters: ply_eval::rc::Stats,
    cycles: Vec<Diagnostic>,
    stopping: Option<Stopped>,
    teardown: Teardown,
    trace: Option<ply_host::trace::Counts>,
    handshakes: Vec<String>,
    hosts: serde_json::Value,
    configuration: serde_json::Value,
}

fn refusal_value(refused: &Refused) -> PlyValue {
    record(vec![
        ("diags", diags_value(&refused.diagnostics)),
        ("places", crate::payload::places_value(&refused.sources)),
        (
            "artifact",
            option(refused.artifact.as_deref().map(PlyValue::str)),
        ),
    ])
}

fn named_values(mains: &[Named]) -> PlyValue {
    PlyValue::list(
        mains
            .iter()
            .map(|m| {
                record(vec![
                    ("name", PlyValue::str(&m.name)),
                    ("module", PlyValue::str(&m.module)),
                    ("path", PlyValue::str(&m.path)),
                    ("at", at_value(&m.at)),
                ])
            })
            .collect(),
    )
}

fn placed_values(modules: &[Placed]) -> PlyValue {
    PlyValue::list(
        modules
            .iter()
            .map(|m| {
                record(vec![
                    ("name", PlyValue::str(&m.name)),
                    ("path", PlyValue::str(&m.path)),
                    ("at", at_value(&m.at)),
                ])
            })
            .collect(),
    )
}

fn at_value(at: &At) -> PlyValue {
    record(vec![
        ("module", PlyValue::Int(i64::from(at.module))),
        ("start", PlyValue::Int(i64::from(at.start))),
        ("end", PlyValue::Int(i64::from(at.end))),
    ])
}

fn found_value(found: Found) -> PlyValue {
    match found {
        Found::Project(p) => crate::payload::ctor(
            "run",
            "Project",
            vec![record(vec![
                ("root", PlyValue::str(&p.root)),
                ("files", strings(p.files.iter().map(String::as_str))),
                ("places", crate::payload::places_value(&p.sources)),
                ("mains", named_values(&p.mains)),
                ("modules", placed_values(&p.modules)),
            ])],
        ),
        Found::Deployed(a) => crate::payload::ctor(
            "run",
            "Deployed",
            vec![record(vec![
                ("path", PlyValue::str(&a.path)),
                ("digest", PlyValue::str(&a.digest)),
                ("entry", PlyValue::str(&a.entry)),
                ("definitions", count(a.definitions)),
                ("unit", PlyValue::Bool(a.unit)),
                ("warnings", diags_value(&a.warnings)),
            ])],
        ),
    }
}

fn disclosed_value(d: Disclosed) -> PlyValue {
    record(vec![
        ("hermetic", PlyValue::Bool(d.hermetic)),
        ("operations", count(d.operations)),
        ("digest", PlyValue::str(&d.digest)),
        ("config", option(d.config.as_deref().map(PlyValue::str))),
        ("trace", option(d.trace.as_deref().map(PlyValue::str))),
        ("database", option(d.database.as_deref().map(PlyValue::str))),
        (
            "signals",
            option(d.signals.as_ref().map(|g| {
                record(vec![
                    ("names", strings(g.names.iter().map(String::as_str))),
                    ("lead_ms", tally(g.lead_ms)),
                    ("drain_ms", tally(g.drain_ms)),
                ])
            })),
        ),
        ("warnings", diags_value(&d.warnings)),
    ])
}

fn outcome_value(o: &Outcome) -> PlyValue {
    record(vec![
        (
            "exit",
            option(o.exit.map(|code| PlyValue::Int(code.into()))),
        ),
        ("value", option(o.value.as_deref().map(PlyValue::str))),
        (
            "raised",
            option(o.raised.as_ref().map(crate::payload::diag_value)),
        ),
        ("counters", counters_value(&o.counters)),
        ("cycles", diags_value(&o.cycles)),
        (
            "stopping",
            option(o.stopping.as_ref().map(|s| {
                record(vec![
                    ("signal", option(s.signal.as_deref().map(PlyValue::str))),
                    ("listeners", count(s.listeners)),
                    ("connections", count(s.connections)),
                    ("scopes", count(s.scopes)),
                    ("elapsed_ms", tally(s.elapsed_ms)),
                ])
            })),
        ),
        ("teardown", teardown_value(&o.teardown)),
        (
            "trace",
            option(o.trace.as_ref().map(|c| {
                record(vec![
                    ("events", tally(c.events)),
                    ("spans", tally(c.spans)),
                    ("abandoned", tally(c.abandoned)),
                    ("flushed", PlyValue::Bool(c.flushed)),
                ])
            })),
        ),
        (
            "handshakes",
            strings(o.handshakes.iter().map(String::as_str)),
        ),
        ("hosts", json(&o.hosts)),
        ("configuration", json(&o.configuration)),
    ])
}

fn counters_value(stats: &ply_eval::rc::Stats) -> PlyValue {
    record(vec![
        ("updates", tally(stats.updates)),
        ("updates_in_place", tally(stats.updates_in_place)),
        (
            "in_place",
            option(
                stats
                    .in_place()
                    .and_then(ply_eval::Decimal::from_f64_retain)
                    .map(PlyValue::Decimal),
            ),
        ),
        ("cycles", tally(stats.cycles)),
    ])
}

fn teardown_value(w: &Teardown) -> PlyValue {
    record(vec![
        ("lead_ms", tally(w.lead_ms)),
        ("drain_ms", tally(w.drain_ms)),
        (
            "transactions_rolled_back",
            count(w.transactions_rolled_back),
        ),
        ("connections_closed", count(w.connections_closed)),
        ("spans_abandoned", count(w.spans_abandoned)),
        ("problems", strings(w.problems.iter().map(String::as_str))),
    ])
}

fn tally(n: u64) -> PlyValue {
    PlyValue::Int(n as i64)
}

// --- Small things -------------------------------------------------------------

#[cold]
fn unspawned(e: &std::io::Error) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("the run could not be started on a thread of its own: {e}"),
    )
    .primary(Span::DUMMY, "nothing was entered")
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
        format!("`{EFFECT}.{op}` was performed before the run was loaded"),
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
        format!("`{EFFECT}.{op}` reached the binding, and `ply run` serves no such operation"),
    )
    .primary(span, "this perform reached `ply run`")
    .note("the effect and its handler are written together; this is Ply's fault")
}

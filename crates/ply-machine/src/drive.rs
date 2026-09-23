//! What a machine does between `load` and `enter`, driven by the ops in `crate`: open the target
//! (a project the front end loaded, or an artifact opened from a file), bind the hosts the entry
//! may reach, enter it, and tear the binding down. A front end is not a value a program can hold,
//! a binding holding a connection pool belongs to the thread that drives it, and an entry does
//! not nest on a thread — so all of it lives here, on the machine's own thread, and the answers
//! cross as values.

use crate::artifact::{self, Artifact};
use crate::config::Configuration;
use crate::hosts::Hosts;
use crate::load::Loaded;
use crate::payload::{count, diags_value, json, option, record, strings};
use crate::support::{describe_schema, enter_constant, prover_backend, select_profile};
use ply_eval::Value as PlyValue;
use ply_host::process::{Executables, ProcessHost, Sink, Stream};
use ply_host::signal::{self, Shutdown};
use ply_span::{Diagnostic, SourceMap, Span, Symbol, codes};
use ply_ty::{CheckOutput, Front, ModuleName};
use std::sync::Arc;

/// What the machine is configured with when it is lent, as plain data: the shell's parsed flags
/// convert into this.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// The target's argument vector: what its `process.args` answers.
    pub argv: Vec<String>,
    /// `--json` promises stdout to the one object, so the program's own lines go to stderr.
    pub json: bool,
    pub steps: i64,
    pub timeout: u64,
    pub seed: Option<ply_eval::Seed>,
    pub host: bool,
    pub tls: crate::options::TlsOptions,
    pub fs: Vec<ply_host::fs::RootSpec>,
    pub exec: Vec<ply_host::process::ExecSpec>,
    pub db: crate::db::DbOptions,
    pub config: crate::config::ConfigOptions,
    pub trace: crate::trace::TraceOptions,
    pub shutdown: crate::options::ShutdownOptions,
    pub backend: Option<String>,
    pub profile: String,
    /// A project load that reads and writes the store; off for a plain `run`.
    pub cache: bool,
}

impl Default for RunOptions {
    fn default() -> RunOptions {
        RunOptions {
            argv: Vec::new(),
            json: false,
            steps: 0,
            timeout: 0,
            seed: None,
            host: false,
            tls: crate::options::TlsOptions::default(),
            fs: Vec::new(),
            exec: Vec::new(),
            db: crate::db::DbOptions::default(),
            config: crate::config::ConfigOptions::default(),
            trace: crate::trace::TraceOptions::default(),
            shutdown: crate::options::ShutdownOptions::default(),
            backend: None,
            profile: "development".to_string(),
            cache: false,
        }
    }
}

// --- The target ---------------------------------------------------------------

/// A program the front end loaded, or an artifact opened from a file.
pub enum Target {
    Project(Box<Loaded>),
    Deployed(Box<Deployment>),
}

pub struct Deployment {
    path: String,
    artifact: Artifact,
    opened: artifact::Opened,
    digest: String,
    /// Whether the embedded unit is one this runtime can enter; a stale one is left aside.
    unit: bool,
    warnings: Vec<Diagnostic>,
}

/// A load's refusal: the diagnostics, the sources they point into, and the file if one was named.
pub struct Refused {
    pub diagnostics: Vec<Diagnostic>,
    pub sources: SourceMap,
    pub artifact: Option<String>,
}

impl Target {
    /// An artifact runs out of its own verified definitions, not a source tree.
    pub fn open(
        path: &std::path::Path,
        cache: bool,
    ) -> Result<(Target, Option<ply_store::Store>), Refused> {
        if path.extension().is_some_and(|e| e == artifact::EXTENSION) {
            return deployment(path).map(|d| (Target::Deployed(Box::new(d)), None));
        }
        let loaded = if cache {
            let mut store = ply_store::Store::open(path)
                .map(|store| store.with_upstream(ply_store::Upstream::from_env()))
                .map_err(|e| Refused {
                    diagnostics: vec![unopened(path, &e)],
                    sources: SourceMap::new(),
                    artifact: None,
                })?;
            crate::driver::load_incremental(path, &mut store)
                .map(|loaded| (loaded, Some(store)))
                .map_err(|err| Refused {
                    diagnostics: err.diagnostics,
                    sources: err.sources,
                    artifact: None,
                })?
        } else {
            (
                crate::load::load(path).map_err(|err| Refused {
                    diagnostics: err.diagnostics,
                    sources: err.sources,
                    artifact: None,
                })?,
                None,
            )
        };
        let (loaded, store) = loaded;
        match crate::costs::broken_promises(&loaded) {
            Some(err) => Err(Refused {
                diagnostics: err.diagnostics,
                sources: err.sources,
                artifact: None,
            }),
            None => Ok((Target::Project(Box::new(loaded)), store)),
        }
    }

    pub fn front(&self) -> &Front {
        match self {
            Target::Project(loaded) => &loaded.front,
            Target::Deployed(d) => &d.opened.front,
        }
    }

    pub fn check(&self) -> &CheckOutput {
        &self.front().check
    }

    /// A closure's positions are in text printed at build time, which no reader wrote.
    pub fn sources(&self) -> SourceMap {
        match self {
            Target::Project(loaded) => loaded.sources.clone(),
            Target::Deployed(_) => SourceMap::new(),
        }
    }

    /// What `load` answers with, as plain data: a `Value` is not `Send`, so the value is built
    /// on the calling thread from this.
    pub fn found_data(&self) -> FoundData {
        match self {
            Target::Project(loaded) => FoundData::Project {
                root: loaded.root.display().to_string(),
                files: loaded.file_names(),
                places: loaded
                    .sources
                    .files()
                    .iter()
                    .map(|f| (f.path.display().to_string(), f.text.as_bytes().to_vec()))
                    .collect(),
                mains: mains_of(loaded),
                modules: modules_of(loaded),
            },
            Target::Deployed(d) => FoundData::Deployed {
                path: d.path.clone(),
                digest: d.digest.clone(),
                entry: d.opened.entry.as_str().to_string(),
                definitions: d.artifact.bodies.len(),
                unit: d.unit,
                warnings: d.warnings.clone(),
            },
        }
    }

    /// The unit this run evaluates on: an artifact's own as built, else one over the sources.
    fn tier(
        &self,
        options: &RunOptions,
    ) -> Result<Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>, Diagnostic> {
        select_profile(&options.profile)?;
        match self {
            Target::Project(loaded) => prover_backend(options.backend.as_ref(), loaded),
            Target::Deployed(d) => {
                let unit = if d.unit {
                    d.artifact.unit.as_ref()
                } else {
                    None
                };
                artifact::tier(&d.opened, options.backend.as_ref(), unit)
            }
        }
    }
}

fn unopened(path: &std::path::Path, e: &dyn std::fmt::Display) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("the store under `{}` did not open: {e:#}", path.display()),
    )
    .primary(Span::DUMMY, "the load was not read and nothing ran")
}

fn deployment(path: &std::path::Path) -> Result<Deployment, Refused> {
    let about = |diagnostics: Vec<Diagnostic>| Refused {
        diagnostics,
        sources: SourceMap::new(),
        artifact: Some(path.display().to_string()),
    };
    let (container, mut warnings) = artifact::read(path).map_err(|d| about(vec![d]))?;
    let opened = artifact::open(&container, path).map_err(about)?;
    let unit = artifact::servable(&container);
    if container.has_unit() && !unit {
        warnings.push(artifact::stale_unit());
    }
    Ok(Deployment {
        path: path.display().to_string(),
        digest: container.digest_short(),
        artifact: container,
        opened,
        unit,
        warnings,
    })
}

// --- The drive ---------------------------------------------------------------

/// What the entry's binding disclosed, held while it runs.
pub struct Bound {
    hosts: Hosts,
    declared: Option<ply_ty::ty::Footprint>,
    tier: Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>,
    shutdown: Option<Arc<Shutdown>>,
}

/// The machine's state on its own thread: the target, the store it was loaded over when it has
/// one, and the binding once `bound` made it.
pub struct Drive {
    options: RunOptions,
    target: Target,
    store: Option<ply_store::Store>,
    bound: Option<(String, Bound)>,
}

impl Drive {
    /// Load the target at `path`; the answer a `load` op hands back.
    pub fn open(options: RunOptions, path: &std::path::Path) -> Result<Drive, Refused> {
        let cache = options.cache;
        let (target, store) = Target::open(path, cache)?;
        Ok(Drive {
            options,
            target,
            store,
            bound: None,
        })
    }

    /// What `load` answers with, as plain data for the calling thread to value-ify.
    pub fn found_data(&self) -> FoundData {
        self.target.found_data()
    }

    /// The load again, for a tree that moved; artifacts are read fresh from their file.
    pub fn reload(&mut self) -> Result<(), Refused> {
        let path = std::path::PathBuf::from(match &self.target {
            Target::Project(loaded) => loaded.root.display().to_string(),
            Target::Deployed(d) => d.path.clone(),
        });
        let cache = self.options.cache;
        let (target, store) = Target::open(&path, cache)?;
        self.target = target;
        self.store = store;
        self.bound = None;
        Ok(())
    }

    /// Bind the hosts `entry` may reach; the disclosure a `bound` op hands back. Before the
    /// entry runs, so a run that fails to bind never started.
    pub fn bound(&mut self, entry: &str) -> Result<Disclosed, Refused> {
        let options = &self.options;
        let target = &self.target;
        let refuse = |diagnostics: Vec<Diagnostic>| Refused {
            diagnostics,
            sources: target.sources(),
            artifact: None,
        };
        // Before anything evaluates; a hermetic run resolves nothing, so no registry can break it.
        let db = match options.db.resolve(options.host) {
            Ok(db) => db,
            Err(diagnostics) => return Err(refuse(diagnostics)),
        };
        // What a host answer is checked against, and whether this run needs a database.
        let declared = target
            .check()
            .defs
            .get(&Symbol::new(entry))
            .map(|d| d.footprint.clone());
        // Before the configuration: its schema is entered on this unit.
        let tier = match target.tier(options) {
            Ok(tier) => tier,
            Err(diagnostic) => return Err(refuse(vec![diagnostic])),
        };
        let constant =
            |name: &str| enter_constant(tier.as_ref().map(|(provider, _)| *provider), name);
        let (configuration, warnings) =
            match Configuration::open(target.check(), options.host, &options.config, &constant) {
                Ok(resolved) => resolved,
                Err(diagnostics) => return Err(refuse(diagnostics)),
            };
        // Before the binding, which decides whether `signal` is bound.
        let shutdown = options
            .host
            .then(|| Shutdown::new(options.shutdown.bounds()));
        if let Some(shutdown) = &shutdown
            && let Err(diagnostic) = signal::listen(shutdown)
        {
            return Err(refuse(vec![diagnostic]));
        }
        let process = match options.host.then(|| process_host(options)).transpose() {
            Ok(process) => process,
            Err(diagnostic) => return Err(refuse(vec![diagnostic])),
        };
        let mut hosts = match Hosts::open_stopping(
            target.check(),
            options.host,
            &options.tls,
            &options.fs,
            db,
            configuration,
            &options.trace,
            declared.as_ref(),
            shutdown.clone(),
            process,
            Vec::new(),
        ) {
            Ok(hosts) => hosts,
            Err(diagnostics) => return Err(refuse(diagnostics)),
        };
        describe_schema(&mut hosts, &constant);
        let disclosed = disclosed(options, &hosts, shutdown.as_ref(), warnings);
        self.bound = Some((
            entry.to_string(),
            Bound {
                hosts,
                declared,
                tier,
                shutdown,
            },
        ));
        Ok(disclosed)
    }

    /// Enter the bound entry and tear the binding down; the answer an `enter` op hands back.
    pub fn enter(&mut self) -> Outcome {
        let Some((entry, bound)) = self.bound.take() else {
            return Outcome::unentered();
        };
        let options = &self.options;
        let target = &self.target;
        let span = target
            .check()
            .defs
            .get(&Symbol::new(&entry))
            .map(|d| d.span)
            .unwrap_or(Span::DUMMY);
        let plan = crate::simulation::run_plan(options.seed.as_ref());
        let compiled = bound.tier.map(|(provider, spec)| provider.attach(&spec));
        // The counters are per thread, and this is the thread the entry runs on.
        ply_eval::rc::reset();
        // The `ply` program performing this is inside a scope that zeroed the thread-local
        // budgets, and the lookup prefers a thread-local to the process value, so the entry's own
        // bounds are set here, on the thread it runs on, and nowhere else.
        let answer = ply_codegen::rt::with_step_budget(options.steps, || {
            ply_codegen::rt::with_time_budget(options.timeout, || {
                evaluate(
                    target.front(),
                    &entry,
                    span,
                    &plan,
                    &bound.hosts,
                    bound.declared.as_ref(),
                    compiled,
                )
            })
        });
        let counters = ply_eval::rc::stats();
        // A cycle among escaped values is never collected, so only this run can report it.
        let cycles = ply_eval::rc::take_cycles();
        // On the machine's own thread, never from a signal handler.
        let report = teardown(
            &bound.hosts,
            bound.shutdown.as_ref(),
            options.shutdown.drain_ms,
        );
        let stopping = bound.shutdown.filter(|s| s.stopping()).map(|s| {
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
            exit: bound.hosts.requested_exit(),
            value: None,
            raised: None,
            counters,
            cycles,
            stopping,
            teardown: Teardown {
                lead_ms: options.shutdown.drain_lead_ms,
                drain_ms: options.shutdown.drain_ms,
                transactions_rolled_back: report.as_ref().map_or(0, |r| r.transactions_rolled_back),
                connections_closed: report.as_ref().map_or(0, |r| r.connections_closed.len()),
                spans_abandoned: report.as_ref().map_or(0, |r| r.spans_abandoned),
                problems: report.map_or_else(Vec::new, |r| r.problems),
            },
            trace: bound.hosts.trace_counts(),
            handshakes: if bound.hosts.is_hermetic() {
                Vec::new()
            } else {
                crate::hosts::handshake_lines(&bound.hosts.handshakes())
            },
            hosts: bound.hosts.summary_json(),
            configuration: bound.hosts.configuration().to_json(),
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
}

/// Rolls back every open transaction, closes spans `Abandoned`, flushes the sink, closes the pool.
pub fn teardown(
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

pub const TEARDOWN_FLOOR_MS: u64 = 1_000;

/// `--json` promises stdout to the one object, so the program's own lines go to stderr instead.
/// Loaded up front so an `--exec` that cannot be started is `E0457` before anything runs.
fn process_host(options: &RunOptions) -> Result<ProcessHost, Diagnostic> {
    let out = if options.json {
        Stream::Err
    } else {
        Stream::Out
    };
    let executables = Executables::load(&options.exec, Span::DUMMY)?;
    Ok(ProcessHost::new(options.argv.clone(), Sink::Real { out }).executing(executables))
}

fn disclosed(
    options: &RunOptions,
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
            lead_ms: options.shutdown.drain_lead_ms,
            drain_ms: options.shutdown.drain_ms,
        }),
        warnings,
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
    // Exploration is a test-time activity; a run takes the one interleaving its seed names.
    ply_test::sim::seed_run(&mut machine, &plan.seeds()[0], plan.steps);
    machine.call(entry, Vec::new(), span)
}

// --- The values that cross --------------------------------------------------------

/// Where a definition is written, as a label points at it.
pub struct At {
    pub module: u32,
    pub start: u32,
    pub end: u32,
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

pub struct Named {
    pub name: String,
    pub module: String,
    pub path: String,
    pub at: At,
}

pub struct Placed {
    pub name: String,
    pub path: String,
    at: At,
}

/// The load's answer as plain data: what `machine.Project`/`machine.Deployed` carry.
pub enum FoundData {
    Project {
        root: String,
        files: Vec<String>,
        places: Vec<(String, Vec<u8>)>,
        mains: Vec<Named>,
        modules: Vec<Placed>,
    },
    Deployed {
        path: String,
        digest: String,
        entry: String,
        definitions: usize,
        unit: bool,
        warnings: Vec<Diagnostic>,
    },
}

/// [`FoundData`] as the value the program reads it as. Called on the calling thread.
pub fn found_value(found: &FoundData) -> PlyValue {
    match found {
        FoundData::Project {
            root,
            files,
            places,
            mains,
            modules,
        } => crate::payload::ctor(
            "machine",
            "Project",
            vec![record(vec![
                ("root", PlyValue::str(root)),
                ("files", strings(files.iter().map(String::as_str))),
                ("places", places_value(places)),
                ("mains", named_values(mains)),
                ("modules", placed_values(modules)),
            ])],
        ),
        FoundData::Deployed {
            path,
            digest,
            entry,
            definitions,
            unit,
            warnings,
        } => crate::payload::ctor(
            "machine",
            "Deployed",
            vec![record(vec![
                ("path", PlyValue::str(path)),
                ("digest", PlyValue::str(digest)),
                ("entry", PlyValue::str(entry)),
                ("definitions", count(*definitions)),
                ("unit", PlyValue::Bool(*unit)),
                ("warnings", diags_value(warnings)),
            ])],
        ),
    }
}

fn places_value(places: &[(String, Vec<u8>)]) -> PlyValue {
    PlyValue::list(
        places
            .iter()
            .map(|(path, text)| {
                record(vec![
                    ("path", PlyValue::str(path)),
                    ("text", PlyValue::bytes(text)),
                ])
            })
            .collect(),
    )
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

/// Every definition named `main`, as the program's `entry` module reads them.
pub fn mains_value(loaded: &Loaded) -> PlyValue {
    named_values(&mains_of(loaded))
}

pub fn modules_value(loaded: &Loaded) -> PlyValue {
    placed_values(&modules_of(loaded))
}

fn file_of(loaded: &Loaded, module: &ModuleName) -> String {
    loaded
        .check
        .modules
        .get(module.as_symbol())
        .map(|m| loaded.path_of(m.source).display().to_string())
        .unwrap_or_else(|| module.to_string())
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

pub struct Signals {
    names: Vec<String>,
    lead_ms: u64,
    drain_ms: u64,
}

pub struct Disclosed {
    hermetic: bool,
    operations: usize,
    digest: String,
    config: Option<String>,
    trace: Option<String>,
    database: Option<String>,
    signals: Option<Signals>,
    warnings: Vec<Diagnostic>,
}

pub struct Stopped {
    signal: Option<String>,
    listeners: usize,
    connections: usize,
    scopes: usize,
    elapsed_ms: u64,
}

pub struct Teardown {
    lead_ms: u64,
    drain_ms: u64,
    transactions_rolled_back: usize,
    connections_closed: usize,
    spans_abandoned: usize,
    problems: Vec<String>,
}

pub struct Outcome {
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

impl Outcome {
    /// `enter` before `bound`: nothing ran, which the caller's own order made impossible to say.
    fn unentered() -> Outcome {
        Outcome {
            exit: None,
            value: None,
            raised: Some(
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    "`machine.enter` was performed before `machine.bound`",
                )
                .note("the operations are performed in order; this is Ply's fault"),
            ),
            counters: ply_eval::rc::Stats::default(),
            cycles: Vec::new(),
            stopping: None,
            teardown: Teardown {
                lead_ms: 0,
                drain_ms: 0,
                transactions_rolled_back: 0,
                connections_closed: 0,
                spans_abandoned: 0,
                problems: Vec::new(),
            },
            trace: None,
            handshakes: Vec::new(),
            hosts: serde_json::Value::Null,
            configuration: serde_json::Value::Null,
        }
    }
}

pub fn refusal_value(refused: &Refused) -> PlyValue {
    record(vec![
        ("diags", diags_value(&refused.diagnostics)),
        ("places", crate::payload::places_value(&refused.sources)),
        (
            "artifact",
            option(refused.artifact.as_deref().map(PlyValue::str)),
        ),
    ])
}

pub fn disclosed_value(d: &Disclosed) -> PlyValue {
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

pub fn outcome_value(o: &Outcome) -> PlyValue {
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

// --- The options the program parses -----------------------------------------------

/// The options record as `machine.ply` declares it, read field by field. The program validated
/// the line already, so a bad value here is an internal error.
pub fn run_options_of(v: &PlyValue, span: Span) -> Result<RunOptions, Diagnostic> {
    let get = |name: &str| -> Result<&PlyValue, Diagnostic> {
        match v {
            PlyValue::Record(fields) => fields
                .iter()
                .find(|(k, _)| k.as_str() == name)
                .map(|(_, value)| value)
                .ok_or_else(|| missing(name, span)),
            _ => Err(missing(name, span)),
        }
    };
    let bool_at = |name: &str| get(name).and_then(|v| v.as_bool(span, name));
    let int_at = |name: &str| get(name).and_then(|v| v.as_int(span, name));
    let str_at = |name: &str| get(name).and_then(|v| v.as_str(span, name).map(str::to_string));
    let opt_str = |name: &str| -> Result<Option<String>, Diagnostic> {
        match get(name)? {
            PlyValue::Ctor { name, args } if name.as_str() == "Some" => Ok(args
                .first()
                .map(|v| v.as_str(span, "a value").map(str::to_string))
                .transpose()?),
            PlyValue::Ctor { name, .. } if name.as_str() == "None" => Ok(None),
            other => Err(shape(other, span)),
        }
    };
    let str_list = |name: &str| -> Result<Vec<String>, Diagnostic> {
        get(name)?
            .as_list(span, name)?
            .iter()
            .map(|v| v.as_str(span, "an entry").map(str::to_string))
            .collect::<Result<Vec<String>, Diagnostic>>()
    };
    let named_list = |name: &str| -> Result<Vec<(String, String)>, Diagnostic> {
        let mut out = Vec::new();
        for item in get(name)?.as_list(span, name)?.iter() {
            let name = field_of(item, "name", span)?
                .as_str(span, "a name")?
                .to_string();
            let path = field_of(item, "path", span)?
                .as_str(span, "a path")?
                .to_string();
            out.push((name, path));
        }
        Ok(out)
    };
    let cred_list = |name: &str| -> Result<Vec<(String, String, String)>, Diagnostic> {
        let mut out = Vec::new();
        for item in get(name)?.as_list(span, name)?.iter() {
            let name = field_of(item, "name", span)?
                .as_str(span, "a name")?
                .to_string();
            let cert = field_of(item, "cert", span)?
                .as_str(span, "a certificate")?
                .to_string();
            let key = field_of(item, "key", span)?
                .as_str(span, "a key")?
                .to_string();
            out.push((name, cert, key));
        }
        Ok(out)
    };
    let seed = match opt_str("seed")? {
        Some(text) => Some(ply_eval::Seed::parse(&text).ok_or_else(|| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("`{text}` was handed to the machine as a seed but does not parse"),
            )
            .primary(span, "the program validates seeds; this is Ply's fault")
        })?),
        None => None,
    };
    let tls = cred_list("tls")?;
    let db_v = get("db")?;
    let config_v = get("config")?;
    let trace_v = get("trace")?;
    Ok(RunOptions {
        argv: str_list("argv")?,
        json: false,
        steps: int_at("steps")?,
        timeout: int_at("timeout")? as u64,
        seed,
        host: bool_at("host")?,
        tls: crate::options::TlsOptions {
            tls: tls
                .iter()
                .map(|(name, cert, key)| ply_host::tls::CredentialSpec {
                    name: name.clone(),
                    certificate: std::path::PathBuf::from(cert),
                    key: std::path::PathBuf::from(key),
                })
                .collect(),
            trust: str_list("trust")?
                .into_iter()
                .map(std::path::PathBuf::from)
                .collect(),
        },
        fs: named_list("fs")?
            .into_iter()
            .map(|(name, path)| ply_host::fs::RootSpec {
                name,
                path: std::path::PathBuf::from(path),
            })
            .collect(),
        exec: named_list("exec")?
            .into_iter()
            .map(|(name, path)| ply_host::process::ExecSpec {
                name,
                path: std::path::PathBuf::from(path),
            })
            .collect(),
        db: crate::db::DbOptions {
            url: opt_str_at(db_v, "url", span)?,
            pool: opt_int_at(db_v, "pool", span)?.map(|n| n as u32),
            acquire_ms: opt_int_at(db_v, "acquire_ms", span)?.map(|n| n as u64),
            statement_ms: opt_int_at(db_v, "statement_ms", span)?.map(|n| n as u64),
            idle_txn_ms: opt_int_at(db_v, "idle_txn_ms", span)?.map(|n| n as u64),
            connect_ms: opt_int_at(db_v, "connect_ms", span)?.map(|n| n as u64),
            statement_cache: opt_int_at(db_v, "statement_cache", span)?.map(|n| n as u32),
            schema: opt_str_at(db_v, "schema", span)?,
        },
        config: crate::config::ConfigOptions {
            set: str_list_at(config_v, "set", span)?,
            files: str_list_at(config_v, "files", span)?
                .into_iter()
                .map(std::path::PathBuf::from)
                .collect(),
            schema: opt_str_at(config_v, "schema", span)?,
        },
        trace: crate::trace::TraceOptions {
            sink: match field_of(trace_v, "sink", span)?.as_str(span, "the trace sink")? {
                "text" => crate::trace::SinkArg::Text,
                "off" => crate::trace::SinkArg::Off,
                _ => crate::trace::SinkArg::Json,
            },
            level: match field_of(trace_v, "level", span)?.as_str(span, "the trace level")? {
                "debug" => crate::trace::LevelArg::Debug,
                "warn" => crate::trace::LevelArg::Warn,
                "error" => crate::trace::LevelArg::Error,
                _ => crate::trace::LevelArg::Info,
            },
        },
        shutdown: crate::options::ShutdownOptions {
            drain_ms: int_at("drain_ms")? as u64,
            drain_lead_ms: int_at("drain_lead_ms")? as u64,
        },
        backend: opt_str("backend")?,
        profile: str_at("profile")?,
        cache: bool_at("cache")?,
    })
}

fn field_of<'a>(value: &'a PlyValue, name: &str, span: Span) -> Result<&'a PlyValue, Diagnostic> {
    match value {
        PlyValue::Record(fields) => fields
            .iter()
            .find(|(k, _)| k.as_str() == name)
            .map(|(_, value)| value)
            .ok_or_else(|| missing(name, span)),
        _ => Err(missing(name, span)),
    }
}

fn opt_str_at(value: &PlyValue, name: &str, span: Span) -> Result<Option<String>, Diagnostic> {
    match field_of(value, name, span)? {
        PlyValue::Ctor { name, args } if name.as_str() == "Some" => Ok(args
            .first()
            .map(|v| v.as_str(span, "a value").map(str::to_string))
            .transpose()?),
        PlyValue::Ctor { name, .. } if name.as_str() == "None" => Ok(None),
        other => Err(shape(other, span)),
    }
}

fn opt_int_at(value: &PlyValue, name: &str, span: Span) -> Result<Option<i64>, Diagnostic> {
    match field_of(value, name, span)? {
        PlyValue::Ctor { name, args } if name.as_str() == "Some" => Ok(args
            .first()
            .map(|v| v.as_int(span, "a number"))
            .transpose()?),
        PlyValue::Ctor { name, .. } if name.as_str() == "None" => Ok(None),
        other => Err(shape(other, span)),
    }
}

fn str_list_at(value: &PlyValue, name: &str, span: Span) -> Result<Vec<String>, Diagnostic> {
    field_of(value, name, span)?
        .as_list(span, name)?
        .iter()
        .map(|v| v.as_str(span, "an entry").map(str::to_string))
        .collect::<Result<Vec<String>, Diagnostic>>()
}

fn missing(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the options record has no `{name}`"),
    )
    .primary(
        span,
        "the program and the machine agree on the record; this is Ply's fault",
    )
}

fn shape(value: &PlyValue, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "the machine read a {} where an Option was expected",
            value.type_name()
        ),
    )
    .primary(
        span,
        "the program and the machine agree on the record; this is Ply's fault",
    )
}

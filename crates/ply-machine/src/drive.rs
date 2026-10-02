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
use crate::support::{enter_constant, prover_backend, select_profile};
use ply_eval::{
    CheckOutput, Diagnostic, Ended, Front, ModuleName, SourceMap, Span, Symbol, Value as PlyValue,
    codes,
};
use ply_host::process::{Executables, ProcessHost, Sink, Stream};
use ply_host::signal::{self, Shutdown};
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;

/// What the machine is configured with when it is lent, as plain data: the options record the
/// program parsed converts into this.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// The front end the CLI ran. A run without one is refused rather than loading again: the
    /// CLI walks the tree and runs the compiler, and this side reads the answer.
    pub front: Option<crate::driver::HandedFront>,
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
    pub config: crate::config::ConfigOptions,
    pub trace: crate::trace::TraceOptions,
    pub shutdown: crate::options::ShutdownOptions,
    pub profile: String,
    /// The privileged families `--allow` granted the program being run, by name. What may drive
    /// a machine is a decision with a name, and one the program has to have declared.
    pub allow: Vec<String>,
}

impl Default for RunOptions {
    fn default() -> RunOptions {
        RunOptions {
            front: None,
            argv: Vec::new(),
            json: false,
            steps: 0,
            timeout: 0,
            seed: None,
            host: false,
            tls: crate::options::TlsOptions::default(),
            fs: Vec::new(),
            exec: Vec::new(),
            config: crate::config::ConfigOptions::default(),
            trace: crate::trace::TraceOptions::default(),
            shutdown: crate::options::ShutdownOptions::default(),
            profile: "development".to_string(),
            allow: Vec::new(),
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
        front: Option<&crate::driver::HandedFront>,
    ) -> Result<Target, Refused> {
        if path.extension().is_some_and(|e| e == artifact::EXTENSION) {
            return deployment(path).map(|d| Target::Deployed(Box::new(d)));
        }
        // A library is a package to depend on, not a program: reading it as sources would report a
        // container as text that is not UTF-8.
        if path
            .extension()
            .is_some_and(|e| e == artifact::LIBRARY_EXTENSION)
        {
            return Err(Refused {
                diagnostics: vec![
                    Diagnostic::error(
                        codes::ARTIFACT_INVALID,
                        format!("`{}` is a library, and nothing to run", path.display()),
                    )
                    .primary(Span::DUMMY, "not a program")
                    .note("a `.plyz` is a package: declare it in a `ply.pkg` and depend on it")
                    .note("`ply build` writes a program's artifact as a `.plyx`"),
                ],
                sources: SourceMap::new(),
                artifact: None,
            });
        }
        // A front end handed over is the CLI's own load, read here rather than repeated. A load with
        // none is a *program* loading a program of its own, at a root it chose while running: nobody
        // could have handed one, so the compiler is lent for that load and that load only.
        let loaded = match front {
            Some(front) => crate::driver::load_over_front(path, front),
            None => crate::load::load(path),
        }
        .map_err(|err| Refused {
            diagnostics: err.diagnostics,
            sources: err.sources,
            artifact: None,
        })?;
        // The program that ran a front end it hands over checked its `reuse fn` promises with it.
        if front.is_some() {
            return Ok(Target::Project(Box::new(loaded)));
        }
        match crate::costs::broken_promises(&loaded) {
            Some(err) => Err(Refused {
                diagnostics: err.diagnostics,
                sources: err.sources,
                artifact: None,
            }),
            None => Ok(Target::Project(Box::new(loaded))),
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
    fn tier(&self, options: &RunOptions) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
        select_profile(&options.profile)?;
        match self {
            Target::Project(loaded) => prover_backend(loaded),
            Target::Deployed(d) => {
                let unit = if d.unit {
                    d.artifact.unit.as_ref()
                } else {
                    None
                };
                artifact::tier(&d.opened, unit)
            }
        }
    }
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
    declared: Option<ply_eval::Footprint>,
    tier: &'static dyn ply_eval::Provider,
    /// The tier, attached once and entered any number of times: building it per call would put
    /// the build in every measurement the call is asked for.
    compiled: RefCell<Option<std::rc::Rc<dyn ply_eval::Compiled>>>,
    shutdown: Option<Arc<Shutdown>>,
}

impl Bound {
    /// This binding's compiled tier, built on the first call that wants it.
    fn compiled(&self) -> std::rc::Rc<dyn ply_eval::Compiled> {
        let mut slot = self.compiled.borrow_mut();
        slot.get_or_insert_with(|| self.tier.attach()).clone()
    }
}

/// What one call, or one entry, measured: the calls the runtime counted, the wall clock it took,
/// and the reuse counters it left behind.
#[derive(Default)]
pub struct Measured {
    pub steps: u64,
    pub micros: u64,
    pub counters: ply_eval::rc::Stats,
}

/// The machine's state on its own thread: the target, and the binding once `bound` made it.
pub struct Drive {
    options: RunOptions,
    target: Target,
    bound: Option<(String, Bound)>,
    /// What the calls since the last `accounting` read measured, reset by that read.
    accounting: Measured,
}

impl Drive {
    /// Load the target at `path`; the answer a `load` op hands back.
    pub fn open(options: RunOptions, path: &std::path::Path) -> Result<Drive, Refused> {
        let target = Target::open(path, options.front.as_ref())?;
        Ok(Drive {
            options,
            target,
            bound: None,
            accounting: Measured::default(),
        })
    }

    /// What `load` answers with, as plain data for the calling thread to value-ify.
    pub fn found_data(&self) -> FoundData {
        self.target.found_data()
    }

    /// The load again, for a tree that moved; artifacts are read fresh from their file.
    ///
    /// The front end comes with the asking: the tree moved, so the one the load was handed is the
    /// tree as it *was*, and whoever noticed the move re-ran the compiler for this one.
    pub fn reload(&mut self, front: &crate::driver::HandedFront) -> Result<(), Refused> {
        let path = std::path::PathBuf::from(match &self.target {
            Target::Project(loaded) => loaded.root.display().to_string(),
            Target::Deployed(d) => d.path.clone(),
        });
        self.target = Target::open(&path, Some(front))?;
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
        let constant = |name: &str| enter_constant(Some(tier), name);
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
        let lent = match crate::policy::granted(target.check(), &options.allow) {
            Ok(lent) => lent,
            Err(diagnostic) => return Err(refuse(vec![diagnostic])),
        };
        let hosts = match Hosts::open_stopping(
            target.check(),
            options.host,
            &options.tls,
            &options.fs,
            configuration,
            &options.trace,
            shutdown.clone(),
            process,
            lent,
        ) {
            Ok(hosts) => hosts,
            Err(diagnostics) => return Err(refuse(diagnostics)),
        };
        let disclosed = disclosed(options, &hosts, shutdown.as_ref(), warnings);
        self.bound = Some((
            entry.to_string(),
            Bound {
                hosts,
                declared,
                tier,
                compiled: RefCell::new(None),
                shutdown,
            },
        ));
        Ok(disclosed)
    }

    /// Enter one definition with arguments, the way `call` asks: the value back, or what it
    /// raised, and what the entry ended with. The binding stays up, so a load may be called any
    /// number of times.
    pub fn call(&mut self, name: &str, args: Vec<ply_eval::Plain>) -> Ended<ply_eval::Plain> {
        let options = &self.options;
        let target = &self.target;
        let span = target
            .check()
            .defs
            .get(&Symbol::new(name))
            .map(|d| d.span)
            .unwrap_or(Span::DUMMY);
        let Some((_, bound)) = self.bound.as_ref() else {
            return Ended::refused(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "`machine.call` before `machine.bound`: nothing is bound to call into".to_string(),
            ));
        };
        let args: Vec<PlyValue> = match args
            .into_iter()
            .map(|a| {
                a.into_value().map_err(|why| {
                    Diagnostic::error(
                        codes::RUNTIME_ERROR,
                        format!("`machine.call` was handed {why}"),
                    )
                    .primary(span, "an argument crosses as data")
                })
            })
            .collect()
        {
            Ok(args) => args,
            Err(refused) => return Ended::refused(refused),
        };
        let seed = options.seed.clone().unwrap_or_default();
        let compiled = bound.compiled();
        ply_eval::rc::reset();
        let started = Instant::now();
        let ended = ply_codegen::rt::with_step_budget(options.steps, || {
            ply_codegen::rt::with_time_budget(options.timeout, || {
                evaluate(
                    target.front(),
                    Call { name, args },
                    span,
                    &seed,
                    &bound.hosts,
                    bound.declared.as_ref(),
                    compiled.clone(),
                )
            })
        });
        // A call that raised still did the work its accounting counts.
        note_measurement(&mut self.accounting, &compiled, started);
        ended.map(|answer| answer.map(|value| ply_eval::Plain::of(&value)))
    }

    /// What the calls since the last read measured, and the read resets it.
    pub fn accounting(&mut self) -> Measured {
        std::mem::take(&mut self.accounting)
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
        let seed = options.seed.clone().unwrap_or_default();
        let compiled = bound.compiled();
        // The counters are per thread, and this is the thread the entry runs on.
        ply_eval::rc::reset();
        let started = Instant::now();
        // The `ply` program performing this is inside a scope that zeroed the thread-local
        // budgets, so the entry's own bounds are set here, on the thread it runs on.
        let ended = ply_codegen::rt::with_step_budget(options.steps, || {
            ply_codegen::rt::with_time_budget(options.timeout, || {
                evaluate(
                    target.front(),
                    Call {
                        name: &entry,
                        args: Vec::new(),
                    },
                    span,
                    &seed,
                    &bound.hosts,
                    bound.declared.as_ref(),
                    compiled.clone(),
                )
            })
        });
        note_measurement(&mut self.accounting, &compiled, started);
        let (answer, warnings) = ended.into_parts();
        let counters = ply_eval::rc::stats();
        // A cycle among escaped values is never collected, so only this run can report it.
        let cycles = ply_eval::rc::take_cycles();
        // On the machine's own thread, never from a signal handler.
        let report = teardown(&bound.hosts);
        let stopping = bound.shutdown.filter(|s| s.stopping()).map(|s| {
            let (listeners, connections) = s.at_stop();
            Stopped {
                signal: s.signal().map(|sig| sig.name().to_string()),
                listeners,
                connections,
                elapsed_ms: s.elapsed().unwrap_or_default().as_millis() as u64,
            }
        });
        let ended = Outcome {
            exit: bound.hosts.requested_exit(),
            value: None,
            raised: None,
            counters,
            cycles,
            warnings,
            stopping,
            teardown: Teardown {
                lead_ms: options.shutdown.drain_lead_ms,
                drain_ms: options.shutdown.drain_ms,
                spans_left_open: report.map_or(0, |r| r.spans_left_open),
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
                value: Some(ply_eval::Plain::shown(&value)),
                ..ended
            },
            Err(diagnostic) => Outcome {
                raised: Some(diagnostic),
                ..ended
            },
        }
    }
}

/// Ends the children still running and flushes the sink; each entry closed its own spans as it
/// ended.
pub fn teardown(hosts: &Hosts) -> Option<ply_eval::ShutdownReport> {
    hosts.runtime().map(|rt| rt.shutdown())
}

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

/// Files one entry's measurement under the accounting an `accounting` read will answer with.
fn note_measurement(
    accounting: &mut Measured,
    compiled: &std::rc::Rc<dyn ply_eval::Compiled>,
    started: Instant,
) {
    accounting.steps += compiled.steps();
    accounting.micros += u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let seen = ply_eval::rc::stats();
    accounting.counters.updates += seen.updates;
    accounting.counters.updates_in_place += seen.updates_in_place;
    accounting.counters.elements_copied += seen.elements_copied;
    accounting.counters.cycles += seen.cycles;
}

/// The definition a call enters: its program-wide name and the arguments it takes.
pub struct Call<'a> {
    pub name: &'a str,
    pub args: Vec<PlyValue>,
}

fn evaluate(
    front: &Front,
    call: Call<'_>,
    span: Span,
    seed: &ply_eval::Seed,
    hosts: &Hosts,
    declared: Option<&ply_eval::Footprint>,
    compiled: std::rc::Rc<dyn ply_eval::Compiled>,
) -> Ended<PlyValue> {
    let mut machine = match ply_eval::Machine::new(front, compiled) {
        Ok(machine) => machine,
        Err(refused) => return Ended::refused(refused),
    };
    machine.set_host_binding(hosts.binding());
    if let Some(runtime) = hosts.runtime_factory() {
        machine.set_host_runtime(runtime);
    }
    if let Some(declared) = declared {
        machine.set_declared_footprint(declared.clone());
    }
    // Exploration is a test-time activity; a run takes the one interleaving its seed names.
    machine.set_seed(seed.clone(), ply_eval::sim::DEFAULT_STEPS);
    machine
        .call(call.name, call.args, span)
        .map(|answer| answer.map_err(|d| place_the_unplaced(d, call.name)))
}

/// A raise with no place says what failed and not what was running, so it names the entry point.
pub fn place_the_unplaced(mut d: Diagnostic, entry: &str) -> Diagnostic {
    let placed = d.labels.iter().any(|l| l.span != Span::DUMMY);
    if !placed {
        d = d.note(format!(
            "this raise has no place in the source: it happened while `{entry}` was running, in \
             code with no stored call site (a library definition rather than a body the program \
             wrote, usually). If a `ply` source you just edited is not the source this run was \
             compiled from, that is the first thing to check."
        ));
    }
    d
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
pub fn found_value(found: &FoundData, module: &str) -> PlyValue {
    match found {
        FoundData::Project {
            root,
            files,
            places,
            mains,
            modules,
        } => crate::payload::ctor(
            module,
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
            module,
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
    signals: Option<Signals>,
    warnings: Vec<Diagnostic>,
}

pub struct Stopped {
    signal: Option<String>,
    listeners: usize,
    connections: usize,
    elapsed_ms: u64,
}

pub struct Teardown {
    lead_ms: u64,
    drain_ms: u64,
    spans_left_open: usize,
}

pub struct Outcome {
    exit: Option<i32>,
    value: Option<ply_eval::Plain>,
    raised: Option<Diagnostic>,
    counters: ply_eval::rc::Stats,
    cycles: Vec<Diagnostic>,
    /// What the entry ended with, such as the spans it left open.
    warnings: Vec<Diagnostic>,
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
            warnings: Vec::new(),
            stopping: None,
            teardown: Teardown {
                lead_ms: 0,
                drain_ms: 0,
                spans_left_open: 0,
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
        (
            "value",
            option(o.value.as_ref().map(crate::payload::plain_value)),
        ),
        (
            "raised",
            option(o.raised.as_ref().map(crate::payload::raised_value)),
        ),
        ("counters", counters_value(&o.counters)),
        ("cycles", diags_value(&o.cycles)),
        ("warnings", diags_value(&o.warnings)),
        (
            "stopping",
            option(o.stopping.as_ref().map(|s| {
                record(vec![
                    ("signal", option(s.signal.as_deref().map(PlyValue::str))),
                    ("listeners", count(s.listeners)),
                    ("connections", count(s.connections)),
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

/// What a `call` answers with: the value or what it raised, and what the entry ended with.
pub fn called_value(called: Ended<ply_eval::Plain>) -> PlyValue {
    let (answer, warnings) = called.into_parts();
    record(vec![
        (
            "answer",
            match answer {
                Ok(plain) => PlyValue::ctor("Ok", vec![crate::payload::plain_value(&plain)]),
                Err(d) => PlyValue::ctor("Err", vec![crate::payload::raised_value(&d)]),
            },
        ),
        ("warnings", diags_value(&warnings)),
    ])
}

/// What an `accounting` read answers with.
pub fn accounting_value(m: &Measured) -> PlyValue {
    record(vec![
        ("steps", tally(m.steps)),
        ("micros", tally(m.micros)),
        ("counters", counters_value(&m.counters)),
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
        ("spans_left_open", count(w.spans_left_open)),
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
                .ok_or_else(|| crate::payload::missing(name, span)),
            _ => Err(crate::payload::missing(name, span)),
        }
    };
    let bool_at = |name: &str| get(name).and_then(|v| v.as_bool(span, name));
    let int_at = |name: &str| get(name).and_then(|v| v.as_int(span, name));
    let str_at = |name: &str| get(name).and_then(|v| v.as_str(span, name).map(str::to_string));
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
            let name = crate::payload::field_of(item, "name", span)?
                .as_str(span, "a name")?
                .to_string();
            let path = crate::payload::field_of(item, "path", span)?
                .as_str(span, "a path")?
                .to_string();
            out.push((name, path));
        }
        Ok(out)
    };
    let cred_list = |name: &str| -> Result<Vec<(String, String, String)>, Diagnostic> {
        let mut out = Vec::new();
        for item in get(name)?.as_list(span, name)?.iter() {
            let name = crate::payload::field_of(item, "name", span)?
                .as_str(span, "a name")?
                .to_string();
            let cert = crate::payload::field_of(item, "cert", span)?
                .as_str(span, "a certificate")?
                .to_string();
            let key = crate::payload::field_of(item, "key", span)?
                .as_str(span, "a key")?
                .to_string();
            out.push((name, cert, key));
        }
        Ok(out)
    };
    let seed = match crate::payload::option_of(get("seed")?, "a seed", span)? {
        Some(seed) => Some(crate::recording::seed_of(seed, span)?),
        None => None,
    };
    let tls = cred_list("tls")?;
    let config_v = get("config")?;
    let trace_v = get("trace")?;
    Ok(RunOptions {
        front: None,
        argv: str_list("argv")?,
        allow: str_list("allow")?,
        json: bool_at("json")?,
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
        config: crate::config::ConfigOptions {
            set: crate::payload::str_list_at(config_v, "set", span)?,
            files: crate::payload::str_list_at(config_v, "files", span)?
                .into_iter()
                .map(std::path::PathBuf::from)
                .collect(),
            schema: crate::payload::opt_str_at(config_v, "schema", span)?,
        },
        trace: crate::trace::TraceOptions {
            sink: match crate::payload::field_of(trace_v, "sink", span)?
                .as_str(span, "the trace sink")?
            {
                "text" => crate::trace::SinkArg::Text,
                "off" => crate::trace::SinkArg::Off,
                _ => crate::trace::SinkArg::Json,
            },
            level: match crate::payload::field_of(trace_v, "level", span)?
                .as_str(span, "the trace level")?
            {
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
        profile: str_at("profile")?,
    })
}

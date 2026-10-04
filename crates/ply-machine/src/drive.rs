//! What a machine does between `load` and `enter`, driven by the ops in `crate`: open the target
//! (a project the front end loaded, or an artifact opened from a file), bind the hosts the entry
//! may reach, enter it, and tear the binding down. A front end is not a value a program can hold,
//! a binding holding a connection pool belongs to the thread that drives it, and an entry does
//! not nest on a thread — so all of it lives here, on the machine's own thread, and the answers
//! cross as values.

use crate::config::Configuration;
use crate::hosts::Hosts;
use crate::load::Loaded;
use crate::payload::{count, diags_value, json, option, record, strings};
use crate::support::{select_profile, unit_of};
use ply_eval::{
    Analysis, CheckOutput, DefHash, Diagnostic, Ended, SourceMap, Span, Symbol, Value as PlyValue,
    codes,
};
use ply_host::process::{Executables, OutputSink, ProcessHost, Stream};
use ply_host::signal::{self, Shutdown};
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;

/// What the machine is configured with when it is lent, as plain data: the options record the
/// program parsed converts into this.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// The front end the CLI ran and the C of the unit it emitted from it: the CLI walks the tree,
    /// runs the compiler and emits, and this side reads the answer and compiles the C.
    pub front: Option<crate::driver::LoadedAnalysis>,
    pub unit: Option<Vec<u8>>,
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
    pub trace: crate::trace::TraceOptions,
    pub shutdown: crate::options::ShutdownOptions,
    pub profile: String,
    /// The privileged families `--allow` granted the program being run, by name. What may drive
    /// a machine is a decision with a name, and one the program has to have declared.
    pub allow: Vec<String>,
    /// No host, clock, load from disk or measurement.
    pub hermetic: bool,
}

impl Default for RunOptions {
    fn default() -> RunOptions {
        RunOptions {
            front: None,
            unit: None,
            argv: Vec::new(),
            json: false,
            steps: 0,
            timeout: 0,
            seed: None,
            host: false,
            tls: crate::options::TlsOptions::default(),
            fs: Vec::new(),
            exec: Vec::new(),
            trace: crate::trace::TraceOptions::default(),
            shutdown: crate::options::ShutdownOptions::default(),
            profile: "development".to_string(),
            allow: Vec::new(),
            hermetic: false,
        }
    }
}

// --- The target ---------------------------------------------------------------

/// A program the front end loaded, with the C of the unit the program emitted for it, or the
/// program an artifact's runnable holds.
pub struct Target {
    loaded: Box<Loaded>,
    unit: Vec<u8>,
    /// Opened from an artifact, whose positions are in text printed at build time, which no
    /// reader wrote.
    deployed: bool,
}

/// A load's refusal: the diagnostics, the sources they point into, and the file if one was named.
pub struct Refused {
    pub diagnostics: Vec<Diagnostic>,
    pub sources: SourceMap,
    pub artifact: Option<String>,
}

/// A module of an artifact's closure, as its caller read it out.
pub struct Closed {
    pub path: String,
    pub text: String,
}

/// A name an artifact holds, and the hash of what it names.
pub struct Hashed {
    pub name: String,
    pub hash: DefHash,
}

impl Target {
    /// The program the caller loaded and emitted: its front end, read here rather than repeated,
    /// and the C of its unit, compiled here when the run first enters it.
    pub fn open(
        path: &std::path::Path,
        front: Option<&crate::driver::LoadedAnalysis>,
        unit: Option<&[u8]>,
        hermetic: bool,
    ) -> Result<Target, Refused> {
        let (Some(front), Some(unit)) = (front, unit) else {
            return Err(unemitted(path));
        };
        let loaded = if hermetic {
            crate::driver::load_over_analysis_in(crate::load::tidy(path), front)
        } else {
            crate::driver::load_over_analysis(path, front)
        }
        .map_err(|err| Refused {
            diagnostics: err.diagnostics,
            sources: err.sources,
            artifact: None,
        })?;
        Ok(Target {
            loaded: Box::new(loaded),
            unit: unit.to_vec(),
            deployed: false,
        })
    }

    /// The program the artifact at `path` holds as its runnable, held to the parts of the artifact
    /// its caller read and checked: the entry its namespace names, the closure it prints, and the
    /// hash it files each name under.
    pub fn deployed(
        path: &std::path::Path,
        runnable: &[u8],
        entry: &str,
        closure: &[Closed],
        names: &[Hashed],
    ) -> Result<Target, Refused> {
        let about = |message: String| Refused {
            diagnostics: vec![
                Diagnostic::error(codes::ARTIFACT_INVALID, message)
                    .primary(Span::DUMMY, format!("in `{}`", path.display()))
                    .note("rebuild it with `ply build`, or transfer the file again"),
            ],
            sources: SourceMap::new(),
            artifact: Some(path.display().to_string()),
        };
        let held = crate::runnable::decode(runnable)
            .map_err(|why| about(format!("the artifact's runnable does not read: {why}")))?;
        if held.entry != entry {
            return Err(about(format!(
                "the artifact's runnable enters `{}`, and its namespace names `{entry}`",
                held.entry
            )));
        }
        let printed = held
            .front
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.text.as_str()));
        if closure.len() > held.front.files.len()
            || !closure
                .iter()
                .map(|c| (c.path.as_str(), c.text.as_str()))
                .eq(printed.take(closure.len()))
        {
            return Err(about(
                "the artifact's closure is not the program its runnable holds".to_string(),
            ));
        }
        let loaded =
            crate::driver::load_over_analysis_taken(crate::load::project_root(path), held.front)
                .map_err(|err| Refused {
                    diagnostics: err.diagnostics,
                    sources: SourceMap::new(),
                    artifact: Some(path.display().to_string()),
                })?;
        let hashes = &loaded.front.hashes;
        if let Some(named) = names.iter().find(|n| {
            let symbol = Symbol::new(&n.name);
            hashes.defs.get(&symbol) != Some(&n.hash) && hashes.decls.get(&symbol) != Some(&n.hash)
        }) {
            return Err(about(format!(
                "the closure does not define `{}` as the artifact does",
                named.name
            )));
        }
        let known: std::collections::BTreeSet<(&str, DefHash)> =
            names.iter().map(|n| (n.name.as_str(), n.hash)).collect();
        let printed = |name: &Symbol| {
            name.as_str()
                .rsplit_once('.')
                .is_some_and(|(module, _)| !crate::shelf::is_shipped_name(module))
        };
        if let Some((name, _)) = hashes
            .defs
            .iter()
            .chain(&hashes.decls)
            .find(|(name, hash)| printed(name) && !known.contains(&(name.as_str(), **hash)))
        {
            return Err(about(format!(
                "the closure defines `{name}`, which the artifact's namespace does not name"
            )));
        }
        Ok(Target {
            loaded: Box::new(loaded),
            unit: held.unit.into_bytes(),
            deployed: true,
        })
    }

    pub fn front(&self) -> &Analysis {
        &self.loaded.front
    }

    pub fn check(&self) -> &CheckOutput {
        &self.front().check
    }

    pub fn sources(&self) -> SourceMap {
        if self.deployed {
            SourceMap::new()
        } else {
            self.loaded.sources.clone()
        }
    }

    /// What `load` answers with, as plain data: a `Value` is not `Send`, so the value is built
    /// on the calling thread from this.
    pub fn found_data(&self) -> FoundData {
        let loaded = &self.loaded;
        FoundData {
            root: loaded.root.display().to_string(),
            files: loaded.file_names(),
            places: loaded
                .sources
                .files()
                .iter()
                .map(|f| (f.path.display().to_string(), f.text.as_bytes().to_vec()))
                .collect(),
        }
    }

    /// The unit this run evaluates on, compiled from the C the target came with.
    fn provider(
        &self,
        options: &RunOptions,
    ) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
        // Which C compiler ran is no part of what a hermetic run answers.
        if !options.hermetic {
            select_profile(&options.profile)?;
        }
        unit_of(&self.loaded.front, &self.unit)
    }
}

fn unemitted(path: &std::path::Path) -> Refused {
    Refused {
        diagnostics: vec![
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!(
                    "`{}` came with no front end or no unit, and a machine loads what it is handed",
                    path.display()
                ),
            )
            .note("the program that drives a machine runs the compiler and emits; this side compiles the C it is handed"),
        ],
        sources: SourceMap::new(),
        artifact: None,
    }
}

// --- The drive ---------------------------------------------------------------

/// What the entry's binding disclosed, held while it runs.
pub struct Bound {
    hosts: Hosts,
    declared: Option<ply_eval::Footprint>,
    provider: &'static dyn ply_eval::Provider,
    /// The provider's backend, attached once and entered any number of times: building it per call
    /// would put the build in every measurement the call is asked for.
    compiled: RefCell<Option<std::rc::Rc<dyn ply_eval::Compiled>>>,
    shutdown: Option<Arc<Shutdown>>,
}

impl Bound {
    /// This binding's C backend, built on the first call that wants it.
    fn compiled(&self) -> std::rc::Rc<dyn ply_eval::Compiled> {
        let mut slot = self.compiled.borrow_mut();
        slot.get_or_insert_with(|| self.provider.attach()).clone()
    }
}

/// What one call, or one entry, measured: the calls the runtime counted, the wall clock it took,
/// and the reuse counters it left behind.
#[derive(Default)]
pub struct Measured {
    pub steps: u64,
    pub micros: u64,
    pub counters: ply_eval::rc::RcStats,
}

/// The machine's state on its own thread: the target, and the binding once `bound` made it.
pub struct Drive {
    options: RunOptions,
    target: Target,
    /// The target's compiled unit, built once for the schema and the binding alike.
    provider: Option<&'static dyn ply_eval::Provider>,
    bound: Option<(String, Bound)>,
    /// What the calls since the last `accounting` read measured, reset by that read.
    accounting: Measured,
}

impl Drive {
    /// Load the target at `path`; the answer a `load` op hands back.
    pub fn open(options: RunOptions, path: &std::path::Path) -> Result<Drive, Refused> {
        let target = Target::open(
            path,
            options.front.as_ref(),
            options.unit.as_deref(),
            options.hermetic,
        )?;
        Ok(Drive::over(options, target))
    }

    /// The program an artifact holds, from the parts of it its caller read and checked.
    pub fn open_deployed(
        options: RunOptions,
        path: &std::path::Path,
        runnable: &[u8],
        entry: &str,
        closure: &[Closed],
        names: &[Hashed],
    ) -> Result<Drive, Refused> {
        let target = Target::deployed(path, runnable, entry, closure, names)?;
        Ok(Drive::over(options, target))
    }

    fn over(options: RunOptions, target: Target) -> Drive {
        Drive {
            options,
            target,
            provider: None,
            bound: None,
            accounting: Measured::default(),
        }
    }

    /// What `load` answers with, as plain data for the calling thread to value-ify.
    pub fn found_data(&self) -> FoundData {
        self.target.found_data()
    }

    /// The load again, for a tree that moved; artifacts are read fresh from their file.
    ///
    /// The front end comes with the asking: the tree moved, so the one the load was handed is the
    /// tree as it *was*, and whoever noticed the move re-ran the compiler for this one.
    pub fn reload(
        &mut self,
        front: &crate::driver::LoadedAnalysis,
        unit: &[u8],
    ) -> Result<(), Refused> {
        let path = self.target.loaded.root.clone();
        self.target = Target::open(&path, Some(front), Some(unit), self.options.hermetic)?;
        self.provider = None;
        self.bound = None;
        Ok(())
    }

    fn provider(&mut self) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
        if let Some(provider) = self.provider {
            return Ok(provider);
        }
        let provider = self.target.provider(&self.options)?;
        self.provider = Some(provider);
        Ok(provider)
    }

    /// The value of the definition `--config-schema` names, entered on the target's own unit.
    pub fn schema(&mut self, name: &str) -> Result<ply_eval::Plain, Diagnostic> {
        let provider = self.provider()?;
        crate::config::schema_of(self.target.check(), Some(provider), name)
    }

    /// Bind the hosts `entry` may reach, answering `config` as the program resolved it; the
    /// disclosure a `bound` op hands back. Before the entry runs, so a run that fails to bind never
    /// started.
    pub fn bound(
        &mut self,
        entry: &str,
        configuration: Configuration,
    ) -> Result<Disclosed, Refused> {
        let provider = self.provider();
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
        let provider = match provider {
            Ok(provider) => provider,
            Err(diagnostic) => return Err(refuse(vec![diagnostic])),
        };
        if options.hermetic && options.host {
            return Err(refuse(vec![hermetic_host()]));
        }
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
        let disclosed = disclosed(options, &hosts, shutdown.as_ref());
        self.bound = Some((
            entry.to_string(),
            Bound {
                hosts,
                declared,
                provider,
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
        note_measurement(&mut self.accounting, &compiled, started, options.hermetic);
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
        note_measurement(&mut self.accounting, &compiled, started, options.hermetic);
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
    Ok(ProcessHost::new(options.argv.clone(), OutputSink::Real { out }).executing(executables))
}

fn disclosed(options: &RunOptions, hosts: &Hosts, shutdown: Option<&Arc<Shutdown>>) -> Disclosed {
    let listing = hosts.listing();
    let facilities = hosts.disclosures();
    Disclosed {
        hermetic: hosts.is_hermetic(),
        operations: listing.rows.len(),
        digest: crate::hosts::digest_short(listing, &facilities),
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
    }
}

/// Files one entry's measurement under the accounting an `accounting` read will answer with.
fn note_measurement(
    accounting: &mut Measured,
    compiled: &std::rc::Rc<dyn ply_eval::Compiled>,
    started: Instant,
    hermetic: bool,
) {
    accounting.steps += compiled.steps();
    if !hermetic {
        accounting.micros += u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    }
    let seen = ply_eval::rc::stats();
    accounting.counters.updates += seen.updates;
    accounting.counters.updates_in_place += seen.updates_in_place;
    accounting.counters.elements_copied += seen.elements_copied;
    accounting.counters.cycles += seen.cycles;
}

#[cold]
fn hermetic_host() -> Diagnostic {
    Diagnostic::error(
        codes::CAPABILITY_UNDECLARED,
        "`hermetic_machine` binds no host, and the run asked for `--host`",
    )
    .note("a program that drives a run reaching the host performs `machine`, and is `test/nondet`")
}

/// The definition a call enters: its program-wide name and the arguments it takes.
pub struct Call<'a> {
    pub name: &'a str,
    pub args: Vec<PlyValue>,
}

fn evaluate(
    front: &Analysis,
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

/// The load's answer as plain data: what `machine.Read` carries.
pub struct FoundData {
    pub root: String,
    pub files: Vec<String>,
    pub places: Vec<(String, Vec<u8>)>,
}

/// [`FoundData`] as the value the program reads it as. Called on the calling thread.
pub fn found_value(found: &FoundData) -> PlyValue {
    record(vec![
        ("root", PlyValue::str(&found.root)),
        ("files", strings(found.files.iter().map(String::as_str))),
        ("places", places_value(&found.places)),
    ])
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

pub struct Signals {
    names: Vec<String>,
    lead_ms: u64,
    drain_ms: u64,
}

pub struct Disclosed {
    hermetic: bool,
    operations: usize,
    digest: String,
    trace: Option<String>,
    signals: Option<Signals>,
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
    counters: ply_eval::rc::RcStats,
    cycles: Vec<Diagnostic>,
    /// What the entry ended with, such as the spans it left open.
    warnings: Vec<Diagnostic>,
    stopping: Option<Stopped>,
    teardown: Teardown,
    trace: Option<ply_host::trace::Counts>,
    handshakes: Vec<String>,
    hosts: serde_json::Value,
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
            counters: ply_eval::rc::RcStats::default(),
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
            option(o.value.as_ref().map(ply_eval::reflect::value_of)),
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
    ])
}

/// What a `call` answers with: the value or what it raised, and what the entry ended with.
pub fn called_value(called: Ended<ply_eval::Plain>) -> PlyValue {
    let (answer, warnings) = called.into_parts();
    record(vec![
        (
            "answer",
            match answer {
                Ok(plain) => PlyValue::ctor("Ok", vec![ply_eval::reflect::value_of(&plain)]),
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

fn counters_value(stats: &ply_eval::rc::RcStats) -> PlyValue {
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
    let trace_v = get("trace")?;
    Ok(RunOptions {
        front: None,
        unit: None,
        hermetic: false,
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

//! What `ply test` loads, binds and runs, as `crates/ply-cli/ply/tests.ply` asks for it through the
//! effect `tester.ply` declares: a unit per program its tests run in, the host binding over the
//! first, the cases a test ranges over, and one test or one interleaving of one on whichever
//! thread asks.
//!
//! A compiled unit, the host binding and a Rust unwind are not values a program can hold, so those
//! stay here. Which tests run, the keys each result is read and filed under, what the cache keeps,
//! how the run concludes, why a failure happened, the mutants and mixtures that are tried and
//! everything said about all of it are the program's.

use crate::hosts::{self, Hosts, LentOp};
use crate::payload::{count, diags_value, field_of, json, option, raised_value, record, strings};
use crate::support::{select_profile, unit_of};
use crate::testrun::{
    Executed, Executor, Hosting, Interleaved, Usage, executed, interleaved, listed, status_word,
};
use ply_eval::host::{HostAnswer, HostHandler, HostRequest, HostRuntime, Linearity, MachineId};
use ply_eval::{Diagnostic, Seed, Span, Value as PlyValue, codes};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

/// The effect `crates/ply-cli/ply/tester.ply` declares.
const EFFECT: &str = "tester";

const HERMETIC: &str = "hermetic_tester";

const OPERATIONS: [(&str, &str); 10] = [
    ("configure", "ply_machine::tester::configure"),
    // The programs tests run in, the schema the first one enters, the binding over it, and what
    // it came to.
    ("unit", "ply_machine::tester::unit"),
    ("schema", "ply_machine::tester::schema"),
    ("bound", "ply_machine::tester::bound"),
    ("hosted", "ply_machine::tester::hosted"),
    ("ended", "ply_machine::tester::ended"),
    // A test's cases, and a test once or one interleaving of it, on whichever thread asks.
    ("cases", "ply_machine::tester::cases"),
    ("executed", "ply_machine::tester::executed"),
    ("interleaved", "ply_machine::tester::interleaved"),
    // The first read a pass's run made that no longer answers as it did.
    ("moved", "ply_machine::tester::moved"),
];

const HERMETIC_OPERATIONS: [(&str, &str); 10] = [
    ("configure", "ply_machine::tester::hermetic::configure"),
    ("unit", "ply_machine::tester::hermetic::unit"),
    ("schema", "ply_machine::tester::hermetic::schema"),
    ("bound", "ply_machine::tester::hermetic::bound"),
    ("hosted", "ply_machine::tester::hermetic::hosted"),
    ("ended", "ply_machine::tester::hermetic::ended"),
    ("cases", "ply_machine::tester::hermetic::cases"),
    ("executed", "ply_machine::tester::hermetic::executed"),
    ("interleaved", "ply_machine::tester::hermetic::interleaved"),
    ("moved", "ply_machine::tester::hermetic::moved"),
];

/// What the binding and the budgets are read from, out of the options record the program parsed.
#[derive(Clone, Debug)]
pub struct TestOptions {
    /// The project the run loads.
    pub path: PathBuf,
    pub steps: i64,
    pub timeout: u64,
    pub profile: String,
    pub host: bool,
    pub tls: crate::options::TlsOptions,
    pub fs: Vec<ply_host::fs::RootSpec>,
    /// The programs a test's `process.spawn` may start: the only `process` operation a test binds.
    pub exec: Vec<ply_host::process::ExecSpec>,
    /// The privileged families `--allow` lends the tests, which the program must declare.
    pub allow: Vec<String>,
}

impl Default for TestOptions {
    fn default() -> TestOptions {
        TestOptions {
            path: PathBuf::from("."),
            steps: ply_eval::DEFAULT_STEP_BUDGET,
            timeout: 60_000,
            profile: "development".to_string(),
            host: false,
            tls: crate::options::TlsOptions::default(),
            fs: Vec::new(),
            exec: Vec::new(),
            allow: Vec::new(),
        }
    }
}

/// One process's tester: every iteration of a watching run is lent the same one, so the cache and
/// the last compiled program outlive the report that opened them.
pub struct Session(Arc<TesterHandler>);

impl Session {
    pub fn new() -> Session {
        Session::of(false)
    }

    /// Binds no host, reads no clock, builds every unit afresh and measures nothing.
    pub fn hermetic() -> Session {
        Session::of(true)
    }

    fn of(hermetic: bool) -> Session {
        Session(Arc::new(TesterHandler {
            hermetic,
            options: Mutex::new(TestOptions::default()),
            run: RwLock::new(Run::default()),
        }))
    }

    pub fn lent(&self) -> Vec<LentOp> {
        let handler: Arc<dyn HostHandler> = Arc::clone(&self.0) as Arc<dyn HostHandler>;
        let (effect, operations) = if self.0.hermetic {
            (HERMETIC, HERMETIC_OPERATIONS)
        } else {
            (EFFECT, OPERATIONS)
        };
        // A watching run asks for report after report from inside one entry.
        operations
            .into_iter()
            .map(|(op, path)| {
                let op = if self.0.hermetic {
                    crate::hosts::hermetic_op(effect, op, Linearity::Repeatable, path)
                } else {
                    crate::hosts::privileged_op(effect, op, Linearity::Repeatable, path)
                };
                (op, Arc::clone(&handler))
            })
            .collect()
    }
}

impl Default for Session {
    fn default() -> Session {
        Session::new()
    }
}

struct TesterHandler {
    hermetic: bool,
    options: Mutex<TestOptions>,
    /// The run in progress, which every thread the program runs a test on reads.
    run: RwLock<Run>,
}

#[derive(Default)]
struct Run {
    /// The loaded program first, then each mutant and mixture built since.
    units: Vec<Arc<Unit>>,
    binding: Option<Bound>,
}

struct Unit {
    front: Arc<ply_eval::Analysis>,
    /// `None` when the run decided to execute nothing and so built nothing to run a test on.
    provider: Option<&'static dyn ply_eval::Provider>,
    /// Whether its tests reach the binding: a mixture is run hermetically, whatever it mixes.
    hosted: bool,
}

struct Bound {
    hosts: Hosts,
    hosting: Hosting,
}

impl HostHandler for TesterHandler {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match req.op.op.as_str() {
            "configure" => {
                let options = test_options_of(arg(req, 0)?, span)?;
                // A run begins whatever the last one left.
                *lock(&self.options) = options;
                *self.run.write().unwrap_or_else(|e| e.into_inner()) = Run::default();
                PlyValue::Unit
            }
            "unit" => self.unit(
                arg(req, 0)?,
                crate::payload::option_of(arg(req, 1)?, "the unit's C", span)?
                    .map(|c| c.as_bytes(span, "the unit's C").map(|b| &b[..]))
                    .transpose()?,
                arg(req, 2)?.as_bool(span, "whether the unit reaches the binding")?,
                span,
            )?,
            "schema" => {
                let name = arg(req, 0)?.as_str(span, "a definition's name")?;
                self.schema(name)?
            }
            "bound" => self.bound(crate::config::Configuration::of(arg(req, 0)?, span)?)?,
            "hosted" => self.hosted()?,
            "ended" => {
                *self.run.write().unwrap_or_else(|e| e.into_inner()) = Run::default();
                PlyValue::Unit
            }
            "cases" => {
                let unit = index_arg(req, 0, "a unit")?;
                let test = index_arg(req, 1, "a test index")?;
                self.cases(unit, test)?
            }
            "executed" => {
                let unit = index_arg(req, 0, "a unit")?;
                let test = index_arg(req, 1, "a test index")?;
                let case = case_arg(req, 2)?;
                self.executed(unit, test, case.as_ref(), req.machine)?
            }
            "interleaved" => {
                let unit = index_arg(req, 0, "a unit")?;
                let test = index_arg(req, 1, "a test index")?;
                let case = case_arg(req, 2)?;
                let seed = crate::recording::seed_of(arg(req, 3)?, span)?;
                let steps = arg(req, 4)?.as_int(span, "a step bound")?;
                let re_executed = arg(req, 5)?.as_bool(span, "whether the test re-runs")?;
                let steps = u32::try_from(steps.max(1)).unwrap_or(u32::MAX);
                self.interleaved(
                    unit,
                    test,
                    case.as_ref(),
                    &seed,
                    steps,
                    re_executed,
                    req.machine,
                )?
            }
            "moved" => {
                let trace = arg(req, 0)?.as_bytes(span, "a trace")?;
                let roots = self.roots();
                crate::payload::option(
                    ply_host::observe::moved(
                        &String::from_utf8_lossy(trace),
                        &self.world(&roots),
                        req.machine,
                    )
                    .map(PlyValue::str),
                )
            }
            other => return Err(crate::hosts::unserved(EFFECT, other, span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn arg<'a>(req: &'a HostRequest<'_>, at: usize) -> Result<&'a PlyValue, Diagnostic> {
    req.args
        .get(at)
        .ok_or_else(|| crate::hosts::unserved(EFFECT, req.op.op.as_str(), req.span))
}

fn index_arg(req: &HostRequest<'_>, at: usize, what: &str) -> Result<usize, Diagnostic> {
    let n = arg(req, at)?.as_int(req.span, what)?;
    usize::try_from(n).map_err(|_| crate::payload::missing(what, req.span))
}

/// The case a test over cases is run for: `None` for a test that ranges over nothing.
fn case_arg(req: &HostRequest<'_>, at: usize) -> Result<Option<ply_eval::Case>, Diagnostic> {
    let span = req.span;
    crate::payload::option_of(arg(req, at)?, "a case", span)?
        .map(|case| {
            Ok(ply_eval::Case {
                at: field_of(case, "at", span)?.as_int(span, "a case's place")?,
                identity: field_of(case, "identity", span)?
                    .as_bytes(span, "a case's identity")?
                    .to_vec(),
            })
        })
        .transpose()
}

fn ok(value: PlyValue) -> PlyValue {
    PlyValue::ctor("Ok", vec![value])
}

fn err(value: PlyValue) -> PlyValue {
    PlyValue::ctor("Err", vec![value])
}

// --- The units and the binding -------------------------------------------------

impl TesterHandler {
    /// A program the CLI ran the front end over and produced the C of, made a unit tests can run
    /// in: the loaded program, a mutant of it, or a mixture of two of its eras. `None` builds
    /// nothing, for a run that executes nothing.
    fn unit(
        &self,
        front: &PlyValue,
        unit: Option<&[u8]>,
        hosted: bool,
        span: Span,
    ) -> Result<PlyValue, Diagnostic> {
        let path = lock(&self.options).path.clone();
        let profile = lock(&self.options).profile.clone();
        // Which C compiler ran is no part of what a test answers.
        if !self.hermetic
            && let Err(diagnostic) = select_profile(&profile)
        {
            return Ok(err(diags_value(&[diagnostic])));
        }
        // Read on this thread: a `Value` may not cross to another.
        let handed = crate::driver::loaded_analysis_of(front, span)?;
        let root = if self.hermetic {
            crate::load::tidy(&path)
        } else {
            crate::load::project_root(&path)
        };
        let loaded = match crate::driver::load_over_analysis_in(root, &handed) {
            Ok(loaded) => loaded,
            Err(refused) => return Ok(err(diags_value(&refused.diagnostics))),
        };
        // Compiled outside the lock: a test of an earlier unit may be running meanwhile.
        let provider = match unit.map(|text| unit_of(&loaded.front, text)).transpose() {
            Ok(provider) => provider,
            Err(diagnostic) => return Ok(err(diags_value(&[diagnostic]))),
        };
        let mut run = self.run.write().unwrap_or_else(|e| e.into_inner());
        run.units.push(Arc::new(Unit {
            front: Arc::clone(&loaded.front),
            provider,
            hosted,
        }));
        Ok(ok(count(run.units.len() - 1)))
    }

    /// The host binding over the first unit: the configuration, the programs `--exec` names, the
    /// families `--allow` lends, and what the program declares. A refusal binds nothing.
    /// The value of the definition `--config-schema` names, entered on the first unit.
    fn schema(&self, name: &str) -> Result<PlyValue, Diagnostic> {
        let run = self.run.read().unwrap_or_else(|e| e.into_inner());
        let unit = run.units.first().ok_or_else(|| out_of_step("schema"))?;
        Ok(crate::config::schema_answer(crate::config::schema_of(
            &unit.front.check,
            unit.provider,
            name,
        )))
    }

    fn bound(&self, configuration: crate::config::Configuration) -> Result<PlyValue, Diagnostic> {
        let options = lock(&self.options).clone();
        let mut run = self.run.write().unwrap_or_else(|e| e.into_inner());
        let unit = run
            .units
            .first()
            .cloned()
            .ok_or_else(|| out_of_step("bound"))?;
        if self.hermetic && options.host {
            return Ok(err(diags_value(&[hermetic_host()])));
        }
        let check = &unit.front.check;
        // A test is not a process: of `process` it binds only what names a program, and only the
        // programs `--exec` names, so under `--host` an unnamed label is unbound rather than withheld.
        let process = if options.host {
            match ply_host::process::Executables::load(&options.exec, Span::DUMMY) {
                Ok(executables) => Some(ply_host::process::ProcessHost::spawning(executables)),
                Err(diagnostic) => return Ok(err(diags_value(&[diagnostic]))),
            }
        } else {
            None
        };
        let lent = match crate::policy::granted(check, &options.allow) {
            Ok(lent) => lent,
            Err(diagnostic) => return Ok(err(diags_value(&[diagnostic]))),
        };
        let hosts = match Hosts::open_stopping(
            check,
            options.host,
            &options.tls,
            &options.fs,
            configuration,
            // A test's `trace` records are discarded: a run reports on tests, not on what they logged.
            &crate::trace::TraceOptions::silent(),
            None,
            process,
            lent,
        ) {
            Ok(hosts) => hosts,
            Err(diagnostics) => return Ok(err(diags_value(&diagnostics))),
        };
        let hosting = Hosting {
            binding: Some(hosts.binding()),
            runtime: hosts.runtime_factory(),
        };
        run.binding = Some(Bound { hosts, hosting });
        Ok(ok(PlyValue::Unit))
    }

    /// What the binding and the first unit's backend came to, once the run is over.
    fn hosted(&self) -> Result<PlyValue, Diagnostic> {
        let run = self.run.read().unwrap_or_else(|e| e.into_inner());
        let unit = run.units.first().ok_or_else(|| out_of_step("hosted"))?;
        let bound = run.binding.as_ref().ok_or_else(|| out_of_step("hosted"))?;
        let hosts = &bound.hosts;
        let reaches: Vec<PlyValue> = unit
            .front
            .check
            .tests
            .iter()
            .enumerate()
            .filter(|(_, t)| hosts.reaches(&t.footprint))
            .map(|(i, _)| count(i))
            .collect();
        let cores = if self.hermetic {
            1
        } else {
            std::thread::available_parallelism().map_or(1, |n| n.get())
        };
        Ok(record(vec![
            ("hermetic", PlyValue::Bool(hosts.is_hermetic())),
            ("label", PlyValue::str(hosts.label())),
            ("operations", count(hosts.listing().rows.len())),
            (
                "digest",
                PlyValue::str(hosts::digest_short(hosts.listing(), &hosts.disclosures())),
            ),
            (
                "handshakes",
                strings(
                    hosts::handshake_lines(&hosts.handshakes())
                        .iter()
                        .map(String::as_str),
                ),
            ),
            ("hosts", json(&hosts.summary_json())),
            ("reaches", PlyValue::list(reaches)),
            ("cores", count(cores)),
            ("backend", compiled_value(unit.provider, self.hermetic)),
        ]))
    }

    /// The unit `index` names, and what its tests may reach.
    fn unit_at(&self, index: usize) -> Result<(Arc<Unit>, Hosting), Diagnostic> {
        let run = self.run.read().unwrap_or_else(|e| e.into_inner());
        let unit = run.units.get(index).cloned().ok_or_else(|| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!(
                    "the program ran a test in unit {index}, and the run holds {}",
                    run.units.len()
                ),
            )
            .note("units are the ones `tester.unit` answered; this is Ply's fault")
        })?;
        let hosting = match (&run.binding, unit.hosted) {
            (Some(bound), true) => bound.hosting.clone(),
            _ => Hosting::default(),
        };
        Ok((unit, hosting))
    }

    /// The program's thread is entered with no budget of its own; a test is bounded by the run's.
    fn budgeted<R>(&self, f: impl FnOnce() -> R) -> R {
        let (steps, timeout) = {
            let options = lock(&self.options);
            (
                options.steps,
                if self.hermetic { 0 } else { options.timeout },
            )
        };
        ply_codegen::rt::with_step_budget(steps, || ply_codegen::rt::with_time_budget(timeout, f))
    }

    /// The run's roots, which a trace's paths are written under.
    fn roots(&self) -> Vec<(String, PathBuf)> {
        let options = lock(&self.options);
        ply_host::fs::Roots::load(&options.fs, Span::DUMMY)
            .map(|roots| {
                roots
                    .listing()
                    .map(|(name, dir)| (name.to_string(), dir.to_path_buf()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// What a trace is read against here: a hermetic run reads no file, so no file it names is
    /// known to stand.
    fn world<'a>(&self, roots: &'a [(String, PathBuf)]) -> ply_host::observe::World<'a> {
        ply_host::observe::World {
            roots: (!self.hermetic).then_some(roots),
            binding: binding_digest(&lock(&self.options)),
            binary: ply_host::observe::Binary {
                shipped: &crate::shipped::module_digest,
                program: crate::shipped::program_digest(),
            },
        }
    }

    /// What a run read, as the trace its pass is filed with; and into the record of whatever is
    /// observing `caller`, which ran this run.
    fn traced(
        &self,
        observed: Option<&Arc<ply_host::observe::Recorder>>,
        usage: &Usage,
        caller: MachineId,
    ) -> Option<String> {
        let observed = observed?;
        ply_host::observe::absorb(caller, observed);
        let roots = self.roots();
        ply_host::observe::finished(observed, &self.world(&roots), usage.host)
    }

    /// The cases a test of `unit` ranges over, or why they could not be listed. A listing
    /// performs nothing, so it reaches no binding and reads nothing a pass would stand on.
    fn cases(&self, unit: usize, test: usize) -> Result<PlyValue, Diagnostic> {
        let (unit, _) = self.unit_at(unit)?;
        let cases = match unit.provider {
            Some(provider) => {
                let executor = Executor {
                    front: &unit.front,
                    hosting: Hosting::default(),
                    provider,
                };
                self.budgeted(|| listed(&executor, test))
            }
            None => Err(crate::testrun::Unlisted {
                failure: nothing_built(),
                panicked: false,
            }),
        };
        Ok(match cases {
            Ok(cases) => ok(cases),
            Err(unlisted) => err(record(vec![
                (
                    "status",
                    PlyValue::str(status_word(Some(&unlisted.failure), unlisted.panicked)),
                ),
                ("failure", raised_value(&unlisted.failure)),
            ])),
        })
    }

    fn executed(
        &self,
        unit: usize,
        test: usize,
        case: Option<&ply_eval::Case>,
        caller: MachineId,
    ) -> Result<PlyValue, Diagnostic> {
        let (unit, hosting) = self.unit_at(unit)?;
        let once = match unit.provider {
            Some(provider) => {
                let executor = Executor {
                    front: &unit.front,
                    hosting,
                    provider,
                };
                self.budgeted(|| executed(&executor, test, case))
            }
            None => Executed::refused(nothing_built()),
        };
        let once = if self.hermetic {
            Executed {
                usage: unmeasured(once.usage),
                ..once
            }
        } else {
            once
        };
        let trace = self.traced(once.observed.as_ref(), &once.usage, caller);
        Ok(executed_value(&once, trace))
    }

    #[allow(clippy::too_many_arguments)]
    fn interleaved(
        &self,
        unit: usize,
        test: usize,
        case: Option<&ply_eval::Case>,
        seed: &Seed,
        steps: u32,
        re_executed: bool,
        caller: MachineId,
    ) -> Result<PlyValue, Diagnostic> {
        let (unit, hosting) = self.unit_at(unit)?;
        let run = match unit.provider {
            Some(provider) => {
                let executor = Executor {
                    front: &unit.front,
                    hosting,
                    provider,
                };
                self.budgeted(|| interleaved(&executor, test, case, seed, steps, re_executed))
            }
            None => Interleaved::refused(nothing_built()),
        };
        let run = if self.hermetic {
            Interleaved {
                usage: unmeasured(run.usage),
                ..run
            }
        } else {
            run
        };
        let trace = self.traced(run.read.as_ref(), &run.usage, caller);
        Ok(interleaved_value(&run, trace))
    }
}

/// What the first unit's backend did, apart from the entries its tests counted.
fn compiled_value(provider: Option<&'static dyn ply_eval::Provider>, hermetic: bool) -> PlyValue {
    let offers = provider.map_or_else(Default::default, ply_eval::Provider::offers);
    // What compiling cost is the machine's state and clock, not the program's.
    let compiled = provider
        .and_then(ply_eval::Provider::compilation)
        .filter(|_| !hermetic);
    let tally = |n: u64| PlyValue::Int(i64::try_from(n).unwrap_or(i64::MAX));
    record(vec![
        (
            "name",
            PlyValue::str(provider.map_or("c", ply_eval::Provider::name)),
        ),
        (
            "fragment",
            count(provider.map_or(0, ply_eval::Provider::len)),
        ),
        ("offered", tally(offers.offered)),
        ("converted_in", tally(offers.converted_in)),
        ("converted_out", tally(offers.converted_out)),
        ("units", option(compiled.map(|c| tally(c.units)))),
        (
            "analysis_nanos",
            option(compiled.map(|c| tally(c.analysis_nanos))),
        ),
        (
            "codegen_nanos",
            option(compiled.map(|c| tally(c.codegen_nanos))),
        ),
        (
            "unbuilt",
            tally(provider.map_or(0, ply_eval::Provider::unbuilt)),
        ),
    ])
}

fn use_value(usage: &Usage) -> PlyValue {
    let tally = |n: u64| PlyValue::Int(i64::try_from(n).unwrap_or(i64::MAX));
    record(vec![
        (
            "duration_us",
            PlyValue::Int(i64::try_from(usage.duration.as_micros()).unwrap_or(i64::MAX)),
        ),
        ("host", PlyValue::Bool(usage.host)),
        ("entries", tally(usage.entries)),
        ("declines", tally(usage.declines)),
        ("performs", tally(usage.performs)),
        ("teardown", diags_value(&usage.teardown)),
    ])
}

/// What a run is configured to bind, apart from the program, which a pass's key covers: whether
/// it binds the host, the roots and programs it lends by name, and the families it allows.
fn binding_digest(options: &TestOptions) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(if options.host {
        b"host\0"
    } else {
        b"hermetic\0"
    });
    for (part, names) in [
        (
            "fs",
            options
                .fs
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
        ),
        (
            "exec",
            options.exec.iter().map(|e| e.name.as_str()).collect(),
        ),
        ("allow", options.allow.iter().map(String::as_str).collect()),
    ] {
        let mut names = names;
        names.sort();
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
        for name in names {
            hasher.update(name.as_bytes());
            hasher.update(&[0]);
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// `trace` is what the run read, which its pass is filed with: `None` when nothing could stand in
/// for the run, which files no pass.
fn executed_value(run: &Executed, trace: Option<String>) -> PlyValue {
    record(vec![
        (
            "status",
            PlyValue::str(status_word(run.failure.as_ref(), run.panicked)),
        ),
        ("failure", option(run.failure.as_ref().map(raised_value))),
        ("usage", use_value(&run.usage)),
        (
            "trace",
            option(trace.map(|t| PlyValue::bytes(t.as_bytes()))),
        ),
    ])
}

/// A failing interleaving's verdict carries the failure itself and how a report classes it.
fn interleaved_value(run: &Interleaved, trace: Option<String>) -> PlyValue {
    let fell = match &run.interleaving.verdict {
        ply_eval::Verdict::Failed(diagnostic) => Some(record(vec![
            (
                "status",
                PlyValue::str(status_word(Some(diagnostic), run.panicked)),
            ),
            ("failure", raised_value(diagnostic)),
        ])),
        ply_eval::Verdict::Passed => None,
    };
    record(vec![
        (
            "run",
            crate::recording::interleaving_value(&run.interleaving, fell),
        ),
        ("observed", PlyValue::Bool(run.observed)),
        ("usage", use_value(&run.usage)),
        (
            "trace",
            option(trace.map(|t| PlyValue::bytes(t.as_bytes()))),
        ),
    ])
}

// --- The options the program parses -----------------------------------------------

/// The options record as the program builds it from the parsed line, read for what the binding
/// and the budgets need. The program validated already, so a bad value here is an internal error.
pub fn test_options_of(v: &PlyValue, span: Span) -> Result<TestOptions, Diagnostic> {
    use crate::payload::str_list_at;
    let bool_at = |name: &str| field_of(v, name, span)?.as_bool(span, name);
    let int_at = |name: &str| field_of(v, name, span)?.as_int(span, name);
    let str_at = |name: &str| {
        field_of(v, name, span)?
            .as_str(span, name)
            .map(str::to_string)
    };
    let named_list = |name: &str| -> Result<Vec<(String, String)>, Diagnostic> {
        let mut out = Vec::new();
        for item in field_of(v, name, span)?.as_list(span, name)?.iter() {
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
    let cred_list = |name: &str| -> Result<Vec<ply_host::tls::CredentialSpec>, Diagnostic> {
        let mut out = Vec::new();
        for item in field_of(v, name, span)?.as_list(span, name)?.iter() {
            out.push(ply_host::tls::CredentialSpec {
                name: field_of(item, "name", span)?
                    .as_str(span, "a name")?
                    .to_string(),
                certificate: PathBuf::from(
                    field_of(item, "cert", span)?.as_str(span, "a certificate")?,
                ),
                key: PathBuf::from(field_of(item, "key", span)?.as_str(span, "a key")?),
            });
        }
        Ok(out)
    };
    Ok(TestOptions {
        path: PathBuf::from(str_at("path")?),
        steps: int_at("steps")?,
        timeout: u64::try_from(int_at("timeout")?).unwrap_or(0),
        profile: str_at("profile")?,
        host: bool_at("host")?,
        tls: crate::options::TlsOptions {
            tls: cred_list("tls")?,
            trust: str_list_at(v, "trust", span)?
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            mtls: str_list_at(v, "mtls", span)?,
        },
        fs: named_list("fs")?
            .into_iter()
            .map(|(name, path)| ply_host::fs::RootSpec {
                name,
                path: PathBuf::from(path),
            })
            .collect(),
        exec: named_list("exec")?
            .into_iter()
            .map(|(name, path)| ply_host::process::ExecSpec {
                name,
                path: PathBuf::from(path),
            })
            .collect(),
        allow: str_list_at(v, "allow", span)?,
    })
}

// --- Small things -------------------------------------------------------------

/// The wall clock is the machine's, not the program's.
fn unmeasured(usage: Usage) -> Usage {
    Usage {
        duration: std::time::Duration::ZERO,
        ..usage
    }
}

#[cold]
fn hermetic_host() -> Diagnostic {
    Diagnostic::error(
        codes::CAPABILITY_UNDECLARED,
        format!("`{HERMETIC}` binds no host, and the run asked for `--host`"),
    )
    .note("a test that drives a run reaching the host performs `tester`, and is `test/nondet`")
}

#[cold]
fn nothing_built() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a test was run in a unit built to run nothing, so no backend was compiled for it",
    )
    .note("this is Ply's fault: the run named no test to run and the program ran one anyway")
}

#[cold]
fn out_of_step(op: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` was performed before the run it asks about was bound"),
    )
    .note("the operations are performed in order; this is Ply's fault")
}

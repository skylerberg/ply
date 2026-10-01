//! What `ply test` loads, binds and runs, as `crates/ply-cli/ply/tests.ply` asks for it through the
//! effect `tester.ply` declares: a unit per program its tests run in, the host binding over the
//! first, one test or one interleaving of one on whichever thread asks, and the reads and writes of
//! the result cache.
//!
//! A compiled unit, the host binding and a Rust unwind are not values a program can hold, and the
//! cache's on-disk format has one reader, so those stay here. Which tests run, the keys each result
//! is read and filed under, how the run concludes, why a failure happened, the mutants and mixtures
//! that are tried and everything said about all of it are the program's.

use crate::hosts::{self, Hosts, Lent};
use crate::payload::{count, diags_value, field_of, json, option, raised_value, record, strings};
use crate::support::{build_backend_over, enter_constant, module_texts, select_profile};
use crate::testrun::{
    Executed, Executor, Hosting, Interleaved, Use, executed, interleaved, status_word,
};
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_eval::{DefHash, Diagnostic, Seed, Span, Symbol, Value as PlyValue, codes};
use ply_store::{Outcome, PassRecord, Store};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

/// The effect `crates/ply-cli/ply/tester.ply` declares.
const EFFECT: &str = "tester";

const OPERATIONS: [(&str, &str); 14] = [
    ("configure", "ply_machine::tester::configure"),
    // The programs tests run in, the binding over the first, and what it came to.
    ("unit", "ply_machine::tester::unit"),
    ("bound", "ply_machine::tester::bound"),
    ("hosted", "ply_machine::tester::hosted"),
    ("ended", "ply_machine::tester::ended"),
    // A test once, or one interleaving of it, on whichever thread asks.
    ("executed", "ply_machine::tester::executed"),
    ("interleaved", "ply_machine::tester::interleaved"),
    // The result cache.
    ("opened", "ply_machine::tester::opened"),
    ("outcomes", "ply_machine::tester::outcomes"),
    ("seen", "ply_machine::tester::seen"),
    ("baselines", "ply_machine::tester::baselines"),
    ("bodies", "ply_machine::tester::bodies"),
    ("interfaces", "ply_machine::tester::interfaces"),
    ("filed", "ply_machine::tester::filed"),
];

/// What the binding and the budgets are read from, out of the options record the program parsed.
#[derive(Clone, Debug)]
pub struct TestOptions {
    /// The project the result cache belongs to.
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
    pub config: crate::config::ConfigOptions,
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
            config: crate::config::ConfigOptions::default(),
        }
    }
}

/// One process's tester: every iteration of a watching run is lent the same one, so the cache and
/// the last compiled program outlive the report that opened them.
pub struct Session(Arc<Site>);

impl Session {
    pub fn new() -> Session {
        Session(Arc::new(Site {
            options: Mutex::new(TestOptions::default()),
            run: RwLock::new(Run::default()),
            warm: Mutex::new(None),
            cache: Mutex::new(None),
        }))
    }

    pub fn lent(&self) -> Vec<Lent> {
        let site: Arc<dyn HostHandler> = Arc::clone(&self.0) as Arc<dyn HostHandler>;
        OPERATIONS
            .into_iter()
            .map(|(op, path)| (registration(op, path), Arc::clone(&site)))
            .collect()
    }
}

impl Default for Session {
    fn default() -> Session {
        Session::new()
    }
}

fn registration(op: &str, path: &'static str) -> HostOp {
    HostOp {
        effect: Symbol::new(EFFECT),
        op: Symbol::new(op),
        resource: HostResource::Any,
        // A tree, a clock and a cache are not functions of program state.
        determinism: Determinism::Nondeterministic,
        // A watching run asks for report after report from inside one entry.
        linearity: Linearity::Repeatable,
        // The answer is in hand when the operation returns.
        blocking: false,
        secrets: false,
        path,
    }
}

struct Site {
    options: Mutex<TestOptions>,
    /// The run in progress, which every thread the program runs a test on reads.
    run: RwLock<Run>,
    /// The compiled program the last run's first unit was, kept for a later run over the same
    /// definitions: a `--watch` save that moved nothing compiles nothing.
    warm: Mutex<Option<Warm>>,
    /// The result cache, opened by the first operation that reads or writes it.
    cache: Mutex<Option<Cache>>,
}

struct Warm {
    /// [`ply_eval::Front::hashes_digest`], which a machine checks the unit against.
    key: DefHash,
    provider: &'static dyn ply_eval::Provider,
}

#[derive(Default)]
struct Run {
    /// The loaded program first, then each mutant and mixture built since.
    units: Vec<Arc<Unit>>,
    binding: Option<Bound>,
}

struct Unit {
    front: Arc<ply_eval::Front>,
    /// `None` when the run decided to execute nothing and so built nothing to run a test on.
    provider: Option<&'static dyn ply_eval::Provider>,
    /// Whether its tests reach the binding: a mixture is run hermetically, whatever it mixes.
    hosted: bool,
}

struct Bound {
    hosts: Hosts,
    hosting: Hosting,
}

impl HostHandler for Site {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match req.op.op.as_str() {
            "configure" => {
                let options = test_options_of(arg(req, 0)?, span)?;
                // A run begins whatever the last one left; the store is kept only for its project.
                let mut held = lock(&self.options);
                if crate::load::project_root(&held.path) != crate::load::project_root(&options.path)
                {
                    *lock(&self.cache) = None;
                }
                *held = options;
                *self.run.write().unwrap_or_else(|e| e.into_inner()) = Run::default();
                PlyValue::Unit
            }
            "unit" => self.unit(
                arg(req, 0)?,
                arg(req, 1)?.as_bool(span, "whether to build")?,
                arg(req, 2)?.as_bool(span, "whether the unit reaches the binding")?,
                span,
            )?,
            "bound" => self.bound()?,
            "hosted" => self.hosted()?,
            "ended" => {
                *self.run.write().unwrap_or_else(|e| e.into_inner()) = Run::default();
                PlyValue::Unit
            }
            "executed" => {
                let unit = index_arg(req, 0, "a unit")?;
                let test = index_arg(req, 1, "a test index")?;
                self.executed(unit, test)?
            }
            "interleaved" => {
                let unit = index_arg(req, 0, "a unit")?;
                let test = index_arg(req, 1, "a test index")?;
                let seed = crate::recording::seed_of(arg(req, 2)?, span)?;
                let steps = arg(req, 3)?.as_int(span, "a step bound")?;
                let re_executed = arg(req, 4)?.as_bool(span, "whether the test re-runs")?;
                let steps = u32::try_from(steps.max(1)).unwrap_or(u32::MAX);
                self.interleaved(unit, test, &seed, steps, re_executed)?
            }
            "opened" => diags_value(&self.cached(|cache| cache.take_warnings())),
            "outcomes" => {
                let keys = texts_of(arg(req, 0)?, span, "the keys to look up")?;
                self.cached(|cache| outcomes(cache, &keys))
            }
            "seen" => {
                let hashes = texts_of(arg(req, 0)?, span, "the hashes asked about")?;
                self.cached(|cache| seen(cache, &hashes))
            }
            "baselines" => {
                let keys = texts_of(arg(req, 0)?, span, "the tests asked about")?;
                self.cached(|cache| baselines(cache, &keys))
            }
            "bodies" => {
                let hashes = texts_of(arg(req, 0)?, span, "the bodies asked for")?;
                self.cached(|cache| bodies(cache, &hashes))
            }
            "interfaces" => {
                let mut asked = Vec::new();
                for item in arg(req, 0)?.as_list(span, "the interfaces asked for")? {
                    asked.push((
                        field_of(item, "hash", span)?
                            .as_str(span, "a hash")?
                            .to_string(),
                        field_of(item, "name", span)?
                            .as_str(span, "a name")?
                            .to_string(),
                    ));
                }
                self.cached(|cache| interfaces(cache, &asked))
            }
            "filed" => {
                let filing = filing_of(arg(req, 0)?, span)?;
                self.cached(|cache| filed(cache, filing))
            }
            other => return Err(unasked(other, span)),
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
        .ok_or_else(|| unasked(req.op.op.as_str(), req.span))
}

fn index_arg(req: &HostRequest<'_>, at: usize, what: &str) -> Result<usize, Diagnostic> {
    let n = arg(req, at)?.as_int(req.span, what)?;
    usize::try_from(n).map_err(|_| crate::payload::missing(what, req.span))
}

fn texts_of(v: &PlyValue, span: Span, what: &str) -> Result<Vec<String>, Diagnostic> {
    let mut out = Vec::new();
    for item in v.as_list(span, what)? {
        out.push(item.as_str(span, what)?.to_string());
    }
    Ok(out)
}

/// `Ok(v)` or `Err(e)`, as the program reads an operation's answer.
fn ok(value: PlyValue) -> PlyValue {
    PlyValue::ctor("Ok", vec![value])
}

fn err(value: PlyValue) -> PlyValue {
    PlyValue::ctor("Err", vec![value])
}

// --- The units and the binding -------------------------------------------------

impl Site {
    /// A program the CLI ran the front end over, made a unit tests can run in: the loaded program,
    /// a mutant of it, or a mixture of two of its eras.
    fn unit(
        &self,
        front: &PlyValue,
        build: bool,
        hosted: bool,
        span: Span,
    ) -> Result<PlyValue, Diagnostic> {
        let path = lock(&self.options).path.clone();
        let profile = lock(&self.options).profile.clone();
        if let Err(diagnostic) = select_profile(&profile) {
            return Ok(err(diags_value(&[diagnostic])));
        }
        // Read on this thread: a `Value` may not cross to another.
        let handed = crate::driver::handed_front_of(front, span)?;
        let loaded = match crate::driver::load_over_front(&path, &handed) {
            Ok(loaded) => loaded,
            Err(refused) => return Ok(err(diags_value(&refused.diagnostics))),
        };
        let first = self
            .run
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .units
            .is_empty();
        // Compiled outside the lock: a test of an earlier unit may be running meanwhile.
        let provider = if !build {
            None
        } else {
            match self.provider(&loaded, first) {
                Ok(provider) => Some(provider),
                Err(diagnostic) => return Ok(err(diags_value(&[diagnostic]))),
            }
        };
        let mut run = self.run.write().unwrap_or_else(|e| e.into_inner());
        run.units.push(Arc::new(Unit {
            front: Arc::clone(&loaded.front),
            provider,
            hosted,
        }));
        Ok(ok(count(run.units.len() - 1)))
    }

    /// The first unit of a run reuses the last run's compiled program when no definition moved.
    fn provider(
        &self,
        loaded: &crate::load::Loaded,
        first: bool,
    ) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
        let mut warm = lock(&self.warm);
        if first
            && let Some(held) = warm.as_ref()
            && held.key == loaded.front.hashes_digest
            && held.provider.relocate(&loaded.front, &loaded.sources)
        {
            return Ok(held.provider);
        }
        let provider =
            build_backend_over(&loaded.front, module_texts(&loaded.check, &loaded.sources))?;
        if first {
            *warm = Some(Warm {
                key: loaded.front.hashes_digest,
                provider,
            });
        }
        Ok(provider)
    }

    /// The host binding over the first unit: the configuration, the programs `--exec` names, the
    /// families `--allow` lends, and what the program declares. A refusal binds nothing.
    fn bound(&self) -> Result<PlyValue, Diagnostic> {
        let options = lock(&self.options).clone();
        let mut run = self.run.write().unwrap_or_else(|e| e.into_inner());
        let unit = run
            .units
            .first()
            .cloned()
            .ok_or_else(|| out_of_step("bound"))?;
        let check = &unit.front.check;
        let constant = |name: &str| enter_constant(unit.provider, name);
        let (configuration, warnings) = match crate::config::Configuration::open(
            check,
            options.host,
            &options.config,
            &constant,
        ) {
            Ok(resolved) => resolved,
            Err(diagnostics) => return Ok(err(diags_value(&diagnostics))),
        };
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
        Ok(ok(diags_value(&warnings)))
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
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
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
            ("backend", compiled_value(unit.provider)),
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
            (options.steps, options.timeout)
        };
        ply_codegen::rt::with_step_budget(steps, || ply_codegen::rt::with_time_budget(timeout, f))
    }

    fn executed(&self, unit: usize, test: usize) -> Result<PlyValue, Diagnostic> {
        let (unit, hosting) = self.unit_at(unit)?;
        let once = match unit.provider {
            Some(provider) => {
                let executor = Executor {
                    front: &unit.front,
                    hosting,
                    provider,
                };
                self.budgeted(|| executed(&executor, test))
            }
            None => Executed::refused(nothing_built()),
        };
        Ok(executed_value(&once))
    }

    fn interleaved(
        &self,
        unit: usize,
        test: usize,
        seed: &Seed,
        steps: u32,
        re_executed: bool,
    ) -> Result<PlyValue, Diagnostic> {
        let (unit, hosting) = self.unit_at(unit)?;
        let run = match unit.provider {
            Some(provider) => {
                let executor = Executor {
                    front: &unit.front,
                    hosting,
                    provider,
                };
                self.budgeted(|| interleaved(&executor, test, seed, steps, re_executed))
            }
            None => Interleaved::refused(nothing_built()),
        };
        Ok(interleaved_value(&run))
    }
}

/// What the first unit's backend did, apart from the entries its tests counted.
fn compiled_value(provider: Option<&'static dyn ply_eval::Provider>) -> PlyValue {
    let offers = provider.map_or_else(Default::default, ply_eval::Provider::offers);
    let compiled = provider.and_then(ply_eval::Provider::compilation);
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

fn use_value(usage: &Use) -> PlyValue {
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

fn executed_value(run: &Executed) -> PlyValue {
    record(vec![
        (
            "status",
            PlyValue::str(status_word(run.failure.as_ref(), run.panicked)),
        ),
        ("failure", option(run.failure.as_ref().map(raised_value))),
        ("usage", use_value(&run.usage)),
    ])
}

/// A failing interleaving's verdict carries the failure itself and how a report classes it.
fn interleaved_value(run: &Interleaved) -> PlyValue {
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
    ])
}

// --- The result cache ----------------------------------------------------------

/// The store a process's runs share, or why there is none: an unusable cache never stops a run.
struct Cache {
    store: Option<Store>,
    /// Said by the next `opened`, once.
    warnings: Vec<Diagnostic>,
}

impl Cache {
    fn open(path: &std::path::Path) -> Cache {
        let root = crate::load::project_root(path);
        match Store::open(&root) {
            Ok(mut store) => {
                let opened = store.take_warnings();
                let mut warnings: Vec<Diagnostic> = crate::migrate::notice(&store, &opened)
                    .into_iter()
                    .collect();
                warnings.extend(opened);
                Cache {
                    store: Some(store.with_upstream(ply_store::Upstream::from_env())),
                    warnings,
                }
            }
            Err(e) => Cache {
                store: None,
                warnings: vec![
                    Diagnostic::warning(
                        codes::RUNTIME_ERROR,
                        format!("could not open the cache under `{}`: {e:#}", root.display()),
                    )
                    .note("every test ran, and nothing this run proved was recorded")
                    .note("check the directory's permissions to get caching back"),
                ],
            },
        }
    }

    fn take_warnings(&mut self) -> Vec<Diagnostic> {
        let mut warnings = std::mem::take(&mut self.warnings);
        if let Some(store) = &mut self.store {
            warnings.extend(store.take_warnings());
        }
        warnings
    }
}

impl Site {
    fn cached<R>(&self, f: impl FnOnce(&mut Cache) -> R) -> R {
        let path = lock(&self.options).path.clone();
        let mut cache = lock(&self.cache);
        f(cache.get_or_insert_with(|| Cache::open(&path)))
    }
}

fn hash(hex: &str) -> Option<DefHash> {
    DefHash::from_hex(hex)
}

/// The store's answer under each key, as a report prints one: `passed`, `failed`, or nothing.
fn outcomes(cache: &mut Cache, keys: &[String]) -> PlyValue {
    PlyValue::list(
        keys.iter()
            .map(|key| {
                let outcome = cache
                    .store
                    .as_ref()
                    .zip(hash(key))
                    .and_then(|(store, h)| store.get(h));
                option(
                    outcome.map(|o| PlyValue::str(if o.is_pass() { "passed" } else { "failed" })),
                )
            })
            .collect(),
    )
}

/// Whether the store has recorded seeing each definition.
fn seen(cache: &mut Cache, hashes: &[String]) -> PlyValue {
    PlyValue::list(
        hashes
            .iter()
            .map(|h| {
                PlyValue::Bool(
                    cache
                        .store
                        .as_ref()
                        .zip(hash(h))
                        .is_some_and(|(store, h)| store.knows_definition(h)),
                )
            })
            .collect(),
    )
}

fn named_value(names: &BTreeMap<Symbol, DefHash>) -> PlyValue {
    PlyValue::list(
        names
            .iter()
            .map(|(name, h)| {
                record(vec![
                    ("name", PlyValue::str(name.as_str())),
                    ("hash", PlyValue::str(h.to_hex())),
                ])
            })
            .collect(),
    )
}

/// The definition set each test was last seen to pass at.
fn baselines(cache: &mut Cache, keys: &[String]) -> PlyValue {
    PlyValue::list(
        keys.iter()
            .map(|key| {
                let held = cache
                    .store
                    .as_ref()
                    .and_then(|store| store.pass_record(&Symbol::new(key.as_str())));
                option(held.map(|r| {
                    record(vec![
                        ("test", PlyValue::str(r.test_hash.to_hex())),
                        ("closure", named_value(&r.closure)),
                        ("decls", named_value(&r.decls)),
                    ])
                }))
            })
            .collect(),
    )
}

/// The stored body each hash is filed under, as the front end wrote it.
fn bodies(cache: &mut Cache, hashes: &[String]) -> PlyValue {
    PlyValue::list(
        hashes
            .iter()
            .map(|h| {
                let body = cache
                    .store
                    .as_ref()
                    .zip(hash(h))
                    .and_then(|(store, h)| store.body(h))
                    .and_then(|b| b.stored());
                option(body.map(|b| PlyValue::bytes(b.as_bytes())))
            })
            .collect(),
    )
}

/// Each definition's interface as the front end filed it under that hash: the compiler's own value,
/// handed back without reading it.
fn interfaces(cache: &mut Cache, asked: &[(String, String)]) -> PlyValue {
    PlyValue::list(
        asked
            .iter()
            .map(|(h, name)| {
                let slot = cache
                    .store
                    .as_ref()
                    .zip(hash(h))
                    .and_then(|(store, h)| store.def_of(h, &Symbol::new(name.as_str())));
                option(slot.and_then(|slot| ply_eval::codec::decode(&slot.value).ok()))
            })
            .collect(),
    )
}

/// What a run or a bisection established, as the program decided to file it.
struct Filing {
    passes: Vec<DefHash>,
    records: Vec<(Symbol, PassRecord)>,
    seen: Vec<DefHash>,
}

fn filing_of(v: &PlyValue, span: Span) -> Result<Filing, Diagnostic> {
    let key = |v: &PlyValue| -> Result<DefHash, Diagnostic> {
        let hex = v.as_str(span, "a key")?;
        hash(hex).ok_or_else(|| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("`{hex}` is not a key the store could be written under"),
            )
            .primary(span, "the program handed this key over")
        })
    };
    let keys = |v: &PlyValue, what: &str| -> Result<Vec<DefHash>, Diagnostic> {
        v.as_list(span, what)?.iter().map(key).collect()
    };
    let named = |v: &PlyValue| -> Result<BTreeMap<Symbol, DefHash>, Diagnostic> {
        let mut out = BTreeMap::new();
        for item in v.as_list(span, "a closure")? {
            out.insert(
                Symbol::new(field_of(item, "name", span)?.as_str(span, "a name")?),
                key(field_of(item, "hash", span)?)?,
            );
        }
        Ok(out)
    };
    let mut records = Vec::new();
    for item in field_of(v, "records", span)?.as_list(span, "the pass records")? {
        let held = field_of(item, "record", span)?;
        records.push((
            Symbol::new(field_of(item, "key", span)?.as_str(span, "a test")?),
            PassRecord {
                test_hash: key(field_of(held, "test", span)?)?,
                closure: named(field_of(held, "closure", span)?)?,
                decls: named(field_of(held, "decls", span)?)?,
            },
        ));
    }
    Ok(Filing {
        passes: keys(field_of(v, "passes", span)?, "the passes")?,
        records,
        seen: keys(field_of(v, "seen", span)?, "the definitions seen")?,
    })
}

/// Writes the filing and flushes, answering why the flush failed when it did, and what the store
/// had to say.
fn filed(cache: &mut Cache, filing: Filing) -> PlyValue {
    let Some(store) = &mut cache.store else {
        return record(vec![
            ("unflushed", option(None)),
            ("warnings", diags_value(&cache.take_warnings())),
        ]);
    };
    for key in filing.passes {
        store.put(key, Outcome::Pass);
    }
    for (key, held) in filing.records {
        store.put_pass_record(key, held);
    }
    if !filing.seen.is_empty() {
        store.observe_definitions(filing.seen);
    }
    let unflushed = store.flush().err().map(|e| PlyValue::str(format!("{e:#}")));
    record(vec![
        ("unflushed", option(unflushed)),
        ("warnings", diags_value(&cache.take_warnings())),
    ])
}

// --- The options the program parses -----------------------------------------------

/// The options record as the program builds it from the parsed line, read for what the binding
/// and the budgets need. The program validated already, so a bad value here is an internal error.
pub fn test_options_of(v: &PlyValue, span: Span) -> Result<TestOptions, Diagnostic> {
    use crate::payload::{opt_str_at, str_list_at};
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
    let config = field_of(v, "config", span)?;
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
        config: crate::config::ConfigOptions {
            set: str_list_at(config, "set", span)?,
            files: str_list_at(config, "files", span)?
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            schema: opt_str_at(config, "schema", span)?,
        },
    })
}

// --- Small things -------------------------------------------------------------

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

#[cold]
fn unasked(op: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` reached the binding, and `ply test` serves no such operation"),
    )
    .primary(span, "this perform reached `ply test`")
    .note("the effect and its handler are written together; this is Ply's fault")
}

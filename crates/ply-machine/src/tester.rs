//! What `ply test` loads, selects, binds and runs, as the program in `crates/ply-cli/ply/tests.ply`
//! performs it.
//!
//! The front end, the result cache, the selection it answers, the conflict grouping, the compiled
//! backend, the host binding, the worker pool, the per-test unwind catching and the bisection stay
//! here: a front end is not a value a program can hold, a Rust unwind is not a Ply value, and a
//! reader of the store's on-disk format written in Ply would be a second implementation of it.
//! What is said about all of it, in both forms, and the code the run exits with are the program's.

use crate::hosts::{self, Hosts, Lent, hosting};
use crate::load::{Loaded, load, project_root};
use crate::options::When;
use crate::payload::{count, diag_value, diags_value, json, option, places_value, record, strings};
use crate::support::{
    build_backend_over, build_pool, enter_constant, module_texts, once_each, select_profile,
};
use ply_eval::Value as PlyValue;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_span::{Diagnostic, SourceMap, Span, Symbol, codes};
use ply_store::Store;
use ply_test::{
    Isolation, Record, RunReport, Selection, Skipped, Status, Suspect, TestResult, Verdict,
};
use ply_ty::{CheckOutput, Footprint, HashOutput, Mode};
use serde_json::{Value, json as jsonlit};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

/// The effect `crates/ply-cli/ply/tests.ply` declares. It is lent to that one entry and nowhere
/// else: no other command runs a corpus.
const EFFECT: &str = "tester";

/// The modules the change set's and the trial's vocabulary is declared in. A constructor the runtime
/// builds has to carry the name the program's own spine gives it, and that name is `<module>::<Case>`
/// — the source's `.` and `payload::ctor`'s `.` are both wrong for a value the program *matches* on.
/// Getting it wrong is a placeless `no arm of this match matched` the moment the program matches.
const DELTA: &str = "suite.delta";
const BISECT: &str = "suite.bisect";

const OPERATIONS: [(&str, &str); 12] = [
    ("configure", "ply_machine::tester::configure"),
    ("loaded", "ply_machine::test::loaded"),
    ("bound", "ply_machine::test::bound"),
    ("ran", "ply_machine::test::ran"),
    ("stamped", "ply_machine::test::stamped"),
    // What a selector computes a selection from, before anything runs.
    ("keys", "ply_machine::tester::keys"),
    ("hashed", "ply_machine::tester::hashed"),
    ("searched", "ply_machine::tester::searched"),
    ("trial", "ply_machine::tester::trial"),
    ("chosen", "ply_machine::tester::chosen"),
    ("outcomes", "ply_machine::tester::outcomes"),
    // The printed union of a set of tests' footprints: the program colours the graph, and the
    // rendering of a colour is the compiler's.
    ("footprint", "ply_machine::tester::footprint"),
];

/// A compiled body honours its call bound on the native stack, where unoptimised frames run to
/// kilobytes. Reserved, not committed.
const RUN_STACK: usize = 256 << 20;

/// One process's machine. The load, the store and the binding are opened on a thread of their own
/// and live as long as this does, so a `--watch` iteration over an unmoved tree re-derives nothing.
/// What `ply test` is configured with, as plain data: the shell's parsed flags convert into
/// this.
#[derive(Clone, Debug)]
pub struct TestOptions {
    pub path: std::path::PathBuf,
    pub json: bool,
    pub explain: bool,
    pub no_cache: bool,
    pub filter: Option<String>,
    pub jobs: Option<u32>,
    pub steps: i64,
    pub timeout: u64,
    pub bisect: When,
    pub bisect_budget: usize,
    pub coverage: bool,
    pub mutate: Option<String>,
    pub mutate_budget: usize,
    /// This command's `--trace` is the definition trace, never the record sink.
    pub trace: When,
    pub profile: String,
    pub watch: bool,
    pub host: bool,
    pub tls: crate::options::TlsOptions,
    pub fs: Vec<ply_host::fs::RootSpec>,
    pub config: crate::config::ConfigOptions,
    pub std: bool,
    pub simulation: crate::simulation::SimOptions,
}

pub struct Session(Arc<Site>);

impl Session {
    pub fn new(args: &TestOptions) -> Session {
        Session(Arc::new(Site {
            args: Mutex::new(args.clone()),
            machine: Mutex::new(None),
        }))
    }

    /// What one entry into the program is lent. Every iteration is lent the same machine.
    pub fn lent(&self) -> Vec<Lent> {
        let site: Arc<dyn HostHandler> = Arc::clone(&self.0) as Arc<dyn HostHandler>;
        OPERATIONS
            .into_iter()
            .map(|(op, path)| (registration(op, path), Arc::clone(&site)))
            .collect()
    }
}

/// The decision, as the program sent it: the same four fields `ply_test::Choice` holds.
fn choice_of(v: &PlyValue, span: Span) -> Result<ply_test::Choice, Diagnostic> {
    use crate::payload::field_of;
    let runs = ints_of(field_of(v, "runs", span)?, span, "the tests to run")?;
    let reasons: Vec<ply_test::Reason> = strs_of(field_of(v, "reasons", span)?, span)?
        .iter()
        .map(|word| {
            ply_test::Reason::parse(word).ok_or_else(|| {
                Diagnostic::error(codes::INTERNAL_ERROR, format!("unknown reason `{word}`"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut narrowed = std::collections::BTreeMap::new();
    for entry in field_of(v, "narrowed", span)?.as_list(span, "the narrowed plans")? {
        let index = field_of(entry, "index", span)?.as_int(span, "a test index")? as usize;
        let roots = ints_of(field_of(entry, "roots", span)?, span, "the roots owed")?
            .into_iter()
            .map(|r| r as u64)
            .collect();
        narrowed.insert(index, roots);
    }
    let mut groups = Vec::new();
    for class in field_of(v, "groups", span)?.as_list(span, "the classes")? {
        groups.push(ints_of(class, span, "a class")?);
    }
    Ok(ply_test::Choice {
        runs,
        reasons,
        narrowed,
        groups,
    })
}

fn ints_of(v: &PlyValue, span: Span, what: &str) -> Result<Vec<usize>, Diagnostic> {
    let mut out = Vec::new();
    for item in v.as_list(span, what)?.iter() {
        out.push(item.as_int(span, "a number")? as usize);
    }
    Ok(out)
}

fn strs_of(v: &PlyValue, span: Span) -> Result<Vec<String>, Diagnostic> {
    let mut out = Vec::new();
    for item in v.as_list(span, "the reasons")?.iter() {
        out.push(item.as_str(span, "a reason")?.to_string());
    }
    Ok(out)
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
        // The answer is in hand when the operation returns: this thread waits for the one the
        // corpus runs on rather than being handed a token to poll.
        blocking: false,
        secrets: false,
        path,
    }
}

struct Site {
    args: Mutex<TestOptions>,
    /// Started by the first operation and joined by the last.
    machine: Mutex<Option<Machine>>,
}

impl HostHandler for Site {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match req.op.op.as_str() {
            "configure" => {
                let options = req
                    .args
                    .first()
                    .ok_or_else(|| unasked("configure", req.span))?;
                *self.args.lock().unwrap_or_else(|e| e.into_inner()) =
                    test_options_of(options, req.span)?;
                ply_eval::Value::Unit
            }
            "loaded" => self.loaded()?,
            "bound" => self.bound()?,
            "ran" => self.ran()?,
            "stamped" => self.stamped(),
            "keys" => self.knowledge(Ask::Keys)?,
            "hashed" => self.knowledge(Ask::Hashed)?,
            "searched" => self.knowledge(Ask::Searched)?,
            "trial" => {
                use crate::payload::field_of;
                let failure = req
                    .args
                    .first()
                    .ok_or_else(|| unasked("trial", req.span))?
                    .as_int(span, "a failure's place in the report")?;
                let keys = req
                    .args
                    .get(1)
                    .ok_or_else(|| unasked("trial", req.span))?
                    .as_list(span, "the keys to flip")?;
                let mut named = Vec::with_capacity(keys.len());
                for key in keys {
                    named.push((
                        field_of(key, "name", span)?
                            .as_str(span, "a definition's name")?
                            .to_string(),
                        field_of(key, "ns", span)?
                            .as_str(span, "a namespace")?
                            .to_string(),
                    ));
                }
                self.trial(usize::try_from(failure).unwrap_or(usize::MAX), named)?
            }
            "chosen" => {
                let value = req
                    .args
                    .first()
                    .ok_or_else(|| unasked("chosen", req.span))?;
                self.chosen(choice_of(value, req.span)?)?
            }
            "outcomes" => {
                let list = req
                    .args
                    .first()
                    .ok_or_else(|| unasked("outcomes", req.span))?
                    .as_list(span, "the keys to look up")?;
                let mut keys = Vec::with_capacity(list.len());
                for item in list {
                    keys.push(item.as_str(span, "a key")?.to_string());
                }
                self.knowledge(Ask::Outcomes(keys))?
            }
            "footprint" => {
                let list = req
                    .args
                    .first()
                    .ok_or_else(|| unasked("footprint", req.span))?
                    .as_list(span, "the tests of a group")?;
                let mut tests = Vec::with_capacity(list.len());
                for item in list {
                    tests.push(item.as_int(span, "a test index")? as usize);
                }
                self.knowledge(Ask::Footprint(tests))?
            }
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
            *held = Some(Machine::start(
                self.args.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            )?);
        }
        let machine = held.as_ref().ok_or_else(|| unstarted("loaded"))?;
        machine.ask(Go::Load)?;
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

    /// How the tree stamps now, one line per `.ply` file under the root, or `None` when the walk
    /// could not read a directory. It reaches no machine: a watching run asks for this between
    /// reports, and a walk is not a front end.
    fn stamped(&self) -> PlyValue {
        let path = self
            .args
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .path
            .clone();
        crate::payload::option(
            crate::warm::tree_stamps(&project_root(&path)).map(|stamps| {
                PlyValue::list(
                    stamps
                        .iter()
                        .map(|(path, stamp)| PlyValue::str(stamp_line(path, stamp)))
                        .collect(),
                )
            }),
        )
    }

    /// One part of what the loaded tree tells a selector: the keys its tests' results are filed
    /// under, the definitions' hashes, or the search a seeded test is keyed on. Nothing is
    /// selected here; the program computes the selection from these.
    fn knowledge(&self, asked: Ask) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted(asked.name()))?;
        machine.ask(match &asked {
            Ask::Keys => Go::Keys,
            Ask::Hashed => Go::Hashed,
            Ask::Searched => Go::Searched,
            Ask::Footprint(tests) => Go::Footprint(tests.clone()),
            Ask::Outcomes(keys) => Go::Outcomes(keys.clone()),
        })?;
        match machine.step()? {
            Step::Knowledge {
                asked: answered,
                value,
            } if answered == asked => Ok(knowledge_value(&value)),
            _ => Err(out_of_step(asked.name())),
        }
    }

    /// The machine is left running: the next iteration is lent the front end this one built.
    /// The program states the decision here. Ordered on the same channel as everything else, so it
    /// arrives before the run that obeys it.
    fn chosen(&self, choice: ply_test::Choice) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("chosen"))?;
        machine.ask(Go::Chosen(choice))?;
        Ok(ply_eval::Value::Unit)
    }

    /// One mixture the program decided to try. The runtime answers it on its own thread, where the
    /// store and the warm front end are; this waits for that answer.
    fn trial(&self, failure: usize, keys: Vec<(String, String)>) -> Result<PlyValue, Diagnostic> {
        let (reply, answers) = mpsc::channel();
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("trial"))?;
        machine.ask(Go::Trial {
            failure,
            keys,
            reply,
        })?;
        match answers.recv() {
            Ok(Ok(trial)) => Ok(trial_value(&trial)),
            Ok(Err(diagnostic)) => Err(diagnostic),
            Err(_) => Err(unanswered()),
        }
    }

    fn ran(&self) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("ran"))?;
        machine.ask(Go::Run)?;
        let step = machine.step()?;
        match step {
            Step::Ran(over) => Ok(ran_value(&over)),
            _ => Err(out_of_step("ran")),
        }
    }
}

/// One file's stamp, whole: the modification time to the nanosecond and the length. Two walks that
/// render the same line found the same file, which is the only question asked of it.
fn stamp_line(path: &Path, (modified, len): &crate::load::Stamp) -> String {
    let at = modified
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_nanos().to_string())
        .unwrap_or_else(|| "-".to_string());
    format!("{} {at} {len}", path.display())
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
    Load,
    Bind,
    /// What the program decided to run. Sent before the binding, because whether a unit has to be
    /// built at all is a function of it: a fully cached run builds none.
    Chosen(ply_test::Choice),
    Run,
    Keys,
    Hashed,
    Searched,
    /// The store's answer under each key the caller names: the narrowing asks about keys the
    /// *program* computes, so no row could have carried them.
    Outcomes(Vec<String>),
    /// The printed union of these tests' footprints. The program colours the graph; the rendering of
    /// a colour is the compiler's, since only its printer can keep a label variable off a name.
    Footprint(Vec<usize>),
    /// One mixture the program decided to try. Answered on this thread, because the store a hybrid
    /// is built over lives here and a `BodyHybrid` is not `Send`.
    Trial {
        failure: usize,
        keys: Vec<(String, String)>,
        reply: mpsc::Sender<Result<ply_test::bisect::Trial, Diagnostic>>,
    },
}

enum Step {
    Loaded(Box<Result<Found, Refused>>),
    Bound(Box<Option<Refused>>),
    Ran(Box<Over>),
    Knowledge {
        asked: Ask,
        value: Box<KnowledgeValue>,
    },
}

/// Which part of what a selector reads to compute a selection.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Ask {
    Keys,
    Hashed,
    Searched,
    Footprint(Vec<usize>),
    Outcomes(Vec<String>),
}

impl Ask {
    fn name(&self) -> &'static str {
        match self {
            Ask::Keys => "keys",
            Ask::Hashed => "hashed",
            Ask::Searched => "searched",
            Ask::Footprint(_) => "footprint",
            Ask::Outcomes(_) => "outcomes",
        }
    }
}

/// One read's answer, as plain data on its way to the caller.
enum KnowledgeValue {
    Keys(Vec<KeyRow>),
    Hashed(Vec<HashedRow>),
    Searched(SearchedRow),
    Footprint(String),
    Outcomes(Vec<Option<String>>),
}

/// The thread this process's machine lives on. The `ply` program performing these operations is
/// itself inside an entry; two entries do not nest on one thread, and a bisection and a mutation
/// each evaluate a program of their own. The load, the store, the binding, the pool and the
/// diagnosis all happen here, and only what a report is written from crosses back — which is also
/// what lets the front end outlive an iteration: it never leaves this thread.
struct Machine {
    go: Option<mpsc::Sender<Go>>,
    steps: mpsc::Receiver<Step>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Machine {
    fn start(args: TestOptions) -> Result<Machine, Diagnostic> {
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

/// One store and one warm front end for the whole process, however many reports are asked of it.
fn serve(args: &TestOptions, told: &mpsc::Sender<Step>, asked: &mpsc::Receiver<Go>) {
    let mut cache = Cache::open(&project_root(&args.path), args.no_cache);
    let mut warm = crate::warm::Warm::default();
    // What the last run kept, so a program can try mixtures of it long after the run finished.
    let mut hybrids: Option<ply_test::Hybrids> = None;
    while let Ok(signal) = asked.recv() {
        // A trial is not an iteration: it asks about the run that just ended.
        if let Go::Trial {
            failure,
            keys,
            reply,
        } = signal
        {
            let answer = match &mut cache {
                Ok(cache) => trial(&mut cache.store, hybrids.as_ref(), failure, &keys),
                Err(diagnostic) => Err(diagnostic.clone()),
            };
            let _ = reply.send(answer);
            continue;
        }
        match &mut cache {
            Ok(cache) => iterate(args, cache, &mut warm, told, asked, &mut hybrids),
            Err(diagnostic) => {
                let _ = told.send(Step::Loaded(Box::new(Err(Refused {
                    diagnostics: vec![diagnostic.clone()],
                    sources: SourceMap::new(),
                }))));
            }
        }
    }
}

fn iterate(
    args: &TestOptions,
    cache: &mut Cache,
    warm: &mut crate::warm::Warm,
    told: &mpsc::Sender<Step>,
    asked: &mpsc::Receiver<Go>,
    hybrids: &mut Option<ply_test::Hybrids>,
) {
    let mut warnings = std::mem::take(&mut cache.warnings);
    let opened = cache.store.take_warnings();
    warnings.extend(crate::migrate::notice(&cache.store, &opened));
    warnings.extend(opened);

    // The front end is a function of the sources, so an unmoved tree reuses it whole.
    let (held, reuse) = warm.take(&project_root(&args.path));
    let loaded = match held {
        Some(loaded) => Ok(loaded),
        None if args.no_cache => load(&args.path),
        None => crate::driver::load_incremental(&args.path, &mut cache.store),
    };
    let loaded = match loaded {
        Ok(mut loaded) => {
            if reuse == crate::warm::Reuse::Whole {
                // Nothing was re-derived, so this iteration reports no phase time.
                loaded.frontend.phases = crate::driver::Phases::default();
            }
            loaded
        }
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

    // A query naming nothing is wrong whatever the run does, so it refuses before anything runs
    // rather than being reported after a suite the user did not ask for.
    if let Some(query) = &args.mutate
        && let Err(diagnostic) = crate::mutate::targets(&loaded, query)
    {
        let _ = told.send(Step::Loaded(Box::new(Err(Refused {
            diagnostics: vec![diagnostic],
            sources: loaded.sources.clone(),
        }))));
        return;
    }

    // Part of a simulated test's cache key, so decided before selection.
    let search = crate::simulation::plan(&args.simulation);
    let hashes = loaded.hashes.clone();
    let plan = Plan::new(&loaded.check, args.filter.as_deref(), args.std);

    if let Some(err) = crate::costs::broken_promises(&loaded) {
        let _ = told.send(Step::Loaded(Box::new(Err(Refused {
            diagnostics: err.diagnostics,
            sources: err.sources,
        }))));
        return;
    }

    let _ = told.send(Step::Loaded(Box::new(Ok(found(
        args, &loaded, &hashes, &plan, &search, warnings,
    )))));
    // What a selector reads, before anything runs. It states its decision on the same channel, so
    // the machine has it by the time the binding decides whether a unit is worth building.
    let mut chosen = None;
    let knowledge = Knowledge::of(&loaded, &hashes, &cache.store, &search);
    if !serve_reads(asked, told, &knowledge, &cache.store, &mut chosen) {
        return;
    }
    if bind(
        args,
        cache,
        warm,
        &loaded,
        &hashes,
        plan,
        &search,
        told,
        asked,
        &knowledge,
        &mut chosen,
        hybrids,
    ) {
        // Only over a report that was written: an iteration that returned early leaves nothing held.
        warm.keep(loaded);
    }
}

/// What a selector reads: computed once per iteration, before anything runs. Plain data, so it
/// can cross from the machine's thread; the caller value-ifies it.
struct Knowledge {
    keys: Vec<KeyRow>,
    hashed: Vec<HashedRow>,
    searched: SearchedRow,
    /// Every test's footprint, in test order: a group's rendering is the union of the ones it names.
    footprints: Vec<Footprint>,
}

/// The store's answer under each key, as a report prints one: `passed`, `failed`, or nothing. The
/// keys are the caller's — a per-root narrowing computes them — so no row could have carried them.
fn outcomes_of(store: &ply_store::Store, keys: &[String]) -> Vec<Option<String>> {
    keys.iter()
        .map(|key| {
            ply_ty::DefHash::from_hex(key)
                .and_then(|hash| store.get(hash))
                .map(|outcome| {
                    if outcome.is_pass() {
                        "passed".to_string()
                    } else {
                        "failed".to_string()
                    }
                })
        })
        .collect()
}

impl Knowledge {
    /// The union of the named tests' footprints, printed. An index no test holds contributes
    /// nothing, so a program that asked about one finds out by the absence rather than a refusal.
    fn footprint_of(&self, tests: &[usize]) -> String {
        tests
            .iter()
            .filter_map(|&i| self.footprints.get(i))
            .fold(Footprint::empty(), |acc, f| acc.union(f))
            .to_string()
    }
}

/// One test the loaded tree declares, and what the store has under the key its result is filed
/// under.
#[derive(Clone)]
struct KeyRow {
    index: usize,
    /// The test's label, as a report prints it.
    label: String,
    /// The test's program-wide name: `<module>.<label>`.
    name: String,
    module: String,
    /// The key the result is filed under; `None` when the front end produced no hash.
    cache_key: Option<String>,
    /// Whether the key is the search's, rather than the test's own hash.
    seeded: bool,
    /// `test/nondet` opts out of the cache in both directions, so a selection never calls it
    /// cached whatever the store holds.
    nondet: bool,
    /// `Some("passed")` or `Some("failed")` when the store holds a result under that key.
    cached: Option<&'static str>,
}

/// One definition or test the loaded tree declares, and its hash.
#[derive(Clone)]
struct HashedRow {
    name: String,
    hash: String,
    test: bool,
}

/// The search a seeded test's key is computed against.
#[derive(Clone)]
struct SearchedRow {
    mode: String,
    roots: Vec<u64>,
    /// Numbers, not their printed forms: the program's own `Plan` type says `Int`, and a record
    /// crosses as a *dynamic* value, so nothing checks the two against each other.
    budget: u32,
    steps: u32,
}

impl Knowledge {
    fn of(
        loaded: &Loaded,
        hashes: &HashOutput,
        store: &ply_store::Store,
        search: &ply_eval::Plan,
    ) -> Knowledge {
        let keys = loaded
            .check
            .tests
            .iter()
            .enumerate()
            .map(|(index, test)| {
                // The same key the selection files a result under: the test's hash, or the
                // search's key when the test is keyed on the plan.
                let hash = hashes.tests.get(index).copied();
                let seeded = ply_test::is_seeded(&test.footprint);
                let key = hash.map(|h| ply_test::result_key(h, seeded, search));
                KeyRow {
                    index,
                    label: test.name.as_str().to_string(),
                    name: test.key.as_str().to_string(),
                    module: test.module.as_str().to_string(),
                    cache_key: key.map(|k| k.to_hex()),
                    seeded,
                    nondet: test.nondet,
                    cached: key.and_then(|k| store.get(k)).map(|outcome| {
                        if outcome.is_pass() {
                            "passed"
                        } else {
                            "failed"
                        }
                    }),
                }
            })
            .collect();
        let mut hashed: Vec<HashedRow> = hashes
            .defs
            .iter()
            .map(|(name, hash)| HashedRow {
                name: name.as_str().to_string(),
                hash: hash.to_hex(),
                test: false,
            })
            .collect();
        hashed.extend(
            loaded
                .check
                .tests
                .iter()
                .enumerate()
                .filter_map(|(index, test)| {
                    hashes.tests.get(index).map(|hash| HashedRow {
                        name: test.key.as_str().to_string(),
                        hash: hash.to_hex(),
                        test: true,
                    })
                }),
        );
        Knowledge {
            keys,
            hashed,
            footprints: loaded
                .check
                .tests
                .iter()
                .map(|t| t.footprint.clone())
                .collect(),
            searched: SearchedRow {
                mode: search.mode.as_str().to_string(),
                roots: search.roots.clone(),
                budget: search.budget,
                steps: search.steps,
            },
        }
    }
}

/// Serves reads until `Bind` arrives. `false` when the iteration is over instead.
fn serve_reads(
    asked: &mpsc::Receiver<Go>,
    told: &mpsc::Sender<Step>,
    knowledge: &Knowledge,
    store: &ply_store::Store,
    chosen: &mut Option<ply_test::Choice>,
) -> bool {
    serve_reads_loop(asked, told, knowledge, store, chosen, Sig::Bind)
}

/// The same, for the reads a selector makes between the binding and the run.
fn serve_reads_until_run(
    asked: &mpsc::Receiver<Go>,
    told: &mpsc::Sender<Step>,
    knowledge: &Knowledge,
    store: &ply_store::Store,
    chosen: &mut Option<ply_test::Choice>,
) -> bool {
    serve_reads_loop(asked, told, knowledge, store, chosen, Sig::Run)
}

/// Which of `Bind`/`Run` ends the wait.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sig {
    Bind,
    Run,
}

/// Answers reads until the signal arrives. Keeps the decision the program sent, if it sent one.
fn serve_reads_loop(
    asked: &mpsc::Receiver<Go>,
    told: &mpsc::Sender<Step>,
    knowledge: &Knowledge,
    store: &ply_store::Store,
    chosen: &mut Option<ply_test::Choice>,
    until: Sig,
) -> bool {
    loop {
        let got = asked.recv();
        // The ask travels back with the answer: a step's answer is only its own if the questions
        // match, and two of these questions carry the caller's arguments.
        let (asked, value) = match got {
            Ok(Go::Bind) => return until == Sig::Bind,
            Ok(Go::Run) => return until == Sig::Run,
            Ok(Go::Chosen(choice)) => {
                *chosen = Some(choice);
                continue;
            }
            Ok(Go::Keys) => (Ask::Keys, KnowledgeValue::Keys(knowledge.keys.clone())),
            Ok(Go::Hashed) => (
                Ask::Hashed,
                KnowledgeValue::Hashed(knowledge.hashed.clone()),
            ),
            Ok(Go::Searched) => (
                Ask::Searched,
                KnowledgeValue::Searched(knowledge.searched.clone()),
            ),
            Ok(Go::Footprint(tests)) => (
                Ask::Footprint(tests.clone()),
                KnowledgeValue::Footprint(knowledge.footprint_of(&tests)),
            ),
            Ok(Go::Outcomes(keys)) => (
                Ask::Outcomes(keys.clone()),
                KnowledgeValue::Outcomes(outcomes_of(store, &keys)),
            ),
            // A trial asks about the run that has *finished*; one arriving here is asking too
            // early, and the honest answer is that there is nothing to try yet.
            Ok(Go::Trial { reply, .. }) => {
                let _ = reply.send(Err(Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    "a mixture was asked for before the run it is a mixture of",
                )));
                continue;
            }
            Ok(Go::Load) | Err(_) => return false,
        };
        let _ = told.send(Step::Knowledge {
            asked,
            value: Box::new(value),
        });
    }
}

/// Whether a report was written, which is what decides if this front end is worth holding.
#[allow(clippy::too_many_arguments)]
fn bind(
    args: &TestOptions,
    cache: &mut Cache,
    warm: &mut crate::warm::Warm,
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: Plan,
    search: &ply_eval::Plan,
    told: &mpsc::Sender<Step>,
    asked: &mpsc::Receiver<Go>,
    knowledge: &Knowledge,
    chosen: &mut Option<ply_test::Choice>,
    hybrids: &mut Option<ply_test::Hybrids>,
) -> bool {
    let refuse = |diagnostics: Vec<Diagnostic>| {
        let _ = told.send(Step::Bound(Box::new(Some(Refused {
            diagnostics,
            sources: loaded.sources.clone(),
        }))));
        false
    };
    if let Err(diagnostic) = select_profile(&args.profile) {
        return refuse(vec![diagnostic]);
    };
    // One per run, shared by the workers; a run that decided to execute nothing builds no unit
    // unless a schema was named.
    let nothing_to_run = chosen.as_ref().map(|c| c.runs.is_empty()).unwrap_or(true);
    let schema_named = args.config.schema.is_some();
    let wanted = !nothing_to_run || schema_named;
    // The last iteration's unit, moved to this layout, when no definition's text changed.
    let held_unit = if wanted {
        warm.unit_for(&loaded.front, &loaded.sources)
    } else {
        None
    };
    let unit = if !wanted {
        None
    } else if let Some(held) = held_unit {
        Some(held)
    } else {
        match build_backend_over(&loaded.front, module_texts(&loaded.check, &loaded.sources)) {
            Ok(provider) => {
                warm.keep_unit(loaded.front.hashes_digest, provider);
                Some(provider)
            }
            Err(diagnostic) => return refuse(vec![diagnostic]),
        }
    };
    let constant = |name: &str| enter_constant(unit, name);
    // Before binding, so a missing required key fails before any host test runs.
    let (configuration, config_warnings) =
        match crate::config::Configuration::open(&loaded.check, args.host, &args.config, &constant)
        {
            Ok(resolved) => resolved,
            Err(diagnostics) => return refuse(diagnostics),
        };
    let hosts = match Hosts::open(
        &loaded.check,
        args.host,
        &args.tls,
        &args.fs,
        configuration,
        // `--trace` on this command names the definition trace, so records are discarded.
        &crate::trace::TraceOptions::silent(),
    ) {
        Ok(hosts) => hosts,
        Err(diagnostics) => return refuse(diagnostics),
    };
    let _ = told.send(Step::Bound(Box::new(None)));
    // A selector may still ask what the tree holds between the binding and the run.
    if !serve_reads_until_run(asked, told, knowledge, &cache.store, chosen) {
        return false;
    }
    let choice = chosen.clone().unwrap_or_default();
    let selection = decided(&choice, &plan, &loaded.check, search);
    let (over, mixtures) = execute(
        args,
        cache,
        loaded,
        hashes,
        &plan,
        &choice,
        &selection,
        search,
        &hosts,
        unit.filter(|_| !nothing_to_run),
        config_warnings,
    );
    // Kept for whatever the report asks next: a trial is about the run that just finished.
    *hybrids = Some(mixtures);
    let _ = told.send(Step::Ran(Box::new(over)));
    true
}

#[allow(clippy::too_many_arguments)]
fn execute(
    args: &TestOptions,
    cache: &mut Cache,
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: &Plan,
    choice: &ply_test::Choice,
    selection: &Selection,
    search: &ply_eval::Plan,
    hosts: &Hosts,
    provider: Option<&'static dyn ply_eval::Provider>,
    mut warnings: Vec<Diagnostic>,
) -> (Over, ply_test::Hybrids) {
    let (pool, workers) = build_pool(args.jobs, &mut warnings);
    let simulation = ply_test::Search::of(selection).measuring(args.simulation.measure_reduction);
    // A factory: a reactor belongs to its thread, and each worker builds its own machine.
    let runtime = hosts.runtime_factory();
    // A pooled test is measured on a worker thread of rayon's, which holds no thread-local budget
    // and reads the process's, so the process's is what bounds a run. The diagnosis and the
    // mutation below are measured on this thread, which sits inside the scope the `ply` program
    // was entered under -- one that zeroed the thread-local budgets -- and the lookup prefers a
    // thread-local to the process value, so those two need the scope as well as the setters.
    ply_codegen::rt::set_step_budget(args.steps);
    ply_codegen::rt::set_time_budget(args.timeout);
    let mut hybrids = ply_test::Hybrids {
        fresh: ply_store::body::of_front(&loaded.front),
        per_failure: Vec::new(),
    };
    let (report, mutants) = ply_codegen::rt::with_step_budget(args.steps, || {
        ply_codegen::rt::with_time_budget(args.timeout, || {
            let mut run = || {
                let mut executor = ply_test::InterpExecutor::new(&loaded.front)
                    .with_search(simulation.clone())
                    .with_hosts(hosting(hosts, &runtime));
                if let Some(provider) = provider {
                    executor = executor.with_backend(provider);
                }
                ply_test::run_with(
                    selection,
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
            // The search is the program's: what changed, what a mixture would need and why nothing
            // could be tried are handed over, and the program that reads the report decides.
            hybrids = ply_test::diagnose_failures(
                &mut report,
                &loaded.texts(),
                &loaded.front,
                &mut cache.store,
                !matches!(args.bisect, When::Never),
            );
            let escapes = hosts_escapes(&report, &loaded.check, hosts);
            let ok = report.is_success() && escapes.is_empty();
            // Only over a green program: a survivor of a red one says nothing.
            let mutants = match (&args.mutate, ok) {
                (Some(query), true) => match crate::mutate::targets(loaded, query) {
                    Ok(targets) => Some(Ok(crate::mutate::run(
                        loaded,
                        hashes,
                        &targets,
                        args.mutate_budget,
                        search,
                        choice,
                        plan,
                        hosts,
                        &runtime,
                    ))),
                    Err(diagnostic) => Some(Err(diagnostic)),
                },
                _ => None,
            };
            (report, mutants)
        })
    });
    warnings.extend(report.warnings.iter().cloned());
    // Pass records are read lazily, so an unreadable baseline only surfaces here.
    warnings.extend(cache.store.take_warnings());

    let counts = counts(plan, selection, &loaded.check, hosts);
    let mut escapes = hosts_escapes(&report, &loaded.check, hosts);
    if let Some(unbuilt) = unbuilt_backend(provider) {
        escapes.push(unbuilt);
    }
    let mutants = match mutants {
        Some(Ok(report)) => Some(mutants_view(&report, loaded)),
        // The query was resolved before the run, so this is a target that moved under it.
        Some(Err(diagnostic)) => {
            escapes.push(diagnostic);
            None
        }
        None => None,
    };
    let over = Over {
        hermetic: hosts.is_hermetic(),
        label: hosts.label().to_string(),
        operations: hosts.listing().rows.len(),
        digest: hosts::digest_short(hosts.listing(), &hosts.disclosures()),
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
        backend: Some(backend_view(provider, &report)),
        failures: report
            .failures
            .iter()
            .enumerate()
            .map(|(i, f)| {
                fault(
                    f,
                    loaded,
                    hashes,
                    &report,
                    hybrids.per_failure.get(i).and_then(|slot| slot.as_ref()),
                )
            })
            .collect(),
        results: report.results.iter().map(outcome).collect(),
        summary: report_summary(&report),
        simulation: report.simulation,
        escapes,
        warnings: once_each(warnings),
        mutants,
        coverage: args
            .coverage
            .then(|| crate::mutate::coverage_json(loaded, hashes)),
    };
    (over, hybrids)
}

// --- Counts under `--filter` --------------------------------------------------

/// Counts under `--filter` use the filtered set as their denominator. What runs is the program's
/// decision; this is only which tests the run reports on.
pub struct Plan {
    /// Test indices still in scope, ascending.
    pub visible: Vec<usize>,
    pub filtered_out: usize,
    /// Test indices this run was never asked to decide: a shipped module's tests without `--std`.
    pub out_of_scope: BTreeSet<usize>,
}

impl Plan {
    /// `std_tests` is `--std`.
    pub fn new(check: &CheckOutput, filter: Option<&str>, std_tests: bool) -> Plan {
        let in_scope = |t: &ply_ty::TestInfo| std_tests || !crate::shelf::is_shipped(&t.module);
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
        Plan {
            filtered_out: scoped - visible.len(),
            visible,
            out_of_scope,
        }
    }
}

/// The runtime's view of what the program decided, under this run's own filter: the tests it keeps,
/// the classes filtered to them, and the roots each still owes. `--filter` cannot change which
/// tests conflict, so a class only loses members.
pub(crate) fn decided(
    choice: &ply_test::Choice,
    plan: &Plan,
    check: &CheckOutput,
    search: &ply_eval::Plan,
) -> Selection {
    let keeps = |i: &usize| plan.visible.binary_search(i).is_ok();
    let filtered = ply_test::Choice {
        runs: choice.runs.iter().copied().filter(keeps).collect(),
        groups: choice
            .groups
            .iter()
            .map(|class| class.iter().copied().filter(keeps).collect::<Vec<usize>>())
            .filter(|class| !class.is_empty())
            .collect(),
        narrowed: choice
            .narrowed
            .iter()
            .filter(|(index, _)| keeps(index))
            .map(|(index, roots)| (*index, roots.clone()))
            .collect(),
        reasons: choice.reasons.clone(),
    };
    let mut selection = Selection::chosen(&filtered, check, &plan.visible, search);
    selection.out_of_scope = plan.out_of_scope.clone();
    selection
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
fn counts(plan: &Plan, selection: &Selection, check: &CheckOutput, hosts: &Hosts) -> hosts::Counts {
    let parallelism = &selection.parallelism;
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
                let isolated = selection
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
    /// The same atoms, unprinted: what a scheduler compares is an effect, a resource and whether
    /// the atom writes. Two atoms conflict when they agree on the first two and one of them writes;
    /// whether a test is isolated, and whether its contention is only over a region label, are
    /// questions about exactly this list, so the program answers them itself.
    contends: Vec<AtomView>,
    seeded: bool,
}

/// One atom of a test's shared footprint, as a scheduler compares them.
struct AtomView {
    effect: String,
    resource: String,
    writes: bool,
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
    filtered_out: usize,
    plan: (String, Vec<u64>, u32, u32),
    warnings: Vec<Diagnostic>,
    options: Value,
}

struct Over {
    hermetic: bool,
    label: String,
    operations: usize,
    digest: String,
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
    fragment: usize,
    offered: u64,
    entered: u64,
    declined: u64,
    converted_in: u64,
    converted_out: u64,
    units: Option<u64>,
    analysis_nanos: Option<u64>,
    codegen_nanos: Option<u64>,
}

struct OutcomeView {
    index: usize,
    name: String,
    hash: Option<String>,
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
    virtual_time_ns: i64,
    failing_seed: Option<String>,
}

struct SuspectView {
    name: String,
    /// The definition's current hash, so a program that re-publishes the list loses nothing: the
    /// artifact's suspects carry it, and the row's did not.
    hash: Option<String>,
    change: Option<String>,
    ran: Option<bool>,
    depth: Option<usize>,
    culprit: bool,
}

struct FaultView {
    key: String,
    diagnostic: Diagnostic,
    /// The interpreter failed rather than the program, and the failing run reached a host handler:
    /// two of the gate's three answers, the third being the program's own `bisect` mode.
    defect: bool,
    host: bool,
    /// What a program that searches for itself needs: the change set, what the classifier could not
    /// tell apart, the reason to give when there is nothing to try, and where each definition is.
    search: Option<ChangeSetView>,
    conclusive: bool,
    requested: bool,
    reason: String,
    /// The verdict as the artifact publishes it: the cases by word, the groups, and the counts a
    /// reader sees beside the answer. A program that searched replaces all of these.
    verdict: &'static str,
    skipped: Option<&'static str>,
    confidence: &'static str,
    groups: Vec<Vec<String>>,
    /// The counts the search would have published. Named `stats` because `search` above is the
    /// change set a program that decides reads.
    stats: ply_test::SearchStats,
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

/// Every name a change set mentions: its own change, the changes, and the fused groups' members.
fn delta_names(delta: &ply_test::bisect::Delta) -> Vec<&Symbol> {
    let mut out: Vec<&Symbol> = Vec::new();
    if let Some(own) = &delta.test {
        out.push(&own.name);
    }
    out.extend(delta.changes.iter().map(|c| &c.name));
    out.extend(delta.clusters.iter().flat_map(|c| c.members.iter()));
    out
}

/// One failure's change set, as the program reads it. The names are the change set's own, and each
/// carries the place `ply` would print for it.
struct ChangeSetView {
    delta: ply_test::bisect::Delta,
    classified: usize,
    test_classified: bool,
    absent: ply_test::bisect::Skipped,
    at: Vec<(String, Option<Span>)>,
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
    args: &TestOptions,
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: &Plan,
    search: &ply_eval::Plan,
    warnings: Vec<Diagnostic>,
) -> Found {
    let check = &loaded.check;
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
                    contends: ply_test::shared_footprint(&test.footprint)
                        .atoms()
                        .map(|a| AtomView {
                            effect: a.effect.to_string(),
                            resource: a.resource.to_string(),
                            writes: a.mode == Mode::Write,
                        })
                        .collect(),
                    seeded: ply_test::is_seeded(&test.footprint),
                })
            })
            .collect(),
        filtered_out: plan.filtered_out,
        plan: (
            search.mode.as_str().to_string(),
            search.roots.clone(),
            search.budget,
            search.steps,
        ),
        warnings,
        options: jsonlit!({
            "bisect": args.bisect.as_str(),
            "bisect_budget": args.bisect_budget,
            "trace": args.trace.as_str(),
            // The whole plan: every field is in a seeded test's cache key.
            "sim": {
                "mode": search.mode.as_str(),
                "seed": args.simulation.seed.as_ref().map(|s| s.to_string()),
                "seeds": search.roots.len(),
                "budget": u64::from(search.budget),
                "steps": u64::from(search.steps),
                "measure_reduction": args.simulation.measure_reduction,
            },
        }),
    }
}

fn backend_view(
    provider: Option<&'static dyn ply_eval::Provider>,
    report: &RunReport,
) -> BackendView {
    let offers = provider.map_or_else(Default::default, ply_eval::Provider::offers);
    let compiled = provider.and_then(ply_eval::Provider::compilation);
    BackendView {
        name: provider.map_or("c", ply_eval::Provider::name).to_string(),
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
            steps: e.steps,
            virtual_time_ns: e.virtual_time,
            failing_seed: e.failure.as_ref().map(|s| s.to_string()),
        }),
        cached: result.recorded.as_ref().map(Record::is_written),
    }
}

fn suspect_view(suspect: &Suspect) -> SuspectView {
    SuspectView {
        name: suspect.name.to_string(),
        hash: suspect.hash.map(|h| h.to_hex()),
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
    input: Option<&ply_test::HybridInput>,
) -> FaultView {
    let check = &loaded.check;
    let index = check.tests.iter().position(|t| t.key == failure.key);
    let test = index.and_then(|i| check.tests.get(i));
    let bisection = &failure.attribution.bisection;
    let search = input.map(|input| ChangeSetView {
        delta: input.delta.clone(),
        classified: input.classified,
        test_classified: input.test_classified,
        absent: input.absent,
        // Every name the change set mentions, with its place: a verdict that this program searched
        // has no spans of its own, and a report prints where each culprit is.
        at: {
            let mut names: Vec<&Symbol> = delta_names(&input.delta);
            names.sort();
            names.dedup();
            names
                .into_iter()
                .map(|name| {
                    (
                        name.as_str().to_string(),
                        check.defs.get(name).map(|def| def.span),
                    )
                })
                .collect()
        },
    });
    FaultView {
        key: failure.key.as_str().to_string(),
        diagnostic: failure.diagnostic.clone(),
        defect: failure.defect,
        host: failure.host,
        search,
        conclusive: bisection.is_conclusive(),
        // Silent when no bisection was asked for.
        requested: !matches!(
            bisection.verdict,
            Verdict::NotAttempted(Skipped::NotRequested)
        ),
        reason: bisection.reason.clone(),
        verdict: bisection.verdict.as_str(),
        skipped: bisection.verdict.skipped().map(|why| why.as_str()),
        confidence: bisection.confidence.as_str(),
        groups: bisection
            .groups
            .iter()
            .map(|group| group.iter().map(|n| n.as_str().to_string()).collect())
            .collect(),
        stats: bisection.search,
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

fn mutants_view(report: &crate::mutate::Report, loaded: &Loaded) -> MutantsView {
    MutantsView {
        killed: report.killed(),
        survived: report.survived(),
        skipped: report.skipped() + report.unresolved(),
        budget_spent: report.budget_spent,
        unreached: report.unreached.iter().map(|n| n.to_string()).collect(),
        survivors: report
            .judged
            .iter()
            .filter(|j| matches!(j.verdict, crate::mutate::Verdict::Survived))
            .map(|j| {
                (
                    j.mutant.definition.as_str().to_string(),
                    j.mutant.from.clone(),
                    j.mutant.to.clone(),
                    j.mutant.span,
                )
            })
            .collect(),
        json: crate::mutate::to_json(report, loaded),
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

fn atoms_value(atoms: &[AtomView]) -> PlyValue {
    PlyValue::list(
        atoms
            .iter()
            .map(|a| {
                record(vec![
                    ("effect", PlyValue::str(&a.effect)),
                    ("resource", PlyValue::str(&a.resource)),
                    ("writes", PlyValue::Bool(a.writes)),
                ])
            })
            .collect(),
    )
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
        ("contends", atoms_value(&case.contends)),
        ("seeded", PlyValue::Bool(case.seeded)),
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
        ("filtered_out", count(found.filtered_out)),
        (
            "plan",
            record(vec![
                ("mode", PlyValue::str(&found.plan.0)),
                (
                    "roots",
                    PlyValue::list(
                        found
                            .plan
                            .1
                            .iter()
                            .map(|&r| PlyValue::Int(r as i64))
                            .collect(),
                    ),
                ),
                ("seeds", count(found.plan.1.len())),
                ("budget", count(found.plan.2 as usize)),
                ("steps", count(found.plan.3 as usize)),
            ]),
        ),
        ("warnings", diags_value(&found.warnings)),
        ("options", json(&found.options)),
    ])
}

/// One read's answer, as the program reads it.
fn knowledge_value(value: &KnowledgeValue) -> PlyValue {
    match value {
        KnowledgeValue::Keys(rows) => PlyValue::list(
            rows.iter()
                .map(|row| {
                    record(vec![
                        ("index", count(row.index)),
                        ("label", PlyValue::str(&row.label)),
                        ("name", PlyValue::str(&row.name)),
                        ("module", PlyValue::str(&row.module)),
                        (
                            "cache_key",
                            option(row.cache_key.as_deref().map(PlyValue::str)),
                        ),
                        ("seeded", PlyValue::Bool(row.seeded)),
                        ("nondet", PlyValue::Bool(row.nondet)),
                        ("cached", option(row.cached.map(PlyValue::str))),
                    ])
                })
                .collect(),
        ),
        KnowledgeValue::Hashed(rows) => PlyValue::list(
            rows.iter()
                .map(|row| {
                    record(vec![
                        ("name", PlyValue::str(&row.name)),
                        ("hash", PlyValue::str(&row.hash)),
                        ("test", PlyValue::Bool(row.test)),
                    ])
                })
                .collect(),
        ),
        KnowledgeValue::Footprint(text) => PlyValue::str(text),
        KnowledgeValue::Outcomes(answers) => PlyValue::list(
            answers
                .iter()
                .map(|answer| option(answer.as_deref().map(PlyValue::str)))
                .collect(),
        ),
        KnowledgeValue::Searched(row) => record(vec![
            (
                "roots",
                PlyValue::list(row.roots.iter().map(|&r| PlyValue::Int(r as i64)).collect()),
            ),
            ("mode", PlyValue::str(&row.mode)),
            ("seeds", count(row.roots.len())),
            ("budget", count(row.budget as usize)),
            ("steps", count(row.steps as usize)),
        ]),
    }
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
        ("virtual_time_ns", PlyValue::Int(search.virtual_time_ns)),
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
        // Two of the gate's three answers; the mode is the program's own flag.
        ("defect", PlyValue::Bool(f.defect)),
        ("host", PlyValue::Bool(f.host)),
        // What a program that searches for itself reads: the change set, what could not be told
        // apart, the reason to give when there is nothing to try, and where each name is.
        ("search", option(f.search.as_ref().map(change_set_value))),
        (
            "bisect",
            record(vec![
                ("conclusive", PlyValue::Bool(f.conclusive)),
                ("requested", PlyValue::Bool(f.requested)),
                ("reason", PlyValue::str(&f.reason)),
                ("verdict", PlyValue::str(f.verdict)),
                ("skipped", option(f.skipped.map(PlyValue::str))),
                ("confidence", PlyValue::str(f.confidence)),
                (
                    "groups",
                    PlyValue::list(
                        f.groups
                            .iter()
                            .map(|g| PlyValue::list(g.iter().map(PlyValue::str).collect()))
                            .collect(),
                    ),
                ),
                (
                    "search",
                    record(vec![
                        ("candidates", count(f.stats.candidates)),
                        ("clusters", count(f.stats.clusters)),
                        ("evaluated", count(f.stats.evaluated)),
                        ("cached", count(f.stats.cached)),
                        ("memoized", count(f.stats.memoized)),
                        ("unresolved", count(f.stats.unresolved)),
                        ("exhausted", PlyValue::Bool(f.stats.exhausted)),
                    ]),
                ),
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
                            ("hash", option(x.hash.as_deref().map(PlyValue::str))),
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
                    ("fragment", count(b.fragment)),
                    ("offered", tally(b.offered)),
                    ("entered", tally(b.entered)),
                    ("declined", tally(b.declined)),
                    ("converted_in", tally(b.converted_in)),
                    ("converted_out", tally(b.converted_out)),
                    ("units", option(b.units.map(tally))),
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

// --- The options the program parses -----------------------------------------------

/// The options record as the program builds it from the parsed line, read field by field. The
/// program validated already, so a bad value here is an internal error.
pub fn test_options_of(v: &PlyValue, span: Span) -> Result<TestOptions, Diagnostic> {
    if std::env::var("PLY_DEBUG_OPTIONS").is_ok() {
        eprintln!("{v}");
    }
    use crate::payload::{field_of, missing, opt_int_at, opt_str_at, str_list_at};
    let bool_at = |name: &str| field_of(v, name, span)?.as_bool(span, name);
    let int_at = |name: &str| field_of(v, name, span)?.as_int(span, name);
    let str_at = |name: &str| {
        field_of(v, name, span)?
            .as_str(span, name)
            .map(str::to_string)
    };
    let when_at = |name: &str| -> Result<When, Diagnostic> {
        Ok(match str_at(name)?.as_str() {
            "always" => When::Always,
            "never" => When::Never,
            _ => When::Auto,
        })
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
                certificate: std::path::PathBuf::from(
                    field_of(item, "cert", span)?.as_str(span, "a certificate")?,
                ),
                key: std::path::PathBuf::from(field_of(item, "key", span)?.as_str(span, "a key")?),
            });
        }
        Ok(out)
    };
    let sim = field_of(v, "sim", span)?;
    let config = field_of(v, "config", span)?;
    Ok(TestOptions {
        path: std::path::PathBuf::from(str_at("path")?),
        json: bool_at("json")?,
        explain: bool_at("explain")?,
        no_cache: bool_at("no_cache")?,
        filter: opt_str_at(v, "filter", span)?,
        jobs: opt_int_at(v, "jobs", span)?.map(|n| n as u32),
        steps: int_at("steps")?,
        timeout: int_at("timeout")? as u64,
        bisect: when_at("bisect")?,
        bisect_budget: int_at("bisect_budget")? as usize,
        coverage: bool_at("coverage")?,
        mutate: opt_str_at(v, "mutate", span)?,
        mutate_budget: int_at("mutate_budget")? as usize,
        trace: when_at("trace")?,
        profile: str_at("profile")?,
        watch: bool_at("watch")?,
        host: bool_at("host")?,
        tls: crate::options::TlsOptions {
            tls: cred_list("tls")?,
            trust: str_list_at(v, "trust", span)?
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
        config: crate::config::ConfigOptions {
            set: str_list_at(config, "set", span)?,
            files: str_list_at(config, "files", span)?
                .into_iter()
                .map(std::path::PathBuf::from)
                .collect(),
            schema: opt_str_at(config, "schema", span)?,
        },
        std: bool_at("std")?,
        simulation: crate::simulation::SimOptions {
            seed: match opt_str_at(sim, "seed", span)? {
                Some(text) => Some(
                    ply_eval::Seed::parse(&text).ok_or_else(|| missing("a parsed seed", span))?,
                ),
                None => None,
            },
            sim: match field_of(sim, "mode", span)?.as_str(span, "the simulation's mode")? {
                "once" => ply_eval::SimMode::Once,
                "random" => ply_eval::SimMode::Random,
                _ => ply_eval::SimMode::Dpor,
            },
            seeds: opt_int_at(sim, "seeds", span)?.map(|n| n as u32),
            sim_budget: opt_int_at(sim, "budget", span)?.map(|n| n as u32),
            sim_steps: opt_int_at(sim, "steps", span)?.map(|n| n as u32),
            measure_reduction: field_of(sim, "measure_reduction", span)?
                .as_bool(span, "measure_reduction")?,
        },
    })
}

impl Default for TestOptions {
    fn default() -> TestOptions {
        TestOptions {
            path: std::path::PathBuf::from("."),
            json: false,
            explain: false,
            no_cache: false,
            filter: None,
            jobs: None,
            steps: ply_eval::DEFAULT_STEP_BUDGET,
            timeout: 60_000,
            bisect: When::Auto,
            bisect_budget: 64,
            coverage: false,
            mutate: None,
            mutate_budget: 64,
            trace: When::Auto,
            profile: "development".to_string(),
            watch: false,
            host: false,
            tls: crate::options::TlsOptions::default(),
            fs: Vec::new(),
            config: crate::config::ConfigOptions::default(),
            std: false,
            simulation: crate::simulation::SimOptions::default(),
        }
    }
}

/// One mixture of one failure, tried on this thread: the store and the warm bodies are here, and
/// the hybrid that swaps definitions is not `Send`.
fn trial(
    store: &mut ply_store::Store,
    hybrids: Option<&ply_test::Hybrids>,
    failure: usize,
    keys: &[(String, String)],
) -> Result<ply_test::bisect::Trial, Diagnostic> {
    let Some(hybrids) = hybrids else {
        return Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("no mixture was kept for failure {failure}"),
        ));
    };
    let Some(Some(input)) = hybrids.per_failure.get(failure) else {
        return Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("no mixture was kept for failure {failure}"),
        ));
    };
    let Some((mixture, test)) = &input.runnable else {
        return Ok(ply_test::bisect::Trial::unresolved(
            ply_test::bisect::Unresolved::MissingBody,
        ));
    };
    let wanted: std::collections::BTreeSet<ply_test::bisect::DefKey> = keys
        .iter()
        .filter_map(|(name, ns)| {
            let ns = match ns.as_str() {
                "value" => ply_test::bisect::Ns::Value,
                "declaration" => ply_test::bisect::Ns::Decl,
                _ => return None,
            };
            Some(ply_test::bisect::DefKey {
                name: Symbol::new(name.as_str()),
                ns,
            })
        })
        .collect();
    let hybrid = ply_test::BodyHybrid::new(
        store,
        &hybrids.fresh,
        mixture.clone(),
        test.clone(),
        input.signature.clone(),
    );
    let mut hybrid = match &input.seed {
        Some(seed) => hybrid.at_seed(seed),
        None => hybrid,
    };
    let trial = hybrid.trial_over(wanted);
    // A mixture that went green is a program whose definitions all pass at once, so what it proved
    // may be cached — under the mixture's own test hash. The failing test's hash is a different
    // test's, so a red test can never be passed by a mixture of it.
    // A mixture that went green is a program whose definitions all pass at once, so what it proved
    // may be cached — under the mixture's own test hash. The failing test's hash is a different
    // test's, so a red test can never be passed by a mixture of it.
    for hash in hybrid.take_proved() {
        store.put(hash, ply_store::Outcome::Pass);
    }
    Ok(trial)
}

/// One trial's outcome, as the program reads it: the case, and whether the runtime answered from a
/// result it already had.
fn trial_value(trial: &ply_test::bisect::Trial) -> PlyValue {
    let outcome = match trial.outcome {
        ply_test::bisect::TrialOutcome::Fails => crate::payload::ctor(BISECT, "Fails", Vec::new()),
        ply_test::bisect::TrialOutcome::Passes => {
            crate::payload::ctor(BISECT, "Passes", Vec::new())
        }
        // The case names are the ones `suite.bisect` declares, so the program matches on them.
        ply_test::bisect::TrialOutcome::Unresolved(why) => crate::payload::ctor(
            BISECT,
            "Unresolved",
            vec![crate::payload::ctor(
                BISECT,
                match why {
                    ply_test::bisect::Unresolved::DoesNotCheck => "DoesNotCheck",
                    ply_test::bisect::Unresolved::DifferentFailure => "DifferentFailure",
                    ply_test::bisect::Unresolved::MissingBody => "MissingBody",
                    ply_test::bisect::Unresolved::BudgetSpent => "BudgetSpent",
                },
                Vec::new(),
            )],
        ),
    };
    record(vec![
        ("outcome", outcome),
        ("cached", PlyValue::Bool(trial.cached)),
    ])
}

/// One failure's change set, as the program reads it.
fn change_set_value(view: &ChangeSetView) -> PlyValue {
    record(vec![
        ("delta", delta_value(&view.delta)),
        ("classified", count(view.classified)),
        ("test_classified", PlyValue::Bool(view.test_classified)),
        (
            "absent",
            crate::payload::ctor(BISECT, skipped_ctor(view.absent), Vec::new()),
        ),
        (
            "at",
            PlyValue::list(
                view.at
                    .iter()
                    .map(|(name, span)| {
                        record(vec![
                            ("name", PlyValue::str(name)),
                            ("at", option(span.and_then(placed))),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// The change set, in the shapes `suite.delta` declares: the names are the cases of its own ADTs,
/// because that is how a value crosses.
fn delta_value(delta: &ply_test::bisect::Delta) -> PlyValue {
    record(vec![
        ("own", option(delta.test.as_ref().map(change_value))),
        (
            "changes",
            PlyValue::list(delta.changes.iter().map(change_value).collect()),
        ),
        (
            "clusters",
            PlyValue::list(delta.clusters.iter().map(cluster_value).collect()),
        ),
        ("unclassified", count(delta.unclassified)),
    ])
}

fn change_value(change: &ply_test::bisect::Change) -> PlyValue {
    record(vec![
        ("name", PlyValue::str(change.name.as_str())),
        (
            "ns",
            crate::payload::ctor(DELTA, ns_ctor(change.ns), Vec::new()),
        ),
        (
            "before",
            option(change.before.map(|h| PlyValue::str(h.to_hex()))),
        ),
        (
            "after",
            option(change.after.map(|h| PlyValue::str(h.to_hex()))),
        ),
        (
            "kind",
            crate::payload::ctor(DELTA, kind_ctor(change.kind), Vec::new()),
        ),
        ("independent", PlyValue::Bool(change.independent)),
    ])
}

fn cluster_value(cluster: &ply_test::bisect::Cluster) -> PlyValue {
    record(vec![
        (
            "members",
            strings(cluster.members.iter().map(|n| n.as_str())),
        ),
        (
            "keys",
            PlyValue::list(
                cluster
                    .keys
                    .iter()
                    .map(|k| {
                        record(vec![
                            ("name", PlyValue::str(k.name.as_str())),
                            ("ns", crate::payload::ctor(DELTA, ns_ctor(k.ns), Vec::new())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "reason",
            crate::payload::ctor(DELTA, reason_ctor(cluster.reason), Vec::new()),
        ),
    ])
}

fn ns_ctor(ns: ply_test::bisect::Ns) -> &'static str {
    match ns {
        ply_test::bisect::Ns::Value => "Value",
        ply_test::bisect::Ns::Decl => "Decl",
    }
}

fn kind_ctor(kind: ply_test::bisect::ChangeKind) -> &'static str {
    match kind {
        ply_test::bisect::ChangeKind::Edited => "Edited",
        ply_test::bisect::ChangeKind::Derived => "Derived",
        ply_test::bisect::ChangeKind::Added => "Added",
        ply_test::bisect::ChangeKind::Removed => "Removed",
    }
}

fn reason_ctor(reason: ply_test::bisect::FusionReason) -> &'static str {
    match reason {
        ply_test::bisect::FusionReason::Independent => "Independent",
        ply_test::bisect::FusionReason::InterfaceChanged => "InterfaceChanged",
        ply_test::bisect::FusionReason::Existence => "Existence",
        ply_test::bisect::FusionReason::Component => "Component",
    }
}

fn skipped_ctor(skipped: ply_test::bisect::Skipped) -> &'static str {
    match skipped {
        ply_test::bisect::Skipped::NotRequested => "NotRequested",
        ply_test::bisect::Skipped::NeverPassed => "NeverPassed",
        ply_test::bisect::Skipped::Host => "Host",
        ply_test::bisect::Skipped::Nondet => "Nondet",
        ply_test::bisect::Skipped::Panicked => "Panicked",
        ply_test::bisect::Skipped::NoChanges => "NoChanges",
        ply_test::bisect::Skipped::NoBodies => "NoBodies",
        ply_test::bisect::Skipped::NoHybrids => "suite.bisect.NoHybrids",
        ply_test::bisect::Skipped::Delegated => "Delegated",
    }
}

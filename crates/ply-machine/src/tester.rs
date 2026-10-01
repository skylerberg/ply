//! What `ply test` loads, binds and runs, as the program in `crates/ply-cli/ply/tests.ply`
//! performs it.
//!
//! The front end, the store, the compiled backend, the host binding, running one test or one
//! interleaving of it, the per-test unwind catching and the building of a mixture stay here: a front
//! end is not a value a program can hold, a Rust unwind is not a Ply value, and the store's on-disk
//! format has one reader. Which tests run, in which classes and lanes, which interleavings a seeded
//! test is searched at, the keys each result is read and filed under, why a failure happened and
//! everything said about it are the program's: this side answers what it knows and does what it is
//! told.

use crate::hosts::{self, Hosts, Lent, hosting};
use crate::load::{Loaded, project_root};
use crate::options::When;
use crate::payload::{
    count, diags_value, json, option, places_value, raised_value, record, strings,
};
use crate::support::{build_backend_over, enter_constant, module_texts, once_each, select_profile};
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_eval::{
    CheckOutput, Diagnostic, Footprint, HashOutput, Mode, SourceMap, Span, Symbol,
    Value as PlyValue, codes,
};
use ply_store::Store;
use ply_test::{Cost, Record, RunReport, Selection, Status, TestResult};
use serde_json::{Value, json as jsonlit};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::{Duration, Instant};

/// The effect `crates/ply-cli/ply/tests.ply` declares. It is lent to that one entry and nowhere
/// else: no other command runs a corpus.
const EFFECT: &str = "tester";

/// Where each type this side marshals is declared, and every case it builds of it.
///
/// A constructor crosses by its program-wide name, `<module>.<Case>`, and a name the program does
/// not declare is a placeless `no arm of this match matched` the moment it matches the value. So a
/// case is built only through [`case`], which refuses one this table does not list, and
/// `every_case_the_tester_builds_is_declared_where_it_says` holds the table to the program.
pub const MARSHALLED: &[(&str, &str, &[&str])] = &[
    ("suite.delta", "Ns", &["Value", "Decl"]),
    ("suite.bisect", "Skipped", &["NoBodies", "NoHybrids"]),
    (
        "suite.bisect",
        "TrialOutcome",
        &["Fails", "Passes", "Unresolved"],
    ),
    (
        "suite.bisect",
        "Unresolved",
        &["DoesNotCheck", "DifferentFailure", "MissingBody"],
    ),
];

/// One case of a type this side marshals, under the name the program declares it by.
fn case(ty: &str, name: &str, args: Vec<PlyValue>) -> PlyValue {
    let (home, _, cases) = MARSHALLED
        .iter()
        .find(|(_, declared, _)| *declared == ty)
        .unwrap_or_else(|| panic!("`{ty}` is not a type this side marshals"));
    assert!(
        cases.contains(&name),
        "`{name}` is not a case of `{ty}` this side builds"
    );
    crate::payload::ctor(home, name, args)
}

const OPERATIONS: [(&str, &str); 18] = [
    ("configure", "ply_machine::tester::configure"),
    ("loaded", "ply_machine::test::loaded"),
    ("bound", "ply_machine::test::bound"),
    ("stamped", "ply_machine::test::stamped"),
    // What a selector computes a selection from, before anything runs.
    ("keys", "ply_machine::tester::keys"),
    ("hashed", "ply_machine::tester::hashed"),
    // The run the program schedules: a test once, or one interleaving of it, on whichever thread
    // asks, and what they came to.
    ("started", "ply_machine::tester::started"),
    ("executed", "ply_machine::tester::executed"),
    ("interleaved", "ply_machine::tester::interleaved"),
    ("concluded", "ply_machine::tester::concluded"),
    // A mutation the program judges a mutant at a time, after a green run.
    ("mutated", "ply_machine::tester::mutated"),
    ("mutant", "ply_machine::tester::mutant"),
    ("mutation", "ply_machine::tester::mutation"),
    ("trial", "ply_machine::tester::trial"),
    ("record", "ply_machine::tester::record"),
    ("chosen", "ply_machine::tester::chosen"),
    ("outcomes", "ply_machine::tester::outcomes"),
    // The printed union of a set of tests' footprints: the program colours the graph, and the
    // rendering of a colour is the compiler's.
    ("footprint", "ply_machine::tester::footprint"),
];

/// A compiled body honours its call bound on the native stack, where unoptimised frames run to
/// kilobytes. Reserved, not committed.
const RUN_STACK: usize = 256 << 20;

/// What `ply test` is configured with, as plain data: the shell's parsed flags convert into
/// this. The machine a configuration starts lives until the next one, so a `--watch` iteration
/// over an unmoved tree re-derives nothing.
#[derive(Clone, Debug)]
pub struct TestOptions {
    pub path: std::path::PathBuf,
    pub json: bool,
    pub explain: bool,
    pub no_cache: bool,
    /// `--filter`'s substrings: a test any of them matches runs, and none runs everything.
    pub filters: Vec<String>,
    pub jobs: Option<u32>,
    pub steps: i64,
    pub timeout: u64,
    pub bisect: When,
    pub bisect_budget: usize,
    pub coverage: bool,
    pub mutate: Option<String>,
    pub mutate_budget: usize,
    pub profile: String,
    pub watch: bool,
    pub host: bool,
    pub tls: crate::options::TlsOptions,
    pub fs: Vec<ply_host::fs::RootSpec>,
    /// The programs a test's `process.spawn` may start: the only `process` operation a test binds.
    pub exec: Vec<ply_host::process::ExecSpec>,
    /// The privileged families `--allow` lends the tests, which the program must declare.
    pub allow: Vec<String>,
    pub config: crate::config::ConfigOptions,
    pub std: bool,
}

pub struct Session(Arc<Site>);

impl Session {
    pub fn new(args: &TestOptions) -> Session {
        Session(Arc::new(Site {
            args: Mutex::new(args.clone()),
            machine: Mutex::new(None),
            running: RwLock::new(None),
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

/// The decision, as the program sent it: the same fields `ply_test::Choice` holds.
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
    let mut groups = Vec::new();
    for class in field_of(v, "groups", span)?.as_list(span, "the classes")? {
        groups.push(ints_of(class, span, "a class")?);
    }
    let mut filed = std::collections::BTreeMap::new();
    for entry in field_of(v, "filed", span)?.as_list(span, "where each pass is filed")? {
        let index = field_of(entry, "index", span)?.as_int(span, "a test index")? as usize;
        let mut keys = Vec::new();
        for key in field_of(entry, "keys", span)?.as_list(span, "the keys a pass is filed under")? {
            keys.push(hash_of(key.as_str(span, "a key")?, span)?);
        }
        filed.insert(index, keys);
    }
    Ok(ply_test::Choice {
        runs,
        reasons,
        groups,
        filed,
    })
}

/// A key the program computed, as the store is keyed.
fn hash_of(hex: &str, span: Span) -> Result<ply_eval::DefHash, Diagnostic> {
    ply_eval::DefHash::from_hex(hex).ok_or_else(|| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("`{hex}` is not a key the store could be read or written under"),
        )
        .primary(span, "the program handed this key over")
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
    /// Started by a run's first load and joined when the next run is configured.
    machine: Mutex<Option<Machine>>,
    /// The run in progress, which every thread the program runs a test on reads.
    running: RwLock<Option<Arc<Running>>>,
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
                let options = test_options_of(options, req.span)?;
                // A configuration begins a run: the last run's machine is dropped, joining its thread.
                let previous = self.held().take();
                drop(previous);
                *self.running.write().unwrap_or_else(|e| e.into_inner()) = None;
                *self.args.lock().unwrap_or_else(|e| e.into_inner()) = options;
                ply_eval::Value::Unit
            }
            "loaded" => {
                let front = req
                    .args
                    .first()
                    .ok_or_else(|| unasked("loaded", req.span))?;
                self.loaded(crate::driver::handed_front_of(front, req.span)?)?
            }
            "bound" => self.bound()?,
            "stamped" => self.stamped(),
            "keys" => self.knowledge(Ask::Keys)?,
            "hashed" => self.knowledge(Ask::Hashed)?,
            "started" => self.started()?,
            "executed" => {
                let unit = index_arg(req, 0, "a unit")?;
                let test = index_arg(req, 1, "a test index")?;
                self.run("executed")?.executed(unit, test)?
            }
            "interleaved" => {
                let unit = index_arg(req, 0, "a unit")?;
                let test = index_arg(req, 1, "a test index")?;
                let seed = crate::recording::seed_of(arg(req, 2)?, span)?;
                let steps = arg(req, 3)?.as_int(span, "a step bound")?;
                let re_executed = arg(req, 4)?.as_bool(span, "whether the test re-runs")?;
                self.run("interleaved")?.interleaved(
                    unit,
                    test,
                    &seed,
                    u32::try_from(steps.max(1)).unwrap_or(u32::MAX),
                    re_executed,
                )?
            }
            "concluded" => {
                let mut searched = Vec::new();
                for entry in arg(req, 0)?.as_list(span, "the settled searches")? {
                    searched.push(settled_of(entry, span)?);
                }
                self.concluded(searched)?
            }
            "mutated" => self.mutated()?,
            "mutant" => self.mutant(index_arg(req, 0, "a mutant's id")?)?,
            "mutation" => {
                let verdicts = verdicts_of(arg(req, 0)?, span)?;
                let budget_spent = arg(req, 1)?.as_bool(span, "whether the budget was spent")?;
                self.mutation(verdicts, budget_spent)?
            }
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
                let filed = match crate::payload::option_of(
                    req.args.get(2).ok_or_else(|| unasked("trial", req.span))?,
                    "the key a pass is filed under",
                    span,
                )? {
                    Some(key) => Some(hash_of(key.as_str(span, "a key")?, span)?),
                    None => None,
                };
                self.trial(usize::try_from(failure).unwrap_or(usize::MAX), named, filed)?
            }
            "record" => self.record()?,
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

    /// One report's load, over the front end the CLI ran for it: a watching run hands one per
    /// report, and the machine its configuration started serves them all.
    fn loaded(&self, front: crate::driver::HandedFront) -> Result<PlyValue, Diagnostic> {
        *self.running.write().unwrap_or_else(|e| e.into_inner()) = None;
        let mut held = self.held();
        if held.is_none() {
            *held = Some(Machine::start(
                self.args.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            )?);
        }
        let machine = held.as_ref().ok_or_else(|| unstarted("loaded"))?;
        machine.ask(Go::Load(Box::new(front)))?;
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
    fn trial(
        &self,
        failure: usize,
        keys: Vec<(String, String)>,
        filed: Option<ply_eval::DefHash>,
    ) -> Result<PlyValue, Diagnostic> {
        let (reply, answers) = mpsc::channel();
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("trial"))?;
        machine.ask(Go::Trial {
            failure,
            keys,
            filed,
            reply,
        })?;
        match answers.recv() {
            Ok(Ok(trial)) => Ok(trial_value(&trial)),
            Ok(Err(diagnostic)) => Err(diagnostic),
            Err(_) => Err(unanswered()),
        }
    }

    /// Writes what the mixtures tried since the run proved: the run's own flush came before them.
    fn record(&self) -> Result<PlyValue, Diagnostic> {
        let (reply, answers) = mpsc::channel();
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("record"))?;
        machine.ask(Go::Record { reply })?;
        match answers.recv() {
            Ok(warnings) => Ok(diags_value(&warnings)),
            Err(_) => Err(unanswered()),
        }
    }

    /// Publishes the run, so the threads the program runs tests on can reach it.
    fn started(&self) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("started"))?;
        machine.ask(Go::Start)?;
        match machine.step()? {
            Step::Started(running) => {
                *self.running.write().unwrap_or_else(|e| e.into_inner()) = Some(running);
                Ok(PlyValue::Unit)
            }
            _ => Err(out_of_step("started")),
        }
    }

    /// The run in progress. It reaches no machine: a test runs on the thread that asks, and many
    /// ask at once.
    fn run(&self, op: &str) -> Result<Arc<Running>, Diagnostic> {
        self.running
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|running| running.is_open())
            .cloned()
            .ok_or_else(|| out_of_step(op))
    }

    fn concluded(&self, searched: Vec<Settled>) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("concluded"))?;
        machine.ask(Go::Conclude(searched))?;
        match machine.step()? {
            Step::Ran(over) => Ok(ran_value(&over)),
            _ => Err(out_of_step("concluded")),
        }
    }

    fn mutated(&self) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("mutated"))?;
        machine.ask(Go::Mutated)?;
        match machine.step()? {
            Step::Mutated(queued) => Ok(queued_value(&queued)),
            _ => Err(out_of_step("mutated")),
        }
    }

    /// The unit a mutant's tests run in, or the verdict a mutant that does not build already is.
    fn mutant(&self, id: usize) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("mutant"))?;
        machine.ask(Go::Mutant(id))?;
        match machine.step()? {
            Step::Mutant(Ok(unit)) => Ok(PlyValue::ctor("Ok", vec![count(unit)])),
            Step::Mutant(Err(verdict)) => Ok(PlyValue::ctor("Err", vec![unbuilt_value(&verdict)])),
            _ => Err(out_of_step("mutant")),
        }
    }

    fn mutation(
        &self,
        verdicts: Vec<(usize, crate::mutate::Verdict)>,
        budget_spent: bool,
    ) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("mutation"))?;
        machine.ask(Go::Mutation {
            verdicts,
            budget_spent,
        })?;
        match machine.step()? {
            Step::Mutation(view) => Ok(mutants_value(&view)),
            _ => Err(out_of_step("mutation")),
        }
    }
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
    /// A report begins, over the front end the CLI ran for it.
    Load(Box<crate::driver::HandedFront>),
    Bind,
    /// What the program decided to run. Sent before the binding, because whether a unit has to be
    /// built at all is a function of it: a fully cached run builds none.
    Chosen(ply_test::Choice),
    /// The run begins: from here every test the program runs, runs on the thread that asks.
    Start,
    /// The run is over, and these are the searches the program settled its seeded tests with.
    Conclude(Vec<Settled>),
    Keys,
    Hashed,
    /// The store's answer under each key the caller names: the narrowing asks about keys the
    /// *program* computes, so no row could have carried them.
    Outcomes(Vec<String>),
    /// The printed union of these tests' footprints. The program colours the graph; the rendering of
    /// a colour is the compiler's, since only its printer can keep a label variable off a name.
    Footprint(Vec<usize>),
    /// One mixture the program decided to try, and the key its pass is filed under. Answered on this
    /// thread, because the store a hybrid is built over lives here and a `BodyHybrid` is not `Send`.
    Trial {
        failure: usize,
        keys: Vec<(String, String)>,
        filed: Option<ply_eval::DefHash>,
        reply: mpsc::Sender<Result<ply_test::bisect::Trial, Diagnostic>>,
    },
    /// Write what the trials since the run filed, and answer what storing it had to say.
    Record {
        reply: mpsc::Sender<Vec<Diagnostic>>,
    },
    /// The mutants of the run's targets that some test reaches, cheapest first.
    Mutated,
    /// One queued mutant, built into a unit a test can run in.
    Mutant(usize),
    /// What the program judged each mutant it ran, and whether its budget stopped it short.
    Mutation {
        verdicts: Vec<(usize, crate::mutate::Verdict)>,
        budget_spent: bool,
    },
}

enum Step {
    Loaded(Box<Result<Found, Refused>>),
    Bound(Box<Option<Refused>>),
    Started(Arc<Running>),
    Ran(Box<Over>),
    Mutated(Box<Queued>),
    Mutant(Result<usize, crate::mutate::Verdict>),
    Mutation(Box<MutantsView>),
    Knowledge {
        asked: Ask,
        value: Box<KnowledgeValue>,
    },
    /// No iteration is running to answer what was asked.
    Idle,
}

/// Which part of what a selector reads to compute a selection.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Ask {
    Keys,
    Hashed,
    Footprint(Vec<usize>),
    Outcomes(Vec<String>),
}

impl Ask {
    fn name(&self) -> &'static str {
        match self {
            Ask::Keys => "keys",
            Ask::Hashed => "hashed",
            Ask::Footprint(_) => "footprint",
            Ask::Outcomes(_) => "outcomes",
        }
    }
}

/// One read's answer, as plain data on its way to the caller.
enum KnowledgeValue {
    Keys(Vec<KeyRow>),
    Hashed(Vec<HashedRow>),
    Footprint(String),
    Outcomes(Vec<Option<String>>),
}

/// The mutants a mutation judges, each beside the tests that reach it, and why there are none when
/// the query no longer names a definition.
struct Queued {
    mutants: Vec<(usize, Vec<usize>)>,
    refused: Vec<Diagnostic>,
}

/// The thread a run's machine lives on. The `ply` program performing these operations is
/// itself inside an entry; the load, the store, the binding and the diagnosis happen here, and only
/// what a report is written from crosses back — which is also what lets the front end outlive an
/// iteration. The tests themselves run on whichever threads the program runs them on, against the
/// [`Running`] this thread publishes.
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

/// One store and one warm front end for the run, however many reports are asked of it.
fn serve(args: &TestOptions, told: &mpsc::Sender<Step>, asked: &mpsc::Receiver<Go>) {
    let mut cache = Cache::open(&project_root(&args.path), args.no_cache);
    let mut warm = crate::warm::Warm::default();
    // What the last run kept, so a program can try mixtures of it long after the run finished.
    let mut hybrids: Option<ply_test::Hybrids> = None;
    // What an iteration received and did not answer, which is the first thing asked after it.
    let mut pending: Option<Go> = None;
    while let Some(signal) = pending.take().or_else(|| asked.recv().ok()) {
        let signal = match answer_after(signal, cache.as_mut().map_err(|e| &*e), hybrids.as_ref()) {
            Ok(()) => continue,
            Err(signal) => signal,
        };
        match signal {
            Go::Load(front) => match &mut cache {
                Ok(cache) => {
                    // A trial asks about the run that finished last, and a new one is starting.
                    hybrids = None;
                    pending = iterate(args, &front, cache, &mut warm, told, asked, &mut hybrids);
                }
                Err(diagnostic) => {
                    let _ = told.send(Step::Loaded(Box::new(Err(Refused {
                        diagnostics: vec![diagnostic.clone()],
                        sources: SourceMap::new(),
                    }))));
                }
            },
            // Stated ahead of a binding that never came.
            Go::Chosen(_) => {}
            _ => {
                let _ = told.send(Step::Idle);
            }
        }
    }
}

/// Answers a trial or a record, which ask about the run that finished last, and hands anything
/// else back.
fn answer_after(
    go: Go,
    cache: Result<&mut Cache, &Diagnostic>,
    hybrids: Option<&ply_test::Hybrids>,
) -> Result<(), Go> {
    match go {
        Go::Trial {
            failure,
            keys,
            filed,
            reply,
        } => {
            let _ = reply.send(match cache {
                Ok(cache) => trial(&mut cache.store, hybrids, failure, &keys, filed),
                Err(diagnostic) => Err(diagnostic.clone()),
            });
        }
        Go::Record { reply } => {
            // A store that never opened, the load already reported.
            let _ = reply.send(cache.map(|c| recorded(&mut c.store)).unwrap_or_default());
        }
        other => return Err(other),
    }
    Ok(())
}

/// One report's iteration, and whatever it was asked that it did not answer.
fn iterate(
    args: &TestOptions,
    front: &crate::driver::HandedFront,
    cache: &mut Cache,
    warm: &mut crate::warm::Warm,
    told: &mpsc::Sender<Step>,
    asked: &mpsc::Receiver<Go>,
    hybrids: &mut Option<ply_test::Hybrids>,
) -> Option<Go> {
    let mut warnings = std::mem::take(&mut cache.warnings);
    let opened = cache.store.take_warnings();
    warnings.extend(crate::migrate::notice(&cache.store, &opened));
    warnings.extend(opened);

    let refuse = |diagnostics: Vec<Diagnostic>, sources: SourceMap| {
        let _ = told.send(Step::Loaded(Box::new(Err(Refused {
            diagnostics,
            sources,
        }))));
        None
    };
    // The front end is a function of the sources, so an unmoved tree reuses it whole.
    let (held, reuse) = warm.take(&project_root(&args.path));
    let loaded = match held {
        Some(loaded) => Ok(loaded),
        None => crate::driver::load_over_front(&args.path, front),
    };
    let loaded = match loaded {
        Ok(mut loaded) => {
            if reuse == crate::warm::Reuse::Whole {
                // Nothing was re-derived, so this iteration reports no phase time.
                loaded.frontend.phases = crate::driver::Phases::default();
            }
            loaded
        }
        Err(err) => return refuse(err.diagnostics, err.sources),
    };
    warnings.extend(cache.store.take_warnings());
    warnings.extend(loaded.frontend.warnings.iter().cloned());

    // A query naming nothing is wrong whatever the run does, so it refuses before anything runs
    // rather than being reported after a suite the user did not ask for.
    if let Some(query) = &args.mutate
        && let Err(diagnostic) = crate::mutate::targets(&loaded, query)
    {
        return refuse(vec![diagnostic], loaded.sources.clone());
    }

    let hashes = loaded.hashes.clone();
    let plan = Plan::new(&loaded, &args.filters, args.std);

    if let Some(err) = crate::costs::broken_promises(&loaded) {
        return refuse(err.diagnostics, err.sources);
    }

    let _ = told.send(Step::Loaded(Box::new(Ok(found(
        args, &loaded, &hashes, &plan, warnings,
    )))));
    // What a selector reads, before anything runs. It states its decision on the same channel, so
    // the machine has it by the time the binding decides whether a unit is worth building.
    let mut chosen = None;
    let knowledge = Knowledge::of(&loaded, &hashes);
    match serve_reads(asked, told, &knowledge, &cache.store, &mut chosen) {
        Some(Go::Bind) => {}
        other => return other,
    }
    let (written, pending) = bind(
        args,
        cache,
        warm,
        &loaded,
        &hashes,
        plan,
        told,
        asked,
        &knowledge,
        &mut chosen,
        hybrids,
    );
    if written {
        // Only over a report that was written: an iteration that returned early leaves nothing held.
        warm.keep(loaded);
    }
    pending
}

/// What a selector reads: computed once per iteration, before anything runs. Plain data, so it
/// can cross from the machine's thread; the caller value-ifies it.
struct Knowledge {
    keys: Vec<KeyRow>,
    hashed: Vec<HashedRow>,
    /// Every test's footprint, in test order: a group's rendering is the union of the ones it names.
    footprints: Vec<Footprint>,
}

/// The store's answer under each key, as a report prints one: `passed`, `failed`, or nothing. The
/// keys are the program's, which is the only side that encodes one.
fn outcomes_of(store: &ply_store::Store, keys: &[String]) -> Vec<Option<String>> {
    keys.iter()
        .map(|key| {
            ply_eval::DefHash::from_hex(key)
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

/// One test the loaded tree declares: its own hash, and whether the search is part of its key.
#[derive(Clone)]
struct KeyRow {
    index: usize,
    /// The test's label, as a report prints it.
    label: String,
    /// The test's program-wide name: `<module>.<label>`.
    name: String,
    module: String,
    /// `None` when the front end produced no hash.
    hash: Option<String>,
    seeded: bool,
    nondet: bool,
}

/// One definition or test the loaded tree declares, and its hash.
#[derive(Clone)]
struct HashedRow {
    name: String,
    hash: String,
    test: bool,
}

impl Knowledge {
    fn of(loaded: &Loaded, hashes: &HashOutput) -> Knowledge {
        let keys = loaded
            .check
            .tests
            .iter()
            .enumerate()
            .map(|(index, test)| KeyRow {
                index,
                label: test.name.as_str().to_string(),
                name: test.key.as_str().to_string(),
                module: test.module.as_str().to_string(),
                hash: hashes.tests.get(index).map(|h| h.to_hex()),
                seeded: ply_test::is_seeded(&test.footprint),
                nondet: test.nondet,
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
        }
    }

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

/// Answers what a selector reads until something else arrives, which it hands back; `None` once
/// the program is gone. Keeps the decision the program sent, if it sent one.
fn serve_reads(
    asked: &mpsc::Receiver<Go>,
    told: &mpsc::Sender<Step>,
    knowledge: &Knowledge,
    store: &ply_store::Store,
    chosen: &mut Option<ply_test::Choice>,
) -> Option<Go> {
    loop {
        match asked.recv().ok()? {
            Go::Chosen(choice) => *chosen = Some(choice),
            go => {
                if let Err(other) = answer_read(go, told, knowledge, store) {
                    return Some(other);
                }
            }
        }
    }
}

/// Answers `go` when it is a read, and hands it back otherwise. The ask travels back with the
/// answer: a step's answer is only its own if the questions match, and two of them carry the
/// caller's arguments.
fn answer_read(
    go: Go,
    told: &mpsc::Sender<Step>,
    knowledge: &Knowledge,
    store: &ply_store::Store,
) -> Result<(), Go> {
    let (asked, value) = match go {
        Go::Keys => (Ask::Keys, KnowledgeValue::Keys(knowledge.keys.clone())),
        Go::Hashed => (
            Ask::Hashed,
            KnowledgeValue::Hashed(knowledge.hashed.clone()),
        ),
        Go::Footprint(tests) => (
            Ask::Footprint(tests.clone()),
            KnowledgeValue::Footprint(knowledge.footprint_of(&tests)),
        ),
        Go::Outcomes(keys) => (
            Ask::Outcomes(keys.clone()),
            KnowledgeValue::Outcomes(outcomes_of(store, &keys)),
        ),
        other => return Err(other),
    };
    let _ = told.send(Step::Knowledge {
        asked,
        value: Box::new(value),
    });
    Ok(())
}

/// Whether a report was written, which is what decides if this front end is worth holding, and
/// whatever was asked that this did not answer.
#[allow(clippy::too_many_arguments)]
fn bind(
    args: &TestOptions,
    cache: &mut Cache,
    warm: &mut crate::warm::Warm,
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: Plan,
    told: &mpsc::Sender<Step>,
    asked: &mpsc::Receiver<Go>,
    knowledge: &Knowledge,
    chosen: &mut Option<ply_test::Choice>,
    hybrids: &mut Option<ply_test::Hybrids>,
) -> (bool, Option<Go>) {
    let refuse = |diagnostics: Vec<Diagnostic>| {
        let _ = told.send(Step::Bound(Box::new(Some(Refused {
            diagnostics,
            sources: loaded.sources.clone(),
        }))));
        (false, None)
    };
    if let Err(diagnostic) = select_profile(&args.profile) {
        return refuse(vec![diagnostic]);
    };
    // One per run, shared by every test; a run that decided to execute nothing builds no unit
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
    // A test is not a process: of `process` it binds only what names a program, and only the
    // programs `--exec` names, so under `--host` an unnamed label is unbound rather than withheld.
    let process = if args.host {
        match ply_host::process::Executables::load(&args.exec, Span::DUMMY) {
            Ok(executables) => Some(ply_host::process::ProcessHost::spawning(executables)),
            Err(diagnostic) => return refuse(vec![diagnostic]),
        }
    } else {
        None
    };
    let lent = match crate::policy::granted(&loaded.check, &args.allow) {
        Ok(lent) => lent,
        Err(diagnostic) => return refuse(vec![diagnostic]),
    };
    let hosts = match Hosts::open_stopping(
        &loaded.check,
        args.host,
        &args.tls,
        &args.fs,
        configuration,
        // A test's `trace` records are discarded: a run reports on tests, not on what they logged.
        &crate::trace::TraceOptions::silent(),
        None,
        process,
        lent,
    ) {
        Ok(hosts) => hosts,
        Err(diagnostics) => return refuse(diagnostics),
    };
    let _ = told.send(Step::Bound(Box::new(None)));
    // A selector may still ask what the tree holds between the binding and the run.
    match serve_reads(asked, told, knowledge, &cache.store, chosen) {
        Some(Go::Start) => {}
        other => return (false, other),
    }
    let choice = chosen.clone().unwrap_or_default();
    let selection = decided(&choice, &plan, &loaded.check);
    let provider = unit.filter(|_| !nothing_to_run);
    let running = Arc::new(Running {
        units: RwLock::new(vec![Unit {
            front: Arc::clone(&loaded.front),
            provider,
        }]),
        hosting: hosting(&hosts, &hosts.runtime_factory()),
        steps: args.steps,
        timeout: args.timeout,
        slots: Mutex::new(BTreeMap::new()),
        open: AtomicBool::new(true),
    });
    let started = Instant::now();
    let _ = told.send(Step::Started(Arc::clone(&running)));
    let searched = match serve_reads(asked, told, knowledge, &cache.store, chosen) {
        Some(Go::Conclude(searched)) => searched,
        other => {
            running.close();
            return (false, other);
        }
    };
    let ran = running.ran(&selection.to_run, searched);
    let (over, mixtures) = concluded(
        args,
        cache,
        loaded,
        hashes,
        &plan,
        &selection,
        &hosts,
        provider,
        ran,
        started.elapsed(),
        config_warnings,
    );
    // Kept for whatever the report asks next: a trial is about the run that just finished.
    *hybrids = Some(mixtures);
    let _ = told.send(Step::Ran(Box::new(over)));
    let pending = after_run(
        args,
        cache,
        loaded,
        hashes,
        told,
        asked,
        knowledge,
        hybrids.as_ref(),
        &running,
    );
    // The binding goes with this frame, and no test runs against a stopped host.
    running.close();
    (true, pending)
}

/// Answers what the program asks once the run is reported -- the mutation it judges, the mixtures
/// it tries and the record of them -- until something else arrives, which it hands back.
#[allow(clippy::too_many_arguments)]
fn after_run(
    args: &TestOptions,
    cache: &mut Cache,
    loaded: &Loaded,
    hashes: &HashOutput,
    told: &mpsc::Sender<Step>,
    asked: &mpsc::Receiver<Go>,
    knowledge: &Knowledge,
    hybrids: Option<&ply_test::Hybrids>,
    running: &Running,
) -> Option<Go> {
    let mut queue = crate::mutate::Queue::default();
    loop {
        let go = match answer_after(asked.recv().ok()?, Ok(&mut *cache), hybrids) {
            Ok(()) => continue,
            Err(go) => go,
        };
        match go {
            Go::Mutated => {
                let refused = match args
                    .mutate
                    .as_deref()
                    .map(|query| crate::mutate::targets(loaded, query))
                {
                    Some(Ok(targets)) => {
                        queue = crate::mutate::queued(loaded, hashes, &targets);
                        Vec::new()
                    }
                    // The query was resolved before the run, so this is a target that moved under it.
                    Some(Err(diagnostic)) => vec![diagnostic],
                    None => Vec::new(),
                };
                let mutants = queue
                    .mutants
                    .iter()
                    .enumerate()
                    .map(|(id, (tests, _))| (id, tests.clone()))
                    .collect();
                let _ = told.send(Step::Mutated(Box::new(Queued { mutants, refused })));
            }
            Go::Mutant(id) => {
                let built = match queue.mutants.get(id) {
                    Some((_, mutant)) => crate::mutate::built(loaded, mutant)
                        .map(|(front, provider)| running.add(front, provider)),
                    None => Err(crate::mutate::Verdict::Unresolved(format!(
                        "no mutant {id} was queued"
                    ))),
                };
                let _ = told.send(Step::Mutant(built));
            }
            Go::Mutation {
                verdicts,
                budget_spent,
            } => {
                let report =
                    mutation_report(std::mem::take(&mut queue), verdicts, budget_spent, loaded);
                let _ = told.send(Step::Mutation(Box::new(mutants_view(&report, loaded))));
            }
            go => {
                if let Err(other) = answer_read(go, told, knowledge, &cache.store) {
                    return Some(other);
                }
            }
        }
    }
}

/// The mutation as the program judged it: each verdict beside its mutant and the tests that reach
/// it, and every mutant the budget never reached left out.
fn mutation_report(
    queue: crate::mutate::Queue,
    verdicts: Vec<(usize, crate::mutate::Verdict)>,
    budget_spent: bool,
    loaded: &Loaded,
) -> crate::mutate::Report {
    let generated = queue.mutants.len();
    let mut mutants: Vec<Option<(Vec<usize>, crate::mutate::Mutant)>> =
        queue.mutants.into_iter().map(Some).collect();
    let judged = verdicts
        .into_iter()
        .filter_map(|(id, verdict)| {
            let (tests, mutant) = mutants.get_mut(id)?.take()?;
            Some(crate::mutate::Judged {
                mutant,
                verdict,
                tests: tests
                    .iter()
                    .filter_map(|&i| loaded.check.tests.get(i).map(|t| t.key.clone()))
                    .collect(),
            })
        })
        .collect();
    crate::mutate::Report {
        definitions: queue.definitions,
        generated,
        judged,
        unreached: queue.unreached,
        budget_spent,
    }
}

// --- The run, on whichever thread asks ------------------------------------------

/// One run, as every thread the program runs a test on reads it: the programs a test can run in --
/// the loaded one, then each mutant built since -- the host they reach, and what each test came to.
struct Running {
    units: RwLock<Vec<Unit>>,
    hosting: ply_test::Hosting,
    steps: i64,
    timeout: u64,
    slots: Mutex<BTreeMap<(usize, usize), Slot>>,
    open: AtomicBool,
}

#[derive(Clone)]
struct Unit {
    front: Arc<ply_eval::Front>,
    /// `None` when the run decided to execute nothing and so built nothing to run a test on.
    provider: Option<&'static dyn ply_eval::Provider>,
}

/// What one test came to, over every call the program made for it.
#[derive(Default)]
struct Slot {
    once: Option<ply_test::Executed>,
    interleavings: ply_test::Interleavings,
}

impl Running {
    fn is_open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }

    fn close(&self) {
        self.open.store(false, Ordering::Release);
    }

    fn unit(&self, unit: usize) -> Result<Unit, Diagnostic> {
        let units = self.units.read().unwrap_or_else(|e| e.into_inner());
        units.get(unit).cloned().ok_or_else(|| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!(
                    "the program ran a test in unit {unit}, and the run holds {}",
                    units.len()
                ),
            )
            .note(
                "units are the loaded program and each mutant `mutant` built; this is Ply's fault",
            )
        })
    }

    fn add(&self, front: Arc<ply_eval::Front>, provider: &'static dyn ply_eval::Provider) -> usize {
        let mut units = self.units.write().unwrap_or_else(|e| e.into_inner());
        units.push(Unit {
            front,
            provider: Some(provider),
        });
        units.len() - 1
    }

    fn slot<R>(&self, unit: usize, test: usize, f: impl FnOnce(&mut Slot) -> R) -> R {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        f(slots.entry((unit, test)).or_default())
    }

    /// The program's thread is entered with no budget of its own; a test is bounded by the run's.
    fn budgeted<R>(&self, f: impl FnOnce() -> R) -> R {
        ply_codegen::rt::with_step_budget(self.steps, || {
            ply_codegen::rt::with_time_budget(self.timeout, f)
        })
    }

    /// One test run once on this thread, and how it ended.
    fn executed(&self, unit: usize, test: usize) -> Result<PlyValue, Diagnostic> {
        let Unit { front, provider } = self.unit(unit)?;
        let once = match provider {
            Some(provider) => {
                let executor = ply_test::InterpExecutor::new(&front, provider)
                    .with_hosts(self.hosting.clone());
                self.budgeted(|| ply_test::executed(&executor, &front.check, test))
            }
            None => ply_test::Executed::refused(test, nothing_built()),
        };
        let status = status_word(once.failure.as_ref(), once.panicked);
        self.slot(unit, test, |slot| slot.once = Some(once));
        Ok(PlyValue::str(status))
    }

    /// One interleaving of a seeded test on this thread, recorded; a failure crosses as the id the
    /// run holds its diagnostic by, and how a report classes it.
    fn interleaved(
        &self,
        unit: usize,
        test: usize,
        seed: &ply_eval::Seed,
        steps: u32,
        re_executed: bool,
    ) -> Result<PlyValue, Diagnostic> {
        let Unit { front, provider } = self.unit(unit)?;
        let run = match provider {
            Some(provider) => {
                let executor = ply_test::InterpExecutor::new(&front, provider)
                    .with_hosts(self.hosting.clone());
                self.budgeted(|| {
                    ply_test::interleaved(&executor, &front.check, test, seed, steps, re_executed)
                })
            }
            None => ply_test::Interleaved::refused(nothing_built()),
        };
        let fell = self.slot(unit, test, |slot| {
            slot.interleavings.add(&run).map(|id| {
                record(vec![
                    ("id", count(id)),
                    (
                        "status",
                        PlyValue::str(status_word(slot.interleavings.held().get(id), run.panicked)),
                    ),
                ])
            })
        });
        Ok(crate::recording::interleaving_value(
            &run.interleaving,
            fell,
        ))
    }

    /// What each test the program was told to run came to: one run once as it ran, a seeded one as
    /// the program's search settled it, and one the program never ran as Ply's fault.
    fn ran(&self, to_run: &[usize], searched: Vec<Settled>) -> Vec<ply_test::Executed> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let mut settled: BTreeMap<usize, Settled> =
            searched.into_iter().map(|s| (s.test, s)).collect();
        to_run
            .iter()
            .map(|&test| {
                let mut slot = slots.remove(&(0, test)).unwrap_or_default();
                match (settled.remove(&test), slot.once.take()) {
                    (Some(search), _) => search.executed(slot.interleavings),
                    (None, Some(once)) => once,
                    (None, None) => ply_test::Executed::refused(test, never_ran(test)),
                }
            })
            .collect()
    }
}

/// How a report classes a run that ended with `failure`.
fn status_word(failure: Option<&Diagnostic>, panicked: bool) -> &'static str {
    match failure {
        None => "passed",
        Some(d) if d.code == codes::RUN_ABANDONED => "abandoned",
        Some(d) if panicked || codes::is_defect(d.code) => "panicked",
        Some(_) => "failed",
    }
}

/// A seeded test's search, as the program settled it: what it ran, why it stopped at a failure,
/// and how many roots it started from.
struct Settled {
    test: usize,
    searched: ply_test::Searched,
    failure: Option<Stopped>,
    seeds: usize,
}

/// Why a search stopped at a seed, and what a less pruned search had to say about reaching it.
struct Stopped {
    why: Why,
    notes: Vec<String>,
}

enum Why {
    /// The run failed, with the diagnostic this run holds under the id.
    Ran(usize),
    /// The recording names no schedule.
    Unscheduled {
        seed: ply_eval::Seed,
        span: Span,
        what: String,
    },
    /// A replay did not take the schedule the seed names.
    Diverged {
        seed: ply_eval::Seed,
        span: Span,
        what: String,
    },
}

impl Settled {
    fn executed(self, runs: ply_test::Interleavings) -> ply_test::Executed {
        let failure = self.failure.map(|stopped| stopped.diagnostic(runs.held()));
        runs.settled(self.test, self.searched, failure, self.seeds)
    }
}

impl Stopped {
    fn diagnostic(self, held: &[Diagnostic]) -> Diagnostic {
        let reproduce = |seed: &ply_eval::Seed| {
            format!(
                "reproduce with `--sim once --seed {seed}`, and report it with the test's source"
            )
        };
        let stopped = match self.why {
            Why::Ran(id) => held.get(id).cloned().unwrap_or_else(|| {
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!(
                        "the search stopped at failure {id}, and the run holds {}",
                        held.len()
                    ),
                )
                .note("`sim.search` and the runtime are one run's two halves; this is Ply's fault")
            }),
            Why::Unscheduled { seed, span, what } => Diagnostic::error(
                codes::INTERNAL_ERROR,
                "the scheduler recorded a step that does not describe a schedule",
            )
            .primary(span, "this step")
            .note(what)
            .note(reproduce(&seed)),
            Why::Diverged { seed, span, what } => Diagnostic::error(
                codes::SIMULATION_DIVERGENCE,
                format!("replaying seed {seed} did not reproduce the recorded schedule"),
            )
            .primary(span, "this scheduling point")
            .note(what)
            .note(
                "a simulated run must be a pure function of its definition set and its seed; this \
                 is a defect in Ply rather than in the program under test",
            )
            .note(reproduce(&seed)),
        };
        self.notes.into_iter().fold(stopped, Diagnostic::note)
    }
}

/// A `tests.Settled`: the test, the `sim.search.Exploration` its search came to, why it stopped,
/// and how many roots it started from.
fn settled_of(v: &PlyValue, span: Span) -> Result<Settled, Diagnostic> {
    use crate::payload::{field_of, option_of};
    let int = |v: &PlyValue, name: &str| field_of(v, name, span)?.as_int(span, name);
    let nat = |v: &PlyValue, name: &str| -> Result<u64, Diagnostic> {
        u64::try_from(int(v, name)?).map_err(|_| crate::payload::missing(name, span))
    };
    let flag = |v: &PlyValue, name: &str| field_of(v, name, span)?.as_bool(span, name);
    let e = field_of(v, "exploration", span)?;
    let cost = |name: &str| -> Result<Option<ply_test::Cost>, Diagnostic> {
        option_of(field_of(e, name, span)?, name, span)?
            .map(|c| {
                Ok(ply_test::Cost {
                    explored: u32::try_from(nat(c, "explored")?).unwrap_or(u32::MAX),
                    bounded: flag(c, "bounded")?,
                })
            })
            .transpose()
    };
    let searched = ply_test::Searched {
        explored: u32::try_from(nat(e, "explored")?).unwrap_or(u32::MAX),
        exhaustive: flag(e, "exhaustive")?,
        exhausted: flag(e, "exhausted")?,
        naive: cost("naive")?,
        blind: cost("blind")?,
        steps: nat(e, "steps")?,
        virtual_time: int(e, "virtual_time")?,
        failure: option_of(field_of(e, "failure", span)?, "a failing seed", span)?
            .map(|seed| crate::recording::seed_of(seed, span))
            .transpose()?,
        race: option_of(field_of(e, "race", span)?, "a race", span)?
            .map(|race| race_of(race, span))
            .transpose()?,
    };
    let failure = option_of(field_of(v, "failure", span)?, "a search's failure", span)?
        .map(|failure| stopped_of(failure, span))
        .transpose()?;
    Ok(Settled {
        test: usize::try_from(nat(v, "test")?).unwrap_or(usize::MAX),
        searched,
        failure,
        seeds: usize::try_from(nat(v, "seeds")?).unwrap_or(usize::MAX),
    })
}

/// A `sim.search.Race`.
fn race_of(v: &PlyValue, span: Span) -> Result<ply_test::Race, Diagnostic> {
    use crate::payload::{field_of, option_of};
    let site = |v: &PlyValue| -> Result<ply_test::RaceSite, Diagnostic> {
        Ok(ply_test::RaceSite {
            task: u64::try_from(field_of(v, "task", span)?.as_int(span, "a task")?)
                .unwrap_or(u64::MAX),
            definition: option_of(field_of(v, "definition", span)?, "a definition", span)?
                .map(|d| d.as_str(span, "a definition").map(Symbol::new))
                .transpose()?,
            access: field_of(v, "access", span)?
                .as_str(span, "an access")?
                .to_string(),
            span: crate::recording::span_of(field_of(v, "span", span)?, span)?,
        })
    };
    Ok(ply_test::Race {
        left: site(field_of(v, "left", span)?)?,
        right: site(field_of(v, "right", span)?)?,
        at: u32::try_from(field_of(v, "at", span)?.as_int(span, "a scheduling point")?)
            .unwrap_or(u32::MAX),
    })
}

/// A `sim.search.Failure<tests.Fell>`.
fn stopped_of(v: &PlyValue, span: Span) -> Result<Stopped, Diagnostic> {
    use crate::payload::field_of;
    let why = field_of(v, "why", span)?;
    let PlyValue::Ctor { name, args } = why else {
        return Err(crate::payload::shape(why, span));
    };
    let payload = args
        .first()
        .ok_or_else(|| crate::payload::shape(why, span))?;
    let at = |payload: &PlyValue| -> Result<(ply_eval::Seed, Span, String), Diagnostic> {
        Ok((
            crate::recording::seed_of(field_of(payload, "seed", span)?, span)?,
            crate::recording::span_of(field_of(payload, "span", span)?, span)?,
            field_of(payload, "what", span)?
                .as_str(span, "what a recording got wrong")?
                .to_string(),
        ))
    };
    let why = match name
        .as_str()
        .rsplit_once('.')
        .map_or(name.as_str(), |(_, n)| n)
    {
        "Ran" => Why::Ran(
            usize::try_from(field_of(payload, "id", span)?.as_int(span, "a failure's id")?)
                .unwrap_or(usize::MAX),
        ),
        "Unscheduled" => {
            let (seed, span, what) = at(payload)?;
            Why::Unscheduled { seed, span, what }
        }
        "Diverged" => {
            let (seed, span, what) = at(payload)?;
            Why::Diverged { seed, span, what }
        }
        _ => return Err(crate::payload::shape(why, span)),
    };
    let mut notes = Vec::new();
    for note in field_of(v, "notes", span)?.as_list(span, "a failure's notes")? {
        notes.push(note.as_str(span, "a note")?.to_string());
    }
    Ok(Stopped { why, notes })
}

/// What the program judged each mutant: `killed`, `survived`, or `skipped` or `unresolved` and why.
fn verdicts_of(
    v: &PlyValue,
    span: Span,
) -> Result<Vec<(usize, crate::mutate::Verdict)>, Diagnostic> {
    use crate::mutate::Verdict;
    use crate::payload::field_of;
    let mut out = Vec::new();
    for entry in v.as_list(span, "the verdicts")? {
        let id = field_of(entry, "id", span)?.as_int(span, "a mutant's id")?;
        let why = field_of(entry, "why", span)?
            .as_str(span, "a verdict's reason")?
            .to_string();
        let verdict = match field_of(entry, "verdict", span)?.as_str(span, "a verdict")? {
            "killed" => Verdict::Killed,
            "survived" => Verdict::Survived,
            "skipped" => Verdict::Skipped(why),
            "unresolved" => Verdict::Unresolved(why),
            other => {
                return Err(Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!("`{other}` is not a verdict a mutant can have"),
                )
                .primary(span, "the program handed this verdict over"));
            }
        };
        out.push((usize::try_from(id).unwrap_or(usize::MAX), verdict));
    }
    Ok(out)
}

/// A mutant `mutant` could not build, as the program reads the verdict it already is.
fn unbuilt_value(verdict: &crate::mutate::Verdict) -> PlyValue {
    use crate::mutate::Verdict;
    let (word, why) = match verdict {
        Verdict::Killed => ("killed", ""),
        Verdict::Survived => ("survived", ""),
        Verdict::Skipped(why) => ("skipped", why.as_str()),
        Verdict::Unresolved(why) => ("unresolved", why.as_str()),
    };
    record(vec![
        ("verdict", PlyValue::str(word)),
        ("why", PlyValue::str(why)),
    ])
}

fn queued_value(queued: &Queued) -> PlyValue {
    record(vec![
        (
            "mutants",
            PlyValue::list(
                queued
                    .mutants
                    .iter()
                    .map(|(id, tests)| {
                        record(vec![
                            ("id", count(*id)),
                            (
                                "tests",
                                PlyValue::list(tests.iter().map(|&i| count(i)).collect()),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("refused", diags_value(&queued.refused)),
    ])
}

#[cold]
fn nothing_built() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a test was run in a run that decided to execute nothing, so no unit was built to run it on",
    )
    .note("this is Ply's fault: the choice named no test to run and the program ran one anyway")
}

#[cold]
fn never_ran(test: usize) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("test {test} was chosen to run, and the program never ran it"),
    )
    .note("a runner must never skip a test it chose; this is Ply's fault")
}

/// How many tests run at once: the lanes `--jobs` names, or one per core.
fn workers(jobs: Option<u32>) -> usize {
    match jobs {
        Some(n) if n > 0 => n as usize,
        _ => std::thread::available_parallelism().map_or(1, |n| n.get()),
    }
}

/// What the run came to, filed under the keys the program named, and what a failure's cause is
/// decided from.
#[allow(clippy::too_many_arguments)]
fn concluded(
    args: &TestOptions,
    cache: &mut Cache,
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: &Plan,
    selection: &Selection,
    hosts: &Hosts,
    provider: Option<&'static dyn ply_eval::Provider>,
    ran: Vec<ply_test::Executed>,
    duration: Duration,
    mut warnings: Vec<Diagnostic>,
) -> (Over, ply_test::Hybrids) {
    let report = ply_test::concluded(
        selection,
        &loaded.check,
        hashes,
        &mut cache.store,
        ran,
        duration,
    );
    // After the run, since a pass recorded now is a valid baseline for another's failure. What
    // changed and what a mixture would need are handed over; the program that reads the report
    // decides everything about the cause.
    let hybrids =
        ply_test::diagnose_failures(&report, &loaded.texts(), &loaded.front, &cache.store);
    warnings.extend(report.warnings.iter().cloned());
    // Pass records are read lazily, so an unreadable baseline only surfaces here.
    warnings.extend(cache.store.take_warnings());

    let mut escapes = hosts_escapes(&report, &loaded.check, hosts);
    if let Some(unbuilt) = unbuilt_backend(provider) {
        escapes.push(unbuilt);
    }
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
        workers: workers(args.jobs),
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
    /// Test indices this run was never asked to decide: a dependency's, and a shipped module's
    /// without `--std`.
    pub out_of_scope: BTreeSet<usize>,
}

impl Plan {
    /// A run tests the package being loaded: a dependency's tests are its own to run, and a shipped
    /// module's are in scope only under `--std`, which `std_tests` is.
    pub fn new(loaded: &Loaded, filters: &[String], std_tests: bool) -> Plan {
        let check = &loaded.check;
        let root = loaded.root_package();
        let in_scope = |t: &ply_eval::TestInfo| {
            root.contains(&t.module) || (std_tests && crate::shelf::is_shipped(&t.module))
        };
        // Against `<module>.<label>`, so `--filter store.` narrows to a module.
        let matches = |t: &ply_eval::TestInfo| {
            filters.is_empty() || filters.iter().any(|n| t.key.as_str().contains(n.as_str()))
        };

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

/// The runtime's view of what the program decided, under this run's own filter: the tests it keeps
/// and the classes filtered to them. `--filter` cannot change which tests conflict, so a class only
/// loses members.
pub(crate) fn decided(choice: &ply_test::Choice, plan: &Plan, check: &CheckOutput) -> Selection {
    let keeps = |i: &usize| plan.visible.binary_search(i).is_ok();
    let filtered = ply_test::Choice {
        runs: choice.runs.iter().copied().filter(keeps).collect(),
        groups: choice
            .groups
            .iter()
            .map(|class| class.iter().copied().filter(keeps).collect::<Vec<usize>>())
            .filter(|class| !class.is_empty())
            .collect(),
        filed: choice
            .filed
            .iter()
            .filter(|(index, _)| keeps(index))
            .map(|(index, keys)| (*index, keys.clone()))
            .collect(),
        reasons: choice.reasons.clone(),
    };
    let mut selection = Selection::chosen(&filtered, check);
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
                "{unbuilt} thread(s) could not build the `{}` backend, and every call they were \
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
    /// Every atom of the footprint: which of them another test could contend over, and what that
    /// makes the test, is the program's to say.
    atoms: Vec<AtomView>,
    seeded: bool,
}

/// One atom of a test's footprint, as a scheduler compares them and as the printer spells it.
struct AtomView {
    effect: String,
    resource: String,
    writes: bool,
    text: String,
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
    workers: usize,
    backend: Option<BackendView>,
    results: Vec<OutcomeView>,
    failures: Vec<FaultView>,
    summary: SummaryView,
    simulation: ply_test::SimSummary,
    escapes: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
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
    performs: u64,
}

struct SearchView {
    explored: u64,
    exhaustive: bool,
    exhausted: bool,
    blind: Option<Cost>,
    naive: Option<Cost>,
    reduction_tenths: Option<i64>,
    steps: u64,
    virtual_time_ns: i64,
    failing_seed: Option<String>,
}

/// A definition in the failing test's closure the store has never seen pass, with its current hash.
struct SuspectView {
    name: String,
    hash: Option<String>,
}

struct FaultView {
    key: String,
    /// The label as the source wrote it.
    name: String,
    diagnostic: Diagnostic,
    /// The interpreter failed rather than the program, and the failing run reached a host handler:
    /// two of the facts a verdict's gate is decided from.
    defect: bool,
    host: bool,
    /// What the cause is decided from, when the test passed before: the facts its change set is
    /// classified from, why no mixture can be tried when none can, and where each name is.
    search: Option<ChangeSetView>,
    suspects: Vec<SuspectView>,
    unchanged: bool,
    seed: Option<String>,
    /// The two steps, and the scheduling point whose reordering flipped the outcome.
    race: Option<(SiteView, SiteView, u32)>,
    module: Option<String>,
    test_hash: Option<String>,
    nondet: Option<bool>,
    status: Option<&'static str>,
    declared: Option<Vec<String>>,
}

/// One failure's facts, as the program reads them. Each name they mention carries the place `ply`
/// would print for it.
struct ChangeSetView {
    facts: ply_test::ChangeSet,
    absent: Option<ply_test::Absent>,
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
                    atoms: test
                        .footprint
                        .atoms()
                        .zip(atoms(&test.footprint))
                        .map(|(a, text)| AtomView {
                            effect: a.effect.to_string(),
                            resource: a.resource.to_string(),
                            writes: a.mode == Mode::Write,
                            text,
                        })
                        .collect(),
                    seeded: ply_test::is_seeded(&test.footprint),
                })
            })
            .collect(),
        filtered_out: plan.filtered_out,
        warnings,
        options: jsonlit!({
            "bisect": args.bisect.as_str(),
            "bisect_budget": args.bisect_budget,
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
            blind: e.blind,
            naive: e.naive,
            // Tenths, so one division answers both the line and the document.
            reduction_tenths: e.reduction().map(|r| (r * 10.0).round() as i64),
            steps: e.steps,
            virtual_time_ns: e.virtual_time,
            failing_seed: e.failure.as_ref().map(|s| s.to_string()),
        }),
        cached: result.recorded.as_ref().map(Record::is_written),
        performs: result.performs,
    }
}

/// A suspect by name, with its current hash: a name that is both a `fn` and a `type` is one suspect,
/// since it is one thing to look at.
fn suspect_view(name: &Symbol, hashes: &HashOutput) -> SuspectView {
    SuspectView {
        name: name.to_string(),
        hash: hashes
            .defs
            .get(name)
            .or_else(|| hashes.decls.get(name))
            .map(|h| h.to_hex()),
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
    let search = input.map(|input| ChangeSetView {
        facts: input.facts.clone(),
        absent: input.absent,
        // Every name the facts mention, with its place: a verdict the program reaches has no spans
        // of its own, and a report prints where each culprit is.
        at: {
            let mut names: Vec<&Symbol> = input.facts.rows.iter().map(|r| &r.key.name).collect();
            names.push(&input.facts.test);
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
        name: failure.name.clone(),
        diagnostic: failure.diagnostic.clone(),
        defect: failure.defect,
        host: failure.host,
        search,
        suspects: failure
            .suspects
            .iter()
            .map(|name| suspect_view(name, hashes))
            .collect(),
        unchanged: failure.suspects.is_empty(),
        seed: failure.seed.as_ref().map(|s| s.to_string()),
        race: failure
            .race
            .as_ref()
            .map(|race| (site_view(&race.left), site_view(&race.right), race.at)),
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
    }
}

fn site_view(site: &ply_test::RaceSite) -> SiteView {
    SiteView {
        task: format!("@{}", site.task),
        definition: site.definition.as_ref().map(|d| d.to_string()),
        access: site.access.to_string(),
        span: site.span,
    }
}

fn atoms(footprint: &Footprint) -> Vec<String> {
    ply_eval::atom_texts(&footprint.0)
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
                    (
                        "atom",
                        record(vec![
                            ("effect", PlyValue::str(&a.effect)),
                            ("resource", PlyValue::str(&a.resource)),
                            ("writes", PlyValue::Bool(a.writes)),
                        ]),
                    ),
                    ("text", PlyValue::str(&a.text)),
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
        ("atoms", atoms_value(&case.atoms)),
        ("seeded", PlyValue::Bool(case.seeded)),
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
                        ("hash", option(row.hash.as_deref().map(PlyValue::str))),
                        ("seeded", PlyValue::Bool(row.seeded)),
                        ("nondet", PlyValue::Bool(row.nondet)),
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
    }
}

fn cost_value(cost: Cost) -> PlyValue {
    record(vec![
        ("explored", tally(u64::from(cost.explored))),
        ("bounded", PlyValue::Bool(cost.bounded)),
        ("rendered", PlyValue::str(cost.to_string())),
    ])
}

fn search_value(search: &SearchView) -> PlyValue {
    record(vec![
        ("explored", tally(search.explored)),
        ("exhaustive", PlyValue::Bool(search.exhaustive)),
        ("exhausted", PlyValue::Bool(search.exhausted)),
        ("blind", option(search.blind.map(cost_value))),
        ("naive", option(search.naive.map(cost_value))),
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
        (
            "diagnostic",
            option(o.diagnostic.as_ref().map(raised_value)),
        ),
        ("search", option(o.search.as_ref().map(search_value))),
        ("cached", option(o.cached.map(PlyValue::Bool))),
        ("performs", tally(o.performs)),
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
        ("name", PlyValue::str(&f.name)),
        ("diagnostic", raised_value(&f.diagnostic)),
        ("defect", PlyValue::Bool(f.defect)),
        ("host", PlyValue::Bool(f.host)),
        ("search", option(f.search.as_ref().map(change_set_value))),
        (
            "suspects",
            PlyValue::list(
                f.suspects
                    .iter()
                    .map(|x| {
                        record(vec![
                            ("name", PlyValue::str(&x.name)),
                            ("hash", option(x.hash.as_deref().map(PlyValue::str))),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("unchanged", PlyValue::Bool(f.unchanged)),
        ("seed", option(f.seed.as_deref().map(PlyValue::str))),
        (
            "race",
            option(f.race.as_ref().map(|(left, right, at)| {
                record(vec![
                    ("left", site_value(left)),
                    ("right", site_value(right)),
                    ("step", count(*at as usize)),
                ])
            })),
        ),
        ("module", option(f.module.as_deref().map(PlyValue::str))),
        (
            "test_hash",
            option(f.test_hash.as_deref().map(PlyValue::str)),
        ),
        ("nondet", option(f.nondet.map(PlyValue::Bool))),
        ("status", option(f.status.map(PlyValue::str))),
        ("declared", opt_texts(f.declared.as_deref())),
    ])
}

fn mutants_value(m: &MutantsView) -> PlyValue {
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
        // The program judges a mutation after the run, and sets this from what it judged.
        ("mutants", option(None)),
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
        eprintln!("{v:?}");
    }
    use crate::payload::{field_of, opt_int_at, opt_str_at, str_list_at};
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
    let config = field_of(v, "config", span)?;
    Ok(TestOptions {
        path: std::path::PathBuf::from(str_at("path")?),
        json: bool_at("json")?,
        explain: bool_at("explain")?,
        no_cache: bool_at("no_cache")?,
        filters: str_list_at(v, "filters", span)?,
        jobs: opt_int_at(v, "jobs", span)?.map(|n| n as u32),
        steps: int_at("steps")?,
        timeout: int_at("timeout")? as u64,
        bisect: when_at("bisect")?,
        bisect_budget: int_at("bisect_budget")? as usize,
        coverage: bool_at("coverage")?,
        mutate: opt_str_at(v, "mutate", span)?,
        mutate_budget: int_at("mutate_budget")? as usize,
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
        exec: named_list("exec")?
            .into_iter()
            .map(|(name, path)| ply_host::process::ExecSpec {
                name,
                path: std::path::PathBuf::from(path),
            })
            .collect(),
        allow: str_list_at(v, "allow", span)?,
        config: crate::config::ConfigOptions {
            set: str_list_at(config, "set", span)?,
            files: str_list_at(config, "files", span)?
                .into_iter()
                .map(std::path::PathBuf::from)
                .collect(),
            schema: opt_str_at(config, "schema", span)?,
        },
        std: bool_at("std")?,
    })
}

impl Default for TestOptions {
    fn default() -> TestOptions {
        TestOptions {
            path: std::path::PathBuf::from("."),
            json: false,
            explain: false,
            no_cache: false,
            filters: Vec::new(),
            jobs: None,
            steps: ply_eval::DEFAULT_STEP_BUDGET,
            timeout: 60_000,
            bisect: When::Auto,
            bisect_budget: 64,
            coverage: false,
            mutate: None,
            mutate_budget: 64,
            profile: "development".to_string(),
            watch: false,
            host: false,
            tls: crate::options::TlsOptions::default(),
            fs: Vec::new(),
            exec: Vec::new(),
            allow: Vec::new(),
            config: crate::config::ConfigOptions::default(),
            std: false,
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
    filed: Option<ply_eval::DefHash>,
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
        return Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("a mixture of failure {failure} was asked for, and none can be built"),
        )
        .note("the report said why none can; this is Ply's fault"));
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
    let trial = hybrid.trial_over(wanted, filed);
    // A mixture that went green is a program whose definitions all pass at once, filed under the
    // mixture's own key: never the failing test's, so a red test is never passed by a mixture of it.
    for key in hybrid.take_proved() {
        store.put(key, ply_store::Outcome::Pass);
    }
    Ok(trial)
}

/// The passes the trials filed since the run's own flush, written, and what storing them had to say.
fn recorded(store: &mut ply_store::Store) -> Vec<Diagnostic> {
    let mut warnings = Vec::new();
    if let Err(e) = store.flush() {
        warnings.push(
            Diagnostic::warning(
                codes::CACHE_UNREADABLE,
                format!("could not write what the bisection's mixtures proved: {e:#}"),
            )
            .note("every verdict stands; the next bisection runs those mixtures again"),
        );
    }
    warnings.extend(store.take_warnings());
    warnings
}

/// One trial's outcome, as the program reads it: the case, and whether the runtime answered from a
/// result it already had.
fn trial_value(trial: &ply_test::bisect::Trial) -> PlyValue {
    let outcome = match trial.outcome {
        ply_test::bisect::TrialOutcome::Fails => case("TrialOutcome", "Fails", Vec::new()),
        ply_test::bisect::TrialOutcome::Passes => case("TrialOutcome", "Passes", Vec::new()),
        ply_test::bisect::TrialOutcome::Unresolved(why) => case(
            "TrialOutcome",
            "Unresolved",
            vec![case(
                "Unresolved",
                match why {
                    ply_test::bisect::Unresolved::DoesNotCheck => "DoesNotCheck",
                    ply_test::bisect::Unresolved::DifferentFailure => "DifferentFailure",
                    ply_test::bisect::Unresolved::MissingBody => "MissingBody",
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

/// One failure's facts, as the program reads them.
fn change_set_value(view: &ChangeSetView) -> PlyValue {
    let facts = &view.facts;
    let hex = |hash: Option<ply_eval::DefHash>| option(hash.map(|h| PlyValue::str(h.to_hex())));
    record(vec![
        (
            "facts",
            record(vec![
                (
                    "own",
                    record(vec![
                        ("name", PlyValue::str(facts.test.as_str())),
                        ("before", PlyValue::str(facts.before.to_hex())),
                        ("after", hex(facts.after)),
                        ("rehashed", hex(facts.rehashed)),
                    ]),
                ),
                (
                    "rows",
                    PlyValue::list(
                        facts
                            .rows
                            .iter()
                            .map(|row| {
                                record(vec![
                                    ("name", PlyValue::str(row.key.name.as_str())),
                                    ("ns", ns_value(row.key.ns)),
                                    ("before", hex(row.before)),
                                    ("after", hex(row.after)),
                                    ("rehashed", hex(row.rehashed)),
                                    ("stable", option(row.stable.map(PlyValue::Bool))),
                                    ("kept", PlyValue::Bool(row.kept)),
                                    ("renamed", PlyValue::Bool(row.renamed)),
                                    (
                                        "component",
                                        PlyValue::list(
                                            row.component.iter().map(key_value).collect(),
                                        ),
                                    ),
                                    (
                                        "referrers",
                                        strings(row.referrers.iter().map(|n| n.as_str())),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
        ),
        (
            "absent",
            option(view.absent.map(|why| {
                case(
                    "Skipped",
                    match why {
                        ply_test::Absent::NoBodies => "NoBodies",
                        ply_test::Absent::NoHybrids => "NoHybrids",
                    },
                    Vec::new(),
                )
            })),
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

fn key_value(key: &ply_test::DefKey) -> PlyValue {
    record(vec![
        ("name", PlyValue::str(key.name.as_str())),
        ("ns", ns_value(key.ns)),
    ])
}

fn ns_value(ns: ply_test::Ns) -> PlyValue {
    case(
        "Ns",
        match ns {
            ply_test::Ns::Value => "Value",
            ply_test::Ns::Decl => "Decl",
        },
        Vec::new(),
    )
}

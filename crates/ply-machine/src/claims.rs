//! What `ply prove` and `ply review` load, discharge, review and accept, as the program in
//! `crates/ply-cli/ply` performs it.
//!
//! The prover's entries stay here: discharging a claim enters compiled bodies. The obligations, the
//! types they are written over and the search each one goes to are the program's (`proof.world`),
//! handed over with the front end; which claims are asked for, the keys their evidence is read and
//! filed under, the evidence and the store that keeps it, what the review, the coverage and the
//! baseline come to, every line and key of both reports and the code each run exits with are the
//! program's too, in `crates/ply-cli/ply/claims.ply`, `prove.ply` and `review.ply`.

use crate::config::Configuration;
use crate::engine::{Interleaved, Judgement, Mode, Obligation};
use crate::hosts::{Hosts, LentOp};
use crate::load::{LoadError, Loaded};
use crate::payload::{ctor, diags_value, places_value, record, strings};
use crate::support::unit_of;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_eval::{Diagnostic, SourceMap, Span, Symbol, Value as PlyValue, codes};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};

/// The effect `crates/ply-cli/ply/claims.ply` declares. It is lent to the two entries that read
/// obligations and nowhere else.
const EFFECT: &str = "prover";

/// A law that reaches the host is refused rather than bound.
const HERMETIC: &str = "hermetic_prover";

/// Where each type this side marshals is declared, by the type's own name, with every case of it
/// this side builds.
///
/// A constructor crosses the substrate boundary by its program-wide name -- `claims.Raised` is not
/// `proof.obligation.Raised` -- so building one says which module declares its type, and this is
/// the only place that says it, save `Refusal`: that is declared beside `prover`, so it is named by
/// the module the lent program declares `prover` in. A case is built only through [`case`], which
/// refuses one this table does not list, and `every_marshalled_type_is_declared_where_this_side_says`
/// holds every row to the program, because a tag that names no declaration is a placeless `no arm
/// of this match matched` the moment the program matches the value.
pub const MARSHALLED: &[(&str, &str, &[&str])] = &[(
    "proof.obligation",
    "Judged",
    &[
        "JHeld",
        "JFailed",
        "JRejected",
        "JRaised",
        "JFaulted",
        "JMeasured",
        "JSpent",
        "JDrew",
    ],
)];

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
    ctor(home, name, args)
}

const OPERATIONS: [(&str, &str); 8] = [
    ("configure", "ply_machine::claims::configure"),
    ("collected", "ply_machine::claims::collected"),
    ("compiled", "ply_machine::claims::compiled"),
    ("schema", "ply_machine::claims::schema"),
    ("prepared", "ply_machine::claims::prepared"),
    ("judged", "ply_machine::claims::judged"),
    ("interleaved", "ply_machine::claims::interleaved"),
    ("ended", "ply_machine::claims::ended"),
];

const HERMETIC_OPERATIONS: [(&str, &str); 8] = [
    ("configure", "ply_machine::claims::hermetic::configure"),
    ("collected", "ply_machine::claims::hermetic::collected"),
    ("compiled", "ply_machine::claims::hermetic::compiled"),
    ("schema", "ply_machine::claims::hermetic::schema"),
    ("prepared", "ply_machine::claims::hermetic::prepared"),
    ("judged", "ply_machine::claims::hermetic::judged"),
    ("interleaved", "ply_machine::claims::hermetic::interleaved"),
    ("ended", "ply_machine::claims::hermetic::ended"),
];

/// A compiled body honours its call bound on the native stack, where unoptimised frames run to
/// kilobytes. Reserved, not committed.
const CLAIMS_STACK: usize = 256 << 20;

/// What the machine is asked to work with, which is every flag that is not about the report.
pub struct Job {
    pub path: PathBuf,
    /// The front end the CLI ran. A run without one is refused rather than loading again: `ply
    /// prove` and `ply review` start one and hand its answer over.
    pub front: Option<crate::driver::LoadedAnalysis>,
    /// The obligations the program owes, as it built them.
    pub obligations: Vec<Obligation>,
    /// What `--host` binds, which a `law/host` is discharged against; `None` without it, which
    /// is every `ply review`.
    pub binding: Option<Binding>,
    /// Loads only what it is handed and binds no host: see [`HERMETIC`].
    pub hermetic: bool,
}

/// What a `law/host` is discharged against.
pub struct Binding {
    pub tls: crate::options::TlsOptions,
    pub fs: Vec<ply_host::fs::RootSpec>,
    pub trace: crate::trace::TraceOptions,
}

/// The operations, for a program that declares `prover` in `module`: a value this side builds of a
/// type that module declares is named as that program names it.
pub fn lent(module: &str) -> Vec<LentOp> {
    let mut ops = lent_by(module, false);
    ops.extend(lent_by(module, true));
    ops
}

fn lent_by(module: &str, hermetic: bool) -> Vec<LentOp> {
    let handler: Arc<dyn HostHandler> = Arc::new(ProverHandler {
        hermetic,
        job: Mutex::new(None),
        machine: Mutex::new(None),
        claims: Mutex::new(0),
        judging: RwLock::new(None),
        module: module.to_string(),
    });
    let operations = if hermetic {
        HERMETIC_OPERATIONS
    } else {
        OPERATIONS
    };
    operations
        .into_iter()
        .map(|(op, path)| (registration(op, path, hermetic), Arc::clone(&handler)))
        .collect()
}

fn registration(op: &str, path: &'static str, hermetic: bool) -> HostOp {
    HostOp {
        effect: Symbol::new(if hermetic { HERMETIC } else { EFFECT }),
        op: Symbol::new(op),
        resource: HostResource::Any,
        // A tree, a cache on disk and a clock are not functions of program state.
        determinism: if hermetic {
            Determinism::Deterministic
        } else {
            Determinism::Nondeterministic
        },
        // Each step is performed once, in order; nothing here is replayed.
        linearity: Linearity::Repeatable,
        // The answer is in hand when the operation returns: this thread waits for the one the
        // work lives on rather than being handed a token to poll.
        blocking: false,
        secrets: false,
        path,
    }
}

struct ProverHandler {
    /// Loads only what it is handed and binds no host.
    hermetic: bool,
    /// Taken by the first operation, which is what starts the machine.
    job: Mutex<Option<Job>>,
    machine: Mutex<Option<Machine>>,
    /// How many claims the collection held, so a re-run can refuse an index that names none
    /// before reaching the thread.
    claims: Mutex<usize>,
    /// The prover, once a discharge has built it: claims are judged on whichever threads ask.
    judging: RwLock<Option<Arc<Judging>>>,
    /// Where the lent program declares `prover`, and so the `Refusal` it matches.
    module: String,
}

impl HostHandler for ProverHandler {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match (req.op.op.as_str(), req.args) {
            ("configure", [options, front, world]) => {
                let mut job = job_of(options, span)?;
                job.hermetic = self.hermetic;
                job.front = Some(crate::driver::loaded_analysis_of(front, span)?);
                job.obligations = crate::engine::obligations_of(
                    ply_eval::decode::AnswerValue::new("the world", world),
                )
                .map_err(|e| unread_world(&e, span))?;
                // A configuration begins a run, whatever the last one was left doing: its machine
                // is dropped, which joins its thread, and its claims are no longer this run's.
                let previous = self.held().take();
                drop(previous);
                *self.claims.lock().unwrap_or_else(|e| e.into_inner()) = 0;
                *self.job.lock().unwrap_or_else(|e| e.into_inner()) = Some(job);
                PlyValue::Unit
            }
            ("collected", _) => self.collected()?,
            ("compiled", [unit]) => self.compiled(unit.as_bytes(span, "the program's unit")?)?,
            ("schema", [name]) => self.schema(name.as_str(span, "a definition's name")?)?,
            ("prepared", [step_budget, config]) => self.prepared(
                step_budget.as_int(span, "the calls an evaluation may make")?,
                Configuration::of(config, span)?,
            )?,
            ("judged", [batches]) => self.judged(batches_of(batches, span)?)?,
            ("interleaved", [claim, point, seed, steps]) => self.interleaved(
                usize::try_from(claim.as_int(span, "the claim's place")?).unwrap_or(usize::MAX),
                points_of(&PlyValue::list(vec![point.clone()]), span)?
                    .pop()
                    .unwrap_or_default(),
                crate::recording::seed_of(seed, span)?,
                u32::try_from(steps.as_int(span, "the scheduling steps")?).unwrap_or(u32::MAX),
            )?,
            ("ended", []) => diags_value(&self.judging("ended")?.take_ended()),
            (other, _) => return Err(unasked(other, span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

/// One batch of points the program sent: whose claim, the points as plain values, and how they are
/// judged.
pub struct Batch {
    claim: usize,
    points: Vec<Vec<ply_eval::Plain>>,
    mode: Mode,
}

/// The `proof.property.Batch`es the program sent.
fn batches_of(value: &PlyValue, span: Span) -> Result<Vec<Batch>, Diagnostic> {
    use crate::payload::field_of;
    let mut out = Vec::new();
    for batch in value.as_list(span, "the batches to judge")? {
        let claim = field_of(batch, "claim", span)?.as_int(span, "a claim's place")?;
        let (mode, args) = case_of(field_of(batch, "mode", span)?, "a mode", span)?;
        out.push(Batch {
            claim: usize::try_from(claim).unwrap_or(usize::MAX),
            points: points_of(field_of(batch, "points", span)?, span)?,
            mode: match mode {
                "MWhole" => Mode::Whole,
                "MWitness" => Mode::Witness,
                "MDomain" => Mode::Domain,
                "MDrawn" => Mode::Drawn,
                "MCost" => match args {
                    [limit] => Mode::Cost {
                        limit: limit.as_int(span, "the steps one size may take")?,
                    },
                    _ => return Err(malformed("`MCost` carries one limit", span)),
                },
                other => return Err(malformed(&format!("`{other}` is no mode"), span)),
            },
        });
    }
    Ok(out)
}

/// Tuples of `std.value.Value`s, as the plain values they name.
fn points_of(value: &PlyValue, span: Span) -> Result<Vec<Vec<ply_eval::Plain>>, Diagnostic> {
    let mut out = Vec::new();
    for point in value.as_list(span, "the points")? {
        let mut values = Vec::new();
        for v in point.as_list(span, "a point")? {
            values.push(ply_eval::reflect::plain_of(v, span)?);
        }
        out.push(values);
    }
    Ok(out)
}

/// A constructor's simple name and its arguments.
fn case_of<'v>(
    value: &'v PlyValue,
    what: &str,
    span: Span,
) -> Result<(&'v str, &'v [PlyValue]), Diagnostic> {
    match value {
        PlyValue::Ctor { name, args } => Ok((
            name.as_str()
                .rsplit_once('.')
                .map_or(name.as_str(), |(_, simple)| simple),
            args,
        )),
        _ => Err(malformed(&format!("{what} is no constructor"), span)),
    }
}

/// A value the program's types promise a shape for, without it: Ply disagreeing with itself.
fn malformed(why: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the program's decision is malformed: {why}"),
    )
    .primary(span, "this is what the program sent")
    .note("`proof.decide` and this reader are one program's two halves; this is Ply's fault")
}

impl ProverHandler {
    fn held(&self) -> std::sync::MutexGuard<'_, Option<Machine>> {
        self.machine.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn collected(&self) -> Result<PlyValue, Diagnostic> {
        let mut held = self.held();
        if held.is_none() {
            let job = self
                .job
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .ok_or_else(|| twice("collected"))?;
            *held = Some(Machine::start(job)?);
        }
        let machine = held.as_ref().ok_or_else(|| unstarted("collected"))?;
        match machine.reply()? {
            Reply::Collected(answer) => {
                if let Ok(collection) = &*answer {
                    *self.claims.lock().unwrap_or_else(|e| e.into_inner()) = collection.obligations;
                }
                Ok(self.answered((*answer).map(collection_value)))
            }
            _ => Err(out_of_step("collected")),
        }
    }

    /// The program's unit, compiled from the C it handed over: what the schema and every batch a
    /// discharge judges run on.
    fn compiled(&self, unit: &[u8]) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("compiled"))?;
        machine.ask(Request::Compiled(unit.to_vec()))?;
        match machine.reply()? {
            Reply::Compiled(Ok(())) => Ok(PlyValue::ctor("Ok", vec![PlyValue::Unit])),
            Reply::Compiled(Err(diagnostic)) => Ok(PlyValue::ctor(
                "Err",
                vec![diags_value(std::slice::from_ref(&diagnostic))],
            )),
            _ => Err(out_of_step("compiled")),
        }
    }

    /// The value of the definition `--config-schema` names, entered on the program's unit.
    fn schema(&self, name: &str) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("schema"))?;
        machine.ask(Request::Schema(name.to_string()))?;
        match machine.reply()? {
            Reply::Schema(answer) => Ok(crate::config::schema_answer(answer)),
            _ => Err(out_of_step("schema")),
        }
    }

    /// Binds the hosts, answering `config` as the program resolved it, and builds the prover a
    /// discharge runs against, once, each evaluation of a claim bounded by `step_budget` calls.
    fn prepared(
        &self,
        step_budget: i64,
        configuration: Configuration,
    ) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("prepared"))?;
        machine.ask(Request::Prepare(step_budget, configuration))?;
        match machine.reply()? {
            Reply::Prepared(answer) => Ok(self.answered((*answer).map(|ready| {
                *self.judging.write().unwrap_or_else(|e| e.into_inner()) = Some(ready.judging);
                PlyValue::Unit
            }))),
            _ => Err(out_of_step("prepared")),
        }
    }

    /// The prover a discharge built. It reaches no machine: the program judges from many threads
    /// at once.
    fn judging(&self, op: &str) -> Result<Arc<Judging>, Diagnostic> {
        self.judging
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|judging| judging.open.load(Ordering::Acquire))
            .cloned()
            .ok_or_else(|| out_of_step(op))
    }

    /// Each batch's points judged in turn, on this thread.
    fn judged(&self, batches: Vec<Batch>) -> Result<PlyValue, Diagnostic> {
        let claims = *self.claims.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = batches.iter().find(|b| b.claim >= claims) {
            return Err(no_such_claim(batch.claim, claims));
        }
        let judging = self.judging("judged")?;
        Ok(PlyValue::list(
            batches
                .iter()
                .map(|batch| {
                    PlyValue::list(judging.judged(batch).iter().map(judged_value).collect())
                })
                .collect(),
        ))
    }

    /// A law over interleavings, run at one of its points under one seed, on this thread.
    fn interleaved(
        &self,
        claim: usize,
        point: Vec<ply_eval::Plain>,
        seed: ply_eval::Seed,
        steps: u32,
    ) -> Result<PlyValue, Diagnostic> {
        let claims = *self.claims.lock().unwrap_or_else(|e| e.into_inner());
        if claim >= claims {
            return Err(no_such_claim(claim, claims));
        }
        let judging = self.judging("interleaved")?;
        Ok(interleaved_value(
            &judging.interleaved(claim, point, &seed, steps),
        ))
    }
}

/// `Ok(v)` or `Err(Refusal)`, as the program reads an operation's answer.
impl ProverHandler {
    fn answered(&self, answer: Result<PlyValue, Refused>) -> PlyValue {
        match answer {
            Ok(value) => PlyValue::ctor("Ok", vec![value]),
            Err(refused) => PlyValue::ctor("Err", vec![refusal_value(&refused, &self.module)]),
        }
    }
}

// --- The thread the work lives on ---------------------------------------------

/// What the program asks the machine for next.
enum Request {
    /// Compile the program's unit from the C it handed over.
    Compiled(Vec<u8>),
    /// Enter the definition `--config-schema` names on the program's unit.
    Schema(String),
    /// Bind the hosts over the configuration the program resolved and build the prover a
    /// discharge runs against, with the calls each evaluation of a claim may make.
    Prepare(i64, Configuration),
}

enum Reply {
    Collected(Box<Result<Collection, Refused>>),
    Compiled(Result<(), Diagnostic>),
    Schema(Result<ply_eval::Plain, Diagnostic>),
    Prepared(Box<Result<Ready, Refused>>),
}

/// The thread the load and the store live on, and the prover is built on. Claims are judged on the
/// threads the program asks from, against the [`Judging`] this thread publishes.
struct Machine {
    requests: Option<mpsc::Sender<Request>>,
    replies: mpsc::Receiver<Reply>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Machine {
    fn start(job: Job) -> Result<Machine, Diagnostic> {
        let (requests, asked) = mpsc::channel();
        let (told, replies) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .stack_size(CLAIMS_STACK)
            .spawn(move || serve(job, &told, &asked))
            .map_err(|e| unspawned(&e))?;
        Ok(Machine {
            requests: Some(requests),
            replies,
            thread: Some(thread),
        })
    }

    fn ask(&self, request: Request) -> Result<(), Diagnostic> {
        match &self.requests {
            Some(sender) => sender.send(request).map_err(|_| unanswered()),
            None => Err(unanswered()),
        }
    }

    fn reply(&self) -> Result<Reply, Diagnostic> {
        self.replies.recv().map_err(|_| unanswered())
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        // Dropping the sender ends whichever wait the thread is parked on, so a run that stopped
        // short of discharging leaves nothing running.
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(job: Job, told: &mpsc::Sender<Reply>, asked: &mpsc::Receiver<Request>) {
    let mut warnings = Vec::new();
    let loaded = match load(&job) {
        Ok(loaded) => loaded,
        Err(err) => {
            let _ = told.send(Reply::Collected(Box::new(Err(Refused {
                kind: RefusalKind::Broken,
                diagnostics: err.diagnostics,
                sources: err.sources,
            }))));
            return;
        }
    };
    warnings.extend(loaded.frontend.warnings.iter().cloned());

    let obligations: &[Obligation] = &job.obligations;

    let _ = told.send(Reply::Collected(Box::new(Ok(Collection {
        sources: loaded.sources.clone(),
        warnings: std::mem::take(&mut warnings),
        obligations: obligations.len(),
        host: job.binding.is_some(),
    }))));

    // The unit the program handed over, and the hosts once bound: the schema and every batch a
    // discharge judges run on the same unit, and the batches over the same hosts.
    let mut backend: Option<Arc<dyn ply_eval::Provider>> = None;
    let mut prepared: Option<Result<Prepared, Refused>> = None;
    loop {
        match asked.recv() {
            Ok(Request::Compiled(unit)) => {
                let built = unit_of(&loaded.front, &unit).map(|unit| {
                    backend = Some(unit);
                });
                let _ = told.send(Reply::Compiled(built));
            }
            Ok(Request::Schema(name)) => {
                let answer = match &backend {
                    Some(unit) => {
                        crate::config::schema_of(&loaded.check, Some(Arc::clone(unit)), &name)
                    }
                    None => Err(uncompiled()),
                };
                let _ = told.send(Reply::Schema(answer));
            }
            Ok(Request::Prepare(step_budget, configuration)) => {
                if prepared.is_none() {
                    let built = backend.clone().ok_or_else(uncompiled);
                    prepared = Some(prepare(&job, &loaded, built, step_budget, configuration));
                }
                let answer = match prepared.as_ref() {
                    Some(Ok(ready)) => Ok(Ready {
                        judging: Arc::clone(&ready.judging),
                    }),
                    Some(Err(refused)) => Err(refused.clone()),
                    None => return,
                };
                let _ = told.send(Reply::Prepared(Box::new(answer)));
            }
            Err(_) => return,
        }
    }
}

/// Every module parsed: a clause the run did not read is a claim nobody checked.
///
/// The walk and the compiler are the CLI's, and what it answered is what this reads.
fn load(job: &Job) -> Result<Loaded, LoadError> {
    let Some(front) = &job.front else {
        return Err(LoadError {
            sources: ply_eval::SourceMap::new(),
            diagnostics: vec![
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    "the CLI handed no front end over, and this side runs none",
                )
                .note(
                    "the CLI walks the tree and runs the compiler; a run without its answer has \
                 nothing to discharge",
                ),
            ],
        });
    };
    if job.hermetic {
        crate::driver::load_over_analysis_in(crate::load::tidy(&job.path), front)
    } else {
        crate::driver::load_over_analysis(&job.path, front)
    }
}

// --- Discharging ---------------------------------------------------------------

/// The prover and the hosts a run discharges and re-runs points with, built when the first step
/// that needs them asks: a discharge of many claims and a re-run of one case are the same engine.
struct Prepared {
    /// Kept alive for the prover's lifetime, which is the run's.
    _hosts: Option<Hosts>,
    judging: Arc<Judging>,
}

impl Drop for Prepared {
    /// Before the hosts go: nothing is judged against a stopped host.
    fn drop(&mut self) {
        self.judging.open.store(false, Ordering::Release);
    }
}

/// What preparing answers: what opening the hosts had to say, and the prover it built.
struct Ready {
    judging: Arc<Judging>,
}

/// What judging a claim needs, owned, so every thread the program judges from can read it.
struct Judging {
    prover: crate::engine::Prover,
    obligations: Vec<Obligation>,
    /// The calls each evaluation of a claim may make; it decides what an evaluation reports.
    step_budget: i64,
    open: AtomicBool,
    /// What every entry judging made ended with, from any thread, until `ended` takes it.
    ended: Mutex<Vec<Diagnostic>>,
}

impl Judging {
    fn judged(&self, batch: &Batch) -> Vec<Judgement> {
        let judgements = match (self.obligations.get(batch.claim), values_of(&batch.points)) {
            (Some(obligation), Ok(values)) => {
                self.prover
                    .judged(obligation, self.step_budget, &values, batch.mode)
            }
            (None, _) => {
                return vec![Judgement::Faulted(no_such_claim(
                    batch.claim,
                    self.obligations.len(),
                ))];
            }
            (_, Err(fault)) => return vec![Judgement::Faulted(fault)],
        };
        self.keep(judgements.warnings);
        judgements.each
    }

    fn interleaved(
        &self,
        claim: usize,
        point: Vec<ply_eval::Plain>,
        seed: &ply_eval::Seed,
        steps: u32,
    ) -> crate::engine::Interleaved {
        let Some(obligation) = self.obligations.get(claim) else {
            return Interleaved::faulted(no_such_claim(claim, self.obligations.len()));
        };
        let mut run = match values_of(&[point]) {
            Ok(mut values) => self.prover.interleaved(
                obligation,
                self.step_budget,
                &values.pop().unwrap_or_default(),
                seed,
                steps,
            ),
            Err(fault) => return Interleaved::faulted(fault),
        };
        self.keep(std::mem::take(&mut run.warnings));
        run
    }

    fn keep(&self, warnings: Vec<Diagnostic>) {
        if !warnings.is_empty() {
            self.ended
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(warnings);
        }
    }

    /// Each warning once: every point a claim is judged at ends an entry, and says the same.
    fn take_ended(&self) -> Vec<Diagnostic> {
        crate::support::once_each(std::mem::take(
            &mut *self.ended.lock().unwrap_or_else(|e| e.into_inner()),
        ))
    }
}

fn prepare(
    job: &Job,
    loaded: &Loaded,
    backend: Result<Arc<dyn ply_eval::Provider>, Diagnostic>,
    step_budget: i64,
    configuration: Configuration,
) -> Result<Prepared, Refused> {
    let unbound = |diagnostics: Vec<Diagnostic>| Refused {
        kind: RefusalKind::Unbound,
        diagnostics,
        sources: loaded.sources.clone(),
    };
    let backend = backend.map_err(|d| unbound(vec![d]))?;
    if job.hermetic && job.binding.is_some() {
        return Err(unbound(vec![hermetic_host()]));
    }
    let hosts = match &job.binding {
        None => None,
        Some(binding) => Some(
            Hosts::open(
                &loaded.check,
                true,
                &binding.tls,
                &binding.fs,
                configuration,
                &binding.trace,
            )
            .map_err(&unbound)?,
        ),
    };
    let hosting = hosts.as_ref().map(|hosts| crate::engine::Hosting {
        binding: hosts.binding(),
        runtime: hosts.runtime_factory(),
    });
    Ok(Prepared {
        _hosts: hosts,
        judging: Arc::new(Judging {
            prover: crate::engine::prover(loaded, hosting, backend),
            obligations: job.obligations.clone(),
            step_budget,
            open: AtomicBool::new(true),
            ended: Mutex::new(Vec::new()),
        }),
    })
}

/// The values plain values name, on the thread that judges them.
fn values_of(points: &[Vec<ply_eval::Plain>]) -> Result<Vec<Vec<PlyValue>>, Diagnostic> {
    points
        .iter()
        .map(|point| {
            point
                .iter()
                .map(|plain| plain.clone().into_value())
                .collect::<Result<Vec<PlyValue>, _>>()
        })
        .collect::<Result<_, _>>()
        .map_err(|why| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("a point the program drew is no runtime value: {why}"),
            )
            .note("the program draws only values of its claims' types; this is Ply's fault")
        })
}

#[derive(Clone)]
enum RefusalKind {
    Broken,
    Unbound,
}

#[derive(Clone)]
struct Refused {
    kind: RefusalKind,
    diagnostics: Vec<Diagnostic>,
    sources: SourceMap,
}

/// What loading the run came to. The obligations, and the definitions and laws a run answers for,
/// are the program's; this counts the obligations, so a re-run can refuse an index that names none.
struct Collection {
    sources: SourceMap,
    warnings: Vec<Diagnostic>,
    obligations: usize,
    /// Whether a `law/host` has a host bound to run against.
    host: bool,
}

// --- The values the program reads -------------------------------------------------

fn refusal_value(refused: &Refused, module: &str) -> PlyValue {
    let named = match refused.kind {
        RefusalKind::Broken => "Broken",
        RefusalKind::Unbound => "Unbound",
    };
    ctor(
        module,
        named,
        vec![record(vec![
            ("diags", diags_value(&refused.diagnostics)),
            ("places", places_value(&refused.sources)),
        ])],
    )
}

fn collection_value(collection: Collection) -> PlyValue {
    record(vec![
        ("places", places_value(&collection.sources)),
        ("warnings", diags_value(&collection.warnings)),
        ("host", PlyValue::Bool(collection.host)),
    ])
}

/// The values a diagnostic's text names, which `std.value.filled` puts in place.
fn shown_values(diagnostic: &Diagnostic) -> PlyValue {
    PlyValue::list(
        diagnostic
            .values
            .iter()
            .map(ply_eval::reflect::value_of)
            .collect(),
    )
}

/// A `proof.property.Interleaved` reply's payload: the recording as `sim.recording` spells it.
fn interleaved_value(run: &crate::engine::Interleaved) -> PlyValue {
    record(vec![
        (
            "interleaving",
            crate::recording::interleaving_value(
                &run.interleaving,
                run.verdict.as_ref().map(judged_value),
            ),
        ),
        ("observed", PlyValue::Bool(run.observed)),
    ])
}

fn judged_value(judgement: &Judgement) -> PlyValue {
    match judgement {
        Judgement::Held => case("Judged", "JHeld", Vec::new()),
        Judgement::Failed => case("Judged", "JFailed", Vec::new()),
        Judgement::Rejected => case("Judged", "JRejected", Vec::new()),
        Judgement::Raised(diagnostic) => case(
            "Judged",
            "JRaised",
            vec![record(vec![
                ("message", PlyValue::str(&diagnostic.message)),
                ("values", shown_values(diagnostic)),
            ])],
        ),
        Judgement::Faulted(diagnostic) => case(
            "Judged",
            "JFaulted",
            vec![record(vec![
                ("code", PlyValue::str(diagnostic.code)),
                ("message", PlyValue::str(&diagnostic.message)),
                (
                    "notes",
                    strings(diagnostic.notes.iter().map(String::as_str)),
                ),
                ("values", shown_values(diagnostic)),
            ])],
        ),
        Judgement::Measured { steps, bound } => case(
            "Judged",
            "JMeasured",
            vec![record(vec![
                ("bound", PlyValue::Int(*bound)),
                ("steps", PlyValue::Int(*steps)),
            ])],
        ),
        Judgement::Spent { limit } => case("Judged", "JSpent", vec![PlyValue::Int(*limit)]),
        Judgement::Drew(plain) => case("Judged", "JDrew", vec![ply_eval::reflect::value_of(plain)]),
    }
}

// --- Small things -------------------------------------------------------------

#[cold]
fn unspawned(e: &std::io::Error) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("the claims could not be read on a thread of their own: {e}"),
    )
    .primary(Span::DUMMY, "nothing was discharged")
}

#[cold]
fn unanswered() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the thread this run's claims live on stopped without answering",
    )
    .note("the program and the thread it drives are written together; this is Ply's fault")
}

#[cold]
fn unstarted(op: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` was performed before the claims were collected"),
    )
    .note("the operations are performed in order; this is Ply's fault")
}

#[cold]
fn twice(op: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` was performed twice, and one load serves the whole command"),
    )
    .note("the operations are performed in order; this is Ply's fault")
}

#[cold]
fn no_such_claim(index: usize, claims: usize) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("there is no claim {index}: the collection holds {claims}"),
    )
    .note("a claim is named by its place in the collection the run read; this is Ply's fault")
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
        format!("`{EFFECT}.{op}` reached the binding, and no command serves such an operation"),
    )
    .primary(span, "this perform reached `ply prove`")
    .note("the effect and its handler are written together; this is Ply's fault")
}

#[cold]
fn unread_world(why: &ply_eval::decode::Error, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the program's world does not read: {why}"),
    )
    .primary(span, "the program handed this over")
    .note("`proof.world` and this reader are written together; this is Ply's fault")
}

// --- The job the program parses ---------------------------------------------------

/// The job record as the program builds it from the parsed line, read field by field.
fn job_of(v: &PlyValue, span: Span) -> Result<Job, Diagnostic> {
    use crate::payload::{field_of, str_list_at};
    let bool_at = |name: &str| field_of(v, name, span)?.as_bool(span, name);
    let str_at = |name: &str| {
        field_of(v, name, span)?
            .as_str(span, name)
            .map(str::to_string)
    };
    let binding = if bool_at("host")? {
        let tls_list = field_of(v, "tls", span)?;
        let mut tls = Vec::new();
        for item in tls_list.as_list(span, "tls")?.iter() {
            tls.push(ply_host::tls::CredentialSpec {
                name: field_of(item, "name", span)?
                    .as_str(span, "a name")?
                    .to_string(),
                certificate: PathBuf::from(
                    field_of(item, "cert", span)?.as_str(span, "a certificate")?,
                ),
                key: PathBuf::from(field_of(item, "key", span)?.as_str(span, "a key")?),
            });
        }
        let fs_list = field_of(v, "fs", span)?;
        let mut fs = Vec::new();
        for item in fs_list.as_list(span, "fs")?.iter() {
            fs.push(ply_host::fs::RootSpec {
                name: field_of(item, "name", span)?
                    .as_str(span, "a name")?
                    .to_string(),
                path: PathBuf::from(field_of(item, "path", span)?.as_str(span, "a path")?),
            });
        }
        let trace = field_of(v, "trace", span)?;
        Some(Binding {
            tls: crate::options::TlsOptions {
                tls,
                trust: str_list_at(v, "trust", span)?
                    .into_iter()
                    .map(PathBuf::from)
                    .collect(),
                mtls: str_list_at(v, "mtls", span)?,
            },
            fs,
            trace: crate::trace::TraceOptions {
                sink: match field_of(trace, "sink", span)?.as_str(span, "the trace sink")? {
                    "text" => crate::trace::SinkArg::Text,
                    "off" => crate::trace::SinkArg::Off,
                    _ => crate::trace::SinkArg::Json,
                },
                level: match field_of(trace, "level", span)?.as_str(span, "the trace level")? {
                    "debug" => crate::trace::LevelArg::Debug,
                    "warn" => crate::trace::LevelArg::Warn,
                    "error" => crate::trace::LevelArg::Error,
                    _ => crate::trace::LevelArg::Info,
                },
            },
        })
    } else {
        None
    };
    Ok(Job {
        path: PathBuf::from(str_at("path")?),
        front: None,
        obligations: Vec::new(),
        binding,
        hermetic: false,
    })
}

#[cold]
fn hermetic_host() -> Diagnostic {
    Diagnostic::error(
        codes::CAPABILITY_UNDECLARED,
        format!("`{HERMETIC}` binds no host, and the run asked for `--host`"),
    )
    .note("a program that discharges a `law/host` reaches the host, and performs `prover`")
}

/// A step that enters the program's unit, asked before the program handed one over.
fn uncompiled() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the prover was asked to enter the program before the program handed its unit over",
    )
    .note("`prover.compiled` comes first: the unit is the program's to produce")
}

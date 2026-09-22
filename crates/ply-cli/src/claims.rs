//! What `ply prove` and `ply review` load, collect, discharge, review and accept, as the program
//! in `crates/ply-cli/ply` performs it.
//!
//! The front end, the store, the prover and the review baseline stay here: a front end is not a
//! value a program can hold, discharging a claim enters compiled bodies, and an entry does not
//! nest on the thread the `ply` program itself runs on. Which claims are asked for, what every
//! line and key of both reports says and the code each run exits with are the program's, in
//! `crates/ply-cli/ply/claims.ply`, `prove.ply` and `review.ply`.

use crate::commands::common::{build_pool, enter_constant, prover_backend};
use crate::config::Configuration;
use crate::hosts::{Hosts, Lent};
use crate::load::{LoadError, Loaded};
use crate::payload::{count, ctor, diags_value, option, places_value, record, strings};
use ply_eval::Value as PlyValue;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_prove::{
    Discharge, Evidence, Frame, Gap, Obligation, ObligationKind, ProvePlan, ProveReport, Tier,
    Vacuity, VacuityKind,
};
use ply_span::{Diagnostic, SourceMap, Span, Symbol, codes};
use ply_store::Store;
use ply_test::obligation::{self, Laws, Moved, Proved, Reason, ReviewReport};
use ply_ty::CheckOutput;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

/// The effect `crates/ply-cli/ply/claims.ply` declares. It is lent to the two entries that read
/// obligations and nowhere else.
const EFFECT: &str = "prover";

/// The module the payload's constructors are declared in, as a program-wide name.
const PAYLOAD: &str = "claims";

const OPERATIONS: [(&str, &str); 4] = [
    ("collected", "ply_cli::claims::collected"),
    ("discharged", "ply_cli::claims::discharged"),
    ("reviewed", "ply_cli::claims::reviewed"),
    ("accepted", "ply_cli::claims::accepted"),
];

/// A compiled body honours its call bound on the native stack, where unoptimised frames run to
/// kilobytes. Reserved, not committed.
const CLAIMS_STACK: usize = 256 << 20;

/// What the machine is asked to work with, which is every flag that is not about the report.
pub struct Job {
    pub path: PathBuf,
    pub incremental: bool,
    pub use_cache: bool,
    /// Also discharge what the shipped modules declare.
    pub std: bool,
    pub jobs: Option<u32>,
    pub backend: Option<String>,
    pub plan: ProvePlan,
    /// `None` for a command that binds nothing at all, which is every `ply review`.
    pub binding: Option<Binding>,
}

/// What a `law/host` is discharged against, and what a hermetic run refuses to reach.
pub struct Binding {
    pub host: bool,
    pub tls: crate::cli::TlsOptions,
    pub fs: crate::cli::FsOptions,
    pub db: crate::db::DbOptions,
    pub config: crate::config::ConfigOptions,
    pub trace: crate::trace::TraceOptions,
}

pub fn lent(job: Job) -> Vec<Lent> {
    let site: Arc<dyn HostHandler> = Arc::new(Site {
        job: Mutex::new(Some(job)),
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
        // A tree, a cache on disk and a clock are not functions of program state.
        determinism: Determinism::Nondeterministic,
        // Each step is performed once, in order; nothing here is replayed.
        linearity: Linearity::Repeatable,
        // The answer is in hand when the operation returns: this thread waits for the one the
        // work lives on rather than being handed a token to poll.
        blocking: false,
        secrets: false,
        path,
    }
}

struct Site {
    /// Taken by the first operation, which is what starts the machine.
    job: Mutex<Option<Job>>,
    machine: Mutex<Option<Machine>>,
}

impl HostHandler for Site {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match (req.op.op.as_str(), req.args) {
            ("collected", _) => self.collected()?,
            ("discharged", [wanted]) => self.discharged(&indices(wanted, span)?)?,
            ("reviewed", _) => self.reviewed()?,
            ("accepted", _) => self.accepted()?,
            (other, _) => return Err(unasked(other, span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

fn indices(value: &PlyValue, span: Span) -> Result<Vec<usize>, Diagnostic> {
    value
        .as_list(span, "the claims to discharge")?
        .iter()
        .map(|item| {
            item.as_int(span, "a claim's place in the collection")
                .map(|index| usize::try_from(index).unwrap_or(usize::MAX))
        })
        .collect()
}

impl Site {
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
        match machine.step()? {
            Step::Collected(answer) => Ok(answered((*answer).map(collection_value))),
            _ => Err(out_of_step("collected")),
        }
    }

    fn discharged(&self, wanted: &[usize]) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("discharged"))?;
        machine.ask(Go::Discharge(wanted.to_vec()))?;
        match machine.step()? {
            Step::Discharged(answer) => Ok(answered((*answer).map(|v| verdicts_value(&v)))),
            _ => Err(out_of_step("discharged")),
        }
    }

    fn reviewed(&self) -> Result<PlyValue, Diagnostic> {
        let mut held = self.held();
        let step = {
            let machine = held.as_ref().ok_or_else(|| unstarted("reviewed"))?;
            machine.ask(Go::Review)?;
            machine.step()?
        };
        // Nothing follows a review: the thread it happened on is joined here.
        held.take();
        match step {
            Step::Reviewed(changes) => Ok(changes_value(&changes)),
            _ => Err(out_of_step("reviewed")),
        }
    }

    fn accepted(&self) -> Result<PlyValue, Diagnostic> {
        let mut held = self.held();
        let step = {
            let machine = held.as_ref().ok_or_else(|| unstarted("accepted"))?;
            machine.ask(Go::Accept)?;
            machine.step()?
        };
        held.take();
        match step {
            Step::Accepted(accepted) => Ok(accepted_value(&accepted)),
            _ => Err(out_of_step("accepted")),
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

// --- The thread the work lives on ---------------------------------------------

/// What the program asks the machine for next.
enum Go {
    Discharge(Vec<usize>),
    Review,
    Accept,
}

enum Step {
    Collected(Box<Result<Collection, Refused>>),
    Discharged(Box<Result<Verdicts, Refused>>),
    Reviewed(Box<Changes>),
    Accepted(Box<Accepted>),
}

/// The thread the load, the store and the prover live on. The `ply` program performing these
/// operations is itself inside an entry, and a compiled body entered while another entry holds the
/// same thread is declined rather than run — which would report every obligation as a gap.
struct Machine {
    go: Option<mpsc::Sender<Go>>,
    steps: mpsc::Receiver<Step>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Machine {
    fn start(job: Job) -> Result<Machine, Diagnostic> {
        let (go, asked) = mpsc::channel();
        let (told, steps) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .stack_size(CLAIMS_STACK)
            .spawn(move || serve(job, &told, &asked))
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
        // Dropping the sender ends whichever wait the thread is parked on, so a run that stopped
        // short of discharging leaves nothing running.
        self.go.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(job: Job, told: &mpsc::Sender<Step>, asked: &mpsc::Receiver<Go>) {
    let root = crate::load::project_root(&job.path);
    let mut store = match Store::open(&root) {
        Ok(store) => store.with_upstream(if job.use_cache {
            ply_store::Upstream::from_env()
        } else {
            None
        }),
        Err(e) => {
            let _ = told.send(Step::Collected(Box::new(Err(Refused {
                why: Why::Trouble,
                diagnostics: vec![unopened(&root, &e)],
                sources: SourceMap::new(),
            }))));
            return;
        }
    };
    let mut warnings = store.take_warnings();
    let loaded = match load(&job, &mut store) {
        Ok(loaded) => loaded,
        Err(err) => {
            let _ = told.send(Step::Collected(Box::new(Err(Refused {
                why: Why::Broken,
                diagnostics: err.diagnostics,
                sources: err.sources,
            }))));
            return;
        }
    };
    warnings.extend(store.take_warnings());
    warnings.extend(loaded.frontend.warnings.iter().cloned());

    let hashes = loaded.hashes.clone();
    let scoped = crate::obligations::project_view(&loaded.check, job.std);
    let laws = Laws::of(&scoped, &hashes);
    let collected = crate::obligations::collect(&loaded.front, &scoped, &hashes);
    warnings.extend(collected.warnings);
    // Before anything is discharged: carrying a claim is a fact about the program.
    let specified = obligation::specified(&scoped, &laws, &collected.obligations);
    let obligations = collected.obligations;
    let labels = law_labels(&loaded.check);

    let _ = told.send(Step::Collected(Box::new(Ok(Collection {
        sources: loaded.sources.clone(),
        warnings: std::mem::take(&mut warnings),
        claims: obligations
            .iter()
            .map(|o| claim_of(o, &loaded, &labels))
            .collect(),
        specified,
        plan: job.plan.clone(),
    }))));

    let mut report: Option<ProveReport> = None;
    loop {
        match asked.recv() {
            Ok(Go::Discharge(wanted)) => {
                let asked_for: Vec<Obligation> = wanted
                    .iter()
                    .filter_map(|&index| obligations.get(index).cloned())
                    .collect();
                match discharge(&job, &loaded, &scoped, &laws, asked_for, &mut store) {
                    Ok(proved) => {
                        let mut warnings = proved.warnings;
                        warnings.extend(flushed(&mut store));
                        let verdicts = verdicts_of(&proved.report, &proved.reasons, warnings);
                        report = Some(proved.report);
                        let _ = told.send(Step::Discharged(Box::new(Ok(verdicts))));
                    }
                    Err(refused) => {
                        let _ = told.send(Step::Discharged(Box::new(Err(refused))));
                        return;
                    }
                }
            }
            Ok(Go::Review) => {
                let Some(report) = report.as_ref() else {
                    return;
                };
                let reviewed = obligation::review(&scoped, &hashes, &laws, &store, report);
                let _ = told.send(Step::Reviewed(Box::new(changes_of(&reviewed))));
                return;
            }
            Ok(Go::Accept) => {
                let definitions = obligation::accept(&scoped, &hashes, &laws, &mut store);
                let trouble = store.flush().err().map(|e| unaccepted(&e));
                let mut warnings = store.take_warnings();
                let stored = trouble.is_none();
                warnings.extend(trouble);
                let _ = told.send(Step::Accepted(Box::new(Accepted {
                    definitions,
                    stored,
                    warnings,
                })));
                return;
            }
            Err(_) => return,
        }
    }
}

/// Every module parsed: a clause the run did not read is a claim nobody checked.
fn load(job: &Job, store: &mut Store) -> Result<Loaded, LoadError> {
    if job.incremental {
        crate::driver::load_incremental(&job.path, store)
    } else {
        crate::load::load(&job.path)
    }
}

fn flushed(store: &mut Store) -> Vec<Diagnostic> {
    let mut out = match store.flush() {
        Ok(()) => Vec::new(),
        Err(e) => vec![
            Diagnostic::warning(codes::CACHE_UNREADABLE, format!("{e:#}"))
                .note("nothing was recorded; the next run discharges everything again"),
        ],
    };
    out.extend(store.take_warnings());
    out
}

/// The label each law was written with, not its `<module>.<label>` key.
fn law_labels(check: &CheckOutput) -> BTreeMap<Symbol, String> {
    check
        .laws
        .iter()
        .map(|law| (law.key.clone(), law.name.clone()))
        .collect()
}

// --- Discharging ---------------------------------------------------------------

fn discharge(
    job: &Job,
    loaded: &Loaded,
    scoped: &CheckOutput,
    laws: &Laws,
    asked_for: Vec<Obligation>,
    store: &mut Store,
) -> Result<Proved, Refused> {
    let unbound = |diagnostics: Vec<Diagnostic>| Refused {
        why: Why::Unbound,
        diagnostics,
        sources: loaded.sources.clone(),
    };
    let backend = prover_backend(job.backend.as_ref(), loaded).map_err(|d| unbound(vec![d]))?;
    let constant =
        |name: &str| enter_constant(backend.as_ref().map(|(provider, _)| *provider), name);
    let mut warnings = Vec::new();
    let hosts = match &job.binding {
        None => None,
        Some(binding) => {
            let db = binding.db.resolve(binding.host).map_err(&unbound)?;
            let (configuration, opened) =
                Configuration::open(&loaded.check, binding.host, &binding.config, &constant)
                    .map_err(&unbound)?;
            warnings.extend(opened);
            // A file whose laws are all hermetic binds nothing.
            let reach = ply_ty::ty::Footprint::from_atoms(
                scoped
                    .laws
                    .iter()
                    .filter(|law| law.host)
                    .flat_map(|law| law.footprint.atoms().cloned()),
            );
            Some(
                Hosts::open(
                    &loaded.check,
                    binding.host,
                    &binding.tls,
                    &binding.fs.fs,
                    db,
                    configuration,
                    &binding.trace,
                    Some(&reach),
                )
                .map_err(&unbound)?,
            )
        }
    };
    let runtime = hosts.as_ref().and_then(Hosts::runtime_factory);
    let hosting = hosts
        .as_ref()
        .filter(|_| job.binding.as_ref().is_some_and(|b| b.host))
        .map(|hosts| crate::engine::Hosting {
            binding: hosts.binding(),
            runtime: runtime
                .as_ref()
                .map(|f| f as &(dyn Fn() -> std::rc::Rc<dyn ply_eval::host::HostRuntime> + Sync)),
        });
    let asked = obligation::Asked::new(asked_for, store, &job.plan, job.use_cache);
    // Built only when the cache left something to discharge.
    let engine: Box<dyn obligation::Discharger + '_> = if asked.pending() {
        match crate::engine::of(loaded, hosting, backend, store) {
            Ok(engine) => engine,
            Err(err) => {
                return Err(Refused {
                    why: Why::Broken,
                    diagnostics: err.diagnostics,
                    sources: err.sources,
                });
            }
        }
    } else {
        Box::new(obligation::Undecided)
    };
    let (pool, _workers) = build_pool(job.jobs, &mut warnings);
    let discharge = || asked.discharge(scoped, laws, store, engine.as_ref());
    let mut proved = match &pool {
        Some(pool) => pool.install(discharge),
        None => discharge(),
    };
    warnings.append(&mut proved.warnings);
    proved.warnings = warnings;
    Ok(proved)
}

// --- What crosses back ----------------------------------------------------------

enum Why {
    Broken,
    Unbound,
    Trouble,
}

struct Refused {
    why: Why,
    diagnostics: Vec<Diagnostic>,
    sources: SourceMap,
}

/// Where a claim is written, as a label points at it.
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

enum Kind {
    Ensures(usize),
    Law(Option<String>),
}

struct Claim {
    key: String,
    owner: String,
    kind: Kind,
    guarded: bool,
    frame: Frame,
    at: At,
    unperformed: Vec<String>,
}

struct Collection {
    sources: SourceMap,
    warnings: Vec<Diagnostic>,
    claims: Vec<Claim>,
    specified: usize,
    plan: ProvePlan,
}

struct Verdicts {
    outcomes: Vec<Discharge>,
    reasons: Vec<Reason>,
    coverage: ply_prove::Coverage,
    cached: usize,
    duration: std::time::Duration,
    warnings: Vec<Diagnostic>,
}

struct Changes {
    reviewed: usize,
    changed: Vec<Change>,
    broken: usize,
    undischarged: usize,
    duration: std::time::Duration,
}

struct Change {
    name: String,
    implementation: Moved,
    spec: Moved,
    obligations: Vec<usize>,
    holding: usize,
}

struct Accepted {
    definitions: usize,
    stored: bool,
    warnings: Vec<Diagnostic>,
}

fn claim_of(o: &Obligation, loaded: &Loaded, labels: &BTreeMap<Symbol, String>) -> Claim {
    Claim {
        key: o.key.to_hex(),
        owner: o.owner.as_str().to_string(),
        kind: match o.kind {
            ObligationKind::Ensures { index } => Kind::Ensures(index),
            ObligationKind::Law => Kind::Law(labels.get(&o.owner).cloned()),
        },
        guarded: o.guarded,
        frame: o.frame.clone(),
        at: At::of(o.span),
        unperformed: unperformed_of(o, loaded),
    }
}

/// The declared atoms an `ensures`'s owner never touched: a frame wider than the body is a weaker
/// claim than it looks. A law states its own frame, so it has none of this.
fn unperformed_of(o: &Obligation, loaded: &Loaded) -> Vec<String> {
    if o.kind == ObligationKind::Law {
        return Vec::new();
    }
    loaded
        .check
        .defs
        .get(&o.owner)
        .map(crate::signature::unperformed)
        .unwrap_or_default()
}

fn verdicts_of(report: &ProveReport, reasons: &[Reason], warnings: Vec<Diagnostic>) -> Verdicts {
    Verdicts {
        outcomes: report
            .obligations
            .iter()
            .map(|(_, discharge)| discharge.clone())
            .collect(),
        reasons: reasons.to_vec(),
        coverage: report.coverage.clone(),
        cached: report.cached,
        duration: report.duration,
        warnings,
    }
}

fn changes_of(review: &ReviewReport) -> Changes {
    Changes {
        reviewed: review.reviewed,
        changed: review
            .changed
            .iter()
            .map(|entry| Change {
                name: entry.name.as_str().to_string(),
                implementation: entry.implementation,
                spec: entry.spec,
                obligations: entry.obligations.clone(),
                holding: entry.holding,
            })
            .collect(),
        broken: review.broken,
        undischarged: review.undischarged,
        duration: review.duration,
    }
}

// --- The values the program reads -------------------------------------------------

fn refusal_value(refused: &Refused) -> PlyValue {
    let named = match refused.why {
        Why::Broken => "Broken",
        Why::Unbound => "Unbound",
        Why::Trouble => "Trouble",
    };
    ctor(
        PAYLOAD,
        named,
        vec![record(vec![
            ("diags", diags_value(&refused.diagnostics)),
            ("places", places_value(&refused.sources)),
        ])],
    )
}

fn at_value(at: &At) -> PlyValue {
    record(vec![
        ("module", PlyValue::Int(i64::from(at.module))),
        ("start", PlyValue::Int(i64::from(at.start))),
        ("end", PlyValue::Int(i64::from(at.end))),
    ])
}

fn kind_value(kind: &Kind) -> PlyValue {
    match kind {
        Kind::Ensures(index) => ctor(PAYLOAD, "Ensures", vec![count(*index)]),
        Kind::Law(label) => ctor(
            PAYLOAD,
            "Law",
            vec![option(label.as_deref().map(PlyValue::str))],
        ),
    }
}

fn frame_value(frame: &Frame) -> PlyValue {
    match frame {
        Frame::Pure => ctor(PAYLOAD, "Pure", Vec::new()),
        Frame::Writes(writes) => {
            let named: Vec<String> = writes
                .iter()
                .map(|(effect, resource)| format!("{effect}[{resource}]"))
                .collect();
            ctor(
                PAYLOAD,
                "Writes",
                vec![strings(named.iter().map(String::as_str))],
            )
        }
    }
}

fn claim_value(claim: &Claim) -> PlyValue {
    record(vec![
        ("key", PlyValue::str(&claim.key)),
        ("owner", PlyValue::str(&claim.owner)),
        ("kind", kind_value(&claim.kind)),
        ("guarded", PlyValue::Bool(claim.guarded)),
        ("frame", frame_value(&claim.frame)),
        ("at", at_value(&claim.at)),
        (
            "unperformed",
            strings(claim.unperformed.iter().map(String::as_str)),
        ),
    ])
}

fn roots_value(roots: &[u64]) -> PlyValue {
    PlyValue::list(roots.iter().map(|&root| tally(root)).collect())
}

fn plan_value(plan: &ProvePlan) -> PlyValue {
    record(vec![
        ("cases", tally(u64::from(plan.cases))),
        ("roots", roots_value(&plan.roots)),
        ("prove_budget", tally(u64::from(plan.prove_budget))),
        ("shrink_budget", tally(u64::from(plan.shrink_budget))),
        ("step_budget", PlyValue::Int(plan.step_budget)),
        (
            "sim",
            record(vec![
                ("mode", PlyValue::str(plan.sim.mode.as_str())),
                ("roots", roots_value(&plan.sim.roots)),
                ("budget", tally(u64::from(plan.sim.budget))),
                ("steps", tally(u64::from(plan.sim.steps))),
            ]),
        ),
    ])
}

fn collection_value(collection: Collection) -> PlyValue {
    record(vec![
        ("places", places_value(&collection.sources)),
        ("warnings", diags_value(&collection.warnings)),
        (
            "claims",
            PlyValue::list(collection.claims.iter().map(claim_value).collect()),
        ),
        ("specified", count(collection.specified)),
        ("plan", plan_value(&collection.plan)),
    ])
}

fn tier_value(tier: Tier) -> PlyValue {
    let named = match tier {
        Tier::Proved => "Proved",
        Tier::Property => "Property",
        Tier::Example => "Example",
    };
    ctor(PAYLOAD, named, Vec::new())
}

fn bindings_value(bindings: &[ply_prove::Binding]) -> PlyValue {
    PlyValue::list(
        bindings
            .iter()
            .map(|b| {
                record(vec![
                    ("name", PlyValue::str(b.name.as_str())),
                    ("ty", PlyValue::str(b.ty.to_string())),
                    ("rendered", PlyValue::str(&b.rendered)),
                ])
            })
            .collect(),
    )
}

/// The prover's own account of a proof, placed under `rules` as it wrote it: the program adds keys
/// around this document rather than deriving a second spelling of it.
fn rules_value(rules: &[ply_prove::Rule]) -> PlyValue {
    crate::payload::json(&serde_json::to_value(rules).unwrap_or(serde_json::Value::Null))
}

fn evidence_value(evidence: &Evidence) -> PlyValue {
    match evidence {
        Evidence::Proof(c) => ctor(
            PAYLOAD,
            "Proof",
            vec![record(vec![
                ("rules", rules_value(&c.rules)),
                ("steps", tally(u64::from(c.steps))),
                ("guard_satisfiable", PlyValue::Bool(c.guard_satisfiable)),
                (
                    "sorts",
                    strings(c.sorts.iter().map(ply_span::Symbol::as_str)),
                ),
            ])],
        ),
        Evidence::Cases(c) => ctor(
            PAYLOAD,
            "Sampled",
            vec![record(vec![
                ("generated", tally(u64::from(c.generated))),
                ("kept", tally(u64::from(c.kept))),
                ("rejected", tally(u64::from(c.rejected))),
                ("roots", roots_value(&c.roots)),
                (
                    "instantiations",
                    PlyValue::list(
                        c.instantiations
                            .iter()
                            .map(|(var, ty)| {
                                record(vec![
                                    ("var", PlyValue::str(var.as_str())),
                                    ("ty", PlyValue::str(ty.to_string())),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])],
        ),
    }
}

fn gap_value(gap: &Gap) -> PlyValue {
    match gap {
        Gap::UnhandledEffect(footprint) => ctor(
            PAYLOAD,
            "UnhandledEffect",
            vec![option(
                (!footprint.is_empty()).then(|| PlyValue::str(footprint.to_string())),
            )],
        ),
        Gap::Ungeneratable { param, ty } => ctor(
            PAYLOAD,
            "Ungeneratable",
            vec![record(vec![
                ("param", PlyValue::str(param.as_str())),
                ("ty", PlyValue::str(ty.to_string())),
            ])],
        ),
        Gap::Raised {
            bindings,
            diagnostic,
        } => ctor(
            PAYLOAD,
            "Raised",
            vec![record(vec![
                ("message", PlyValue::str(&diagnostic.message)),
                ("bindings", bindings_value(bindings)),
            ])],
        ),
        Gap::GuardNotSampled { generated, witness } => ctor(
            PAYLOAD,
            "GuardNotSampled",
            vec![record(vec![
                ("generated", tally(u64::from(*generated))),
                ("witness", bindings_value(witness)),
            ])],
        ),
        Gap::ReachesHost(footprint) => ctor(
            PAYLOAD,
            "ReachesHost",
            vec![PlyValue::str(footprint.to_string())],
        ),
    }
}

fn vacuity_value(vacuity: &Vacuity) -> PlyValue {
    record(vec![
        ("guard", at_value(&At::of(vacuity.guard))),
        (
            "why",
            match vacuity.kind {
                VacuityKind::ProvedUnsatisfiable => ctor(PAYLOAD, "Unsatisfiable", Vec::new()),
                VacuityKind::NoCaseKept { generated } => {
                    ctor(PAYLOAD, "NoCaseKept", vec![tally(u64::from(generated))])
                }
            },
        ),
    ])
}

fn outcome_value(discharge: &Discharge) -> PlyValue {
    match discharge {
        Discharge::Held(evidence) => ctor(
            PAYLOAD,
            "Held",
            vec![record(vec![
                ("tier", tier_value(evidence.tier())),
                ("evidence", evidence_value(evidence)),
            ])],
        ),
        Discharge::Refuted(cx) => ctor(
            PAYLOAD,
            "Refuted",
            vec![record(vec![
                ("bindings", bindings_value(&cx.bindings)),
                ("original", bindings_value(&cx.original)),
                ("shrinks", tally(u64::from(cx.shrinks))),
                ("root", tally(cx.root)),
                ("case", tally(u64::from(cx.case))),
                (
                    "seed",
                    option(cx.sim_seed.as_ref().map(|s| PlyValue::str(s.to_string()))),
                ),
            ])],
        ),
        Discharge::Vacuous(vacuity) => ctor(PAYLOAD, "Vacuous", vec![vacuity_value(vacuity)]),
        Discharge::Unattempted(gap) => ctor(PAYLOAD, "Unattempted", vec![gap_value(gap)]),
    }
}

fn reason_value(reason: Reason) -> PlyValue {
    let named = match reason {
        Reason::New => "Fresh",
        Reason::Proved => "FromProof",
        Reason::Sampled => "FromSample",
        Reason::Uncached => "Uncached",
        Reason::Refused => "CacheRefused",
    };
    ctor(PAYLOAD, named, Vec::new())
}

fn coverage_value(coverage: &ply_prove::Coverage) -> PlyValue {
    record(vec![
        ("definitions", count(coverage.definitions)),
        ("covered", count(coverage.covered)),
        (
            "uncovered",
            strings(coverage.uncovered.iter().map(ply_span::Symbol::as_str)),
        ),
        (
            "by_tier",
            PlyValue::list(
                coverage
                    .by_tier
                    .iter()
                    .map(|(tier, n)| {
                        record(vec![("tier", tier_value(*tier)), ("count", count(*n))])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn verdicts_value(verdicts: &Verdicts) -> PlyValue {
    record(vec![
        (
            "outcomes",
            PlyValue::list(verdicts.outcomes.iter().map(outcome_value).collect()),
        ),
        (
            "reasons",
            PlyValue::list(
                verdicts
                    .reasons
                    .iter()
                    .map(|&reason| reason_value(reason))
                    .collect(),
            ),
        ),
        ("coverage", coverage_value(&verdicts.coverage)),
        ("cached", count(verdicts.cached)),
        ("duration_ms", millis(verdicts.duration)),
        ("warnings", diags_value(&verdicts.warnings)),
    ])
}

fn moved_value(moved: Moved) -> PlyValue {
    let named = match moved {
        Moved::Unchanged => "Unchanged",
        Moved::Changed => "Changed",
        Moved::Never => "Never",
    };
    ctor(PAYLOAD, named, Vec::new())
}

fn changes_value(changes: &Changes) -> PlyValue {
    record(vec![
        ("reviewed", count(changes.reviewed)),
        (
            "changed",
            PlyValue::list(
                changes
                    .changed
                    .iter()
                    .map(|entry| {
                        record(vec![
                            ("name", PlyValue::str(&entry.name)),
                            ("implementation", moved_value(entry.implementation)),
                            ("spec", moved_value(entry.spec)),
                            (
                                "obligations",
                                PlyValue::list(
                                    entry.obligations.iter().map(|&i| count(i)).collect(),
                                ),
                            ),
                            ("holding", count(entry.holding)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("broken", count(changes.broken)),
        ("undischarged", count(changes.undischarged)),
        ("duration_ms", millis(changes.duration)),
    ])
}

fn accepted_value(accepted: &Accepted) -> PlyValue {
    record(vec![
        ("definitions", count(accepted.definitions)),
        ("stored", PlyValue::Bool(accepted.stored)),
        ("warnings", diags_value(&accepted.warnings)),
    ])
}

fn tally(n: u64) -> PlyValue {
    PlyValue::Int(n as i64)
}

/// Milliseconds to three places, which is what both reports publish as `duration_ms`.
fn millis(d: std::time::Duration) -> PlyValue {
    let ms = (d.as_secs_f64() * 1_000_000.0).round() / 1000.0;
    ply_eval::Decimal::from_f64_retain(ms)
        .map(PlyValue::Decimal)
        .unwrap_or(PlyValue::Decimal(ply_eval::Decimal::ZERO))
}

// --- Small things -------------------------------------------------------------

#[cold]
fn unopened(root: &std::path::Path, e: &impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("could not open the cache under `{}`: {e:#}", root.display()),
    )
    .note("check the directory's permissions")
}

#[cold]
fn unaccepted(e: &impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(codes::CACHE_UNREADABLE, format!("{e:#}"))
        .note("nothing was accepted; the baseline is unchanged")
}

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

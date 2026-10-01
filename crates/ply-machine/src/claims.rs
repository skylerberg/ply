//! What `ply prove` and `ply review` load, discharge, review and accept, as the program in
//! `crates/ply-cli/ply` performs it.
//!
//! The store and the prover stay here: discharging a claim enters compiled bodies, and an entry does
//! not nest on the thread the `ply` program itself runs on. The obligations, the types they are
//! written over and the search each one goes to are the program's (`proof.world`), handed over with
//! the front end; which claims are asked for, the keys their evidence is read and filed under, what
//! the review, the coverage and the baseline come to, every line and key of both reports and the
//! code each run exits with are the program's too, in `crates/ply-cli/ply/claims.ply`, `prove.ply`
//! and `review.ply`.

use crate::config::Configuration;
use crate::engine::{Judgement, Mode};
use crate::hosts::{Hosts, Lent};
use crate::load::{LoadError, Loaded};
use crate::payload::{count, ctor, diags_value, option, places_value, record, strings};
use crate::support::{build_pool, enter_constant, prover_backend};
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_eval::{DefHash, Diagnostic, SourceMap, Span, Symbol, Value as PlyValue, Value, codes};
use ply_prove::concurrency::ValueDomain;
use ply_prove::{
    Certificate, Discharge, Evidence, Fault, Gap, Obligation, ProvePlan, Rule, Tier, Vacuity,
    VacuityKind, World,
};
use ply_store::ReviewRecord;
use ply_store::Store;
use ply_test::obligation::{from_cached, to_cached};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

/// The effect `crates/ply-cli/ply/claims.ply` declares. It is lent to the two entries that read
/// obligations and nowhere else.
const EFFECT: &str = "prover";

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
pub const MARSHALLED: &[(&str, &str, &[&str])] = &[
    ("proof.obligation", "Evidence", &["Proof", "Sampled"]),
    (
        "proof.obligation",
        "Outcome",
        &["Held", "Refuted", "Vacuous", "Unattempted", "Defect"],
    ),
    (
        "proof.obligation",
        "Judged",
        &["JHeld", "JFailed", "JRejected", "JRaised", "JFaulted"],
    ),
    (
        "proof.rules",
        "Rule",
        &[
            "GroundEvaluation",
            "ExhaustiveEnumeration",
            "LinearArithmetic",
            "Propositional",
            "CaseSplit",
            "Congruence",
            "Injectivity",
            "Unfold",
            "Induction",
            "ExhaustiveInterleaving",
        ],
    ),
    ("proof.obligation", "Gap", &["Raised"]),
    (
        "proof.obligation",
        "Tier",
        &["Proved", "Property", "Example"],
    ),
    (
        "proof.obligation",
        "Vacuity",
        &["Unsatisfiable", "NoCaseKept"],
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
    ctor(home, name, args)
}

const OPERATIONS: [(&str, &str); 10] = [
    ("configure", "ply_machine::claims::configure"),
    ("collected", "ply_machine::claims::collected"),
    ("outcomes", "ply_machine::claims::outcomes"),
    ("prepared", "ply_machine::claims::prepared"),
    ("cached", "ply_machine::claims::cached"),
    ("judged", "ply_machine::claims::judged"),
    ("searched", "ply_machine::claims::searched"),
    ("record", "ply_machine::claims::record"),
    ("baselines", "ply_machine::claims::baselines"),
    ("accepted", "ply_machine::claims::accepted"),
];

/// A compiled body honours its call bound on the native stack, where unoptimised frames run to
/// kilobytes. Reserved, not committed.
const CLAIMS_STACK: usize = 256 << 20;

/// What the machine is asked to work with, which is every flag that is not about the report.
pub struct Job {
    pub path: PathBuf,
    /// The front end the CLI ran. A run without one is refused rather than loading again: `ply
    /// prove` and `ply review` start one and hand its answer over.
    pub front: Option<crate::driver::HandedFront>,
    /// The program as the prover reads it, and the obligations it owes, as the program built them.
    pub world: World,
    pub obligations: Vec<Obligation>,
    pub use_cache: bool,
    pub jobs: Option<u32>,
    pub plan: ProvePlan,
    /// `None` for a command that binds nothing at all, which is every `ply review`.
    pub binding: Option<Binding>,
}

/// What a `law/host` is discharged against, and what a hermetic run refuses to reach.
pub struct Binding {
    pub host: bool,
    pub tls: crate::options::TlsOptions,
    pub fs: Vec<ply_host::fs::RootSpec>,
    pub config: crate::config::ConfigOptions,
    pub trace: crate::trace::TraceOptions,
}

/// The operations, for a program that declares `prover` in `module`: a value this side builds of a
/// type that module declares is named as that program names it.
pub fn lent(module: &str) -> Vec<Lent> {
    let site: Arc<dyn HostHandler> = Arc::new(Site {
        job: Mutex::new(None),
        machine: Mutex::new(None),
        claims: Mutex::new(0),
        module: module.to_string(),
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
    /// How many claims the collection held, so a re-run can refuse an index that names none
    /// before reaching the thread.
    claims: Mutex<usize>,
    /// Where the lent program declares `prover`, and so the `Refusal` it matches.
    module: String,
}

impl HostHandler for Site {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match (req.op.op.as_str(), req.args) {
            ("configure", [options, front, world]) => {
                let mut job = job_of(options, span)?;
                job.front = Some(crate::driver::handed_front_of(front, span)?);
                (job.world, job.obligations) =
                    World::decode(ply_eval::decode::At::new("the world", world))
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
            ("outcomes", [keys]) => self.outcomes(&texts_of(keys, "the keys to look up", span)?)?,
            ("prepared", _) => self.prepared()?,
            ("cached", [keys]) => self.cached(&texts_of(keys, "the keys to read", span)?)?,
            ("judged", [batches]) => self.judged(batches_of(batches, span)?)?,
            ("searched", [claim, points, domain]) => self.searched(
                usize::try_from(claim.as_int(span, "the claim's place")?).unwrap_or(usize::MAX),
                points_of(points, span)?,
                domain_of(domain, span)?,
            )?,
            ("record", [entries]) => self.record(filed_of(entries, span)?)?,
            ("baselines", [names]) => self.baselines(names_of(names, span)?)?,
            ("accepted", [records]) => self.accepted(records_of(records, span)?)?,
            (other, _) => return Err(unasked(other, span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

/// A list of text the program sent.
fn texts_of(value: &PlyValue, what: &str, span: Span) -> Result<Vec<String>, Diagnostic> {
    let mut out = Vec::new();
    for item in value.as_list(span, what)? {
        out.push(item.as_str(span, what)?.to_string());
    }
    Ok(out)
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
        let (mode, _) = case_of(field_of(batch, "mode", span)?, "a mode", span)?;
        out.push(Batch {
            claim: usize::try_from(claim).unwrap_or(usize::MAX),
            points: points_of(field_of(batch, "points", span)?, span)?,
            mode: match mode {
                "MWhole" => Mode::Whole,
                "MWitness" => Mode::Witness,
                "MDomain" => Mode::Domain,
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
            values.push(crate::payload::value_plain(v, span)?);
        }
        out.push(values);
    }
    Ok(out)
}

/// A `proof.property.Searched`: what a law over interleavings was searched at.
fn domain_of(value: &PlyValue, span: Span) -> Result<ValueDomain, Diagnostic> {
    use crate::payload::field_of;
    let (name, args) = case_of(value, "a searched domain", span)?;
    let fields = args
        .first()
        .ok_or_else(|| malformed("a searched domain is missing its fields", span))?;
    Ok(match name {
        "SEnumerated" => ValueDomain::Enumerated {
            domain: Symbol::new(field_of(fields, "domain", span)?.as_str(span, "a domain")?),
            points: narrow(field_of(fields, "points", span)?, "points", span)?,
            kept: narrow(field_of(fields, "kept", span)?, "kept", span)?,
        },
        "SSampled" => ValueDomain::Sampled {
            generated: narrow(field_of(fields, "generated", span)?, "generated", span)?,
            kept: narrow(field_of(fields, "kept", span)?, "kept", span)?,
            rejected: narrow(field_of(fields, "rejected", span)?, "rejected", span)?,
            instantiations: instantiations_of(field_of(fields, "instantiations", span)?, span)?,
        },
        other => return Err(malformed(&format!("`{other}` is no searched domain"), span)),
    })
}

fn instantiations_of(value: &PlyValue, span: Span) -> Result<Vec<(Symbol, String)>, Diagnostic> {
    use crate::payload::field_of;
    value
        .as_list(span, "instantiations")?
        .iter()
        .map(|i| {
            Ok((
                Symbol::new(field_of(i, "var", span)?.as_str(span, "a variable")?),
                field_of(i, "ty", span)?.as_str(span, "a type")?.to_string(),
            ))
        })
        .collect()
}

/// A `proof.obligation.Evidence`.
fn evidence_of(value: &PlyValue, span: Span) -> Result<Evidence, Diagnostic> {
    use crate::payload::field_of;
    let (name, args) = case_of(value, "evidence", span)?;
    let fields = args
        .first()
        .ok_or_else(|| malformed("evidence is missing its fields", span))?;
    Ok(match name {
        "Proof" => Evidence::Proof(certificate_of(fields, span)?),
        "Sampled" => Evidence::Cases(ply_prove::CaseReport {
            generated: narrow(field_of(fields, "generated", span)?, "generated", span)?,
            kept: narrow(field_of(fields, "kept", span)?, "kept", span)?,
            rejected: narrow(field_of(fields, "rejected", span)?, "rejected", span)?,
            roots: field_of(fields, "roots", span)?
                .as_list(span, "roots")?
                .iter()
                .map(|r| narrow(r, "a root", span))
                .collect::<Result<_, _>>()?,
            instantiations: instantiations_of(field_of(fields, "instantiations", span)?, span)?,
        }),
        other => return Err(malformed(&format!("`{other}` is no evidence"), span)),
    })
}

/// The evidence the program decided to file, each under the key it chose.
fn filed_of(value: &PlyValue, span: Span) -> Result<Vec<(DefHash, Evidence)>, Diagnostic> {
    use crate::payload::field_of;
    let mut out = Vec::new();
    for entry in value.as_list(span, "the evidence to file")? {
        let key = key_of(field_of(entry, "key", span)?.as_str(span, "a key")?, span)?;
        out.push((key, evidence_of(field_of(entry, "evidence", span)?, span)?));
    }
    Ok(out)
}

/// A `proof.obligation.Certificate`.
fn certificate_of(value: &PlyValue, span: Span) -> Result<Certificate, Diagnostic> {
    use crate::payload::field_of;
    Ok(Certificate {
        rules: field_of(value, "rules", span)?
            .as_list(span, "a proof's rules")?
            .iter()
            .map(|rule| rule_of(rule, span))
            .collect::<Result<_, _>>()?,
        steps: narrow(field_of(value, "steps", span)?, "a proof's steps", span)?,
        guard_satisfiable: field_of(value, "guard_satisfiable", span)?
            .as_bool(span, "whether the guard is satisfiable")?,
        sorts: field_of(value, "sorts", span)?
            .as_list(span, "a proof's sorts")?
            .iter()
            .map(|sort| Ok(Symbol::new(sort.as_str(span, "a sort's name")?)))
            .collect::<Result<_, Diagnostic>>()?,
    })
}

/// A `proof.rules.Rule`.
fn rule_of(value: &PlyValue, span: Span) -> Result<Rule, Diagnostic> {
    use crate::payload::field_of;
    let (name, args) = case_of(value, "a rule", span)?;
    let fields = || {
        args.first()
            .ok_or_else(|| malformed(&format!("`{name}` is missing its fields"), span))
    };
    let field = |key: &str| field_of(fields()?, key, span);
    let text = |key: &str| -> Result<Symbol, Diagnostic> {
        Ok(Symbol::new(field(key)?.as_str(span, key)?))
    };
    Ok(match name {
        "GroundEvaluation" => Rule::GroundEvaluation,
        "ExhaustiveEnumeration" => Rule::ExhaustiveEnumeration {
            domain: text("domain")?,
            points: narrow(field("points")?, "points", span)?,
        },
        "LinearArithmetic" => Rule::LinearArithmetic,
        "Propositional" => Rule::Propositional,
        "CaseSplit" => Rule::CaseSplit {
            ty: text("ty")?,
            arms: narrow(field("arms")?, "arms", span)?,
        },
        "Congruence" => Rule::Congruence,
        "Injectivity" => Rule::Injectivity,
        "Unfold" => Rule::Unfold {
            def: text("def")?,
            depth: narrow(field("depth")?, "depth", span)?,
        },
        "Induction" => Rule::Induction {
            binder: text("binder")?,
            def: text("def")?,
        },
        "ExhaustiveInterleaving" => Rule::ExhaustiveInterleaving {
            interleavings: narrow(fields()?, "interleavings", span)?,
        },
        other => return Err(malformed(&format!("`{other}` is no rule"), span)),
    })
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

/// A count the program sent, in the width the runtime holds it at.
fn narrow<T: TryFrom<i64>>(value: &PlyValue, what: &str, span: Span) -> Result<T, Diagnostic> {
    T::try_from(value.as_int(span, what)?)
        .map_err(|_| malformed(&format!("{what} is out of range"), span))
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

/// The baselines the program decided to record, each keyed by the definition's name.
fn records_of(value: &PlyValue, span: Span) -> Result<Vec<(Symbol, ReviewRecord)>, Diagnostic> {
    use crate::payload::field_of;
    let mut out = Vec::new();
    for entry in value.as_list(span, "the baselines to record")? {
        let name = field_of(entry, "name", span)?.as_str(span, "a definition's name")?;
        let record = field_of(entry, "record", span)?;
        let def_hash = key_of(
            field_of(record, "def_hash", span)?.as_str(span, "a definition's hash")?,
            span,
        )?;
        let mut specs = Vec::new();
        for spec in field_of(record, "specs", span)?.as_list(span, "a definition's spec")? {
            specs.push(key_of(spec.as_str(span, "a spec's hash")?, span)?);
        }
        out.push((Symbol::new(name), ReviewRecord::new(def_hash, specs)));
    }
    Ok(out)
}

/// A hash the program handed over, as the store keys one.
fn key_of(hex: &str, span: Span) -> Result<DefHash, Diagnostic> {
    DefHash::from_hex(hex).ok_or_else(|| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("`{hex}` is not a hash the store could be read or written under"),
        )
        .primary(span, "the program handed this over")
    })
}

/// Definitions by program-wide name, as the program handed them over.
fn names_of(value: &PlyValue, span: Span) -> Result<Vec<Symbol>, Diagnostic> {
    value
        .as_list(span, "the definitions to read baselines for")?
        .iter()
        .map(|item| Ok(Symbol::new(item.as_str(span, "a definition's name")?)))
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
            Step::Collected(answer) => {
                if let Ok(collection) = &*answer {
                    *self.claims.lock().unwrap_or_else(|e| e.into_inner()) = collection.obligations;
                }
                Ok(self.answered((*answer).map(collection_value)))
            }
            _ => Err(out_of_step("collected")),
        }
    }

    /// The store's answer under each key, as a report prints one: `passed`, `failed`, or nothing.
    fn outcomes(&self, keys: &[String]) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("outcomes"))?;
        machine.ask(Go::Outcomes(keys.to_vec()))?;
        match machine.step()? {
            Step::Outcomes(answers) => Ok(PlyValue::list(
                answers
                    .iter()
                    .map(|answer| crate::payload::option(answer.as_deref().map(PlyValue::str)))
                    .collect(),
            )),
            _ => Err(out_of_step("outcomes")),
        }
    }

    /// Binds the hosts and builds the prover a discharge runs against, once, and answers what
    /// opening the hosts had to say.
    fn prepared(&self) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("prepared"))?;
        machine.ask(Go::Prepare)?;
        match machine.step()? {
            Step::Prepared(answer) => Ok(self.answered((*answer).map(|w| diags_value(&w)))),
            _ => Err(out_of_step("prepared")),
        }
    }

    /// The evidence the store holds under each key, where it can be read.
    fn cached(&self, keys: &[String]) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("cached"))?;
        machine.ask(Go::Cached(keys.to_vec()))?;
        match machine.step()? {
            Step::Cached(read) => Ok(PlyValue::list(
                read.iter()
                    .map(|evidence| crate::payload::option(evidence.as_ref().map(evidence_value)))
                    .collect(),
            )),
            _ => Err(out_of_step("cached")),
        }
    }

    /// Each batch's points judged, the batches at once.
    fn judged(&self, batches: Vec<Batch>) -> Result<PlyValue, Diagnostic> {
        let claims = *self.claims.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = batches.iter().find(|b| b.claim >= claims) {
            return Err(no_such_claim(batch.claim, claims));
        }
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("judged"))?;
        machine.ask(Go::Judge(batches))?;
        match machine.step()? {
            Step::Judged(judged) => Ok(PlyValue::list(
                judged
                    .iter()
                    .map(|batch| PlyValue::list(batch.iter().map(judged_value).collect()))
                    .collect(),
            )),
            _ => Err(out_of_step("judged")),
        }
    }

    /// A law over interleavings, searched at the points its guard kept.
    fn searched(
        &self,
        claim: usize,
        points: Vec<Vec<ply_eval::Plain>>,
        domain: ValueDomain,
    ) -> Result<PlyValue, Diagnostic> {
        let claims = *self.claims.lock().unwrap_or_else(|e| e.into_inner());
        if claim >= claims {
            return Err(no_such_claim(claim, claims));
        }
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("searched"))?;
        machine.ask(Go::Search {
            claim,
            points,
            domain,
        })?;
        match machine.step()? {
            Step::Searched(discharge) => Ok(outcome_value(&discharge)),
            _ => Err(out_of_step("searched")),
        }
    }

    /// Files the evidence the program chose, each under the key it chose.
    fn record(&self, entries: Vec<(DefHash, Evidence)>) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("record"))?;
        machine.ask(Go::Record(entries))?;
        match machine.step()? {
            Step::Recorded(warnings) => Ok(diags_value(&warnings)),
            _ => Err(out_of_step("record")),
        }
    }

    /// The baseline a reader accepted for each of these definitions, where there is one.
    fn baselines(&self, names: Vec<Symbol>) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("baselines"))?;
        machine.ask(Go::Baselines(names))?;
        match machine.step()? {
            Step::Baselines(baselines) => Ok(PlyValue::list(
                baselines
                    .iter()
                    .map(|(name, baseline)| record_value(name, baseline))
                    .collect(),
            )),
            _ => Err(out_of_step("baselines")),
        }
    }

    fn accepted(&self, records: Vec<(Symbol, ReviewRecord)>) -> Result<PlyValue, Diagnostic> {
        let mut held = self.held();
        let step = {
            let machine = held.as_ref().ok_or_else(|| unstarted("accepted"))?;
            machine.ask(Go::Accept(records))?;
            machine.step()?
        };
        // Nothing follows an acceptance: the thread it happened on is joined here.
        held.take();
        match step {
            Step::Accepted(accepted) => Ok(accepted_value(&accepted)),
            _ => Err(out_of_step("accepted")),
        }
    }
}

/// `Ok(v)` or `Err(Refusal)`, as the program reads an operation's answer.
impl Site {
    fn answered(&self, answer: Result<PlyValue, Refused>) -> PlyValue {
        match answer {
            Ok(value) => PlyValue::ctor("Ok", vec![value]),
            Err(refused) => PlyValue::ctor("Err", vec![refusal_value(&refused, &self.module)]),
        }
    }
}

// --- The thread the work lives on ---------------------------------------------

/// What the program asks the machine for next.
enum Go {
    /// What the store holds under these keys. The program computes them — a plan key is part of
    /// the obligation's own encoding — so no row could carry the answers.
    Outcomes(Vec<String>),
    /// Bind the hosts and build the prover a discharge runs against.
    Prepare,
    /// The evidence filed under these keys.
    Cached(Vec<String>),
    Judge(Vec<Batch>),
    Search {
        claim: usize,
        points: Vec<Vec<ply_eval::Plain>>,
        domain: ValueDomain,
    },
    /// File this evidence, each under the key the program chose for it.
    Record(Vec<(DefHash, Evidence)>),
    /// The baseline a reader accepted for each of these definitions.
    Baselines(Vec<Symbol>),
    Accept(Vec<(Symbol, ReviewRecord)>),
}

enum Step {
    Collected(Box<Result<Collection, Refused>>),
    /// The store's answer under each key asked about: `passed`, `failed`, or nothing.
    Outcomes(Vec<Option<String>>),
    Prepared(Box<Result<Vec<Diagnostic>, Refused>>),
    Cached(Vec<Option<Evidence>>),
    Judged(Vec<Vec<Judgement>>),
    Searched(Box<Discharge>),
    Recorded(Vec<Diagnostic>),
    Baselines(Vec<(String, ReviewRecord)>),
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
    let loaded = match load(&job) {
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

    let obligations: &[Obligation] = &job.obligations;

    let _ = told.send(Step::Collected(Box::new(Ok(Collection {
        sources: loaded.sources.clone(),
        warnings: std::mem::take(&mut warnings),
        obligations: obligations.len(),
        plan: job.plan.clone(),
        host: job.binding.as_ref().is_some_and(|b| b.host),
    }))));

    // Built by the first step that runs an obligation, and kept: every batch a discharge judges
    // is judged by the same prover over the same hosts.
    let mut prepared: Option<Result<Prepared, Refused>> = None;
    let mut pool_warnings = Vec::new();
    let (pool, _workers) = build_pool(job.jobs, &mut pool_warnings);
    loop {
        match asked.recv() {
            Ok(Go::Outcomes(keys)) => {
                // What the *obligation* cache holds under each key, as a word: the program applies
                // the rule (a proof only under the bare key, a sample only under its plan's), and
                // only the runtime can decode an entry.
                let answers: Vec<Option<String>> = keys
                    .iter()
                    .map(|key| {
                        Some(match read_evidence(&store, key)? {
                            Ok(Evidence::Proof(_)) => "proof".to_string(),
                            Ok(Evidence::Cases(_)) => "sample".to_string(),
                            Err(_) => "other".to_string(),
                        })
                    })
                    .collect();
                let _ = told.send(Step::Outcomes(answers));
            }
            Ok(Go::Prepare) => {
                if prepared.is_none() {
                    prepared = Some(prepare(&job, &loaded));
                }
                let answer = match prepared.as_ref() {
                    Some(Ok(ready)) => {
                        let mut warnings = ready.warnings.clone();
                        warnings.append(&mut pool_warnings);
                        Ok(warnings)
                    }
                    Some(Err(refused)) => Err(refused.clone()),
                    None => return,
                };
                let _ = told.send(Step::Prepared(Box::new(answer)));
            }
            Ok(Go::Cached(keys)) => {
                let read = keys
                    .iter()
                    .map(|key| read_evidence(&store, key).and_then(Result::ok))
                    .collect();
                let _ = told.send(Step::Cached(read));
            }
            Ok(Go::Judge(batches)) => {
                let Some(Ok(ready)) = prepared.as_ref() else {
                    return;
                };
                let judge = || {
                    use rayon::prelude::*;
                    batches
                        .par_iter()
                        .map(|batch| {
                            judge_batch(&ready.prover, &obligations[batch.claim], &job.plan, batch)
                        })
                        .collect::<Vec<_>>()
                };
                let judged = match &pool {
                    Some(pool) => pool.install(judge),
                    None => judge(),
                };
                let _ = told.send(Step::Judged(judged));
            }
            Ok(Go::Search {
                claim,
                points,
                domain,
            }) => {
                let Some(Ok(ready)) = prepared.as_ref() else {
                    return;
                };
                let discharge = match values_of(&points) {
                    Ok(values) => {
                        ready
                            .prover
                            .searched(&obligations[claim], &job.plan, values, domain)
                    }
                    Err(fault) => Discharge::Faulted(fault),
                };
                let _ = told.send(Step::Searched(Box::new(discharge)));
            }
            Ok(Go::Record(entries)) => {
                for (key, evidence) in entries {
                    store.put_obligation(key, to_cached(&evidence));
                }
                let warnings = flushed(&mut store);
                let _ = told.send(Step::Recorded(warnings));
            }
            Ok(Go::Baselines(names)) => {
                let baselines = names
                    .iter()
                    .filter_map(|name| {
                        Some((
                            name.as_str().to_string(),
                            store.review_record(name)?.clone(),
                        ))
                    })
                    .collect();
                let _ = told.send(Step::Baselines(baselines));
            }
            Ok(Go::Accept(records)) => {
                let definitions = records.len();
                for (name, record) in records {
                    store.put_review_record(name, record);
                }
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
    crate::driver::load_over_front(&job.path, front)
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

// --- Discharging ---------------------------------------------------------------

/// The prover and the hosts a run discharges and re-runs points with, built when the first step
/// that needs them asks: a discharge of many claims and a re-run of one case are the same engine.
struct Prepared<'a> {
    /// Kept alive for the prover's lifetime, which is the run's.
    _hosts: Option<Hosts>,
    prover: crate::engine::Prover<'a>,
    /// What opening the hosts had to say, reported by the discharge that reads them.
    warnings: Vec<Diagnostic>,
}

fn prepare<'a>(job: &'a Job, loaded: &'a Loaded) -> Result<Prepared<'a>, Refused> {
    let unbound = |diagnostics: Vec<Diagnostic>| Refused {
        why: Why::Unbound,
        diagnostics,
        sources: loaded.sources.clone(),
    };
    let backend = prover_backend(loaded).map_err(|d| unbound(vec![d]))?;
    let constant = |name: &str| enter_constant(Some(backend), name);
    let mut warnings = Vec::new();
    let hosts = match &job.binding {
        None => None,
        Some(binding) => {
            let (configuration, opened) =
                Configuration::open(&loaded.check, binding.host, &binding.config, &constant)
                    .map_err(&unbound)?;
            warnings.extend(opened);
            Some(
                Hosts::open(
                    &loaded.check,
                    binding.host,
                    &binding.tls,
                    &binding.fs,
                    configuration,
                    &binding.trace,
                )
                .map_err(&unbound)?,
            )
        }
    };
    let hosting = hosts
        .as_ref()
        .filter(|_| job.binding.as_ref().is_some_and(|b| b.host))
        .map(|hosts| crate::engine::Hosting {
            binding: hosts.binding(),
            runtime: hosts.runtime_factory().map(|f| {
                Arc::new(f)
                    as Arc<dyn Fn() -> std::rc::Rc<dyn ply_eval::host::HostRuntime> + Sync + Send>
            }),
        });
    Ok(Prepared {
        _hosts: hosts,
        prover: crate::engine::prover(loaded, hosting, backend),
        warnings,
    })
}

/// The evidence filed under a key, read back: `None` for nothing there, an error for something this
/// side cannot read.
fn read_evidence(store: &Store, key: &str) -> Option<Result<Evidence, String>> {
    let entry = DefHash::from_hex(key).and_then(|hash| store.obligation(hash))?;
    Some(from_cached(&entry))
}

/// The values plain values name, on the thread that judges them.
fn values_of(points: &[Vec<ply_eval::Plain>]) -> Result<Vec<Vec<Value>>, Fault> {
    points
        .iter()
        .map(|point| {
            point
                .iter()
                .map(|plain| plain.clone().into_value())
                .collect::<Result<Vec<Value>, _>>()
        })
        .collect::<Result<_, _>>()
        .map_err(|why| Fault {
            bindings: Vec::new(),
            diagnostic: Box::new(
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!("a point the program drew is no runtime value: {why}"),
                )
                .note("the program draws only values of its claims' types; this is Ply's fault"),
            ),
        })
}

fn judge_batch(
    prover: &crate::engine::Prover<'_>,
    obligation: &Obligation,
    plan: &ProvePlan,
    batch: &Batch,
) -> Vec<Judgement> {
    match values_of(&batch.points) {
        Ok(values) => prover.judged(obligation, plan, &values, batch.mode),
        Err(fault) => vec![Judgement::Faulted(*fault.diagnostic)],
    }
}

#[derive(Clone)]
enum Why {
    Broken,
    Unbound,
    Trouble,
}

#[derive(Clone)]
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

/// What loading the run came to. The obligations, and the definitions and laws a run answers for,
/// are the program's; this counts the obligations, so a re-run can refuse an index that names none.
struct Collection {
    sources: SourceMap,
    warnings: Vec<Diagnostic>,
    obligations: usize,
    plan: ProvePlan,
    /// Whether a `law/host` has a host bound to run against.
    host: bool,
}

struct Accepted {
    definitions: usize,
    stored: bool,
    warnings: Vec<Diagnostic>,
}

// --- The values the program reads -------------------------------------------------

fn refusal_value(refused: &Refused, module: &str) -> PlyValue {
    let named = match refused.why {
        Why::Broken => "Broken",
        Why::Unbound => "Unbound",
        Why::Trouble => "Trouble",
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

fn at_value(at: &At) -> PlyValue {
    record(vec![
        ("module", PlyValue::Int(i64::from(at.module))),
        ("start", PlyValue::Int(i64::from(at.start))),
        ("end", PlyValue::Int(i64::from(at.end))),
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
                (
                    "path",
                    PlyValue::list(plan.sim.path.iter().map(|&c| tally(u64::from(c))).collect()),
                ),
            ]),
        ),
    ])
}

fn hashes_value(hashes: &[DefHash]) -> PlyValue {
    PlyValue::list(hashes.iter().map(|h| PlyValue::str(h.to_hex())).collect())
}

fn collection_value(collection: Collection) -> PlyValue {
    record(vec![
        ("places", places_value(&collection.sources)),
        ("warnings", diags_value(&collection.warnings)),
        ("plan", plan_value(&collection.plan)),
        ("host", PlyValue::Bool(collection.host)),
    ])
}

fn tier_value(tier: Tier) -> PlyValue {
    let named = match tier {
        Tier::Proved => "Proved",
        Tier::Property => "Property",
        Tier::Example => "Example",
    };
    case("Tier", named, Vec::new())
}

fn bindings_value(bindings: &[ply_prove::Binding]) -> PlyValue {
    PlyValue::list(
        bindings
            .iter()
            .map(|b| {
                record(vec![
                    ("name", PlyValue::str(b.name.as_str())),
                    ("ty", PlyValue::str(&b.ty)),
                    ("value", crate::payload::plain_value(&b.value)),
                ])
            })
            .collect(),
    )
}

/// The prover's own account of a proof, placed under `rules` as it wrote it: the program adds keys
/// around this document rather than deriving a second spelling of it.
fn rules_value(rules: &[Rule]) -> PlyValue {
    PlyValue::list(rules.iter().map(rule_value).collect())
}

/// A rule as `proof.rules` declares it.
fn rule_value(rule: &Rule) -> PlyValue {
    let fields = |fs: Vec<(&str, PlyValue)>| vec![record(fs)];
    match rule {
        Rule::GroundEvaluation => case("Rule", "GroundEvaluation", Vec::new()),
        Rule::ExhaustiveEnumeration { domain, points } => case(
            "Rule",
            "ExhaustiveEnumeration",
            fields(vec![
                ("domain", PlyValue::str(domain.as_str())),
                ("points", tally(*points)),
            ]),
        ),
        Rule::LinearArithmetic => case("Rule", "LinearArithmetic", Vec::new()),
        Rule::Propositional => case("Rule", "Propositional", Vec::new()),
        Rule::CaseSplit { ty, arms } => case(
            "Rule",
            "CaseSplit",
            fields(vec![
                ("ty", PlyValue::str(ty.as_str())),
                ("arms", tally(u64::from(*arms))),
            ]),
        ),
        Rule::Congruence => case("Rule", "Congruence", Vec::new()),
        Rule::Injectivity => case("Rule", "Injectivity", Vec::new()),
        Rule::Unfold { def, depth } => case(
            "Rule",
            "Unfold",
            fields(vec![
                ("def", PlyValue::str(def.as_str())),
                ("depth", tally(u64::from(*depth))),
            ]),
        ),
        Rule::Induction { binder, def } => case(
            "Rule",
            "Induction",
            fields(vec![
                ("binder", PlyValue::str(binder.as_str())),
                ("def", PlyValue::str(def.as_str())),
            ]),
        ),
        Rule::ExhaustiveInterleaving { interleavings } => case(
            "Rule",
            "ExhaustiveInterleaving",
            vec![tally(u64::from(*interleavings))],
        ),
    }
}

fn evidence_value(evidence: &Evidence) -> PlyValue {
    match evidence {
        Evidence::Proof(c) => case(
            "Evidence",
            "Proof",
            vec![record(vec![
                ("rules", rules_value(&c.rules)),
                ("steps", tally(u64::from(c.steps))),
                ("guard_satisfiable", PlyValue::Bool(c.guard_satisfiable)),
                (
                    "sorts",
                    strings(c.sorts.iter().map(ply_eval::Symbol::as_str)),
                ),
            ])],
        ),
        Evidence::Cases(c) => case(
            "Evidence",
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
                                    ("ty", PlyValue::str(ty)),
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
    let Gap::Raised {
        bindings,
        diagnostic,
        ..
    } = gap;
    case(
        "Gap",
        "Raised",
        vec![record(vec![
            ("message", PlyValue::str(&diagnostic.message)),
            ("values", shown_values(diagnostic)),
            ("bindings", bindings_value(bindings)),
        ])],
    )
}

fn fault_value(fault: &Fault) -> PlyValue {
    record(vec![
        ("code", PlyValue::str(fault.diagnostic.code)),
        ("message", PlyValue::str(&fault.diagnostic.message)),
        (
            "notes",
            strings(fault.diagnostic.notes.iter().map(String::as_str)),
        ),
        ("values", shown_values(&fault.diagnostic)),
        ("bindings", bindings_value(&fault.bindings)),
    ])
}

/// The values a diagnostic's text names, which `std.value.filled` puts in place.
fn shown_values(diagnostic: &Diagnostic) -> PlyValue {
    PlyValue::list(
        diagnostic
            .values
            .iter()
            .map(crate::payload::plain_value)
            .collect(),
    )
}

/// One judgement as `proof.property` reads it.
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
    }
}

fn vacuity_value(vacuity: &Vacuity) -> PlyValue {
    record(vec![
        ("guard", at_value(&At::of(vacuity.guard))),
        (
            "why",
            match vacuity.kind {
                VacuityKind::ProvedUnsatisfiable => case("Vacuity", "Unsatisfiable", Vec::new()),
                VacuityKind::NoCaseKept { generated } => {
                    case("Vacuity", "NoCaseKept", vec![tally(u64::from(generated))])
                }
            },
        ),
    ])
}

fn outcome_value(discharge: &Discharge) -> PlyValue {
    match discharge {
        Discharge::Held(evidence) => case(
            "Outcome",
            "Held",
            vec![record(vec![
                ("tier", tier_value(evidence.tier())),
                ("evidence", evidence_value(evidence)),
            ])],
        ),
        Discharge::Refuted(cx) => case(
            "Outcome",
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
        Discharge::Vacuous(vacuity) => case("Outcome", "Vacuous", vec![vacuity_value(vacuity)]),
        Discharge::Unattempted(gap) => case("Outcome", "Unattempted", vec![gap_value(gap)]),
        Discharge::Faulted(fault) => case("Outcome", "Defect", vec![fault_value(fault)]),
    }
}

/// A baseline as the program reads one: the definition's hash and its spec, keyed by its name.
fn record_value(name: &str, baseline: &ReviewRecord) -> PlyValue {
    record(vec![
        ("name", PlyValue::str(name)),
        (
            "record",
            record(vec![
                ("def_hash", PlyValue::str(baseline.def_hash.to_hex())),
                ("specs", hashes_value(&baseline.specs)),
            ]),
        ),
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
    use crate::payload::{field_of, opt_int_at, opt_str_at, str_list_at};
    let bool_at = |name: &str| field_of(v, name, span)?.as_bool(span, name);
    let str_at = |name: &str| {
        field_of(v, name, span)?
            .as_str(span, name)
            .map(str::to_string)
    };
    let sim = field_of(v, "sim", span)?;
    let prove = field_of(v, "prove", span)?;
    let prove_opts = crate::simulation::ProveOptions {
        prove_cases: opt_int_at(prove, "cases", span)?.map(|n| n as u32),
        prove_roots: opt_int_at(prove, "roots", span)?.map(|n| n as u32),
        prove_budget: opt_int_at(prove, "budget", span)?.map(|n| n as u32),
        shrink_budget: opt_int_at(prove, "shrink_budget", span)?.map(|n| n as u32),
        prove_steps: opt_int_at(prove, "steps", span)?,
    };
    let sim_opts = crate::simulation::sim_options_of(sim, span)?;
    let host = bool_at("host")?;
    let binding = if host {
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
        let config = field_of(v, "config", span)?;
        let trace = field_of(v, "trace", span)?;
        Some(Binding {
            host,
            tls: crate::options::TlsOptions {
                tls,
                trust: str_list_at(v, "trust", span)?
                    .into_iter()
                    .map(PathBuf::from)
                    .collect(),
            },
            fs,
            config: crate::config::ConfigOptions {
                set: str_list_at(config, "set", span)?,
                files: str_list_at(config, "files", span)?
                    .into_iter()
                    .map(PathBuf::from)
                    .collect(),
                schema: opt_str_at(config, "schema", span)?,
            },
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
        world: World::default(),
        obligations: Vec::new(),
        use_cache: !bool_at("no_cache")?,
        jobs: opt_int_at(v, "jobs", span)?.map(|n| n as u32),
        plan: crate::simulation::prove_plan(&prove_opts, &sim_opts),
        binding,
    })
}

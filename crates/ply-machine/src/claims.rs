//! What `ply prove` and `ply review` load, collect, discharge, review and accept, as the program
//! in `crates/ply-cli/ply` performs it.
//!
//! The front end, the store and the prover stay here: a front end is not a value a program can hold,
//! discharging a claim enters compiled bodies, and an entry does not nest on the thread the `ply`
//! program itself runs on. Which claims are asked for, the keys their evidence is read and filed
//! under, what the review, the coverage and the baseline come to, every line and key of both
//! reports and the code each run exits with are the program's, in `crates/ply-cli/ply/claims.ply`,
//! `prove.ply` and `review.ply`.

use crate::config::Configuration;
use crate::engine::Point;
use crate::hosts::{Hosts, Lent};
use crate::load::{LoadError, Loaded};
use crate::payload::{count, ctor, diags_value, option, places_value, record, strings};
use crate::support::{build_pool, enter_constant, prover_backend};
use ply_eval::Value as PlyValue;
use ply_eval::Value;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_prove::domain::Shape;
use ply_prove::property::{GenStream, TypeWorld, generate};
use ply_prove::shrink::Target;
use ply_prove::{
    Discharge, Evidence, Frame, Gap, Obligation, ObligationKind, ProvePlan, ProveReport, Tier,
    Vacuity, VacuityKind,
};
use ply_span::{Diagnostic, SourceMap, Span, Symbol, codes};
use ply_store::ReviewRecord;
use ply_store::Store;
use ply_test::obligation::{self, from_cached, to_cached};
use ply_ty::CheckOutput;
use ply_ty::DefHash;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

/// The effect `crates/ply-cli/ply/claims.ply` declares. It is lent to the two entries that read
/// obligations and nowhere else.
const EFFECT: &str = "prover";

/// Where each type this side marshals is declared, by the type's own name.
///
/// A constructor crosses the substrate boundary by its program-wide name -- `claims.Raised` is not
/// `proof.obligation.Raised` -- so building one says which module declares its type, and this is
/// the only place that says it, save `Refusal`: that is declared beside `prover`, so it is named by
/// the module the lent program declares `prover` in.
/// `every_marshalled_type_is_declared_where_this_side_says` holds every row to the program, because
/// a tag that names no declaration is a placeless `no arm of this match matched` the moment the
/// program matches the value.
pub const MARSHALLED: &[(&str, &str)] = &[
    ("proof.domain", "Ty"),
    ("proof.obligation", "Frame"),
    ("proof.obligation", "Evidence"),
    ("proof.obligation", "Outcome"),
    ("proof.obligation", "Point"),
    ("proof.obligation", "Gap"),
    ("proof.obligation", "Kind"),
    ("proof.obligation", "Tier"),
    ("proof.obligation", "Vacuity"),
];

/// The module that declares `ty`, which is where a value of it crosses by.
fn home(ty: &str) -> &'static str {
    MARSHALLED
        .iter()
        .find(|(_, name)| *name == ty)
        .unwrap_or_else(|| panic!("`{ty}` is not a type this side marshals"))
        .0
}

const OPERATIONS: [(&str, &str); 14] = [
    ("configure", "ply_machine::claims::configure"),
    ("collected", "ply_machine::claims::collected"),
    ("typed", "ply_machine::claims::typed"),
    ("outcomes", "ply_machine::claims::outcomes"),
    ("discharged", "ply_machine::claims::discharged"),
    ("record", "ply_machine::claims::record"),
    ("replay", "ply_machine::claims::replay"),
    ("shrink", "ply_machine::claims::shrink"),
    ("offers", "ply_machine::claims::offers"),
    ("would", "ply_machine::claims::would"),
    ("accept", "ply_machine::claims::accept"),
    ("settled", "ply_machine::claims::settled"),
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
    pub incremental: bool,
    pub use_cache: bool,
    /// Also discharge what the shipped modules declare.
    pub std: bool,
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
            ("configure", [options, front]) => {
                let mut job = job_of(options, span)?;
                job.front = Some(crate::driver::handed_front_of(front, span)?);
                // A configuration begins a run, whatever the last one was left doing: its machine
                // is dropped, which joins its thread, and its claims are no longer this run's.
                let previous = self.held().take();
                drop(previous);
                *self.claims.lock().unwrap_or_else(|e| e.into_inner()) = 0;
                *self.job.lock().unwrap_or_else(|e| e.into_inner()) = Some(job);
                PlyValue::Unit
            }
            ("collected", _) => self.collected()?,
            ("typed", _) => self.typed()?,
            ("discharged", [choice]) => self.discharged(choice_of(choice, span)?)?,
            ("record", [entries]) => self.record(filed_of(entries, span)?)?,
            ("outcomes", [keys]) => {
                let list = keys.as_list(span, "the keys to look up")?;
                let mut named = Vec::with_capacity(list.len());
                for item in list {
                    named.push(item.as_str(span, "a key")?.to_string());
                }
                self.outcomes(&named)?
            }
            ("shrink", [claim]) => self.shrink(
                usize::try_from(claim.as_int(span, "the claim's place")?).unwrap_or(usize::MAX),
            )?,
            ("offers", [i]) => self.offers(
                usize::try_from(i.as_int(span, "the value's place")?).unwrap_or(usize::MAX),
            )?,
            ("would", [i, position]) => self.would(
                usize::try_from(i.as_int(span, "the value's place")?).unwrap_or(usize::MAX),
                position.as_int(span, "the candidate's place")?,
            )?,
            ("accept", [i, position]) => self.take(
                usize::try_from(i.as_int(span, "the value's place")?).unwrap_or(usize::MAX),
                position.as_int(span, "the candidate's place")?,
            )?,
            ("settled", _) => self.settled()?,
            ("replay", [index, root, case]) => self.replay(
                usize::try_from(index.as_int(span, "the claim's place")?).unwrap_or(usize::MAX),
                u64::try_from(root.as_int(span, "the generator's root")?).unwrap_or(0),
                u32::try_from(case.as_int(span, "the case to draw")?).unwrap_or(u32::MAX),
            )?,
            ("baselines", _) => self.baselines()?,
            ("accepted", [records]) => self.accepted(records_of(records, span)?)?,
            (other, _) => return Err(unasked(other, span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

/// The decision, as the program sent it.
fn choice_of(value: &PlyValue, span: Span) -> Result<obligation::Choice, Diagnostic> {
    use crate::payload::field_of;
    let claims = indices(field_of(value, "claims", span)?, span)?;
    let mut domains = Vec::new();
    for entry in field_of(value, "domains", span)?.as_list(span, "the measured domains")? {
        let claim = field_of(entry, "claim", span)?.as_int(span, "a claim's place")? as usize;
        let shapes = field_of(entry, "shapes", span)?
            .as_list(span, "a binder's shape")?
            .iter()
            .map(|shape| shape_of(shape, span))
            .collect::<Result<Vec<Shape>, Diagnostic>>()?;
        let name = field_of(entry, "name", span)?
            .as_str(span, "a domain's name")?
            .to_string();
        // Keyed by the obligation's position in the run, which is how the discharge reads it back.
        // A domain for a claim this run does not report on is dropped rather than refused.
        if let Some(position) = claims.iter().position(|&c| c == claim) {
            domains.push((position, obligation::Domain { shapes, name }));
        }
    }
    Ok(obligation::Choice {
        claims,
        to_discharge: indices(field_of(value, "runs", span)?, span)?,
        read: filed_of(field_of(value, "read", span)?, span)?,
        domains,
    })
}

/// One binder's shape as `proof.domain` measured it: a builtin, a declared type's cases or a
/// record's fields, each node with the size the program decided.
pub fn shape_of(value: &PlyValue, span: Span) -> Result<Shape, Diagnostic> {
    use crate::payload::field_of;
    let size = |v: &PlyValue| -> Result<u64, Diagnostic> {
        Ok(u64::try_from(v.as_int(span, "a size")?).unwrap_or(0))
    };
    let PlyValue::Ctor { name, args } = value else {
        return Err(unshaped(span));
    };
    let arg = |i: usize| args.get(i).ok_or_else(|| unshaped(span));
    match name.as_str().rsplit('.').next().unwrap_or("") {
        "Scalar" => Ok(Shape::Scalar {
            name: arg(0)?.as_str(span, "a builtin's name")?.to_string(),
            size: size(arg(1)?)?,
        }),
        "Cases" => Ok(Shape::Cases {
            size: size(arg(0)?)?,
            cases: arg(1)?
                .as_list(span, "a type's cases")?
                .iter()
                .map(|case| {
                    Ok(ply_prove::domain::Case {
                        name: Symbol::new(field_of(case, "name", span)?.as_str(span, "a case")?),
                        size: size(field_of(case, "size", span)?)?,
                        fields: field_of(case, "fields", span)?
                            .as_list(span, "a case's fields")?
                            .iter()
                            .map(|field| shape_of(field, span))
                            .collect::<Result<_, Diagnostic>>()?,
                    })
                })
                .collect::<Result<_, Diagnostic>>()?,
        }),
        "Fields" => Ok(Shape::Fields {
            size: size(arg(0)?)?,
            fields: arg(1)?
                .as_list(span, "a record's fields")?
                .iter()
                .map(|field| {
                    Ok((
                        Symbol::new(field_of(field, "name", span)?.as_str(span, "a field")?),
                        shape_of(field_of(field, "shape", span)?, span)?,
                    ))
                })
                .collect::<Result<_, Diagnostic>>()?,
        }),
        _ => Err(unshaped(span)),
    }
}

#[cold]
fn unshaped(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a measured domain holds something that is not a shape",
    )
    .primary(span, "the program handed this domain over")
    .note("the program and the thread it drives are written together; this is Ply's fault")
}

/// Positions in the run's claims, each with the key the program named for it.
fn filed_of(value: &PlyValue, span: Span) -> Result<Vec<(usize, DefHash)>, Diagnostic> {
    use crate::payload::field_of;
    let mut out = Vec::new();
    for entry in value.as_list(span, "the keys evidence is filed under")? {
        let at = field_of(entry, "at", span)?.as_int(span, "a claim's place in the run")?;
        let key = key_of(field_of(entry, "key", span)?.as_str(span, "a key")?, span)?;
        out.push((usize::try_from(at).unwrap_or(usize::MAX), key));
    }
    Ok(out)
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

/// The tuple a counterexample's draw gives, regenerated: the values a walk needs are the ones the
/// generator produced, and a draw is reproducible from where it was made.
fn drawn(obligation: &Obligation, root: u64, case: u32, world: &TypeWorld) -> Option<Vec<Value>> {
    let mut stream = GenStream::new(root, obligation.key);
    obligation
        .generated()
        .iter()
        .map(|binder| generate(&binder.ty, world, &mut stream, case).ok())
        .collect()
}

/// What a claim's discharge left to walk: a refutation shrinks from the values its draw gives, a
/// raise from the values it reported. Everything else has nothing to walk — a race is an
/// interleaving, an enumerated point is already the smallest its search saw, and a claim that held
/// has no counterexample at all.
fn walkable(
    claim: usize,
    obligations: &[Obligation],
    report: &Option<ProveReport>,
    world: &TypeWorld,
) -> Option<Shrinking> {
    let obligation = obligations.get(claim)?;
    let report = report.as_ref()?;
    let discharge = report
        .obligations
        .iter()
        .find(|(o, _)| o.key == obligation.key)
        .map(|(_, discharge)| discharge)?;
    let (values, original, root, case, target) = match discharge {
        Discharge::Refuted(cx) => (
            drawn(obligation, cx.root, cx.case, world)?,
            cx.original.clone(),
            cx.root,
            cx.case,
            Target::Falsifies,
        ),
        Discharge::Unattempted(Gap::Raised {
            root,
            case,
            bindings,
            ..
        }) => (
            drawn(obligation, *root, *case, world)?,
            bindings.clone(),
            *root,
            *case,
            Target::Raises,
        ),
        _ => return None,
    };
    let _ = (root, case);
    Some(Shrinking {
        claim,
        values,
        types: obligation
            .generated()
            .iter()
            .map(|b| b.ty.clone())
            .collect(),
        target,
        original,
    })
}

/// The value at `i` as it stands, with its candidates and each one's size.
fn offer(s: &Shrinking, i: usize, world: &TypeWorld) -> Option<Offer> {
    let value = s.values.get(i)?;
    let ty = s.types.get(i)?;
    let here = ply_prove::shrink::size(value, world);
    let candidates = ply_prove::shrink::candidates(value, ty, world)
        .iter()
        .enumerate()
        .map(|(position, candidate)| (position as u64, ply_prove::shrink::size(candidate, world)))
        .collect();
    Some(Offer { here, candidates })
}

/// The tuple with the candidate at `position` of the value at `i` taken.
fn candidate_at(s: &Shrinking, i: usize, position: i64, world: &TypeWorld) -> Option<Vec<Value>> {
    let value = s.values.get(i)?;
    let ty = s.types.get(i)?;
    let candidates = ply_prove::shrink::candidates(value, ty, world);
    let picked = usize::try_from(position)
        .ok()
        .and_then(|position| candidates.get(position))
        .cloned()?;
    let mut values = s.values.clone();
    values[i] = picked;
    Some(values)
}

/// The counterexample as it now stands: the walk's accepted values, and the ones it started from.
fn settled_of(s: &Shrinking) -> Settled {
    let bind = |values: &[Value]| -> Vec<(String, String, String)> {
        s.types
            .iter()
            .zip(values)
            .enumerate()
            .map(|(i, (ty, value))| {
                let name = match s.original.get(i) {
                    Some(binding) => binding.name.as_str().to_string(),
                    None => format!("v{i}"),
                };
                (name, ty.to_string(), value.render())
            })
            .collect()
    };
    Settled {
        bindings: bind(&s.values),
        original: s
            .original
            .iter()
            .map(|binding| {
                (
                    binding.name.as_str().to_string(),
                    binding.ty.to_string(),
                    binding.rendered.clone(),
                )
            })
            .collect(),
    }
}

/// A counterexample's bindings, as a report prints them: name, type, rendered value.
fn texts_of_bindings(bindings: &[(String, String, String)]) -> PlyValue {
    PlyValue::list(
        bindings
            .iter()
            .map(|(name, ty, rendered)| {
                record(vec![
                    ("name", PlyValue::str(name)),
                    ("ty", PlyValue::str(ty)),
                    ("rendered", PlyValue::str(rendered)),
                ])
            })
            .collect(),
    )
}

/// What a program needs to walk one value: how big it is, and its candidates with theirs, as
/// `(position, size)` — the position being what the program hands back to try one.
struct Offer {
    here: u64,
    candidates: Vec<(u64, u64)>,
}

/// The counterexample as it now stands, in the words a report prints: `(name, type, rendered)`.
struct Settled {
    bindings: Vec<(String, String, String)>,
    original: Vec<(String, String, String)>,
}

/// One counterexample being walked down. The values live here because a program cannot hold a value
/// of a type it never named: it decides which candidate to take, and this is where taking it lands.
struct Shrinking {
    claim: usize,
    values: Vec<Value>,
    types: Vec<ply_ty::Type>,
    target: Target,
    original: Vec<ply_prove::Binding>,
}

/// The types the laws are written over, as the machine's thread hands them over: plain data, because
/// a `PlyValue` cannot cross a thread and this side has no need of one.
struct Typed {
    decls: Vec<TypedDecl>,
    claims: Vec<TypedClaim>,
}

struct TypedDecl {
    name: String,
    variants: Vec<TypedVariant>,
}

struct TypedVariant {
    name: String,
    fields: Vec<ply_ty::Type>,
}

struct TypedClaim {
    claim: usize,
    /// Each binder's name, the text the runtime would print for its type, and the type itself.
    binders: Vec<(String, String, ply_ty::Type)>,
}

fn typed_of(
    world: &ply_prove::property::TypeWorld,
    obligations: &[ply_prove::Obligation],
) -> Typed {
    Typed {
        decls: world
            .declared()
            .map(|(name, decl)| TypedDecl {
                name: name.as_str().to_string(),
                variants: decl
                    .variants
                    .iter()
                    .map(|variant| TypedVariant {
                        name: variant.name.as_str().to_string(),
                        fields: variant.fields.clone(),
                    })
                    .collect(),
            })
            .collect(),
        claims: obligations
            .iter()
            .enumerate()
            .map(|(claim, obligation)| TypedClaim {
                claim,
                binders: obligation
                    .generated()
                    .iter()
                    .map(|binder| {
                        (
                            binder.name.as_str().to_string(),
                            binder.ty.to_string(),
                            binder.ty.clone(),
                        )
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn typed_value(typed: Typed) -> PlyValue {
    record(vec![
        (
            "decls",
            PlyValue::list(
                typed
                    .decls
                    .iter()
                    .map(|decl| {
                        record(vec![
                            ("name", PlyValue::str(&decl.name)),
                            (
                                "variants",
                                PlyValue::list(
                                    decl.variants
                                        .iter()
                                        .map(|variant| {
                                            record(vec![
                                                ("name", PlyValue::str(&variant.name)),
                                                (
                                                    "fields",
                                                    PlyValue::list(
                                                        variant
                                                            .fields
                                                            .iter()
                                                            .map(ty_value)
                                                            .collect(),
                                                    ),
                                                ),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "claims",
            PlyValue::list(
                typed
                    .claims
                    .iter()
                    .map(|claim| {
                        record(vec![
                            ("claim", PlyValue::Int(claim.claim as i64)),
                            (
                                "binders",
                                PlyValue::list(
                                    claim
                                        .binders
                                        .iter()
                                        .map(|(name, text, ty)| {
                                            record(vec![
                                                ("name", PlyValue::str(name)),
                                                ("text", PlyValue::str(text)),
                                                ("ty", ty_value(ty)),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn ty_value(ty: &ply_ty::Type) -> PlyValue {
    use ply_ty::Type;
    match ty {
        Type::Var(var) => {
            crate::payload::ctor(home("Ty"), "Var", vec![PlyValue::Int(i64::from(var.0))])
        }
        Type::Fn { .. } => crate::payload::ctor(home("Ty"), "Fn", Vec::new()),
        Type::Record(fields) => crate::payload::ctor(
            home("Ty"),
            "Record",
            vec![PlyValue::list(
                fields
                    .iter()
                    .map(|(name, field)| {
                        record(vec![
                            ("name", PlyValue::str(name.as_str())),
                            ("ty", ty_value(field)),
                        ])
                    })
                    .collect(),
            )],
        ),
        Type::Con(name, args) => crate::payload::ctor(
            home("Ty"),
            "Con",
            vec![
                PlyValue::str(name.as_str()),
                PlyValue::list(args.iter().map(ty_value).collect()),
            ],
        ),
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
            Step::Collected(answer) => {
                if let Ok(collection) = &*answer {
                    *self.claims.lock().unwrap_or_else(|e| e.into_inner()) =
                        collection.claims.len();
                }
                Ok(self.answered((*answer).map(collection_value)))
            }
            _ => Err(out_of_step("collected")),
        }
    }

    /// The types the laws are written over: every declared type, and each collected obligation's
    /// binders with the text the runtime would print for them. A program sizes a domain from this
    /// and never spells a type out, so nothing in Ply can disagree with the compiler about one.
    fn typed(&self) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("typed"))?;
        machine.ask(Go::Typed)?;
        match machine.step()? {
            Step::Typed(answer) => Ok(self.answered((*answer).map(typed_value))),
            _ => Err(out_of_step("typed")),
        }
    }

    /// Start walking this claim's counterexample down, and answer how many values it has. Nothing is
    /// a claim with nothing to walk: a race is an interleaving, an enumerated point is already the
    /// smallest thing its search saw, and a claim that held has no counterexample at all.
    fn shrink(&self, claim: usize) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("shrink"))?;
        machine.ask(Go::Shrink(claim))?;
        match machine.step()? {
            Step::Shrink(answer) => {
                Ok(self.answered((*answer).map(|width| crate::payload::option(width.map(count)))))
            }
            _ => Err(out_of_step("shrink")),
        }
    }

    /// The value at `i` as it stands, with its candidates.
    fn offers(&self, i: usize) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("offers"))?;
        machine.ask(Go::Offers(i))?;
        let Step::Offers(answer) = machine.step()? else {
            return Err(out_of_step("offers"));
        };
        let value = (*answer).map(|offer| {
            crate::payload::option(offer.map(|offer| {
                let candidates = offer
                    .candidates
                    .iter()
                    .map(|(position, size)| {
                        record(vec![
                            ("position", PlyValue::Int(*position as i64)),
                            ("size", PlyValue::Int(*size as i64)),
                        ])
                    })
                    .collect();
                record(vec![
                    ("here", PlyValue::Int(offer.here as i64)),
                    ("candidates", PlyValue::list(candidates)),
                ])
            }))
        });
        Ok(self.answered(value))
    }

    fn would(&self, i: usize, position: i64) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("would"))?;
        machine.ask(Go::Would { i, position })?;
        match machine.step()? {
            Step::Would(answer) => Ok(self.answered((*answer).map(PlyValue::Bool))),
            _ => Err(out_of_step("would")),
        }
    }

    fn take(&self, i: usize, position: i64) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("accept"))?;
        machine.ask(Go::Take { i, position })?;
        match machine.step()? {
            Step::Took(answer) => Ok(self.answered((*answer).map(|_| PlyValue::Unit))),
            _ => Err(out_of_step("accept")),
        }
    }

    /// The counterexample as it now stands, with the walk's choices taken into account.
    fn settled(&self) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("settled"))?;
        machine.ask(Go::Settled)?;
        let Step::Settled(answer) = machine.step()? else {
            return Err(out_of_step("settled"));
        };
        let value = (*answer).map(|settled| {
            crate::payload::option(settled.map(|settled| {
                record(vec![
                    ("bindings", texts_of_bindings(&settled.bindings)),
                    ("original", texts_of_bindings(&settled.original)),
                ])
            }))
        });
        Ok(self.answered(value))
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

    fn discharged(&self, choice: obligation::Choice) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("discharged"))?;
        machine.ask(Go::Discharge(choice))?;
        match machine.step()? {
            Step::Discharged(answer) => Ok(self.answered((*answer).map(|v| verdicts_value(&v)))),
            _ => Err(out_of_step("discharged")),
        }
    }

    /// Files the evidence of the discharges just made under the keys the program chose.
    fn record(&self, entries: Vec<(usize, DefHash)>) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("record"))?;
        machine.ask(Go::Record(entries))?;
        match machine.step()? {
            Step::Recorded(warnings) => Ok(diags_value(&warnings)),
            _ => Err(out_of_step("record")),
        }
    }

    fn replay(&self, index: usize, root: u64, case: u32) -> Result<PlyValue, Diagnostic> {
        let held = self.claims.lock().unwrap_or_else(|e| e.into_inner());
        if index >= *held {
            return Err(no_such_claim(index, *held));
        }
        drop(held);
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("replay"))?;
        machine.ask(Go::Replay { index, root, case })?;
        match machine.step()? {
            Step::Replayed(answer) => Ok(self.answered((*answer).map(|point| point_value(&point)))),
            _ => Err(out_of_step("replay")),
        }
    }

    /// The baseline a reader accepted for each definition in scope, where there is one.
    fn baselines(&self) -> Result<PlyValue, Diagnostic> {
        let held = self.held();
        let machine = held.as_ref().ok_or_else(|| unstarted("baselines"))?;
        machine.ask(Go::Baselines)?;
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
    /// The types the obligations are written over, so a program can measure a binder's domain
    /// rather than sample it. The decision is the program's; the type world is not.
    Typed,
    /// Start walking this claim's counterexample down. The answer is how many values it has, or
    /// nothing when there is nothing to walk — a race, an enumerated point, a claim that held.
    Shrink(usize),
    /// The value at `i` as it now stands: its size and its candidates with theirs.
    Offers(usize),
    /// Whether replacing the value at `i` with that candidate still leaves the counterexample a
    /// counterexample.
    Would {
        i: usize,
        position: i64,
    },
    /// Take that candidate: the program decided it was smaller and still held.
    Take {
        i: usize,
        position: i64,
    },
    /// The counterexample as it now stands.
    Settled,
    /// What the store holds under these keys. The program computes them — a plan key is part of
    /// the obligation's own encoding — so no row could carry the answers.
    Outcomes(Vec<String>),
    Discharge(obligation::Choice),
    /// File the evidence of the discharges just made, each at a position in the run, under the key
    /// the program chose for it.
    Record(Vec<(usize, DefHash)>),
    /// One point of one obligation's guard: its place in the collection, the generator's root,
    /// and which case to draw.
    Replay {
        index: usize,
        root: u64,
        case: u32,
    },
    Baselines,
    Accept(Vec<(Symbol, ReviewRecord)>),
}

enum Step {
    Collected(Box<Result<Collection, Refused>>),
    Typed(Box<Result<Typed, Refused>>),
    Shrink(Box<Result<Option<usize>, Refused>>),
    Offers(Box<Result<Option<Offer>, Refused>>),
    Would(Box<Result<bool, Refused>>),
    Took(Box<Result<(), Refused>>),
    Settled(Box<Result<Option<Settled>, Refused>>),
    /// The store's answer under each key asked about: `passed`, `failed`, or nothing.
    Outcomes(Vec<Option<String>>),
    Discharged(Box<Result<Verdicts, Refused>>),
    Recorded(Vec<Diagnostic>),
    Replayed(Box<Result<Point, Refused>>),
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
    let collected = crate::obligations::collect(&loaded.front, &scoped, &hashes);
    warnings.extend(collected.warnings);
    let obligations = collected.obligations;
    let labels = law_labels(&loaded.check);

    let _ = told.send(Step::Collected(Box::new(Ok(Collection {
        sources: loaded.sources.clone(),
        warnings: std::mem::take(&mut warnings),
        claims: obligations
            .iter()
            .map(|o| claim_of(o, &loaded, &labels))
            .collect(),
        defs: defs_of(&scoped, &hashes),
        laws: laws_of(&scoped, &hashes),
        plan: job.plan.clone(),
    }))));

    let mut report: Option<ProveReport> = None;
    // The counterexample being walked down, if a program is walking one: the values live here
    // because a program cannot hold a value of a type it never named.
    let mut shrinking: Option<Shrinking> = None;
    // Built by the first step that runs an obligation, and kept: a discharge of many claims and a
    // re-run of one case are the same prover over the same hosts.
    let mut prepared: Option<Result<Prepared, Refused>> = None;
    loop {
        match asked.recv() {
            Ok(Go::Outcomes(keys)) => {
                // What the *obligation* cache holds under each key, as a word: the program applies
                // the rule (a proof only under the bare key, a sample only under its plan's), and
                // only the runtime can decode an entry.
                let answers: Vec<Option<String>> = keys
                    .iter()
                    .map(|key| {
                        let entry = ply_ty::DefHash::from_hex(key)
                            .and_then(|hash| store.obligation(hash))?;
                        Some(match from_cached(&entry) {
                            Ok(Evidence::Proof(_)) => "proof".to_string(),
                            Ok(Evidence::Cases(_)) => "sample".to_string(),
                            Err(_) => "other".to_string(),
                        })
                    })
                    .collect();
                let _ = told.send(Step::Outcomes(answers));
            }
            Ok(Go::Shrink(claim)) => {
                if prepared.is_none() {
                    prepared = Some(prepare(&job, &loaded, &mut store));
                }
                let start = match prepared.as_ref() {
                    Some(Ok(ready)) => walkable(claim, &obligations, &report, ready.prover.world()),
                    Some(Err(refused)) => {
                        let _ = told.send(Step::Shrink(Box::new(Err(refused.clone()))));
                        return;
                    }
                    None => return,
                };
                let width = start.as_ref().map(|s| s.values.len());
                shrinking = start;
                let _ = told.send(Step::Shrink(Box::new(Ok(width))));
            }
            Ok(Go::Offers(i)) => {
                let answer = match (&shrinking, prepared.as_ref()) {
                    (Some(s), Some(Ok(ready))) => offer(s, i, ready.prover.world()),
                    _ => None,
                };
                let _ = told.send(Step::Offers(Box::new(Ok(answer))));
            }
            Ok(Go::Would { i, position }) => {
                let answer = match (&shrinking, prepared.as_ref()) {
                    (Some(s), Some(Ok(ready))) => {
                        candidate_at(s, i, position, ready.prover.world()).is_some_and(|values| {
                            ready
                                .prover
                                .judge_at(&obligations[s.claim], &job.plan, &values)
                                .matches(s.target)
                        })
                    }
                    _ => false,
                };
                let _ = told.send(Step::Would(Box::new(Ok(answer))));
            }
            Ok(Go::Take { i, position }) => {
                let picked = match (&shrinking, prepared.as_ref()) {
                    (Some(s), Some(Ok(ready))) => {
                        candidate_at(s, i, position, ready.prover.world())
                            .map(|values| values[i].clone())
                    }
                    _ => None,
                };
                if let (Some(value), Some(s)) = (picked, shrinking.as_mut())
                    && i < s.values.len()
                {
                    s.values[i] = value;
                }
                let _ = told.send(Step::Took(Box::new(Ok(()))));
            }
            Ok(Go::Settled) => {
                let answer = shrinking.as_ref().map(settled_of);
                let _ = told.send(Step::Settled(Box::new(Ok(answer))));
            }
            Ok(Go::Typed) => {
                if prepared.is_none() {
                    prepared = Some(prepare(&job, &loaded, &mut store));
                }
                match prepared.as_ref() {
                    Some(Ok(ready)) => {
                        let typed = typed_of(ready.prover.world(), &obligations);
                        let _ = told.send(Step::Typed(Box::new(Ok(typed))));
                    }
                    Some(Err(refused)) => {
                        let _ = told.send(Step::Typed(Box::new(Err(refused.clone()))));
                        return;
                    }
                    None => return,
                }
            }
            Ok(Go::Discharge(wanted)) => {
                if prepared.is_none() {
                    prepared = Some(prepare(&job, &loaded, &mut store));
                }
                let ready = match prepared.as_ref() {
                    Some(Ok(ready)) => ready,
                    Some(Err(refused)) => {
                        let _ = told.send(Step::Discharged(Box::new(Err(refused.clone()))));
                        return;
                    }
                    None => return,
                };
                let asked_for: Vec<Obligation> = wanted
                    .claims
                    .iter()
                    .filter_map(|&index| obligations.get(index).cloned())
                    .collect();
                let (proved, mut warnings) = discharge(&job, asked_for, &wanted, ready, &store);
                warnings.extend(flushed(&mut store));
                let verdicts = verdicts_of(&proved, warnings);
                report = Some(proved);
                let _ = told.send(Step::Discharged(Box::new(Ok(verdicts))));
            }
            Ok(Go::Record(entries)) => {
                let mut warnings = Vec::new();
                for (at, key) in entries {
                    match report.as_ref().and_then(|r| r.obligations.get(at)) {
                        Some((_, Discharge::Held(evidence))) => {
                            store.put_obligation(key, to_cached(evidence))
                        }
                        _ => warnings.push(unfiled(at)),
                    }
                }
                warnings.extend(flushed(&mut store));
                let _ = told.send(Step::Recorded(warnings));
            }
            Ok(Go::Replay { index, root, case }) => {
                if prepared.is_none() {
                    prepared = Some(prepare(&job, &loaded, &mut store));
                }
                let answer = match prepared.as_ref() {
                    Some(Ok(ready)) => {
                        Ok(ready
                            .prover
                            .point_at(&obligations[index], root, case, &job.plan))
                    }
                    Some(Err(refused)) => Err(refused.clone()),
                    None => return,
                };
                let _ = told.send(Step::Replayed(Box::new(answer)));
            }
            Ok(Go::Baselines) => {
                let baselines = scoped
                    .defs
                    .keys()
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
/// The walk and the compiler are the CLI's, and what it answered is what this reads. `incremental`
/// still decides whether the store takes part, exactly as it did when this side ran the front end.
fn load(job: &Job, store: &mut Store) -> Result<Loaded, LoadError> {
    let Some(front) = &job.front else {
        return Err(LoadError {
            sources: ply_span::SourceMap::new(),
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
    let mode = if job.incremental {
        crate::driver::Mode::Incremental
    } else {
        crate::driver::Mode::Full
    };
    let store = job.incremental.then_some(store);
    crate::driver::load_over_front(
        &job.path,
        &front.files,
        &front.packages,
        &front.dump,
        front.read,
        front.front,
        mode,
        store,
    )
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

/// The prover and the hosts a run discharges and re-runs points with, built when the first step
/// that needs them asks: a discharge of many claims and a re-run of one case are the same engine.
struct Prepared<'a> {
    /// Kept alive for the prover's lifetime, which is the run's.
    _hosts: Option<Hosts>,
    prover: crate::engine::Prover<'a>,
    /// What opening the hosts had to say, reported by the discharge that reads them.
    warnings: Vec<Diagnostic>,
}

fn prepare<'a>(job: &Job, loaded: &'a Loaded, store: &mut Store) -> Result<Prepared<'a>, Refused> {
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
    let prover =
        crate::engine::prover(loaded, hosting, Some(backend), store).map_err(|err| Refused {
            why: Why::Broken,
            diagnostics: err.diagnostics,
            sources: err.sources,
        })?;
    Ok(Prepared {
        _hosts: hosts,
        prover,
        warnings,
    })
}

/// The program's decision carried out: the evidence it named read back, and the rest discharged.
fn discharge(
    job: &Job,
    asked_for: Vec<Obligation>,
    choice: &obligation::Choice,
    prepared: &Prepared<'_>,
    store: &Store,
) -> (ProveReport, Vec<Diagnostic>) {
    let mut warnings = prepared.warnings.clone();
    let asked = obligation::Asked::chosen(asked_for, choice, store, &job.plan);
    let (pool, _workers) = build_pool(job.jobs, &mut warnings);
    let discharge = || asked.discharge(&prepared.prover);
    let report = match &pool {
        Some(pool) => pool.install(discharge),
        None => discharge(),
    };
    (report, warnings)
}

// --- What crosses back ----------------------------------------------------------

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

enum Kind {
    Ensures(usize),
    Law(Option<String>),
}

struct Claim {
    key: String,
    /// `law/host`: a verdict against the world, which is never cached.
    host: bool,
    owner: String,
    kind: Kind,
    guarded: bool,
    frame: Frame,
    at: At,
    unperformed: Vec<String>,
}

/// A definition in scope: its hash, when the front end produced one, and the spec text of each of its
/// own clauses.
struct Def {
    name: String,
    hash: Option<DefHash>,
    specs: Vec<DefHash>,
}

/// A law as the program wrote it: the hash of its text, and every name it mentions.
struct Written {
    key: String,
    text: DefHash,
    mentions: Vec<String>,
}

struct Collection {
    sources: SourceMap,
    warnings: Vec<Diagnostic>,
    claims: Vec<Claim>,
    defs: Vec<Def>,
    laws: Vec<Written>,
    plan: ProvePlan,
}

struct Verdicts {
    outcomes: Vec<Discharge>,
    /// Parallel to `outcomes`: what the static tier alone answered, when this run asked it.
    reaches: Vec<Option<ply_prove::prove::Reach>>,
    duration: std::time::Duration,
    warnings: Vec<Diagnostic>,
}

struct Accepted {
    definitions: usize,
    stored: bool,
    warnings: Vec<Diagnostic>,
}

fn claim_of(o: &Obligation, loaded: &Loaded, labels: &BTreeMap<Symbol, String>) -> Claim {
    Claim {
        key: o.key.to_hex(),
        host: o.host,
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

/// Every definition in scope, in name order, with its hash and its own clauses' spec text.
fn defs_of(scoped: &CheckOutput, hashes: &ply_ty::HashOutput) -> Vec<Def> {
    scoped
        .defs
        .keys()
        .map(|name| Def {
            name: name.as_str().to_string(),
            hash: hashes.defs.get(name).copied(),
            specs: hashes.spec_texts.get(name).cloned().unwrap_or_default(),
        })
        .collect()
}

/// Every law in scope the front end hashed: the hash of its text, or of the law when its text has
/// none, and every name it mentions.
fn laws_of(scoped: &CheckOutput, hashes: &ply_ty::HashOutput) -> Vec<Written> {
    scoped
        .laws
        .iter()
        .filter_map(|law| {
            let hash = *hashes.laws.get(law.index)?;
            Some(Written {
                key: law.key.as_str().to_string(),
                text: hashes.law_texts.get(law.index).copied().unwrap_or(hash),
                mentions: hashes
                    .deps
                    .get(&law.key)
                    .into_iter()
                    .flatten()
                    .map(|name| name.as_str().to_string())
                    .collect(),
            })
        })
        .collect()
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

fn verdicts_of(report: &ProveReport, warnings: Vec<Diagnostic>) -> Verdicts {
    Verdicts {
        outcomes: report
            .obligations
            .iter()
            .map(|(_, discharge)| discharge.clone())
            .collect(),
        reaches: report.reaches.clone(),
        duration: report.duration,
        warnings,
    }
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

fn kind_value(kind: &Kind) -> PlyValue {
    match kind {
        Kind::Ensures(index) => ctor(home("Kind"), "Ensures", vec![count(*index)]),
        Kind::Law(label) => ctor(
            home("Kind"),
            "Law",
            vec![option(label.as_deref().map(PlyValue::str))],
        ),
    }
}

fn frame_value(frame: &Frame) -> PlyValue {
    match frame {
        Frame::Pure => ctor(home("Frame"), "Pure", Vec::new()),
        Frame::Writes(writes) => {
            let named: Vec<String> = writes
                .iter()
                .map(|(effect, resource)| format!("{effect}{resource}"))
                .collect();
            ctor(
                home("Frame"),
                "Writes",
                vec![strings(named.iter().map(String::as_str))],
            )
        }
    }
}

fn claim_value(claim: &Claim) -> PlyValue {
    record(vec![
        ("key", PlyValue::str(&claim.key)),
        ("host", PlyValue::Bool(claim.host)),
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
        (
            "claims",
            PlyValue::list(collection.claims.iter().map(claim_value).collect()),
        ),
        (
            "defs",
            PlyValue::list(
                collection
                    .defs
                    .iter()
                    .map(|d| {
                        record(vec![
                            ("name", PlyValue::str(&d.name)),
                            ("hash", option(d.hash.map(|h| PlyValue::str(h.to_hex())))),
                            ("specs", hashes_value(&d.specs)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "laws",
            PlyValue::list(
                collection
                    .laws
                    .iter()
                    .map(|w| {
                        record(vec![
                            ("key", PlyValue::str(&w.key)),
                            ("text", PlyValue::str(w.text.to_hex())),
                            ("mentions", strings(w.mentions.iter().map(String::as_str))),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("plan", plan_value(&collection.plan)),
    ])
}

fn tier_value(tier: Tier) -> PlyValue {
    let named = match tier {
        Tier::Proved => "Proved",
        Tier::Property => "Property",
        Tier::Example => "Example",
    };
    ctor(home("Tier"), named, Vec::new())
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
            home("Evidence"),
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
            home("Evidence"),
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
    match gap {
        Gap::UnhandledEffect(footprint) => ctor(
            home("Gap"),
            "UnhandledEffect",
            vec![option(
                (!footprint.is_empty()).then(|| PlyValue::str(footprint.to_string())),
            )],
        ),
        Gap::Ungeneratable { param, ty } => ctor(
            home("Gap"),
            "Ungeneratable",
            vec![record(vec![
                ("param", PlyValue::str(param.as_str())),
                ("ty", PlyValue::str(ty.to_string())),
            ])],
        ),
        Gap::Raised {
            bindings,
            diagnostic,
            ..
        } => ctor(
            home("Gap"),
            "Raised",
            vec![record(vec![
                ("message", PlyValue::str(&diagnostic.message)),
                ("bindings", bindings_value(bindings)),
            ])],
        ),
        Gap::GuardNotSampled { generated, witness } => ctor(
            home("Gap"),
            "GuardNotSampled",
            vec![record(vec![
                ("generated", tally(u64::from(*generated))),
                ("witness", bindings_value(witness)),
            ])],
        ),
        Gap::ReachesHost(footprint) => ctor(
            home("Gap"),
            "ReachesHost",
            vec![PlyValue::str(footprint.to_string())],
        ),
        Gap::NotDrawn => ctor(home("Gap"), "NotDrawn", Vec::new()),
    }
}

/// One point as `claims.ply` reads it: the same constructors the whole-run outcomes use, minus
/// the tier, because a case that held says nothing about how the obligation as a whole was shown.
fn point_value(point: &Point) -> PlyValue {
    match point {
        Point::Kept(bindings) => ctor(home("Point"), "Kept", vec![bindings_value(bindings)]),
        Point::Falsified(bindings) => {
            ctor(home("Point"), "Falsified", vec![bindings_value(bindings)])
        }
        Point::Rejected => ctor(home("Point"), "Rejected", Vec::new()),
        Point::Undrawn(gap) => ctor(home("Point"), "Undrawn", vec![gap_value(gap)]),
    }
}

fn vacuity_value(vacuity: &Vacuity) -> PlyValue {
    record(vec![
        ("guard", at_value(&At::of(vacuity.guard))),
        (
            "why",
            match vacuity.kind {
                VacuityKind::ProvedUnsatisfiable => {
                    ctor(home("Vacuity"), "Unsatisfiable", Vec::new())
                }
                VacuityKind::NoCaseKept { generated } => ctor(
                    home("Vacuity"),
                    "NoCaseKept",
                    vec![tally(u64::from(generated))],
                ),
            },
        ),
    ])
}

fn outcome_value(discharge: &Discharge) -> PlyValue {
    match discharge {
        Discharge::Held(evidence) => ctor(
            home("Outcome"),
            "Held",
            vec![record(vec![
                ("tier", tier_value(evidence.tier())),
                ("evidence", evidence_value(evidence)),
            ])],
        ),
        Discharge::Refuted(cx) => ctor(
            home("Outcome"),
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
        Discharge::Vacuous(vacuity) => {
            ctor(home("Outcome"), "Vacuous", vec![vacuity_value(vacuity)])
        }
        Discharge::Unattempted(gap) => ctor(home("Outcome"), "Unattempted", vec![gap_value(gap)]),
    }
}

fn verdicts_value(verdicts: &Verdicts) -> PlyValue {
    record(vec![
        (
            "outcomes",
            PlyValue::list(verdicts.outcomes.iter().map(outcome_value).collect()),
        ),
        (
            "reaches",
            PlyValue::list(
                verdicts
                    .reaches
                    .iter()
                    .map(|reach| option(reach.as_ref().map(reach_value)))
                    .collect(),
            ),
        ),
        ("duration_ms", millis(verdicts.duration)),
        ("warnings", diags_value(&verdicts.warnings)),
    ])
}

/// What the static tier alone answered for one obligation, as the product carries it.
fn reach_value(reach: &ply_prove::prove::Reach) -> PlyValue {
    record(vec![
        ("decision", PlyValue::str(reach.decision.as_str())),
        ("steps", tally(u64::from(reach.decision.steps()))),
        (
            "blockers",
            PlyValue::list(
                reach
                    .blockers
                    .iter()
                    .map(|blocker| {
                        let (kind, about) = blocker.parts();
                        record(vec![
                            ("kind", PlyValue::str(kind)),
                            ("about", option(about.map(PlyValue::str))),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
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
fn unfiled(at: usize) -> Diagnostic {
    Diagnostic::warning(
        codes::INTERNAL_ERROR,
        format!("the program asked to file claim {at}'s evidence, and it holds none"),
    )
    .note("nothing was filed for it, so the next run discharges it again")
    .note("the program and the thread it drives are written together; this is Ply's fault")
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
        incremental: !bool_at("no_incremental")?,
        use_cache: !bool_at("no_cache")?,
        std: bool_at("std")?,
        jobs: opt_int_at(v, "jobs", span)?.map(|n| n as u32),
        plan: crate::simulation::prove_plan(&prove_opts, &sim_opts),
        binding,
    })
}

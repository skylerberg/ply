//! The runtime's half of a discharge: a claim's propositions entered at the points the program
//! drew, and a law over interleavings searched at the points its guard kept. What a claim's points
//! are, and what their judgements come to, is the program's.

use crate::load::Loaded;
use ply_eval::decode::{At, Error as DecodeError};
use ply_eval::host::HostBinding;
use ply_eval::{
    DEFAULT_MAX_CALLS, DefInfo, Diagnostic, Front, LawInfo, Machine, Seed, SourceId, Span, Symbol,
    Value, codes,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// One claim the program owes, as far as the runtime enters it: whose it is, where it is written,
/// what a point assigns, the guards it is judged after, and which machine its propositions run on.
/// Everything else about it is `proof.world`'s.
#[derive(Clone, Debug)]
pub struct Obligation {
    /// `<module>.<def>` for a clause, `<module>.<label>` for a law.
    pub owner: Symbol,
    pub kind: ObligationKind,
    pub span: Span,
    /// What a point assigns: the owner's parameters for a clause, the `forall` binders for a law.
    pub binders: Vec<Binder>,
    /// A clause's `result`, which is the owner's answer and never drawn.
    pub result: Option<Binder>,
    /// Each guard's place: an owner's `requires` clauses, or a law's `where`.
    pub guards: Vec<Span>,
    pub strategy: Strategy,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObligationKind {
    /// Its place among the owner's `ensures` clauses.
    Ensures {
        index: usize,
    },
    Law,
}

/// One binder of a claim: what a report calls it, and its type as the compiler prints it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Binder {
    pub name: Symbol,
    pub text: String,
}

/// Which machine a claim's propositions run on, as `proof.world` decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// A law over a `simulate` region: its interleavings are searched at each of its points.
    Interleave,
    /// A `law/host`, run against the host the run binds.
    Hosted,
    /// The static prover first, then the claim's points.
    Static,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Binding {
    pub name: Symbol,
    /// The binder's type as the compiler prints it.
    pub ty: String,
    pub value: ply_eval::Plain,
}

/// Ply's own failure while judging a claim, as [`ply_eval::codes::is_defect`] tells it apart.
#[derive(Clone, Debug)]
pub struct Fault {
    /// The point being judged when Ply failed, or none when it failed before a point was drawn.
    pub bindings: Vec<Binding>,
    pub diagnostic: Box<Diagnostic>,
}

/// The obligations a `proof.world.World` owes, in the order the program listed them, which is the
/// order it names them by.
pub fn obligations_of(world: At<'_>) -> Result<Vec<Obligation>, DecodeError> {
    world.field("obligations")?.items(obligation_of)
}

fn obligation_of(at: At<'_>) -> Result<Obligation, DecodeError> {
    let kind = at.field("kind")?.ctor()?;
    let binder = |b: At<'_>| -> Result<Binder, DecodeError> {
        Ok(Binder {
            name: Symbol::new(b.field("name")?.str()?),
            text: b.field("text")?.str()?.to_string(),
        })
    };
    let span = |s: At<'_>| -> Result<Span, DecodeError> {
        Ok(Span::new(
            SourceId(s.field("module")?.number()?),
            s.field("start")?.number()?,
            s.field("end")?.number()?,
        ))
    };
    let strategy = at.field("strategy")?.ctor()?;
    Ok(Obligation {
        owner: Symbol::new(at.field("owner")?.str()?),
        kind: match kind.name() {
            "Ensures" => ObligationKind::Ensures {
                index: kind.arg(0)?.number()?,
            },
            "Law" => ObligationKind::Law,
            _ => return Err(kind.unknown()),
        },
        span: span(at.field("at")?)?,
        binders: at.field("binders")?.items(binder)?,
        result: match at.field("result")?.option()? {
            Some(result) => Some(binder(result)?),
            None => None,
        },
        guards: at.field("guards")?.items(span)?,
        strategy: match strategy.name() {
            "Interleave" => Strategy::Interleave,
            "Hosted" => Strategy::Hosted,
            "Static" => Strategy::Static,
            _ => return Err(strategy.unknown()),
        },
    })
}

/// The prover this build drives over the program. One built once serves a whole run.
pub fn prover(
    loaded: &Loaded,
    hosting: Option<Hosting>,
    backend: &'static dyn ply_eval::Provider,
) -> Prover {
    let prover = Prover::new(loaded, backend);
    match hosting {
        Some(hosting) => prover.with_hosting(hosting),
        None => prover,
    }
}

/// Where an obligation's claim is written, found once per run.
enum Claim<'s> {
    Ensures {
        owner: &'s DefInfo,
        /// Its place among the owner's `ensures` clauses, which names its root.
        index: usize,
    },
    Law {
        info: &'s LawInfo,
        /// Its place among the module's laws, which names its roots.
        ordinal: usize,
    },
}

/// It owns what it judges, so a run can judge on whichever threads the program asks from.
pub struct Prover {
    front: Arc<Front>,
    /// Each law's place among its module's laws, which names its roots, and its place in the
    /// program's.
    laws: HashMap<Symbol, (usize, usize)>,
    /// What a `law/host` is discharged against.
    hosting: Option<Hosting>,
    /// A compiled unit holding the laws' and clauses' roots, where those propositions are entered.
    backend: &'static dyn ply_eval::Provider,
}

/// The binding and the reactor a `law/host` runs against. The factory is owned rather than
/// borrowed, so a prover holding a hosting borrows the program and nothing else, and a caller can
/// keep one prover across many steps.
pub struct Hosting {
    pub binding: Arc<HostBinding>,
    pub runtime: Option<ply_eval::RuntimeFactory>,
}

impl Prover {
    /// `backend` is the unit built from `loaded`, laws' and clauses' roots included.
    pub fn new(loaded: &Loaded, backend: &'static dyn ply_eval::Provider) -> Prover {
        let mut laws = HashMap::new();
        let mut ordinals: HashMap<&Symbol, usize> = HashMap::new();
        for (at, law) in loaded.front.check.laws.iter().enumerate() {
            let ordinal = ordinals.entry(law.module.as_symbol()).or_default();
            laws.insert(law.key.clone(), (*ordinal, at));
            *ordinal += 1;
        }
        Prover {
            front: Arc::clone(&loaded.front),
            laws,
            hosting: None,
            backend,
        }
    }

    /// The unit, attached once per thread: claims are judged on whichever threads the program asks from.
    fn compiled(&self) -> Rc<dyn ply_eval::Compiled> {
        thread_local! {
            static ATTACHED: RefCell<Vec<(usize, Rc<dyn ply_eval::Compiled>)>> =
                const { RefCell::new(Vec::new()) };
        }
        let key = std::ptr::from_ref(self.backend).cast::<()>() as usize;
        ATTACHED.with(|attached| {
            if let Some((_, c)) = attached.borrow().iter().find(|(k, _)| *k == key) {
                return Rc::clone(c);
            }
            let c = self.backend.attach();
            attached.borrow_mut().push((key, Rc::clone(&c)));
            c
        })
    }

    /// A proposition's body root: its program-wide name in the unit.
    fn body_root(&self, claim: &Claim<'_>) -> Symbol {
        match claim {
            Claim::Ensures { owner, index, .. } => owner.module.qualify(
                &ply_codegen::clause_root_name(&owner.simple_name, "ensures", *index),
            ),
            Claim::Law { info, ordinal, .. } => info
                .module
                .qualify(&ply_codegen::law_root_name(*ordinal, "body")),
        }
    }

    /// Each guard's compiled root, in [`Obligation::guards`] order, which `source.rs` numbers
    /// alike.
    fn guard_roots(&self, obligation: &Obligation, claim: &Claim<'_>) -> Vec<Symbol> {
        match claim {
            Claim::Ensures { owner, .. } => (0..obligation.guards.len())
                .map(|k| {
                    owner.module.qualify(&ply_codegen::clause_root_name(
                        &owner.simple_name,
                        "requires",
                        k,
                    ))
                })
                .collect(),
            Claim::Law { info, ordinal } => obligation
                .guards
                .iter()
                .map(|_| {
                    info.module
                        .qualify(&ply_codegen::law_root_name(*ordinal, "guard"))
                })
                .collect(),
        }
    }

    /// Bind the host, so that a `law/host` is attempted rather than reported as a gap.
    pub fn with_hosting(mut self, hosting: Hosting) -> Prover {
        self.hosting = Some(hosting);
        self
    }

    fn claim(&self, obligation: &Obligation) -> Option<Claim<'_>> {
        match obligation.kind {
            ObligationKind::Ensures { index } => Some(Claim::Ensures {
                owner: self.front.check.defs.get(&obligation.owner)?,
                index,
            }),
            ObligationKind::Law => {
                let &(ordinal, at) = self.laws.get(&obligation.owner)?;
                let info = self.front.check.laws.get(at)?;
                Some(Claim::Law { info, ordinal })
            }
        }
    }

    /// What every entry a claim makes goes through, its owner's call included: this thread's tier,
    /// bound to the run's host and a reactor for this thread when the claim is a `law/host`.
    fn machine(&self, obligation: &Obligation) -> Result<Machine<'_>, Diagnostic> {
        let mut machine =
            Machine::new(&self.front, self.compiled())?.with_max_calls(DEFAULT_MAX_CALLS);
        if let Strategy::Hosted = obligation.strategy {
            let hosting = self.hosting.as_ref().ok_or_else(|| unhosted(obligation))?;
            machine.set_host_binding(Arc::clone(&hosting.binding));
            if let Some(factory) = &hosting.runtime {
                machine.set_host_runtime(Arc::clone(factory));
            }
        }
        Ok(machine)
    }
}

/// What one point of a claim came to.
#[derive(Debug)]
pub enum Judgement {
    Held,
    Failed,
    Rejected,
    /// The program raised.
    Raised(Diagnostic),
    /// Ply failed rather than the program, so the point says nothing about the claim.
    Faulted(Diagnostic),
}

impl Judgement {
    pub(crate) fn stopped(diagnostic: Diagnostic) -> Judgement {
        if codes::is_defect(diagnostic.code) {
            Judgement::Faulted(diagnostic)
        } else {
            Judgement::Raised(diagnostic)
        }
    }
}

/// How a batch's points are judged, and which judgement ends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The whole claim, until a point settles it.
    Whole,
    /// The guard alone, until it admits a point.
    Witness,
    /// The guard alone at every point, until it raises.
    Domain,
}

impl Mode {
    fn ends(self, judgement: &Judgement) -> bool {
        matches!(
            (self, judgement),
            (_, Judgement::Faulted(_))
                | (Mode::Whole, Judgement::Failed | Judgement::Raised(_))
                | (Mode::Witness, Judgement::Held)
                | (Mode::Domain, Judgement::Raised(_))
        )
    }
}

/// The world named an obligation no claim of the front end's states: Ply disagreeing with itself.
#[cold]
fn unclaimed(obligation: &Obligation) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the prover holds no claim for `{}`", obligation.owner),
    )
    .primary(
        obligation.span,
        "this obligation was named, and no claim states it",
    )
    .note("`proof.world` and the front end's claims are read from one program; this is Ply's fault")
}

/// A `law/host` judged with no host bound: the program asked for what the run never opened.
#[cold]
fn unhosted(obligation: &Obligation) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "`{}` reaches the host, and this run binds none",
            obligation.owner
        ),
    )
    .primary(
        obligation.span,
        "a `law/host` is judged against the host a run binds",
    )
    .note("the program judges a `law/host` only under `--host`; this is Ply's fault")
}

impl Prover {
    /// One claim's points, judged in order until one ends the batch, and what the entries that
    /// judged them ended with.
    pub fn judged(
        &self,
        obligation: &Obligation,
        step_budget: i64,
        points: &[Vec<Value>],
        mode: Mode,
    ) -> Judgements {
        let mut cases = match self.cases(obligation, step_budget) {
            Ok(cases) => cases,
            Err(diagnostic) => {
                return Judgements {
                    each: vec![Judgement::Faulted(diagnostic)],
                    warnings: Vec::new(),
                };
            }
        };
        let mut each = Vec::with_capacity(points.len());
        for values in points {
            let judgement = match mode {
                Mode::Whole => cases.judge(values),
                Mode::Witness | Mode::Domain => match cases.guard(values) {
                    Ok(true) => Judgement::Held,
                    Ok(false) => Judgement::Rejected,
                    Err(diagnostic) => Judgement::stopped(diagnostic),
                },
            };
            let ends = mode.ends(&judgement);
            each.push(judgement);
            if ends {
                break;
            }
        }
        Judgements {
            each,
            warnings: cases.warnings,
        }
    }

    /// A law whose body reaches a `simulate` region, run once at `values` under `seed`: the
    /// interleaving it took, how it ended, and whether it entered a region at all.
    pub fn interleaved(
        &self,
        obligation: &Obligation,
        step_budget: i64,
        values: &[Value],
        seed: &Seed,
        steps: u32,
    ) -> Interleaved {
        let mut cases = match self.cases(obligation, step_budget) {
            Ok(cases) => cases,
            Err(diagnostic) => return Interleaved::faulted(diagnostic),
        };
        cases.machine.set_seed(seed.clone(), steps);
        let body_root = cases.body_root.clone();
        let judged = match cases.enter(&body_root, values.to_vec()) {
            Ok(Value::Bool(true)) => None,
            Ok(Value::Bool(false)) => Some(Judgement::Failed),
            Ok(other) => Some(Judgement::Faulted(body_was_not_boolean(
                &other,
                obligation.span,
            ))),
            Err(diagnostic) => Some(Judgement::stopped(diagnostic)),
        };
        let outcome = match &judged {
            None => Ok(()),
            Some(_) => Err(Diagnostic::error(
                codes::OBLIGATION_REFUTED,
                "the law failed",
            )),
        };
        let record = cases.machine.simulated();
        Interleaved {
            interleaving: record.map_or_else(
                || ply_eval::Interleaving::passed(Vec::new()),
                |r| r.interleaving(&outcome),
            ),
            verdict: judged,
            observed: record.is_some(),
            warnings: cases.warnings,
        }
    }

    fn cases(&self, obligation: &Obligation, step_budget: i64) -> Result<Cases<'_>, Diagnostic> {
        let claim = self
            .claim(obligation)
            .ok_or_else(|| unclaimed(obligation))?;
        let call = match claim {
            Claim::Ensures { .. } => Some(obligation.owner.clone()),
            Claim::Law { .. } => None,
        };
        Ok(Cases {
            machine: self.machine(obligation)?,
            guard_roots: self.guard_roots(obligation, &claim),
            body_root: self.body_root(&claim),
            span: obligation.span,
            call,
            result: obligation.result.as_ref().map(|b| b.name.clone()),
            step_budget,
            warnings: Vec::new(),
        })
    }
}

/// What a batch of points came to, and what the entries that judged them ended with.
pub struct Judgements {
    pub each: Vec<Judgement>,
    pub warnings: Vec<Diagnostic>,
}

/// Each binder beside the value it was given, as a report prints them.
pub fn bindings(binders: &[Binder], values: &[Value]) -> Vec<Binding> {
    binders
        .iter()
        .zip(values)
        .map(|(binder, value)| Binding {
            name: binder.name.clone(),
            ty: binder.text.clone(),
            value: ply_eval::Plain::shown(value),
        })
        .collect()
}

/// How a tuple of binder values is judged: guard first, always.
struct Cases<'a> {
    machine: Machine<'a>,
    guard_roots: Vec<Symbol>,
    body_root: Symbol,
    span: Span,
    /// The definition an `ensures` is attached to, called to produce `result`.
    call: Option<Symbol>,
    result: Option<Symbol>,
    step_budget: i64,
    /// What every entry so far ended with.
    warnings: Vec<Diagnostic>,
}

impl Cases<'_> {
    /// One entry through the claim's machine, within the claim's budget, keeping what it ended
    /// with.
    fn enter(&mut self, root: &Symbol, args: Vec<Value>) -> Result<Value, Diagnostic> {
        let (machine, span) = (&mut self.machine, self.span);
        let (answer, warnings) = ply_codegen::rt::with_step_budget(self.step_budget, || {
            machine.call(root.as_str(), args, span)
        })
        .into_parts();
        self.warnings.extend(warnings);
        answer
    }

    fn boolean(&self, value: Value) -> Result<bool, Diagnostic> {
        match value {
            Value::Bool(b) => Ok(b),
            other => Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("a spec expression came to `{}` rather than to a Boolean", ply_eval::slot(0)),
            )
            .primary(self.span, "a spec states a proposition, so its type is `Bool`")
            .note("the type checker rejects a non-`Bool` clause with E0201, so reaching this is a defect in Ply")
            .showing(vec![ply_eval::Plain::shown(&other)])),
        }
    }
}

impl Cases<'_> {
    /// Guard first, always.
    fn judge(&mut self, values: &[Value]) -> Judgement {
        match self.guard(values) {
            Err(d) => Judgement::stopped(d),
            Ok(false) => Judgement::Rejected,
            Ok(true) => match self.body(values) {
                Err(d) => Judgement::stopped(d),
                Ok(true) => Judgement::Held,
                Ok(false) => Judgement::Failed,
            },
        }
    }

    fn guard(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        for at in 0..self.guard_roots.len() {
            let root = self.guard_roots[at].clone();
            let value = self.enter(&root, values.to_vec())?;
            if !self.boolean(value)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        // A law's binders, or an owner's parameters then `result`: the order `source.rs` expects.
        let mut args = values.to_vec();
        if let (Some(name), Some(_)) = (self.call.clone(), &self.result) {
            args.push(self.enter(&name, values.to_vec())?);
        }
        let root = self.body_root.clone();
        let value = self.enter(&root, args)?;
        self.boolean(value)
    }
}

/// One run of a law over interleavings: the schedule it took, how its body ended, and whether it
/// entered a `simulate` region, without which there was no schedule to take.
pub struct Interleaved {
    pub interleaving: ply_eval::Interleaving,
    /// `None` when the body held.
    pub verdict: Option<Judgement>,
    pub observed: bool,
    /// What the run's entry ended with.
    pub warnings: Vec<Diagnostic>,
}

impl Interleaved {
    pub fn faulted(diagnostic: Diagnostic) -> Interleaved {
        Interleaved {
            interleaving: ply_eval::Interleaving::passed(Vec::new()),
            verdict: Some(Judgement::Faulted(diagnostic)),
            observed: false,
            warnings: Vec::new(),
        }
    }
}

fn body_was_not_boolean(value: &Value, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a law body came to `{}` rather than to a Boolean", ply_eval::slot(0)),
    )
    .showing(vec![ply_eval::Plain::shown(value)])
    .primary(span, "a law is a proposition, so its body is `Bool`")
    .note("the type checker rejects a non-`Bool` law body with E0201, so reaching this is a defect in Ply")
}

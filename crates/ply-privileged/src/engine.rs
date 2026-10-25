//! The runtime's half of a discharge: a claim's propositions entered at the points the program
//! names, and a law over interleavings run at the points its guard kept. What a claim's points
//! are, and what their judgements come to, is the program's.

use crate::load::Loaded;
use ply_core::decode::{AnswerValue, Error as DecodeError};
use ply_core::{
    Analysis, DEFAULT_MAX_CALLS, DefInfo, Diagnostic, LawInfo, Machine, Seed, SourceId, Span,
    Symbol, Value, codes,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// One claim the program owes, as far as the runtime enters it: whose it is, where it is written,
/// the guards it is judged after, and which machine its propositions run on. Everything else about
/// it is `proof.world`'s.
#[derive(Clone, Debug)]
pub struct Obligation {
    /// `<module>.<def>` for a clause, `<module>.<label>` for a law.
    pub owner: Symbol,
    pub kind: ObligationKind,
    pub span: Span,
    /// Whether the clause binds `result`, the owner's answer, which no point assigns.
    pub result: bool,
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
    /// Read off its owner's body and never entered: a `cost` or `space` bound, or the claim that
    /// what the body builds of a type stating an invariant satisfies it.
    Bound,
    Law,
}

/// Which machine a claim's propositions run on, as `proof.world` decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// A law over a `simulate` region: its interleavings are searched at each of its points.
    Interleave,
    /// The static prover first, then the claim's points.
    Static,
    /// A claim nothing runs: a `cost` or `space` bound, or what a body builds.
    Bounded,
}

/// The obligations a `proof.world.World` owes, in the order the program listed them, which is the
/// order it names them by.
pub fn obligations_of(world: AnswerValue<'_>) -> Result<Vec<Obligation>, DecodeError> {
    world.field("obligations")?.items(obligation_of)
}

fn obligation_of(at: AnswerValue<'_>) -> Result<Obligation, DecodeError> {
    let kind = at.field("kind")?.ctor()?;
    let span = |s: AnswerValue<'_>| -> Result<Span, DecodeError> {
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
            "Cost" | "Space" | "Builds" => ObligationKind::Bound,
            "Law" => ObligationKind::Law,
            _ => return Err(kind.unknown()),
        },
        span: span(at.field("at")?)?,
        result: at.field("result")?.option()?.is_some(),
        guards: at.field("guards")?.items(span)?,
        strategy: match strategy.name() {
            "Interleave" => Strategy::Interleave,
            "Static" => Strategy::Static,
            "Bounded" => Strategy::Bounded,
            _ => return Err(strategy.unknown()),
        },
    })
}

/// The prover this build drives over the program. One built once serves a whole run.
pub fn prover(loaded: &Loaded, backend: Arc<dyn ply_core::Provider>) -> Prover {
    Prover::new(loaded, backend)
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
    },
}

/// It owns what it judges, so a run can judge on whichever threads the program asks from.
pub struct Prover {
    front: Arc<Analysis>,
    /// Each law's place among the program's, by its key.
    laws: HashMap<Symbol, usize>,
    /// A compiled unit holding the laws' and clauses' roots, where those propositions are entered.
    backend: Arc<dyn ply_core::Provider>,
}

impl Prover {
    /// `backend` is the unit built from `loaded`, laws' and clauses' roots included.
    pub fn new(loaded: &Loaded, backend: Arc<dyn ply_core::Provider>) -> Prover {
        let laws = (loaded.front.check.laws.iter().enumerate())
            .map(|(at, law)| (law.key.clone(), at))
            .collect();
        Prover {
            front: Arc::clone(&loaded.front),
            laws,
            backend,
        }
    }

    /// The unit, attached once per thread: claims are judged on whichever threads the program asks from.
    fn compiled(&self) -> Rc<dyn ply_core::Compiled> {
        thread_local! {
            static ATTACHED: crate::support::Attached = const { RefCell::new(Vec::new()) };
        }
        ATTACHED.with(|attached| crate::support::attached_on(attached, &self.backend))
    }

    /// A proposition's body root: its program-wide name in the unit.
    fn body_root(&self, claim: &Claim<'_>) -> Symbol {
        match claim {
            Claim::Ensures { owner, index, .. } => owner.module.qualify(
                &ply_compiled::clause_root_name(&owner.simple_name, "ensures", *index),
            ),
            Claim::Law { info } => info.root("body"),
        }
    }

    /// Each guard's compiled root, in [`Obligation::guards`] order, which `source.rs` numbers
    /// alike.
    fn guard_roots(&self, obligation: &Obligation, claim: &Claim<'_>) -> Vec<Symbol> {
        match claim {
            Claim::Ensures { owner, .. } => (0..obligation.guards.len())
                .map(|k| {
                    owner.module.qualify(&ply_compiled::clause_root_name(
                        &owner.simple_name,
                        "requires",
                        k,
                    ))
                })
                .collect(),
            Claim::Law { info } => obligation
                .guards
                .iter()
                .map(|_| info.root("guard"))
                .collect(),
        }
    }

    fn claim(&self, obligation: &Obligation) -> Option<Claim<'_>> {
        match obligation.kind {
            ObligationKind::Ensures { index } => Some(Claim::Ensures {
                owner: self.front.check.defs.get(&obligation.owner)?,
                index,
            }),
            ObligationKind::Bound => None,
            ObligationKind::Law => {
                let info = self
                    .front
                    .check
                    .laws
                    .get(*self.laws.get(&obligation.owner)?)?;
                Some(Claim::Law { info })
            }
        }
    }

    /// What every entry a claim makes goes through, its owner's call included: this thread's
    /// backend, with no host bound.
    fn machine(&self) -> Result<Machine<'_>, Diagnostic> {
        Ok(Machine::new(&self.front, self.compiled())?.with_max_calls(DEFAULT_MAX_CALLS))
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
                || ply_core::Interleaving::passed(Vec::new()),
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
            Claim::Ensures { .. } if obligation.result => Some(obligation.owner.clone()),
            Claim::Ensures { .. } | Claim::Law { .. } => None,
        };
        Ok(Cases {
            machine: self.machine()?,
            guard_roots: self.guard_roots(obligation, &claim),
            body_root: self.body_root(&claim),
            span: obligation.span,
            call,
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

/// How a tuple of binder values is judged: guard first, always.
struct Cases<'a> {
    machine: Machine<'a>,
    guard_roots: Vec<Symbol>,
    body_root: Symbol,
    span: Span,
    /// The definition an `ensures` that binds `result` is attached to, called to produce it.
    call: Option<Symbol>,
    step_budget: i64,
    /// What every entry so far ended with.
    warnings: Vec<Diagnostic>,
}

impl Cases<'_> {
    /// One entry through the claim's machine, within the claim's budget, keeping what it ended
    /// with.
    fn enter(&mut self, root: &Symbol, args: Vec<Value>) -> Result<Value, Diagnostic> {
        let (machine, span, step_budget) = (&mut self.machine, self.span, self.step_budget);
        let (answer, warnings) = ply_compiled::rt::with_step_budget(step_budget, || {
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
                format!("a spec expression came to `{}` rather than to a Boolean", ply_core::slot(0)),
            )
            .primary(self.span, "a spec states a proposition, so its type is `Bool`")
            .note("the type checker rejects a non-`Bool` clause with E0201, so reaching this is a defect in Ply")
            .showing(vec![ply_core::Plain::shown(&other)])),
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
        if let Some(name) = self.call.clone() {
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
    pub interleaving: ply_core::Interleaving,
    /// `None` when the body held.
    pub verdict: Option<Judgement>,
    pub observed: bool,
    /// What the run's entry ended with.
    pub warnings: Vec<Diagnostic>,
}

impl Interleaved {
    pub fn faulted(diagnostic: Diagnostic) -> Interleaved {
        Interleaved {
            interleaving: ply_core::Interleaving::passed(Vec::new()),
            verdict: Some(Judgement::Faulted(diagnostic)),
            observed: false,
            warnings: Vec::new(),
        }
    }
}

fn body_was_not_boolean(value: &Value, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a law body came to `{}` rather than to a Boolean", ply_core::slot(0)),
    )
    .showing(vec![ply_core::Plain::shown(value)])
    .primary(span, "a law is a proposition, so its body is `Bool`")
    .note("the type checker rejects a non-`Bool` law body with E0201, so reaching this is a defect in Ply")
}

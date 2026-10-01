//! The runtime's half of a discharge: a claim's propositions entered at the points the program
//! drew, and a law over interleavings searched at the points its guard kept. What a claim's points
//! are, and what their judgements come to, is the program's.

use crate::load::Loaded;
use ply_eval::host::HostBinding;
use ply_eval::{
    CheckOutput, DEFAULT_MAX_CALLS, DefInfo, Diagnostic, Front, LawInfo, Machine, Seed, Span,
    Symbol, Value, codes,
};
use ply_prove::{Binder, Binding, Fault, Obligation, ObligationKind, ProvePlan, Strategy};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// The prover this build drives over the program. One built once serves a whole run.
pub fn prover<'a>(
    loaded: &'a Loaded,
    hosting: Option<Hosting>,
    backend: &'static dyn ply_eval::Provider,
) -> Prover<'a> {
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

pub struct Prover<'a> {
    check: &'a CheckOutput,
    front: &'a Front,
    laws: HashMap<Symbol, (usize, &'a LawInfo)>,
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

impl<'a> Prover<'a> {
    /// `backend` is the unit built from `loaded`, laws' and clauses' roots included.
    pub fn new(loaded: &'a Loaded, backend: &'static dyn ply_eval::Provider) -> Prover<'a> {
        let check = &loaded.check;
        let mut laws = HashMap::new();
        let mut ordinals: HashMap<&Symbol, usize> = HashMap::new();
        for law in &check.laws {
            let ordinal = ordinals.entry(law.module.as_symbol()).or_default();
            laws.insert(law.key.clone(), (*ordinal, law));
            *ordinal += 1;
        }
        Prover {
            check,
            front: &loaded.front,
            laws,
            hosting: None,
            backend,
        }
    }

    /// The unit, attached once per thread: obligations are discharged on pool threads.
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
    pub fn with_hosting(mut self, hosting: Hosting) -> Prover<'a> {
        self.hosting = Some(hosting);
        self
    }

    fn claim(&self, obligation: &Obligation) -> Option<Claim<'_>> {
        match obligation.kind {
            ObligationKind::Ensures { index } => Some(Claim::Ensures {
                owner: self.check.defs.get(&obligation.owner)?,
                index,
            }),
            ObligationKind::Law => {
                let &(ordinal, info) = self.laws.get(&obligation.owner)?;
                Some(Claim::Law { info, ordinal })
            }
        }
    }

    /// What an owner is called through to produce `result`: the tier its propositions are entered
    /// on, attached afresh.
    fn machine(&self) -> Result<Machine<'a>, Fault> {
        Machine::new(self.front, self.backend.attach())
            .map(|machine| machine.with_max_calls(DEFAULT_MAX_CALLS))
            .map_err(|refused| Fault {
                bindings: Vec::new(),
                diagnostic: Box::new(refused),
            })
    }

    /// The machine a `law/host`'s body runs on: the run's binding and a reactor for this thread.
    fn host_machine(&self, hosting: &Hosting) -> Result<Machine<'a>, Fault> {
        let mut machine = self.machine()?;
        machine.set_host_binding(Arc::clone(&hosting.binding));
        if let Some(factory) = &hosting.runtime {
            machine.set_host_runtime(Arc::clone(factory));
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
fn unclaimed(obligation: &Obligation) -> Fault {
    Fault {
        bindings: Vec::new(),
        diagnostic: Box::new(
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the prover holds no claim for `{}`", obligation.owner),
            )
            .primary(
                obligation.span,
                "this obligation was named, and no claim states it",
            )
            .note(
                "`proof.world` and the front end's claims are read from one program; this is \
                 Ply's fault",
            ),
        ),
    }
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

impl<'a> Prover<'a> {
    /// One claim's points, judged in order until one ends the batch.
    pub fn judged(
        &self,
        obligation: &Obligation,
        plan: &ProvePlan,
        points: &[Vec<Value>],
        mode: Mode,
    ) -> Vec<Judgement> {
        let Some(claim) = self.claim(obligation) else {
            return vec![Judgement::Faulted(*unclaimed(obligation).diagnostic)];
        };
        let mut cases = match self.cases(obligation, &claim, plan) {
            Ok(cases) => cases,
            Err(fault) => return vec![Judgement::Faulted(*fault.diagnostic)],
        };
        if let Strategy::Hosted = obligation.strategy {
            let Some(hosting) = &self.hosting else {
                return vec![Judgement::Faulted(unhosted(obligation))];
            };
            cases.machine = match self.host_machine(hosting) {
                Ok(machine) => machine,
                Err(fault) => return vec![Judgement::Faulted(*fault.diagnostic)],
            };
        }
        let mut out = Vec::with_capacity(points.len());
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
            out.push(judgement);
            if ends {
                break;
            }
        }
        out
    }

    /// A law whose body reaches a `simulate` region, run once at `values` under `seed`: the
    /// interleaving it took, how it ended, and whether it entered a region at all.
    pub fn interleaved(
        &self,
        obligation: &Obligation,
        plan: &ProvePlan,
        values: &[Value],
        seed: &Seed,
        steps: u32,
    ) -> Interleaved {
        let Some(claim) = self.claim(obligation) else {
            return Interleaved::faulted(*unclaimed(obligation).diagnostic);
        };
        let cases = match self.cases(obligation, &claim, plan) {
            Ok(cases) => cases,
            Err(fault) => return Interleaved::faulted(*fault.diagnostic),
        };
        let compiled = self.compiled();
        compiled.set_seed(seed.clone(), steps);
        let entered = ply_codegen::rt::with_step_budget(plan.step_budget, || {
            compiled.enter_whole(&cases.body_root, values, DEFAULT_MAX_CALLS)
        });
        let (value, record) = match entered {
            ply_eval::Entered::Answered(value) => (Ok(value), compiled.simulated()),
            ply_eval::Entered::Raised(raised) => (Err(raised), compiled.simulated()),
            ply_eval::Entered::Declined => (
                Err(ply_eval::err_not_compiled(
                    &cases.body_root,
                    obligation.span,
                )),
                None,
            ),
        };
        let judged = match value {
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
        Interleaved {
            interleaving: record.as_ref().map_or_else(
                || ply_eval::Interleaving::passed(Vec::new()),
                |r| r.interleaving(&outcome),
            ),
            verdict: judged,
            observed: record.is_some(),
        }
    }

    fn cases(
        &self,
        obligation: &Obligation,
        claim: &Claim<'_>,
        plan: &ProvePlan,
    ) -> Result<Cases<'a>, Fault> {
        let call = match claim {
            Claim::Ensures { .. } => Some(obligation.owner.clone()),
            Claim::Law { .. } => None,
        };
        Ok(Cases {
            machine: self.machine()?,
            compiled: self.compiled(),
            guard_roots: self.guard_roots(obligation, claim),
            body_root: self.body_root(claim),
            span: obligation.span,
            call,
            result: obligation.result.as_ref().map(|b| b.name.clone()),
            step_budget: plan.step_budget,
        })
    }
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
    compiled: Rc<dyn ply_eval::Compiled>,
    guard_roots: Vec<Symbol>,
    body_root: Symbol,
    span: Span,
    /// The definition an `ensures` is attached to, called to produce `result`.
    call: Option<Symbol>,
    result: Option<Symbol>,
    step_budget: i64,
}

impl Cases<'_> {
    /// The proposition entered on the tier, the only evaluator a proposition has.
    fn on_tier(&self, root: &Symbol, args: &[Value]) -> Result<Value, Diagnostic> {
        let entered = ply_codegen::rt::with_step_budget(self.step_budget, || {
            self.compiled.enter_whole(root, args, DEFAULT_MAX_CALLS)
        });
        match entered {
            ply_eval::Entered::Answered(value) => Ok(value),
            ply_eval::Entered::Raised(d) => Err(d),
            ply_eval::Entered::Declined => Err(ply_eval::err_not_compiled(root, self.span)),
        }
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
        for root in &self.guard_roots {
            let value = self.on_tier(root, values)?;
            if !self.boolean(value)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        // A law's binders, or an owner's parameters then `result`: the order `source.rs` expects.
        let mut args = values.to_vec();
        if let (Some(name), Some(_)) = (&self.call, &self.result) {
            let returned = self
                .machine
                .call(name.as_str(), values.to_vec(), self.span)?;
            args.push(returned);
        }
        let value = self.on_tier(&self.body_root, &args)?;
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
}

impl Interleaved {
    fn faulted(diagnostic: Diagnostic) -> Interleaved {
        Interleaved {
            interleaving: ply_eval::Interleaving::passed(Vec::new()),
            verdict: Some(Judgement::Faulted(diagnostic)),
            observed: false,
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

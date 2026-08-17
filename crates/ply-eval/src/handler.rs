//! `perform`, `handle`, `with_cell`, and the continuations that connect them.
//!
//! These are the transitions of ADR 0005 §1.3 that are not mechanical, factored
//! out of the machine's loop so that the rules they encode are stated once and
//! testable on their own. Each returns a [`Transition`]: the stack that results
//! and what the machine does next. The machine's own state maps onto [`State`]
//! one to one.
//!
//! Two rules carry the whole design and both are visible in [`perform`]:
//!
//! - **A clause body runs on the stack *below* its own handler.** [`Stack::capture`]
//!   cuts the handler's own segment away, so a clause that performs the
//!   operation it handles reaches the next handler out instead of catching
//!   itself forever.
//! - **State is threaded, never snapshotted.** Nothing here reads or writes the
//!   cell arena except [`open_cell`], which allocates. A resumption therefore
//!   observes the arena as of the handler's call to `resume`, which is what
//!   makes a cell-backed state handler writable.

use crate::arena::{Pin, RegionKind};
use crate::code::{Clause, Code, ReturnArm};
use crate::cont::{Continuation, Frame, Prompt, SimId, Stack, Target};
use crate::env::Env;
use crate::interp::arity_error;
use crate::task_regions::TaskRegions;
use crate::value::Value;
use ply_core::ty::{EffectAtom, Resource};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Mode;
use std::rc::Rc;

/// The machine's own state has these same three shapes plus `Halt`, which no
/// handler transition can produce.
pub enum State {
    Eval { code: Code, env: Env, module: usize },
    Return(Value),
    Perform(Request),
}

pub struct Request {
    /// The program-wide effect name, resolved where the `perform` was written.
    /// A clause names the same effect from its own module and the two only meet
    /// once both are qualified.
    pub effect: Symbol,
    pub op: Symbol,
    pub resource: Option<Symbol>,
    pub args: Vec<Value>,
    pub span: Span,
}

/// Inference rules every failure below out, so reaching one means the evaluator
/// was handed a module that was never checked; naming the mistake beats
/// guessing at it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpDecl {
    Declared {
        resource_param: bool,
        mode: Mode,
    },
    NoSuchOp,
    /// Nothing in view declares this effect. The stack still decides: a perform
    /// and the clause meant to handle it agree on the name either way.
    UnknownEffect,
}

/// The atom this `perform` contributes to the observed footprint. It must be
/// built exactly as inference builds the declared one, or an observed footprint
/// cannot be read against a declared row. An undeclared operation contributes
/// nothing: the perform that named it is already a diagnostic.
pub fn performed_atom(
    effect: &Symbol,
    resource: Option<&Symbol>,
    decl: OpDecl,
) -> Option<EffectAtom> {
    let OpDecl::Declared { mode, .. } = decl else {
        return None;
    };
    let resource = match resource {
        Some(r) => Resource::Named(r.clone()),
        None => Resource::Singleton,
    };
    Some(EffectAtom::new(effect.clone(), resource, mode))
}

pub struct Transition {
    pub stack: Stack,
    pub state: State,
}

/// Who answered a `perform`.
pub enum Answered {
    Handler(Transition),
    /// A `simulate` region's delimiter was reached first. The machine holds the
    /// scheduler, so [`perform`] does the search and the split and hands over
    /// the pieces rather than deciding anything about tasks.
    Scheduler(Scheduled),
    /// Nothing on the stack handles this. The host binding is the handler of
    /// **last resort** — consulted only here, after the whole stack has been
    /// walked innermost-first — so a `handle` or a `simulate` in scope shadows a
    /// real socket by the ordinary rule and with no special case. The machine
    /// holds the binding, so the request comes back untouched rather than being
    /// resolved here.
    Unhandled(Request),
}

/// A `task.*`, `clock.*` or `random.*` perform, split at its region's
/// delimiter.
pub struct Scheduled {
    pub region: SimId,
    pub effect: Symbol,
    pub op: Symbol,
    pub args: Vec<Value>,
    pub span: Span,
    /// The performing task's control, up to and including the region's
    /// delimiter. Resuming it reinstalls the delimiter, so the task's next
    /// perform finds the scheduler again.
    ///
    /// The stack it was cut from is not carried: it is the stack the region
    /// itself sits on, which the region already holds, and two copies of one
    /// stack is two things that can disagree.
    pub k: Continuation,
}

impl Transition {
    fn eval(stack: Stack, code: &Code, env: Env, module: usize) -> Transition {
        Transition {
            stack,
            state: State::Eval {
                code: code.clone(),
                env,
                module,
            },
        }
    }
}

/// `⟨Eval(handle b with H, ρ, m), K, W⟩ → ⟨Eval(b, ρ, m), K ◁ Prompt(H, ρ, m), W⟩`
///
/// `effects` is parallel to `clauses`: each clause's effect under its
/// program-wide name, which only the machine can resolve.
#[allow(clippy::too_many_arguments)]
pub fn enter_handle(
    stack: &Stack,
    body: &Code,
    clauses: &Rc<Vec<Clause>>,
    effects: Rc<Vec<Symbol>>,
    ret: Option<&Rc<ReturnArm>>,
    env: &Env,
    module: usize,
    span: Span,
) -> Transition {
    let prompt = Rc::new(Prompt {
        clauses: Rc::clone(clauses),
        effects,
        ret: ret.cloned(),
        env: env.clone(),
        module,
        span,
    });
    Transition::eval(stack.push_prompt(prompt), body, env.clone(), module)
}

/// `Next::Leave` — the delimited body finished.
///
/// A resumption reinstalls the prompt along with the rest of the captured
/// control, so this runs once per resumption rather than once per `handle`.
pub fn leave_handle(prompt: &Prompt, value: Value, below: Stack) -> Transition {
    match &prompt.ret {
        Some(arm) => Transition::eval(
            below,
            &arm.body,
            prompt.env.bind(arm.binder.clone(), value),
            prompt.module,
        ),
        None => Transition {
            stack: below,
            state: State::Return(value),
        },
    }
}

/// Drives a `perform`'s arguments left to right and then hands the machine the
/// [`Request`]. Entering a `perform` is this with `done` empty and `next` zero;
/// a `Frame::PerformArgs` firing is this with the value it received pushed onto
/// `done`.
#[allow(clippy::too_many_arguments)]
pub fn perform_args(
    stack: &Stack,
    effect: &Symbol,
    op: &Symbol,
    resource: &Option<Symbol>,
    done: Vec<Value>,
    args: &Rc<Vec<Code>>,
    next: usize,
    env: &Env,
    module: usize,
    span: Span,
) -> Transition {
    match args.get(next) {
        Some(arg) => {
            let stack = stack.push(Frame::PerformArgs {
                effect: effect.clone(),
                op: op.clone(),
                resource: resource.clone(),
                done,
                args: Rc::clone(args),
                next: next + 1,
                env: crate::rc::carry(env, next + 1 < args.len()),
                module,
                span,
            });
            Transition::eval(stack, arg, env.clone(), module)
        }
        None => Transition {
            stack: stack.clone(),
            state: State::Perform(Request {
                effect: effect.clone(),
                op: op.clone(),
                resource: resource.clone(),
                args: done,
                span,
            }),
        },
    }
}

/// `⟨Perform(e, op, r, v̄, σ), K, W⟩` — search, split, dispatch.
///
/// A tail-resumptive clause gets a [`Frame::Resume`] pushed for it, which is
/// the whole of `op(x̄) -> e` being `op(x̄) resume κ -> κ(e)`.
///
/// The arena is not a parameter, and capture and resumption still do not read
/// or write it. `pin` is the one thing it hands over: the claim a capture makes
/// on every region open at it, taken by the caller — which is the only party
/// holding an arena — and carried by whatever continuation this cuts out, so a
/// region's lexical close can tell "no one can reach this" from "a continuation
/// still can".
///
/// `born` is the machine's at-most-once host-operation count, stamped onto
/// every continuation this captures. It is what the linearity rule compares
/// against later.
pub fn perform(
    stack: &Stack,
    request: Request,
    decl: OpDecl,
    born: u64,
    pin: &mut dyn FnMut() -> Option<Pin>,
) -> Result<Answered, Diagnostic> {
    let Request {
        effect,
        op,
        resource,
        args,
        span,
    } = request;
    check_operation(decl, &effect, &op, resource.is_some(), span)?;

    let Some(found) = stack.find_handler(&effect, &op, resource.as_ref()) else {
        return Ok(Answered::Unhandled(Request {
            effect,
            op,
            resource,
            args,
            span,
        }));
    };
    let (prompt, clause_at) = match found.target {
        Target::Ply { prompt, clause } => (prompt, clause),
        Target::Sim(region) => {
            let (k, _region_stack) = stack.capture(found.segments, born);
            // The scheduler holds this until it resumes the task, which may be
            // after every region open here has closed.
            let k = k.pinned(pin());
            return Ok(Answered::Scheduler(Scheduled {
                region,
                effect,
                op,
                args,
                span,
                k,
            }));
        }
    };
    let clause = &prompt.clauses[clause_at];
    if clause.params.len() != args.len() {
        return Err(arity_error(
            span,
            &format!("the handler clause for `{effect}.{op}`"),
            clause.params.len(),
            args.len(),
        ));
    }

    let (k, below) = stack.capture(found.segments, born);
    let mut scope = prompt.env.clone();
    for (p, v) in clause.params.iter().zip(args) {
        scope = scope.bind(p.clone(), v);
    }

    let stack = match &clause.resume {
        Some(binder) => {
            // A named continuation may be stored, returned or resumed after the
            // regions open here have closed, so it claims them.
            scope = scope.bind(
                binder.clone(),
                Value::Continuation(Rc::new(k.pinned(pin()))),
            );
            below
        }
        // A tail-resumptive clause takes no pin, and that is a claim about the
        // stack rather than a saving: the only thing that will ever splice this
        // continuation is the `Resume` frame pushed here, one frame above the
        // `CloseRegion` frames of every region open at the capture. It is
        // consumed before any of them runs, and it is reachable from nothing
        // else — there is no binder for a clause body to store it through.
        //
        // Where the clause's own body performs an operation answered further
        // out, the capture that answers *that* takes the segments this frame
        // sits in, so a claim on these regions is made there if one is needed.
        //
        // It matters: `perform` is on the request path, a pin is an `Rc`, and
        // tail-resumptive is what essentially every handler in the standard
        // library is.
        None => below.pushed(Frame::Resume { k: Rc::new(k) }),
    };
    Ok(Answered::Handler(Transition::eval(
        stack,
        &clause.body,
        scope,
        prompt.module,
    )))
}

/// A resumption refused because replaying the control would replay an
/// irreversible host operation. Carries the ordinal so the diagnostic can say
/// which resumption it is; the machine holds the operation being protected and
/// builds `E0426` from both.
pub struct Replayed {
    pub resumes: u32,
}

/// `Frame::Resume(k)` — hand a value to a captured continuation.
///
/// The segments splice onto whatever stack is current, which may be a different
/// stack from the one they were cut out of and may already carry a previous
/// resumption's leftovers. Each captured segment carries its own prompt, so the
/// handler is reinstalled by the act of resuming: handlers are deep.
///
/// **Every** resumption in the language goes through here, which is why the
/// linearity check is a parameter of this function rather than something its
/// callers remember to do. A second resumption across a host operation is the
/// one defect in this system that is silent and sends a packet twice, so it may
/// not be reachable by adding a call site.
pub fn resume(
    stack: &Stack,
    k: &Continuation,
    value: Value,
    host_ops: u64,
) -> Result<Transition, Replayed> {
    k.admit(host_ops).map_err(|resumes| Replayed { resumes })?;
    Ok(Transition {
        stack: stack.resume(k),
        state: State::Return(value),
    })
}

/// Applying a `Value::Continuation`. It takes exactly one argument — the value
/// the `perform` it was captured at should have produced.
pub fn continuation_argument(mut args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    if args.len() != 1 {
        return Err(arity_error(span, "a continuation", 1, args.len()));
    }
    Ok(args.pop().expect("a one-argument call has an argument"))
}

/// `⟨Eval(with_cell[r](i){x→b}, ρ, m), K, W⟩ → ⟨Eval(i, ρ, m), K·WithCellBody, W⟩`
#[allow(clippy::too_many_arguments)]
pub fn enter_with_cell(
    stack: &Stack,
    resource: &Symbol,
    binder: &Symbol,
    init: &Code,
    body: &Code,
    env: &Env,
    module: usize,
    region: Span,
) -> Transition {
    let stack = stack.push(Frame::WithCellBody {
        resource: resource.clone(),
        binder: binder.clone(),
        body: body.clone(),
        env: env.clone(),
        module,
        region,
    });
    Transition::eval(stack, init, env.clone(), module)
}

/// `Frame::WithCellBody`.
///
/// `kind` is what [`crate::region_kind::Regions`] decided about this span, and
/// `None` says the span opens no region of its own — either the inference never
/// saw it, or it is a `with_cell[r]` nested in a `with_region[r]` that already
/// opened `r`. Opening nothing is the safe answer to both: the cell lands in the
/// enclosing region and nothing reclaims it before that region's close.
///
/// When a region does open, a [`Frame::CloseRegion`] goes under the body. What
/// that close does with the slots is decided by the pins a capture took, not by
/// `kind` — see [`TaskRegions::close_region`].
#[allow(clippy::too_many_arguments)]
pub fn open_cell(
    cells: &mut TaskRegions,
    binder: &Symbol,
    body: &Code,
    env: &Env,
    module: usize,
    initial: Value,
    stack: Stack,
    kind: Option<RegionKind>,
    region_span: Span,
) -> Result<Transition, Diagnostic> {
    let stack = match kind {
        Some(kind) => {
            let region = cells.open_region(kind, region_span);
            stack.push(Frame::CloseRegion { region })
        }
        None => stack,
    };
    let Some(cell) = cells.alloc(initial) else {
        return Err(err_cells_exhausted(body.span));
    };
    Ok(Transition::eval(
        stack,
        body,
        env.bind(binder.clone(), Value::Cell(cell)),
        module,
    ))
}

/// `⟨Eval(with_region[r]{b}, ρ, m), K, W⟩` — the region with no cell in it.
///
/// `None` for `kind` is the inference never having seen this span, and then the
/// body runs in the enclosing region exactly as it did before this construct
/// opened anything.
pub fn enter_with_region(
    cells: &mut TaskRegions,
    stack: &Stack,
    body: &Code,
    env: &Env,
    module: usize,
    kind: Option<RegionKind>,
    region_span: Span,
) -> Transition {
    let stack = match kind {
        Some(kind) => {
            let region = cells.open_region(kind, region_span);
            stack.push(Frame::CloseRegion { region })
        }
        None => stack.clone(),
    };
    Transition::eval(stack, body, env.clone(), module)
}

/// The declaration-level checks a `perform` makes before it looks at the stack.
/// [`perform`] calls this; it is public so a machine that wants to report the
/// mistake earlier can.
pub fn check_operation(
    decl: OpDecl,
    effect: &Symbol,
    op: &Symbol,
    has_resource: bool,
    span: Span,
) -> Result<(), Diagnostic> {
    match decl {
        OpDecl::Declared {
            resource_param: true,
            ..
        } if !has_resource => Err(Diagnostic::error(
            codes::RESOURCE_REQUIRED,
            format!("`{effect}.{op}` is resource-parameterized and needs a `[resource]`"),
        )
        .primary(span, "missing resource label")),
        OpDecl::Declared { .. } | OpDecl::UnknownEffect => Ok(()),
        OpDecl::NoSuchOp => Err(Diagnostic::error(
            codes::UNKNOWN_OPERATION,
            format!("effect `{effect}` has no operation `{op}`"),
        )
        .primary(span, "unknown operation")),
    }
}

#[cold]
#[inline(never)]
pub(crate) fn err_cells_exhausted(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        "this run has allocated every cell an arena can hold",
    )
    .primary(span, "this region has no slot left to allocate")
    .note("nothing reclaims a cell within a run, so a `with_cell` in a hot loop retains one entry per iteration")
    .note("hoist the region out of the loop, or reuse one cell across the iterations")
}

/// `E0303`, and deliberately not `E0424`: this one means inference should have
/// prevented the perform and did not, so it is a bug-catcher. A run that
/// reaches the boundary with a host handler registered for the operation is a
/// correctly-typed program in a hermetic run, which is the opposite situation
/// and calls for the opposite response.
#[cold]
#[inline(never)]
pub fn err_unhandled(
    span: Span,
    effect: &Symbol,
    op: &Symbol,
    resource: Option<&Symbol>,
) -> Diagnostic {
    let label = match resource {
        Some(r) => format!("{effect}.{op}[{r}]"),
        None => format!("{effect}.{op}"),
    };
    Diagnostic::error(codes::UNHANDLED_EFFECT, format!("no handler for `{label}`"))
        .primary(span, "performed here with no enclosing handler")
        .note("wrap this in a `handle ... with { ... }` that names the operation")
}

#[cfg(test)]
mod tests;

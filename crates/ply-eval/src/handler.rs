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
//! - **The world is threaded, never snapshotted.** Nothing here reads or writes
//!   a [`World`] except [`open_cell`], which allocates. A resumption therefore
//!   observes the world as of the handler's call to `resume`, which is what
//!   makes a cell-backed state handler writable.

use crate::code::{Clause, Code, ReturnArm};
use crate::cont::{Continuation, Frame, Prompt, SimId, Stack, Target};
use crate::env::Env;
use crate::interp::arity_error;
use crate::value::Value;
use crate::world::World;
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
                env: env.clone(),
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
/// `W` is not a parameter. Capture and resumption do not touch the world.
pub fn perform(stack: &Stack, request: Request, decl: OpDecl) -> Result<Answered, Diagnostic> {
    let Request {
        effect,
        op,
        resource,
        args,
        span,
    } = request;
    check_operation(decl, &effect, &op, resource.is_some(), span)?;

    let Some(found) = stack.find_handler(&effect, &op, resource.as_ref()) else {
        return Err(err_unhandled(span, &effect, &op, resource.as_ref()));
    };
    let (prompt, clause_at) = match found.target {
        Target::Ply { prompt, clause } => (prompt, clause),
        Target::Sim(region) => {
            let (k, _region_stack) = stack.capture(found.segments);
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

    let (k, below) = stack.capture(found.segments);
    let mut scope = prompt.env.clone();
    for (p, v) in clause.params.iter().zip(args) {
        scope = scope.bind(p.clone(), v);
    }

    let stack = match &clause.resume {
        Some(binder) => {
            scope = scope.bind(binder.clone(), Value::Continuation(Rc::new(k)));
            below
        }
        None => below.pushed(Frame::Resume { k: Rc::new(k) }),
    };
    Ok(Answered::Handler(Transition::eval(
        stack,
        &clause.body,
        scope,
        prompt.module,
    )))
}

/// `Frame::Resume(k)` — hand a value to a captured continuation.
///
/// The segments splice onto whatever stack is current, which may be a different
/// stack from the one they were cut out of and may already carry a previous
/// resumption's leftovers. Each captured segment carries its own prompt, so the
/// handler is reinstalled by the act of resuming: handlers are deep.
pub fn resume(stack: &Stack, k: &Continuation, value: Value) -> Transition {
    Transition {
        stack: stack.resume(k),
        state: State::Return(value),
    }
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
) -> Transition {
    let stack = stack.push(Frame::WithCellBody {
        resource: resource.clone(),
        binder: binder.clone(),
        body: body.clone(),
        env: env.clone(),
        module,
    });
    Transition::eval(stack, init, env.clone(), module)
}

/// `Frame::WithCellBody`.
///
/// The region does nothing on the way out. The world is monotone, which is what
/// lets a continuation captured inside the region be resumed after it returned
/// and still read the cell instead of finding a hole.
pub fn open_cell(
    world: &mut World,
    binder: &Symbol,
    body: &Code,
    env: &Env,
    module: usize,
    initial: Value,
    stack: Stack,
) -> Result<Transition, Diagnostic> {
    let Some(cell) = world.try_alloc(initial) else {
        return Err(err_cells_exhausted(body.span));
    };
    Ok(Transition::eval(
        stack,
        body,
        env.bind(binder.clone(), Value::Cell(cell)),
        module,
    ))
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
fn err_cells_exhausted(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        "this run has allocated every cell a world can hold",
    )
    .primary(span, "this region has no id left to allocate")
    .note("nothing reclaims a cell within a run, so a `with_cell` in a hot loop retains one entry per iteration")
    .note("hoist the region out of the loop, or reuse one cell across the iterations")
}

#[cold]
#[inline(never)]
fn err_unhandled(
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

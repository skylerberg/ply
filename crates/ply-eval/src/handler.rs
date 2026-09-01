//! `perform`, `handle`, `with_cell`, and the continuations that connect them.

use crate::arena::{Pin, RegionKind};
use crate::code::{Clause, Code, ReturnArm};
use crate::cont::{Continuation, Frame, Prompt, SimId, Stack, Target};
use crate::env::Env;
use crate::semantics::arity_error;
use crate::task_regions::TaskRegions;
use crate::value::Value;
use ply_core::ty::{EffectAtom, Resource};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Mode;
use std::rc::Rc;

/// The machine's own state has these same three shapes plus `Halt`, which no handler transition can
/// produce.
pub enum State {
    Eval { code: Code, env: Env, module: usize },
    Return(Value),
    Perform(Request),
}

pub struct Request {
    /// The program-wide effect name, resolved where the `perform` was written.
    pub effect: Symbol,
    pub op: Symbol,
    pub resource: Option<Symbol>,
    pub args: Vec<Value>,
    pub span: Span,
}

/// Inference rules every failure below out, so reaching one means the evaluator was handed a module
/// that was never checked; naming the mistake beats guessing at it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpDecl {
    Declared {
        resource_param: bool,
        mode: Mode,
    },
    NoSuchOp,
    /// Nothing in view declares this effect.
    UnknownEffect,
}

/// The atom this `perform` contributes to the observed footprint.
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
    /// A `simulate` region's delimiter was reached first.
    Scheduler(Scheduled),
    /// Nothing on the stack handles this.
    Unhandled(Request),
}

/// A `task.*`, `clock.*` or `random.*` perform, split at its region's delimiter.
pub struct Scheduled {
    pub region: SimId,
    pub effect: Symbol,
    pub op: Symbol,
    pub args: Vec<Value>,
    pub span: Span,
    /// The performing task's control, up to and including the region's delimiter.
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

/// `⟨Eval(handle b with H, ρ, m), K, W⟩ → ⟨Eval(b, ρ, m), K ◁ Prompt(H, ρ, m), W⟩`.
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
pub fn leave_handle(prompt: &Prompt, value: Value, below: Stack) -> Transition {
    match &prompt.ret {
        Some(arm) => Transition::eval(
            below,
            &arm.body,
            match &arm.free {
                Some(free) if crate::rc::probe_carries() => prompt.env.keep_only(free),
                _ => prompt.env.clone(),
            }
            .bind(arm.binder.clone(), value),
            prompt.module,
        ),
        None => Transition {
            stack: below,
            state: State::Return(value),
        },
    }
}

/// Drives a `perform`'s arguments left to right and then hands the machine the [`Request`].
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
            // The scheduler holds this until it resumes the task, which may be after every region
            // open here has closed.
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
    // The clause's own frame: what its body reads from the handler's scope, and nothing else.
    let mut scope = match &clause.free {
        Some(free) if crate::rc::probe_carries() => prompt.env.keep_only(free),
        _ => prompt.env.clone(),
    };
    for (p, v) in clause.params.iter().zip(args) {
        scope = scope.bind(p.clone(), v);
    }

    let stack = match &clause.resume {
        Some(binder) => {
            // A named continuation may be stored, returned or resumed after the regions open here
            // have closed, so it claims them.
            scope = scope.bind(
                binder.clone(),
                Value::Continuation(Rc::new(k.pinned(pin()))),
            );
            below
        }
        // A tail-resumptive clause takes no pin, and that is a claim about the stack rather than a
        // saving: the only thing that will ever splice this continuation is the `Resume` frame
        // pushed here, one frame above the `CloseRegion` frames of every region open at the
        // capture.
        None => below.pushed(Frame::Resume { k: Rc::new(k) }),
    };
    Ok(Answered::Handler(Transition::eval(
        stack,
        &clause.body,
        scope,
        prompt.module,
    )))
}

/// A resumption refused because replaying the control would replay an irreversible host operation.
pub struct Replayed {
    pub resumes: u32,
}

/// `Frame::Resume(k)` — hand a value to a captured continuation.
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

/// Applying a `Value::Continuation`.
pub fn continuation_argument(mut args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    if args.len() != 1 {
        return Err(arity_error(span, "a continuation", 1, args.len()));
    }
    Ok(args.pop().expect("a one-argument call has an argument"))
}

/// `⟨Eval(with_cell[r](i){x→b}, ρ, m), K, W⟩ → ⟨Eval(i, ρ, m), K·WithCellBody, W⟩`.
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

/// `E0303`, and deliberately not `E0424`: this one means inference should have prevented the
/// perform and did not, so it is a bug-catcher.
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

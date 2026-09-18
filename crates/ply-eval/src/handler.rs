//! `perform`, `handle`, `with_cell`, and the continuations that connect them.

use crate::arena::{Pin, RegionKind};
use crate::code::{Clause, Code};
use crate::cont::{Continuation, Extent, Frame, Prompt, Stack, Target};
use crate::semantics::arity_error;
use crate::task_regions::TaskRegions;
use crate::value::Value;
use crate::window::Windows;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_ty::Mode;
use ply_ty::{EffectAtom, Resource};
use std::rc::Rc;

/// The machine's states minus `Halt`, which no handler transition produces.
pub enum State {
    Eval { code: Code, module: usize },
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

/// Inference rules out every failure here; reaching one means the module was never checked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpDecl {
    Declared { resource_param: bool, mode: Mode },
    NoSuchOp,
    UnknownEffect,
}

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

pub enum Answered {
    Handler(Transition),
    /// A `simulate` region's delimiter was reached first.
    Scheduler(Scheduled),
    Unhandled(Request),
}

/// A `task.*`, `clock.*` or `random.*` perform, split at its region's delimiter.
pub struct Scheduled {
    pub region: crate::cont::SimId,
    pub effect: Symbol,
    pub op: Symbol,
    pub args: Vec<Value>,
    pub span: Span,
    /// The performing task's control, up to and including the region's delimiter.
    pub k: Continuation,
}

impl Transition {
    fn eval(stack: Stack, code: &Code, module: usize) -> Transition {
        Transition {
            stack,
            state: State::Eval {
                code: code.clone(),
                module,
            },
        }
    }
}

/// `window` is the current window's size; a capture at this prompt subtracts it to find its floor.
pub fn enter_handle(
    stack: &Stack,
    body: &Code,
    prompt: Rc<Prompt>,
    module: usize,
    window: u32,
) -> Transition {
    Transition::eval(stack.push_prompt(prompt, window), body, module)
}

#[allow(clippy::too_many_arguments)]
pub fn perform_args(
    stack: &Stack,
    effect: &Symbol,
    op: &Symbol,
    resource: &Option<Symbol>,
    done: Vec<Value>,
    args: &Rc<Vec<Code>>,
    next: usize,
    module: usize,
    span: Span,
) -> Transition {
    match args.get(next) {
        Some(arg) => {
            crate::rc::note_carry();
            let stack = stack.push(Frame::PerformArgs {
                effect: effect.clone(),
                op: op.clone(),
                resource: resource.clone(),
                done,
                args: Rc::clone(args),
                next: next + 1,
                module,
                span,
            });
            Transition::eval(stack, arg, module)
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

/// Snapshots the slots from the capturing activation's floor to the top into `k`.
pub(crate) fn seal(k: Continuation, windows: &mut Windows) -> Continuation {
    let t = windows.len();
    let entry = t - k.cut_deltas();
    let floor = entry - k.cut_window();
    let saved = windows.cut(floor, entry);
    let base_offset = (t - windows.base) as u32;
    k.with_extent(
        Extent::Saved {
            slots: Rc::new(saved),
        },
        base_offset,
    )
}

fn open_clause(windows: &mut Windows, clause: &Clause, captured: &[Value], args: Vec<Value>) {
    let base = windows.enter(clause.size);
    windows.base = base;
    for (j, dst) in clause.captures.dst.iter().enumerate() {
        windows.write(*dst, captured[j].clone());
    }
    for (i, v) in args.into_iter().enumerate() {
        windows.write(i as u32, v);
    }
}

pub fn perform(
    stack: &Stack,
    windows: &mut Windows,
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
            // The scheduler may resume this after every region open here has closed.
            let k = seal(k, windows).pinned(pin());
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
    let t = windows.len();
    let entry = t - k.cut_deltas();
    let floor = entry - k.cut_window();

    let stack = match &clause.resume {
        Some(_) => {
            // A named continuation can outlive both the regions open here and the slots under it.
            let k = seal(k, windows).pinned(pin());
            debug_assert_eq!(windows.len(), entry);
            let below = below.pushed(Frame::Exit {
                callee_window: clause.size,
                caller_window: (entry - floor) as u32,
            });
            open_clause(windows, clause, &prompt.clause_captures[clause_at], args);
            windows.write(clause.params.len() as u32, Value::Continuation(Rc::new(k)));
            below
        }
        // Tail-resumptive: only the `Resume` frame pushed here splices `k`, and until then only the
        // clause's activation runs, above the extent, so neither a pin nor a snapshot is needed.
        None => {
            let k = k.with_extent(Extent::InPlace, (t - windows.base) as u32);
            let below = below
                .pushed(Frame::Resume { k: Rc::new(k) })
                .pushed(Frame::Exit {
                    callee_window: clause.size,
                    caller_window: (t - floor) as u32,
                });
            open_clause(windows, clause, &prompt.clause_captures[clause_at], args);
            below
        }
    };
    Ok(Answered::Handler(Transition::eval(
        stack,
        &clause.body,
        prompt.module,
    )))
}

/// A resumption refused because replaying the control would replay an irreversible host operation.
pub struct Replayed {
    pub resumes: u32,
}

/// Applying a `Value::Continuation`.
pub fn continuation_argument(mut args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    if args.len() != 1 {
        return Err(arity_error(span, "a continuation", 1, args.len()));
    }
    let value = args.pop().expect("a one-argument call has an argument");
    crate::argv::give(args);
    Ok(value)
}

#[allow(clippy::too_many_arguments)]
pub fn enter_with_cell(
    stack: &Stack,
    resource: &Symbol,
    binder: &Symbol,
    slot: Option<u32>,
    init: &Code,
    body: &Code,
    module: usize,
    region: Span,
) -> Transition {
    let stack = stack.push(Frame::WithCellBody {
        resource: resource.clone(),
        binder: binder.clone(),
        slot,
        body: body.clone(),
        module,
        region,
    });
    Transition::eval(stack, init, module)
}

#[allow(clippy::too_many_arguments)]
pub fn open_cell(
    cells: &mut TaskRegions,
    windows: &mut Windows,
    binder_slot: Option<u32>,
    body: &Code,
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
    if let Some(slot) = binder_slot {
        windows.write(slot, Value::Cell(cell));
    }
    Ok(Transition::eval(stack, body, module))
}

pub fn enter_with_region(
    cells: &mut TaskRegions,
    stack: &Stack,
    body: &Code,
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
    Transition::eval(stack, body, module)
}

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

/// Deliberately not `E0424`: inference should have prevented this perform, so it is a bug-catcher.
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

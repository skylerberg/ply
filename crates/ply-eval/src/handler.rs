//! `perform`, `handle`, `with_cell`, and the continuations that connect them.

use crate::arena::{Pin, RegionKind};
use crate::code::{Clause, Code};
use crate::cont::{Continuation, Extent, Frame, Prompt, Stack, Target};
use crate::semantics::arity_error;
use crate::task_regions::TaskRegions;
use crate::value::Value;
use crate::window::Windows;
use ply_core::ty::{EffectAtom, Resource};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Mode;
use std::rc::Rc;

/// The machine's own state has these same three shapes plus `Halt`, which no handler transition can
/// produce.
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

/// `⟨Eval(handle b with H, ρ, m), K, W⟩ → ⟨Eval(b, m), K ◁ Prompt(H), W⟩`. The prompt already
/// carries the clause captures the machine copied out of the current window; `window` is that
/// window's size, which a capture at this prompt subtracts to find its snapshot's floor.
pub fn enter_handle(
    stack: &Stack,
    body: &Code,
    prompt: Rc<Prompt>,
    module: usize,
    window: u32,
) -> Transition {
    Transition::eval(stack.push_prompt(prompt, window), body, module)
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

/// Seals a capture's slot snapshot onto the continuation: everything from the floor of the
/// activation that pushed the captured prompt up to the top. The extent above the prompt's push
/// height is moved out of the stack; the portion below it — shared with the activation continuing
/// under the capture — is cloned.
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

/// Opens a clause's activation window on top of the slot stack and fills it: captures from the
/// prompt, then the operation's arguments into the parameter slots.
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

/// `⟨Perform(e, op, r, v̄, σ), K, W⟩` — search, split, dispatch.
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
            // The scheduler holds this until it resumes the task, which may be after every region
            // open here has closed.
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
            // A named continuation may be stored, returned or resumed after the regions open here
            // have closed, so it claims them — and it snapshots the windows it was captured over,
            // because the machine's own slots keep moving under whoever holds it.
            let k = seal(k, windows).pinned(pin());
            debug_assert_eq!(windows.len(), entry);
            let below = below.pushed(Frame::Exit {
                callee_window: clause.size,
                caller_window: (entry - floor) as u32,
            });
            open_clause(windows, clause, &prompt.clause_captures[clause_at], args);
            windows.write(
                clause.params.len() as u32,
                Value::Continuation(Rc::new(k)),
            );
            below
        }
        // A tail-resumptive clause takes no pin, and no snapshot either. Both are claims about
        // the stack rather than savings: the only thing that will ever splice this continuation
        // is the `Resume` frame pushed here, and nothing runs between this capture and that
        // splice except the clause's own activation, above the extent — so the extent's windows
        // stay exactly where they are. This is what keeps a plain perform free of slot traffic,
        // which is the hot path of every effect operation.
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
    Ok(args.pop().expect("a one-argument call has an argument"))
}

/// `⟨Eval(with_cell[r](i){x→b}, ρ, m), K, W⟩ → ⟨Eval(i, m), K·WithCellBody, W⟩`.
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

/// `Frame::WithCellBody`.
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

/// `⟨Eval(with_region[r]{b}, ρ, m), K, W⟩` — the region with no cell in it.
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

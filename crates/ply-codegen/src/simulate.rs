//! `simulate` in the compiled tier: a frame the runtime serves, driving the scheduler over stacks.
//!
//! The body runs as the root task on a stack of its own, and every task the region spawns gets
//! one. A `perform` of an operation the region answers — `task`, `clock`, `random` — hands the
//! request to the scheduler by switching back to the stack that called [`rt_simulate`], where
//! the loop below applies it, asks the scheduler who runs next, and switches to that task. The
//! scheduler is the machine's, instantiated over a saved stack pointer where the machine has a
//! continuation, so a plan chooses the same interleaving on both sides.

use crate::heap::{self, Word};
use crate::rt::{Ctx, FAILED_UNWIND, call_value, values_taken};
use crate::stack::{Stack, switch};
use ply_eval::sched::{Resumption, Scheduler, Turn};
use ply_eval::sim::{Access, Answer, Handlers, OpSignature, TaskId, signature};
use ply_eval::{SimId, Value};
use ply_span::{Diagnostic, Span, Symbol, codes};

pub struct Simulation {
    sched: Scheduler<usize, Word>,
    handlers: Handlers,
    /// Indexed by [`TaskId`], alongside the scheduler's tasks.
    tasks: Vec<TaskStack>,
    /// The stack [`rt_simulate`] was called on, which the loop runs on.
    scheduler_sp: usize,
    running: Option<TaskId>,
    request: Option<Request>,
    /// What the task being switched to gets back from the `perform` it stopped at.
    answer: Word,
    /// The stack the region was entered on, whose frames every task's chain to.
    stack: usize,
    floor_below: usize,
    /// The closure the task being started runs, read by its entry on the new stack.
    starting: Option<Word>,
    root: Word,
}

#[derive(Default)]
struct TaskStack {
    stack: Option<Stack>,
    frames: usize,
    sp: usize,
}

enum Request {
    Spawn(Word),
    Join(TaskId),
    Yield,
    Seeded(&'static OpSignature, Vec<Value>),
    Finished(Word),
    Failed,
}

impl Simulation {
    pub fn new(
        id: SimId,
        root_seed: u64,
        drawn: u64,
        steps: u32,
        stack: usize,
        floor_below: usize,
        root: Word,
    ) -> Simulation {
        Simulation {
            sched: Scheduler::new(id, Span::DUMMY).with_step_budget(steps),
            handlers: Handlers::at(root_seed, drawn),
            tasks: vec![TaskStack::default()],
            scheduler_sp: 0,
            running: None,
            request: None,
            answer: 0,
            stack,
            floor_below,
            starting: None,
            root,
        }
    }
}

/// The region's loop, on the stack that entered it. Returns the body's answer, or zero with the
/// context failed.
pub unsafe fn run(ctx: *mut Ctx) -> Word {
    loop {
        let c = unsafe { &mut *ctx };
        let sim = c.sims.last_mut().expect("a region is running");
        let turn = sim.sched.next(sim.handlers.clock_mut(), &mut c.trail);
        let (task, resumption) = match turn {
            Err(d) => return c.fail(d),
            Ok(Turn::Complete(value)) => {
                c.trail
                    .leave(sim.handlers.clock().now(), sim.handlers.rand().drawn());
                return c.word(&value);
            }
            Ok(Turn::Run { task, resumption }) => (task, resumption),
        };
        let at = task.0 as usize;
        let root = sim.root;
        let sp = match resumption {
            Resumption::Enter => unsafe { start(ctx, at, root) },
            Resumption::Start { body, .. } => unsafe { start(ctx, at, body) },
            Resumption::Resume { k, value } => {
                let w = c.word(&value);
                let sim = c.sims.last_mut().expect("a region is running");
                sim.answer = w;
                k
            }
        };
        let c = unsafe { &mut *ctx };
        let sim = c.sims.last_mut().expect("a region is running");
        let slot = &mut sim.tasks[at];
        c.stack_floor = slot.stack.as_ref().expect("a task has a stack").floor();
        c.current = slot.frames;
        sim.running = Some(task);
        let from = &mut sim.scheduler_sp as *mut usize;
        unsafe { switch(&mut *from, sp) };

        let c = unsafe { &mut *ctx };
        let sim = c.sims.last_mut().expect("a region is running");
        sim.running = None;
        c.current = sim.stack;
        c.stack_floor = sim.floor_below;
        if sim.sched.records_steps() {
            c.trail.end_step(Span::DUMMY);
        }
        let k = sim.tasks[at].sp;
        let applied = match sim.request.take() {
            Some(Request::Spawn(closure)) => {
                let id = sim.sched.spawn(closure, Span::DUMMY, None);
                sim.tasks.push(TaskStack::default());
                sim.sched.suspend(k, Value::Task(id))
            }
            Some(Request::Join(target)) => sim.sched.join(k, target, Span::DUMMY),
            Some(Request::Yield) => sim.sched.suspend(k, Value::Unit),
            Some(Request::Seeded(sig, args)) => {
                match sim.handlers.dispatch(sig, task, &args, Span::DUMMY) {
                    Ok(Answer::Value(value)) => {
                        if let Some(access) = sig.step_access() {
                            c.trail.record_access(access);
                        }
                        sim.sched.suspend(k, value)
                    }
                    Ok(Answer::Sleeping { deadline }) => {
                        sim.sched.sleep_until(k, deadline, Span::DUMMY)
                    }
                    Err(d) => Err(d),
                }
            }
            Some(Request::Finished(word)) => {
                sim.tasks[at].stack = None;
                let value = c.value(word);
                heap::dec(word);
                let sim = c.sims.last_mut().expect("a region is running");
                sim.sched.finish(value)
            }
            Some(Request::Failed) => {
                sim.tasks[at].stack = None;
                if c.failed == FAILED_UNWIND {
                    return 0;
                }
                let failure = c.diagnostic.take().unwrap_or_else(|| {
                    Diagnostic::error(codes::INTERNAL_ERROR, "a task failed without a diagnostic")
                });
                let seed = c.trail.seed().clone();
                let sim = c.sims.last_mut().expect("a region is running");
                let failure = sim.sched.fail(failure, &seed);
                c.failed = 0;
                return c.fail(failure);
            }
            None => Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "a task gave control back to the scheduler without a request",
            )),
        };
        if let Err(d) = applied {
            return c.fail(d);
        }
    }
}

unsafe fn start(ctx: *mut Ctx, at: usize, closure: Word) -> usize {
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    let stack = Stack::new();
    let sp = stack.prepare(task_entry, ctx as usize);
    let parent = sim.stack;
    let frames = c.open_stack(Some(parent));
    let sim = c.sims.last_mut().expect("a region is running");
    sim.tasks[at] = TaskStack {
        stack: Some(stack),
        frames,
        sp,
    };
    sim.starting = Some(closure);
    sp
}

extern "C" fn task_entry(arg: usize) {
    let ctx = arg as *mut Ctx;
    let (closure, at) = {
        let c = unsafe { &mut *ctx };
        let sim = c.sims.last_mut().expect("a region is running");
        (
            sim.starting.take().expect("a starting task has a body"),
            sim.running.expect("a starting task is running").0 as usize,
        )
    };
    let r = call_value(ctx, closure, &[]);
    heap::dec(closure);
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    sim.request = Some(if c.failed != 0 {
        Request::Failed
    } else {
        Request::Finished(r)
    });
    let from = &mut sim.tasks[at].sp as *mut usize;
    let to = sim.scheduler_sp;
    unsafe { switch(&mut *from, to) };
    std::process::abort();
}

/// A `perform` of an operation the region answers, from the task that performed it: hands the
/// request to the loop and comes back with the answer once the scheduler runs this task again.
pub unsafe fn perform(ctx: *mut Ctx, effect: &Symbol, op: &Symbol, args: &[Word]) -> Word {
    let c = unsafe { &mut *ctx };
    let request = match (effect.as_str(), op.as_str()) {
        ("task", "spawn") => Request::Spawn(args[0]),
        ("task", "join") => {
            let handle = c.value(args[0]);
            heap::dec(args[0]);
            match handle.as_task(Span::DUMMY, "`task.join`") {
                Ok(target) => Request::Join(target),
                Err(d) => return c.fail(d),
            }
        }
        ("task", "yield") => Request::Yield,
        _ => {
            let Some(sig) = signature(effect.as_str(), op.as_str()) else {
                let d = Diagnostic::error(
                    codes::UNKNOWN_OPERATION,
                    format!("`{effect}` has no operation `{op}`"),
                )
                .note("a `simulate` region answers `task`, `clock` and `random`");
                return c.fail(d);
            };
            Request::Seeded(sig, values_taken(c, args))
        }
    };
    let sim = c.sims.last_mut().expect("a region is running");
    let Some(task) = sim.running else {
        let d = Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("`{effect}.{op}` was performed while no task was running"),
        );
        return c.fail(d);
    };
    sim.request = Some(request);
    let from = &mut sim.tasks[task.0 as usize].sp as *mut usize;
    let to = sim.scheduler_sp;
    unsafe { switch(&mut *from, to) };
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    std::mem::take(&mut sim.answer)
}

/// The access a cell builtin makes, for the running step's footprint.
pub fn cell_access(ctx: &Ctx, b: ply_eval::Builtin, args: &[Word]) -> Option<Access> {
    use ply_eval::Builtin;
    use ply_syntax::ast::Mode;
    let mode = match b {
        Builtin::CellGet => Mode::Read,
        Builtin::CellSet | Builtin::CellUpdate => Mode::Write,
        _ => return None,
    };
    let Value::Cell(slot) = ctx.value(*args.first()?) else {
        return None;
    };
    Some(Access::Cell { id: slot, mode })
}

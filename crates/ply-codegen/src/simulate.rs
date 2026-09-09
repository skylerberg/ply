//! A region in the compiled tier: `simulate`, a frame the runtime serves, and the production
//! region the host policy opens; both drive the scheduler over stacks.
//!
//! The body runs as the root task on a stack of its own, and every task the region spawns gets
//! one. A `perform` of an operation the region answers — `task`, `clock`, `random` — hands the
//! request to the scheduler by switching back to the stack that called [`rt_simulate`], where
//! the loop below applies it, asks the scheduler who runs next, and switches to that task. The
//! scheduler is the machine's, instantiated over a saved stack pointer where the machine has a
//! continuation, so a plan chooses the same interleaving on both sides.
//!
//! A production region is opened by a `task` operation outside any `simulate` when the binding
//! permits one: the stack that performed it becomes the root task, and the loop runs on a stack
//! of its own, scheduling against the host runtime. The root finishes when its entry returns to
//! the backend, which then lets the loop drain the other tasks before answering.

use crate::heap::{self, Word};
use crate::rt::{Ctx, FAILED_UNWIND, call_value, drop_frame, inherit_frames, values_taken};
use crate::stack::{Stack, switch};
use ply_eval::Unbound;
use ply_eval::host::Pending;
use ply_eval::sched::{HostPolicy, Policy, ROOT, Resumption, Scheduler, Turn};
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
    /// What the task named asked when it last gave control back, until the loop applies it.
    request: Option<(TaskId, Request)>,
    /// What the task being switched to gets back from the `perform` it stopped at.
    answer: Word,
    /// The stack the region was entered on, whose frames every task's chain to.
    stack: usize,
    floor_below: usize,
    /// The closure the task being started runs, read by its entry on the new stack.
    starting: Option<Word>,
    root: Word,
    policy: Policy,
    /// A production region's loop runs here rather than on the stack that opened the region,
    /// since that stack is the root task.
    loop_stack: Option<Stack>,
    /// The loop returned, with the region's answer or its failure in place; nothing may switch
    /// into its stack again.
    loop_done: bool,
    /// Where the region was opened, for a diagnostic that names it.
    pub(crate) site: Span,
}

#[derive(Default)]
struct TaskStack {
    /// Owned by a spawned task; a production region's root runs on the stack that opened it.
    stack: Option<Stack>,
    frames: usize,
    floor: usize,
    sp: usize,
    /// The stack its spawn was performed on. The frames from there up to the region's
    /// boundary are copied under the task's own when it starts: a task performs against the
    /// handlers around its spawn.
    inherits: Option<usize>,
}

enum Request {
    /// The body, and the stack the spawn was performed on: the handlers around it are the
    /// task's.
    Spawn(Word, usize),
    Join(TaskId),
    Yield,
    Seeded(&'static OpSignature, Vec<Value>),
    Park(Pending),
    Finished(Word),
    Failed,
}

impl Simulation {
    /// A seeded region over `sched`, opened at `site`.
    pub fn new(
        sched: Scheduler<usize, Word>,
        site: Span,
        root_seed: u64,
        drawn: u64,
        stack: usize,
        floor_below: usize,
        root: Word,
    ) -> Simulation {
        Simulation {
            sched,
            site,
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
            policy: Policy::Seeded,
            loop_stack: None,
            loop_done: false,
        }
    }

    pub fn is_production(&self) -> bool {
        self.policy == Policy::Host
    }
}

/// Opens the production region a `task` operation outside any `simulate` asks for, with the
/// performer's stack as the root task. `false` with the context failed when the binding permits
/// none.
pub unsafe fn open_production(ctx: *mut Ctx, effect: &Symbol, op: &Symbol) -> bool {
    let c = unsafe { &mut *ctx };
    let Some(permit) = HostPolicy::of(&c.binding) else {
        let operation = ply_eval::host::operation_label(effect, op, None);
        let path = c
            .binding
            .would_serve(effect, op, None)
            .unwrap_or("ply_host::sched::spawn");
        c.fail(ply_eval::host::err_hermetic(c.site(), &operation, path));
        return false;
    };
    let id = SimId(c.entered_sims);
    c.entered_sims += 1;
    let sched = match Scheduler::production(id, c.site(), permit).rooted_running() {
        Ok(sched) => sched,
        Err(d) => {
            c.fail(d);
            return false;
        }
    };
    let loop_stack = Stack::new();
    let scheduler_sp = loop_stack.prepare(loop_entry, ctx as usize);
    let loop_frames = c.open_stack(None);
    let root = TaskStack {
        stack: None,
        frames: c.current,
        floor: c.stack_floor,
        sp: 0,
        inherits: None,
    };
    let site = c.site();
    c.sims.push(Simulation {
        sched,
        handlers: Handlers::at(0, 0),
        tasks: vec![root],
        scheduler_sp,
        running: Some(ROOT),
        request: None,
        answer: 0,
        stack: loop_frames,
        floor_below: loop_stack.floor(),
        starting: None,
        root: 0,
        policy: Policy::Host,
        loop_stack: Some(loop_stack),
        loop_done: false,
        site,
    });
    true
}

/// A production region's loop, on its own stack: runs the scheduler until the region completes
/// and hands the answer back to the root, which is waiting in the backend.
extern "C" fn loop_entry(arg: usize) {
    let ctx = arg as *mut Ctx;
    let r = unsafe { run(ctx) };
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    sim.answer = r;
    sim.loop_done = true;
    let to = sim.tasks[ROOT.0 as usize].sp;
    let from = &mut sim.scheduler_sp as *mut usize;
    unsafe { switch(&mut *from, to) };
    std::process::abort();
}

/// From the backend, once a production region's root entry has returned with `value`: the root
/// is finished, the loop drains the other tasks, and the region's answer comes back.
pub unsafe fn finish_root(ctx: *mut Ctx, value: Word) -> Word {
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    if sim.loop_done {
        ended_under_root(c);
        return value;
    }
    sim.request = Some((
        ROOT,
        if c.failed != 0 {
            Request::Failed
        } else {
            Request::Finished(value)
        },
    ));
    let to = sim.scheduler_sp;
    let from = &mut sim.tasks[ROOT.0 as usize].sp as *mut usize;
    unsafe { switch(&mut *from, to) };
    let c = unsafe { &mut *ctx };
    let sim = c.sims.pop().expect("the region that just completed");
    c.current = sim.tasks[ROOT.0 as usize].frames;
    c.stack_floor = sim.tasks[ROOT.0 as usize].floor;
    drop(sim.loop_stack);
    sim.answer
}

/// The region's loop: on the stack that entered a `simulate`, on its own stack for a production
/// region. Returns the body's answer, or zero with the context failed. A request left by the
/// task that gave control back, the root's opening `spawn` included, is applied before the
/// scheduler is asked.
/// The production loop ended -- on a failure -- while the root was suspended in it. The loop
/// resumes the root directly, so the root takes the loop's answer back on its own stack here.
fn ended_under_root(c: &mut Ctx) -> Word {
    let sim = c.sims.pop().expect("the region that just ended");
    c.current = sim.tasks[ROOT.0 as usize].frames;
    c.stack_floor = sim.tasks[ROOT.0 as usize].floor;
    sim.answer
}

/// A task that owned its stack is done with it and with the frames it held: the handlers it
/// inherited, and any it failed under. The production root runs on the stack that opened the
/// region and keeps its frames.
fn release(c: &mut Ctx, at: usize) {
    let sim = c.sims.last_mut().expect("a region is running");
    if sim.tasks[at].stack.take().is_none() {
        return;
    }
    let frames = sim.tasks[at].frames;
    for f in std::mem::take(&mut c.stacks[frames].list) {
        drop_frame(f);
    }
}

pub unsafe fn run(ctx: *mut Ctx) -> Word {
    loop {
        let c = unsafe { &mut *ctx };
        let sim = c.sims.last_mut().expect("a region is running");
        if let Some((task, request)) = sim.request.take() {
            match unsafe { apply(ctx, task, request) } {
                Ok(()) => {}
                Err(Some(d)) => return c.fail(d),
                Err(None) => return 0,
            }
        }
        let c = unsafe { &mut *ctx };
        let runtime = c.runtime.clone();
        let sim = c.sims.last_mut().expect("a region is running");
        let turn = match sim.policy {
            Policy::Seeded => sim.sched.next(sim.handlers.clock_mut(), &mut c.trail),
            Policy::Host => match &runtime {
                Some(rt) => sim.sched.next_host(rt.as_ref()),
                None => sim.sched.next_host(&Unbound),
            },
        };
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
        c.stack_floor = slot.floor;
        c.current = slot.frames;
        sim.running = Some(task);
        let from = &mut sim.scheduler_sp as *mut usize;
        unsafe { switch(&mut *from, sp) };

        let c = unsafe { &mut *ctx };
        let site = c.site();
        let sim = c.sims.last_mut().expect("a region is running");
        sim.running = None;
        c.current = sim.stack;
        c.stack_floor = sim.floor_below;
        if sim.sched.records_steps() {
            c.trail.end_step(site);
        }
    }
}

/// Applies what `task` asked when it gave control back. `Err(None)` when the context is already
/// failed with what the loop should return.
unsafe fn apply(ctx: *mut Ctx, task: TaskId, request: Request) -> Result<(), Option<Diagnostic>> {
    let c = unsafe { &mut *ctx };
    let site = c.site();
    let sim = c.sims.last_mut().expect("a region is running");
    let at = task.0 as usize;
    let k = sim.tasks[at].sp;
    let applied = match request {
        Request::Spawn(closure, from) => {
            let id = sim.sched.spawn(closure, site, None);
            sim.tasks.push(TaskStack {
                inherits: Some(from),
                ..TaskStack::default()
            });
            sim.sched.suspend(k, Value::Task(id))
        }
        Request::Join(target) => sim.sched.join(k, target, site),
        Request::Yield => sim.sched.suspend(k, Value::Unit),
        Request::Park(pending) => sim.sched.park_on_host(k, pending, site),
        Request::Seeded(sig, args) => match sim.handlers.dispatch(sig, task, &args, site) {
            Ok(Answer::Value(value)) => {
                if let Some(access) = sig.step_access() {
                    c.trail.record_access(access);
                }
                sim.sched.suspend(k, value)
            }
            Ok(Answer::Sleeping { deadline }) => sim.sched.sleep_until(k, deadline, site),
            Err(d) => Err(d),
        },
        Request::Finished(word) => {
            release(c, at);
            let value = c.value(word);
            heap::dec(word);
            let sim = c.sims.last_mut().expect("a region is running");
            sim.sched.finish(value)
        }
        Request::Failed => {
            release(c, at);
            if c.failed == FAILED_UNWIND {
                return Err(None);
            }
            let failure = c.diagnostic.take().unwrap_or_else(|| {
                Diagnostic::error(codes::INTERNAL_ERROR, "a task failed without a diagnostic")
            });
            let seed = c.trail.seed().clone();
            let sim = c.sims.last_mut().expect("a region is running");
            let failure = sim.sched.fail(failure, &seed);
            c.failed = 0;
            return Err(Some(failure));
        }
    };
    applied.map_err(Some)
}

unsafe fn start(ctx: *mut Ctx, at: usize, closure: Word) -> usize {
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    let stack = Stack::new();
    let sp = stack.prepare(task_entry, ctx as usize);
    let (parent, inherits) = (sim.stack, sim.tasks[at].inherits);
    let frames = c.open_stack(Some(parent));
    if let Some(from) = inherits {
        let mut chain = Vec::new();
        let mut s = from;
        loop {
            chain.push(s);
            match c.stacks[s].parent {
                Some(p) if p != parent => s = p,
                _ => break,
            }
        }
        let mut list = Vec::new();
        for s in chain.into_iter().rev() {
            list.extend(inherit_frames(&c.stacks[s].list));
        }
        c.stacks[frames].list = list;
    }
    let sim = c.sims.last_mut().expect("a region is running");
    let floor = stack.floor();
    sim.tasks[at] = TaskStack {
        stack: Some(stack),
        frames,
        floor,
        sp,
        inherits,
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
    let me = TaskId(at as u32);
    sim.request = Some((
        me,
        if c.failed != 0 {
            Request::Failed
        } else {
            Request::Finished(r)
        },
    ));
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
        ("task", "spawn") => Request::Spawn(args[0], c.current),
        ("task", "join") => {
            let handle = c.value(args[0]);
            heap::dec(args[0]);
            match handle.as_task(c.site(), "`task.join`") {
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
    sim.request = Some((task, request));
    let from = &mut sim.tasks[task.0 as usize].sp as *mut usize;
    let to = sim.scheduler_sp;
    unsafe { switch(&mut *from, to) };
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    if sim.loop_done {
        return ended_under_root(c);
    }
    std::mem::take(&mut sim.answer)
}

/// From a task of a production region: parks it on the host's pending answer and comes back
/// with the value the runtime resolved it to.
pub unsafe fn park(ctx: *mut Ctx, pending: Pending) -> Word {
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    let Some(task) = sim.running else {
        let d = Diagnostic::error(
            codes::INTERNAL_ERROR,
            "a pending host answer arrived while no task was running",
        );
        return c.fail(d);
    };
    sim.request = Some((task, Request::Park(pending)));
    let from = &mut sim.tasks[task.0 as usize].sp as *mut usize;
    let to = sim.scheduler_sp;
    unsafe { switch(&mut *from, to) };
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    if sim.loop_done {
        return ended_under_root(c);
    }
    std::mem::take(&mut sim.answer)
}

/// Whether the innermost region is a production one with a task running, so that a host
/// answer parks rather than blocks, and the request names the task.
pub fn running_task_of_production(c: &Ctx) -> Option<TaskId> {
    let sim = c.sims.last()?;
    if sim.policy != Policy::Host {
        return None;
    }
    sim.running
}

pub fn innermost_is_seeded(c: &Ctx) -> bool {
    c.sims
        .last()
        .is_some_and(|sim| sim.policy == Policy::Seeded)
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

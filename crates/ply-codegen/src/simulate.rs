//! `simulate` and production regions in the compiled tier: each task runs on its own stack and
//! switches back to the machine's scheduler loop to perform `task`, `clock` or `random`.

use crate::heap::{self, Word};
use crate::rt::{
    Ctx, FAILED_CANCELLED, FAILED_UNWIND, Frames, HandlerFrame, call_value, drop_frame,
    inherit_frames, values_taken,
};
use crate::stack::{Stack, switch};
use ply_eval::host::Pending;
use ply_eval::sched::{HostPolicy, Policy, ROOT, Resumption, Scheduler, TaskHandle, Turn};
use ply_eval::sim::{Access, Answer, Handlers, OpSignature, TaskId, liveness, signature};
use ply_eval::{Diagnostic, Mode, SimId, Span, Symbol, Unbound, Value, codes};
use std::collections::BTreeMap;

pub struct Simulation {
    sched: Scheduler<usize, Word>,
    handlers: Handlers,
    /// Alongside the scheduler's tasks; a production region drops a task's once it finishes.
    tasks: BTreeMap<TaskId, TaskStack>,
    /// The stack [`rt_simulate`] was called on, which the loop runs on.
    scheduler_sp: usize,
    running: Option<TaskId>,
    request: Option<(TaskId, Request)>,
    answer: Word,
    /// The stack the region was entered on, which every task's frames chain to.
    stack: usize,
    floor_below: usize,
    starting: Option<Word>,
    root: Word,
    policy: Policy,
    /// A production region's loop stack; the opening stack is the root task.
    loop_stack: Option<Stack>,
    /// The loop returned; nothing may switch into its stack again.
    loop_done: bool,
    pub(crate) site: Span,
}

#[derive(Default)]
struct TaskStack {
    /// Owned by a spawned task; a production region's root runs on the stack that opened it.
    stack: Option<Stack>,
    frames: usize,
    floor: usize,
    sp: usize,
    /// Copies of the handlers around its spawn as they stood then, held until the task starts.
    inherited: Vec<HandlerFrame>,
}

enum Request {
    /// The body, and the stack the spawn was performed on.
    Spawn(Word, usize),
    Join(TaskHandle),
    Await(TaskHandle),
    Cancel(TaskHandle),
    Yield,
    Seeded(&'static OpSignature, Vec<Value>),
    Park(Pending),
    Finished(Word),
    Failed,
    /// The task unwound after a cancel.
    Cancelled,
}

impl Simulation {
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
            tasks: BTreeMap::from([(ROOT, TaskStack::default())]),
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

    /// The tasks whose stacks this region still holds.
    pub fn task_slots(&self) -> usize {
        self.tasks.len()
    }

    /// The tasks the scheduler still keeps.
    pub fn scheduled(&self) -> usize {
        self.sched.tasks()
    }

    fn slot(&mut self, task: TaskId) -> &mut TaskStack {
        self.tasks
            .get_mut(&task)
            .expect("a task the scheduler hands out has its stack")
    }
}

/// Opens a production region for a `task` operation outside `simulate`, the performer as root.
/// `false`, with the context failed, when the binding permits none.
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
    let sched = match Scheduler::production(id, c.site(), permit, c.id).rooted_running() {
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
        inherited: Vec::new(),
    };
    let site = c.site();
    c.sims.push(Simulation {
        sched,
        handlers: Handlers::at(0, 0),
        tasks: BTreeMap::from([(ROOT, root)]),
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

/// A production region's loop: runs the scheduler, then hands the answer to the waiting root.
extern "C" fn loop_entry(arg: usize) {
    let ctx = arg as *mut Ctx;
    let r = unsafe { run(ctx) };
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    sim.answer = r;
    sim.loop_done = true;
    let to = sim.slot(ROOT).sp;
    let from = &mut sim.scheduler_sp as *mut usize;
    unsafe { switch(&mut *from, to) };
    std::process::abort();
}

/// Once a production root's entry has returned: the loop drains the other tasks and answers.
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
    let from = &mut sim.slot(ROOT).sp as *mut usize;
    unsafe { switch(&mut *from, to) };
    let c = unsafe { &mut *ctx };
    let mut sim = end(c);
    let root = sim.slot(ROOT);
    c.current = root.frames;
    c.stack_floor = root.floor;
    drop(sim.loop_stack);
    sim.answer
}

/// The production loop failed while the root was suspended in it, and resumed the root directly.
fn ended_under_root(c: &mut Ctx) -> Word {
    let mut sim = end(c);
    let root = sim.slot(ROOT);
    c.current = root.frames;
    c.stack_floor = root.floor;
    sim.answer
}

/// Drops a task's stack, frames and the regions a failure or an unwind left open on it, or the
/// frames it holds unstarted; the production root runs on the opening stack and keeps all three.
fn release(c: &mut Ctx, task: TaskId) {
    let sim = c.sims.last_mut().expect("a region is running");
    let slot = sim.slot(task);
    for f in std::mem::take(&mut slot.inherited) {
        drop_frame(f);
    }
    if slot.stack.take().is_none() {
        return;
    }
    let frames = slot.frames;
    for f in std::mem::take(&mut c.stacks[frames].list) {
        drop_frame(f);
    }
    c.release_regions(frames);
}

/// A finished production task's slot and stack index go back, since nothing names a dead stack.
fn recycle(c: &mut Ctx, task: TaskId) {
    let sim = c.sims.last_mut().expect("a region is running");
    if sim.policy != Policy::Host || task == ROOT {
        return;
    }
    if let Some(slot) = sim.tasks.remove(&task) {
        c.close_stack(slot.frames);
    }
}

/// Pops the innermost region, releasing each task it never finished and each body it never started.
pub(crate) fn end(c: &mut Ctx) -> Simulation {
    let sim = c.sims.last_mut().expect("a region is running");
    for body in sim.sched.unstarted() {
        heap::dec(body);
    }
    let tasks: Vec<TaskId> = sim.tasks.keys().copied().collect();
    for task in tasks {
        release(c, task);
    }
    c.sims.pop().expect("a region is running")
}

/// The region's loop. Returns the body's answer, or zero with the context failed.
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
        let root = sim.root;
        let sp = match resumption {
            Resumption::Enter => unsafe { start(ctx, task, root) },
            Resumption::Start { body, .. } => unsafe { start(ctx, task, body) },
            Resumption::Resume { k, value } => {
                let w = c.word(&value);
                let sim = c.sims.last_mut().expect("a region is running");
                sim.answer = w;
                k
            }
            // The task's perform answers nothing and finds the context failed, so every frame
            // returns, releasing what it holds, back to its entry.
            Resumption::Cancel { k } => {
                c.failed = FAILED_CANCELLED;
                k
            }
            Resumption::Raise { k, failure } => {
                c.fail(failure);
                k
            }
        };
        let c = unsafe { &mut *ctx };
        let sim = c.sims.last_mut().expect("a region is running");
        let slot = sim.slot(task);
        c.stack_floor = slot.floor;
        c.current = slot.frames;
        sim.running = Some(task);
        let from = &mut sim.scheduler_sp as *mut usize;
        unsafe { switch(&mut *from, sp) };

        let c = unsafe { &mut *ctx };
        let yielded = c.step_site();
        let sim = c.sims.last_mut().expect("a region is running");
        sim.running = None;
        c.current = sim.stack;
        c.stack_floor = sim.floor_below;
        if sim.sched.records_steps() {
            c.trail.end_step(yielded);
        }
    }
}

/// Applies what `task` asked when it gave control back. `Err(None)`: the context is already failed.
unsafe fn apply(ctx: *mut Ctx, task: TaskId, request: Request) -> Result<(), Option<Diagnostic>> {
    let c = unsafe { &mut *ctx };
    let site = c.site();
    let runtime = c.runtime.clone();
    let sim = c.sims.last_mut().expect("a region is running");
    let k = sim.slot(task).sp;
    let applied = match request {
        Request::Spawn(closure, from) => {
            let inherited = handlers_around(&c.stacks, from, sim.stack);
            let handle = sim.sched.spawn(closure, site);
            sim.tasks.insert(
                handle.id(),
                TaskStack {
                    inherited,
                    ..TaskStack::default()
                },
            );
            sim.sched.suspend(k, Value::Task(handle))
        }
        Request::Join(target) => sim.sched.join(k, &target, site),
        Request::Await(target) => sim.sched.await_task(k, &target, site),
        Request::Cancel(target) => {
            let clock = match sim.policy {
                Policy::Seeded => Some(sim.handlers.clock_mut()),
                Policy::Host => None,
            };
            match sim.sched.cancel(k, &target, site, clock) {
                Err(d) => Err(d),
                Ok(unstarted) => {
                    let region = target.region();
                    let id = target.id();
                    let records = sim.sched.records_steps();
                    if let Some(body) = unstarted {
                        heap::dec(body);
                        release(c, id);
                        recycle(c, id);
                    }
                    // A cancel writes the task's liveness and each step it took reads it, so the
                    // search tries cancelling earlier and later.
                    if records {
                        c.record_access(liveness(id, Mode::Write));
                        c.trail.mark_steps_of(region, id, liveness(id, Mode::Read));
                    }
                    Ok(())
                }
            }
        }
        Request::Yield => sim.sched.suspend(k, Value::Unit),
        Request::Park(pending) => match &runtime {
            Some(rt) => sim.sched.park_on_host(k, pending, site, rt.as_ref()),
            None => sim.sched.park_on_host(k, pending, site, &Unbound),
        },
        Request::Seeded(sig, args) => match sim.handlers.dispatch(sig, task, &args, site) {
            Ok(Answer::Value(value)) => sim.sched.suspend(k, value),
            Ok(Answer::Sleeping { deadline }) => sim.sched.sleep_until(k, deadline, site),
            Err(d) => Err(d),
        },
        Request::Finished(word) => {
            release(c, task);
            recycle(c, task);
            let value = c.value(word);
            heap::dec(word);
            let sim = c.sims.last_mut().expect("a region is running");
            sim.sched.finish(value)
        }
        Request::Cancelled => {
            release(c, task);
            recycle(c, task);
            c.failed = 0;
            let sim = c.sims.last_mut().expect("a region is running");
            sim.sched.finish_cancelled()
        }
        Request::Failed => {
            release(c, task);
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

/// Copies of the handlers around a spawn on `from`, outermost first, short of the region's own.
fn handlers_around(stacks: &[Frames], from: usize, region: usize) -> Vec<HandlerFrame> {
    let mut chain = vec![from];
    let mut s = from;
    while let Some(p) = stacks[s].parent
        && p != region
    {
        chain.push(p);
        s = p;
    }
    chain
        .into_iter()
        .rev()
        .flat_map(|s| inherit_frames(&stacks[s].list))
        .collect()
}

unsafe fn start(ctx: *mut Ctx, task: TaskId, closure: Word) -> usize {
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    let stack = Stack::new();
    let sp = stack.prepare(task_entry, ctx as usize);
    let parent = sim.stack;
    let entered_from = match sim.policy {
        Policy::Host => sim.slot(ROOT).frames,
        Policy::Seeded => sim.stack,
    };
    let inherited = std::mem::take(&mut sim.slot(task).inherited);
    let frames = c.open_stack(Some(parent));
    c.stacks[frames].list = inherited;
    c.stacks[frames].entered_from = Some(entered_from);
    let sim = c.sims.last_mut().expect("a region is running");
    let floor = stack.floor();
    *sim.slot(task) = TaskStack {
        stack: Some(stack),
        frames,
        floor,
        sp,
        inherited: Vec::new(),
    };
    sim.starting = Some(closure);
    sp
}

extern "C" fn task_entry(arg: usize) {
    let ctx = arg as *mut Ctx;
    let (closure, me) = {
        let c = unsafe { &mut *ctx };
        let sim = c.sims.last_mut().expect("a region is running");
        (
            sim.starting.take().expect("a starting task has a body"),
            sim.running.expect("a starting task is running"),
        )
    };
    let r = call_value(ctx, closure, &[]);
    heap::dec(closure);
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    sim.request = Some((
        me,
        if c.failed == FAILED_CANCELLED {
            Request::Cancelled
        } else if c.failed != 0 {
            Request::Failed
        } else {
            Request::Finished(r)
        },
    ));
    let from = &mut sim.slot(me).sp as *mut usize;
    let to = sim.scheduler_sp;
    unsafe { switch(&mut *from, to) };
    std::process::abort();
}

/// A `perform` the region answers: switch to the loop, and return once the task runs again.
pub unsafe fn perform(ctx: *mut Ctx, effect: &Symbol, op: &Symbol, args: &[Word]) -> Word {
    let c = unsafe { &mut *ctx };
    let request = match (effect.as_str(), op.as_str()) {
        ("task", "spawn") => Request::Spawn(args[0], c.current),
        ("task", "join") => {
            let handle = c.value(args[0]);
            heap::dec(args[0]);
            match handle.as_task(c.site(), "`task.join`") {
                Ok(target) => Request::Join(target.clone()),
                Err(d) => return c.fail(d),
            }
        }
        ("task", "await") | ("task", "cancel") => {
            let handle = c.value(args[0]);
            heap::dec(args[0]);
            let what = if op.as_str() == "await" {
                "`task.await`"
            } else {
                "`task.cancel`"
            };
            match handle.as_task(c.site(), what) {
                Ok(target) if op.as_str() == "await" => Request::Await(target.clone()),
                Ok(target) => Request::Cancel(target.clone()),
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
    let from = &mut sim.slot(task).sp as *mut usize;
    let to = sim.scheduler_sp;
    unsafe { switch(&mut *from, to) };
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    if sim.loop_done {
        return ended_under_root(c);
    }
    std::mem::take(&mut sim.answer)
}

/// Parks a production task on the host's pending answer, returning its resolved value.
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
    let from = &mut sim.slot(task).sp as *mut usize;
    let to = sim.scheduler_sp;
    unsafe { switch(&mut *from, to) };
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    if sim.loop_done {
        return ended_under_root(c);
    }
    std::mem::take(&mut sim.answer)
}

/// The running task of an innermost production region, so a host answer parks rather than blocks.
pub fn running_task_of_production(c: &Ctx) -> Option<TaskId> {
    let sim = c.sims.last()?;
    if sim.policy != Policy::Host {
        return None;
    }
    sim.running
}

/// The innermost region's site when that region is seeded, so a refusal can name it.
pub fn seeded_region(c: &Ctx) -> Option<Span> {
    let sim = c.sims.last()?;
    (sim.policy == Policy::Seeded).then_some(sim.site)
}

pub fn cell_access(ctx: &Ctx, b: ply_eval::Builtin, args: &[Word]) -> Option<Access> {
    use ply_eval::{Builtin, Mode};
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

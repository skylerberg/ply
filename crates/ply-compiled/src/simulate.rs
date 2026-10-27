//! Production regions in the C backend: a `task` operation outside `simulate` opens one, the
//! performer as its root, and each other task runs on its own stack and switches back to the
//! region's loop to perform `task` or sleep. What the tasks pass each other is read as values or
//! kept with their types; emitted code's arguments cross in through its operation's types, and
//! what it is answered is written as PLV1 and lifted through them.

use crate::emitted::{Block, Carried, Handed, PlyOp, PlyType};
use crate::heap::Word;
use crate::rt::{
    Beside, Ctx, FAILED_ABORT, FAILED_CANCELLED, FAILED_PENDING, FAILED_TARGET, FAILED_UNWIND,
    Frames, HandlerFrame, Lifted, drop_frame, err_crossing, inherit_frames, lifted_from,
    lowered_values, lowering_of, no_stack_to_take, raise_error, released, running, switch_keeping,
};
use crate::stack::{Stack, switch};
use ply_core::codec::{self, Reader, Writer, tag};
use ply_core::host::Pending;
use ply_core::sched::{
    ChanHandle, HostPolicy, ROOT, Reply, Resumption, Scheduler, Shield, TaskHandle, Turn,
};
use ply_core::sim::TaskId;
use ply_core::{Diagnostic, SimId, Span, Symbol, Value, codes};
use std::collections::BTreeMap;

/// A value a region's tasks pass each other, as its scheduler holds it: copied for each task that
/// hears it, and let go when the region lets it go.
pub(crate) struct Item(Option<Passed>);

/// What an [`Item`] is: a value read from what a task passed, or what it passed kept with its type.
pub(crate) enum Passed {
    Value(Value),
    Carried(Carried),
}

impl Item {
    pub(crate) fn value(v: Value) -> Item {
        Item(Some(Passed::Value(v)))
    }

    pub(crate) fn carried(v: Carried) -> Item {
        Item(Some(Passed::Carried(v)))
    }

    fn take(mut self) -> Passed {
        self.0.take().expect("an item is taken once")
    }
}

impl Clone for Item {
    fn clone(&self) -> Item {
        Item(self.0.as_ref().map(|p| match p {
            Passed::Value(v) => Passed::Value(v.clone()),
            Passed::Carried(v) => Passed::Carried(v.dup(running())),
        }))
    }
}

impl Drop for Item {
    fn drop(&mut self) {
        if let Some(Passed::Carried(v)) = self.0.take() {
            v.release(running());
        }
    }
}

/// A task's body before it starts: a closure block, which holds its row's words, beside the type
/// of what it answers.
pub(crate) struct Body(pub(crate) Block, pub(crate) &'static PlyType);

impl Body {
    pub(crate) fn release(self, ctx: *mut Ctx) {
        unsafe { self.0.let_go_function(ctx) }
    }

    /// Calls the body, letting go of it: what it answered, or `None` where its task failed.
    fn run(self, ctx: *mut Ctx) -> Option<Item> {
        let Body(f, ty) = self;
        let c = unsafe { &mut *ctx };
        (c.entering_desc, c.entering_words) = (0, 0);
        let r = unsafe { f.call(ctx, &[]) };
        unsafe { f.let_go_function(ctx) };
        (unsafe { (*ctx).failed } == 0)
            .then(|| Item::carried(unsafe { Carried::new(Handed::of(r), ty) }))
    }
}

pub struct Simulation {
    sched: Scheduler<usize, Body, Item>,
    /// Alongside the scheduler's tasks, each dropped once it finishes.
    tasks: BTreeMap<TaskId, TaskStack>,
    /// Where the loop's stack stands while a task runs.
    scheduler_sp: usize,
    running: Option<TaskId>,
    request: Option<(TaskId, Request)>,
    /// What the task the loop resumes was answered, until the perform it resumes in reads it.
    heard: Option<Reply<Item>>,
    /// What the region answered, once its loop has returned.
    answer: Option<Item>,
    /// The stack the region was entered on, which every task's frames chain to.
    stack: usize,
    starting: Option<Body>,
    /// The loop's own stack; the opening stack is the root task's.
    loop_stack: Option<Stack>,
    /// The loop returned; nothing may switch into its stack again.
    loop_done: bool,
    /// Where the operation that opened it was performed: its root's id, its start and its end.
    pub(crate) site: [i64; 3],
}

#[derive(Default)]
struct TaskStack {
    /// Owned by a spawned task; the root runs on the stack that opened the region.
    stack: Option<Stack>,
    frames: usize,
    sp: usize,
    /// Copies of the handlers around its spawn as they stood then, held until the task starts.
    inherited: Vec<HandlerFrame>,
    /// What `Ctx.task` says while it runs: the root's opener's, a spawned task's its own.
    runs_as: i64,
}

enum Request {
    /// The body, and the stack the spawn was performed on.
    Spawn(Body, usize),
    Join(TaskHandle),
    Await(TaskHandle),
    Cancel(TaskHandle),
    Channel(usize),
    Send(ChanHandle, Item),
    Recv(ChanHandle),
    Close(ChanHandle),
    /// Each arm's channel beside what it sends, and whether to wait when none can go.
    Select(Vec<(ChanHandle, Option<Item>)>, bool),
    Yield,
    Park(Pending),
    /// The nanoseconds a production task waits on the host's clock.
    Sleep(i64),
    Finished(Item),
    Failed,
    /// The task unwound after a cancel.
    Cancelled,
}

impl Simulation {
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
        let operation = ply_core::host::operation_label(effect, op, None);
        c.fail(ply_core::host::err_unbound(c.site(), &operation));
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
    let Some(loop_stack) = Stack::take() else {
        no_stack_to_take(c);
        return false;
    };
    let scheduler_sp = loop_stack.prepare(loop_entry, ctx as usize);
    let loop_frames = c.open_stack(None);
    let root = TaskStack {
        stack: None,
        frames: c.current,
        sp: 0,
        inherited: Vec::new(),
        runs_as: c.task,
    };
    c.sims.push(Simulation {
        sched,
        tasks: BTreeMap::from([(ROOT, root)]),
        scheduler_sp,
        running: Some(ROOT),
        request: None,
        heard: None,
        answer: None,
        stack: loop_frames,
        starting: None,
        loop_stack: Some(loop_stack),
        loop_done: false,
        site: [c.site_root, c.site_start, c.site_end],
    });
    c.rearm_recording();
    true
}

/// A production region's loop: runs the scheduler, then hands the answer to the waiting root.
extern "C" fn loop_entry(arg: usize) {
    let ctx = arg as *mut Ctx;
    {
        let c = unsafe { &mut *ctx };
        let sim = c.sims.last().expect("a region is running");
        let own = sim
            .loop_stack
            .as_ref()
            .expect("a production loop runs on its own stack");
        c.stack_floor = unsafe { own.enter() };
        c.frames = std::ptr::null_mut();
    }
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

/// Once a production root's entry has returned, having written its answer's encoding itself: the
/// loop drains the other tasks.
pub unsafe fn finish_root(ctx: *mut Ctx) {
    let c = unsafe { &mut *ctx };
    if c.sims.last().expect("a region is running").loop_done {
        ended_under_root(c);
        return;
    }
    let request = if c.failed != 0 {
        Request::Failed
    } else {
        Request::Finished(Item::value(Value::Unit))
    };
    let sim = c.sims.last_mut().expect("a region is running");
    sim.request = Some((ROOT, request));
    let to = sim.scheduler_sp;
    let from = &mut sim.slot(ROOT).sp as *mut usize;
    unsafe { switch_keeping(ctx, from, to) };
    let c = unsafe { &mut *ctx };
    let mut sim = end(c);
    c.current = sim.slot(ROOT).frames;
    drop(sim.loop_stack.take());
}

/// The production loop failed while the root was suspended in it, and resumed the root directly.
fn ended_under_root(c: &mut Ctx) -> Option<Item> {
    let mut sim = end(c);
    c.current = sim.slot(ROOT).frames;
    sim.answer.take()
}

/// Drops a task's stack and frames, or the frames it holds unstarted; the root runs on the opening
/// stack and keeps both.
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
}

/// A finished task's slot and stack index go back, since nothing names a dead stack.
fn recycle(c: &mut Ctx, task: TaskId) {
    let sim = c.sims.last_mut().expect("a region is running");
    if task == ROOT {
        return;
    }
    if let Some(slot) = sim.tasks.remove(&task) {
        c.close_stack(slot.frames);
    }
}

/// Pops the innermost region, releasing each task it never finished and each body it never started.
/// What its scheduler still holds is let go as the region it answers is dropped.
pub(crate) fn end(c: &mut Ctx) -> Simulation {
    let ctx: *mut Ctx = c;
    let bodies = c
        .sims
        .last_mut()
        .expect("a region is running")
        .sched
        .unstarted();
    for body in bodies {
        body.release(ctx);
    }
    let tasks: Vec<TaskId> = c
        .sims
        .last()
        .expect("a region is running")
        .tasks
        .keys()
        .copied()
        .collect();
    for task in tasks {
        release(c, task);
    }
    let sim = c.sims.pop().expect("a region is running");
    c.rearm_recording();
    sim
}

/// The region's loop. Answers what the body answered, or `None` with the context failed.
pub(crate) unsafe fn run(ctx: *mut Ctx) -> Option<Item> {
    loop {
        let c = unsafe { &mut *ctx };
        let sim = c.sims.last_mut().expect("a region is running");
        if let Some((task, request)) = sim.request.take() {
            match unsafe { apply(ctx, task, request) } {
                Ok(()) => {}
                Err(Some(d)) => {
                    c.fail(d);
                    return None;
                }
                Err(None) => return None,
            }
        }
        let c = unsafe { &mut *ctx };
        let runtime = c.host_runtime();
        let sim = c.sims.last_mut().expect("a region is running");
        let (task, resumption) = match sim.sched.next_host(runtime.as_ref()) {
            Err(d) => {
                c.fail(d);
                return None;
            }
            Ok(Turn::Complete(value)) => return Some(value),
            Ok(Turn::Run { task, resumption }) => (task, resumption),
        };
        let sp = match resumption {
            Resumption::Enter => {
                c.fail(Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    "a production region entered a body of its own",
                ));
                return None;
            }
            Resumption::Start { body, .. } => unsafe { start(ctx, task, body) },
            Resumption::Resume { k, value } => {
                sim.heard = Some(value);
                Some(k)
            }
            // The task's perform answers nothing and finds the context failed, so every frame
            // returns, releasing what it holds, back to its entry.
            Resumption::Cancel { k } => {
                c.failed = FAILED_CANCELLED;
                Some(k)
            }
            Resumption::Raise { k, failure } => {
                let message = failure.message.clone();
                // Bound from the task's own stack: its handles answer before the region's.
                c.current = sim.slot(task).frames;
                c.pend(failure, message);
                Some(k)
            }
        };
        let sp = sp?;
        let c = unsafe { &mut *ctx };
        let sim = c.sims.last_mut().expect("a region is running");
        c.current = sim.slot(task).frames;
        sim.running = Some(task);
        let from = &mut sim.scheduler_sp as *mut usize;
        unsafe { switch_keeping(ctx, from, sp) };

        let c = unsafe { &mut *ctx };
        let sim = c.sims.last_mut().expect("a region is running");
        sim.running = None;
        c.current = sim.stack;
    }
}

/// Applies what `task` asked when it gave control back. `Err(None)`: the context is already failed.
unsafe fn apply(ctx: *mut Ctx, task: TaskId, request: Request) -> Result<(), Option<Diagnostic>> {
    let c = unsafe { &mut *ctx };
    let site = c.site();
    let runtime = c.host_runtime();
    let sim = c.sims.last_mut().expect("a region is running");
    let k = sim.slot(task).sp;
    let applied = match request {
        Request::Spawn(body, from) => {
            let inherited = handlers_around(&c.stacks, from, sim.stack);
            let handle = sim.sched.spawn(body, site);
            sim.tasks.insert(
                handle.id(),
                TaskStack {
                    inherited,
                    runs_as: crate::tasks::task_number(),
                    ..TaskStack::default()
                },
            );
            sim.sched.suspend(k, Value::Task(handle))
        }
        Request::Join(target) => sim.sched.join(k, &target, site),
        Request::Await(target) => sim.sched.await_task(k, &target, site),
        Request::Cancel(target) => match sim.sched.cancel(k, &target, site) {
            Err(d) => Err(d),
            Ok(stopped) => {
                if let Some(body) = stopped.unstarted {
                    let id = target.id();
                    body.release(ctx);
                    release(c, id);
                    recycle(c, id);
                }
                Ok(())
            }
        },
        Request::Channel(capacity) => sim.sched.channel(k, capacity),
        Request::Send(chan, value) => sim.sched.send(k, &chan, value, site),
        Request::Recv(chan) => sim.sched.recv(k, &chan, site),
        Request::Close(chan) => sim.sched.close(k, &chan, site),
        Request::Select(arms, wait) => sim.sched.select(k, arms, wait, site),
        Request::Yield => sim.sched.suspend(k, Value::Unit),
        Request::Park(pending) => sim.sched.park_on_host(k, pending, site, runtime.as_ref()),
        Request::Sleep(nanos) => sim.sched.sleep_on_host(k, nanos, site, runtime.as_ref()),
        Request::Finished(value) => {
            release(c, task);
            recycle(c, task);
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
            // Bound for a frame around the region, which carries it on once the region is over.
            if c.failed == FAILED_UNWIND || c.failed == FAILED_ABORT || c.failed == FAILED_TARGET {
                return Err(None);
            }
            // A raise no frame answered stays pending for the code around the region, whose
            // handles may answer it, noted with where in the region it was raised.
            if c.failed == FAILED_PENDING {
                let failure = c.diagnostic.take();
                let sim = c.sims.last().expect("a region is running");
                let noted = failure.map(|d| sim.sched.noted(d));
                c.diagnostic = noted;
                return Err(None);
            }
            // A raise nothing in the task answers fails the task, as one no frame answered did.
            if let Some(raise) = c.pending.take() {
                raise.release();
            }
            let failure = c.diagnostic.take().unwrap_or_else(|| {
                Diagnostic::error(codes::INTERNAL_ERROR, "a task failed without a diagnostic")
            });
            let sim = c.sims.last_mut().expect("a region is running");
            let failure = sim.sched.fail(failure);
            c.failed = 0;
            return Err(Some(failure));
        }
    };
    applied.map_err(Some)?;
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    sim.sched.refuse_wait(task);
    Ok(())
}

/// A `bracket`'s `acquire` or `release` the running task of the innermost region stands in.
pub(crate) struct Shielded {
    regions: usize,
    task: TaskId,
    shield: Shield,
}

/// Enters `shield` for the running task, where a region is running one.
pub(crate) fn shield(c: &mut Ctx, shield: Shield) -> Option<Shielded> {
    let regions = c.sims.len();
    let task = c.sims.last_mut()?.sched.shield(shield)?;
    Some(Shielded {
        regions,
        task,
        shield,
    })
}

/// Leaves what [`shield`] entered. Answers whether a cancel was held back and lands now.
pub(crate) fn unshield(c: &mut Ctx, entered: Option<Shielded>) -> bool {
    let Some(entered) = entered else {
        return false;
    };
    if c.sims.len() != entered.regions {
        return false;
    }
    c.sims
        .last_mut()
        .is_some_and(|sim| sim.sched.unshield(entered.task, entered.shield))
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

/// Lays out `task`'s stack to run `body`, which it takes: answers the stack pointer to switch to,
/// or `None` with the context failed when no stack can be had.
unsafe fn start(ctx: *mut Ctx, task: TaskId, body: Body) -> Option<usize> {
    let c = unsafe { &mut *ctx };
    let Some(stack) = Stack::take() else {
        body.release(ctx);
        no_stack_to_take(c);
        return None;
    };
    let sim = c.sims.last_mut().expect("a region is running");
    let sp = stack.prepare(task_entry, ctx as usize);
    let parent = sim.stack;
    let entered_from = sim.slot(ROOT).frames;
    let inherited = std::mem::take(&mut sim.slot(task).inherited);
    let frames = c.open_stack(Some(parent));
    c.stacks[frames].list = inherited;
    c.stacks[frames].entered_from = Some(entered_from);
    let sim = c.sims.last_mut().expect("a region is running");
    let slot = sim.slot(task);
    slot.stack = Some(stack);
    slot.frames = frames;
    slot.sp = sp;
    sim.starting = Some(body);
    Some(sp)
}

extern "C" fn task_entry(arg: usize) {
    let ctx = arg as *mut Ctx;
    let (body, me) = {
        let c = unsafe { &mut *ctx };
        let sim = c.sims.last_mut().expect("a region is running");
        let me = sim.running.expect("a starting task is running");
        let body = sim.starting.take().expect("a starting task has a body");
        let own = sim.slot(me).stack.as_ref();
        let floor = unsafe { own.expect("a task starts on its own stack").enter() };
        c.stack_floor = floor;
        c.frames = std::ptr::null_mut();
        c.task = sim.slot(me).runs_as;
        (body, me)
    };
    let answered = body.run(ctx);
    let c = unsafe { &mut *ctx };
    let request = match answered {
        Some(value) => Request::Finished(value),
        None if c.failed == FAILED_CANCELLED => Request::Cancelled,
        None => Request::Failed,
    };
    let sim = c.sims.last_mut().expect("a region is running");
    sim.request = Some((me, request));
    let from = &mut sim.slot(me).sp as *mut usize;
    let to = sim.scheduler_sp;
    unsafe { switch(&mut *from, to) };
    std::process::abort();
}

/// A `perform` the region answers: switch to the loop, and return once the task runs again with
/// what it was answered. `described` is the operation performed, whose types its arguments cross in
/// through and its answer is lifted through.
pub unsafe fn perform(
    ctx: *mut Ctx,
    effect: &Symbol,
    op: &Symbol,
    args: &[Word],
    described: &PlyOp,
) -> Word {
    let Some(request) = (unsafe { request_of(ctx, effect, op, args, described) }) else {
        return 0;
    };
    let c = unsafe { &mut *ctx };
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
    unsafe { switch_keeping(ctx, from, to) };
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    if sim.loop_done {
        ended_under_root(c);
        return 0;
    }
    match sim.heard.take() {
        Some(reply) => unsafe { heard(ctx, reply, described) },
        None => 0,
    }
}

/// Parks a production task on the host's pending answer: the value it resolved to, or `None` with
/// the context failed.
pub unsafe fn park(ctx: *mut Ctx, pending: Pending) -> Option<Value> {
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    let Some(task) = sim.running else {
        let d = Diagnostic::error(
            codes::INTERNAL_ERROR,
            "a pending host answer arrived while no task was running",
        );
        c.fail(d);
        return None;
    };
    sim.request = Some((task, Request::Park(pending)));
    let from = &mut sim.slot(task).sp as *mut usize;
    let to = sim.scheduler_sp;
    unsafe { switch_keeping(ctx, from, to) };
    let c = unsafe { &mut *ctx };
    let sim = c.sims.last_mut().expect("a region is running");
    if sim.loop_done {
        ended_under_root(c);
        return None;
    }
    match sim.heard.take() {
        Some(Reply::Value(v)) => Some(v),
        Some(_) => {
            c.fail(err_crossing("a host's answer heard as a task's"));
            None
        }
        None => None,
    }
}

/// The running task of an innermost production region, so a host answer parks rather than blocks.
pub fn running_task_of_production(c: &Ctx) -> Option<TaskId> {
    c.sims.last()?.running
}

/// An argument of a scheduled operation, as the region takes it: read as a value, or, in the
/// place of the operation's own type parameter, kept whole.
enum Arg {
    Read(Value),
    Kept(Item),
}

impl Arg {
    fn read(self) -> Result<Value, Diagnostic> {
        match self {
            Arg::Read(v) => Ok(v),
            Arg::Kept(_) => Err(err_crossing(
                "a value kept with its type where the region reads one",
            )),
        }
    }

    fn item(self) -> Item {
        match self {
            Arg::Read(v) => Item::value(v),
            Arg::Kept(v) => v,
        }
    }
}

/// Why a scheduled operation is refused: a failure of the entry, or a raise the program may answer.
enum Refused {
    Fail(Diagnostic),
    Raise(Diagnostic),
}

impl From<Diagnostic> for Refused {
    fn from(d: Diagnostic) -> Refused {
        Refused::Fail(d)
    }
}

/// What `effect.op` performed with `args`, which this takes, asks of the region; `None` with the
/// context failed or a raise under way.
unsafe fn request_of(
    ctx: *mut Ctx,
    effect: &Symbol,
    op: &Symbol,
    args: &[Word],
    described: &PlyOp,
) -> Option<Request> {
    let from = unsafe { &*ctx }.current;
    match (effect.as_str(), op.as_str()) {
        ("task", "spawn") => {
            let body = unsafe { body_of(ctx, args[0], described) }?;
            return Some(Request::Spawn(body, from));
        }
        ("task", "select") => return unsafe { select_of(ctx, args, described) },
        _ => {}
    }
    let args = unsafe { taken(ctx, args, described) }?;
    let c = unsafe { &mut *ctx };
    match request(c, effect, op, args) {
        Ok(request) => Some(request),
        Err(Refused::Fail(d)) => {
            c.fail(d);
            None
        }
        Err(Refused::Raise(d)) => {
            raise_error(c, d);
            None
        }
    }
}

fn request(c: &Ctx, effect: &Symbol, op: &Symbol, args: Vec<Arg>) -> Result<Request, Refused> {
    let site = c.site();
    let what = format!("`{effect}.{op}`");
    let mut args = args.into_iter();
    let mut next = || {
        args.next().ok_or_else(|| {
            err_crossing("an operation performed with fewer arguments than it takes")
        })
    };
    Ok(match (effect.as_str(), op.as_str()) {
        ("task", "join") => Request::Join(next()?.read()?.as_task(site, &what)?.clone()),
        ("task", "await") => Request::Await(next()?.read()?.as_task(site, &what)?.clone()),
        ("task", "cancel") => Request::Cancel(next()?.read()?.as_task(site, &what)?.clone()),
        ("task", "channel") => {
            let n = next()?.read()?.as_int(site, &what)?;
            Request::Channel(ply_core::sched::capacity_of(n, site).map_err(Refused::Raise)?)
        }
        ("task", "send") => {
            let chan = next()?.read()?.as_chan(site, &what)?;
            Request::Send(chan, next()?.item())
        }
        ("task", "recv") => Request::Recv(next()?.read()?.as_chan(site, &what)?),
        ("task", "close") => Request::Close(next()?.read()?.as_chan(site, &what)?),
        ("task", "yield") => Request::Yield,
        ("clock", "sleep") => {
            Request::Sleep(ply_core::sim::nanos_of(&next()?.read()?, site, &what)?)
        }
        _ => {
            return Err(Refused::Fail(
                Diagnostic::error(
                    codes::UNKNOWN_OPERATION,
                    format!("`{effect}` has no operation `{op}`"),
                )
                .note("a production region answers `task` and a task's sleep"),
            ));
        }
    })
}

/// The arguments `args` stand for, which this takes: each lowered through its operation's
/// parameter but those in the place of the operation's own type parameter, which are kept with
/// their types. `None` with the context failed where one does not lower.
unsafe fn taken(ctx: *mut Ctx, args: &[Word], op: &PlyOp) -> Option<Vec<Arg>> {
    let mut kept = Vec::with_capacity(args.len());
    let (mut read, mut types) = (Vec::new(), Vec::new());
    for (i, (w, ty)) in args.iter().zip(op.params()).enumerate() {
        if op.owns(i) {
            kept.push(Some(Item::carried(unsafe {
                Carried::new(Handed::of(*w), *ty)
            })));
        } else {
            kept.push(None);
            read.push(*w);
            types.push(*ty);
        }
    }
    let values = lowered_values(ctx, &read, &types);
    for (w, ty) in read.iter().zip(&types) {
        unsafe { Carried::new(Handed::of(*w), *ty) }.release(ctx);
    }
    let mut values = match values {
        Ok(values) => values.into_iter(),
        Err(why) => {
            unsafe { &mut *ctx }.fail(err_crossing(&why));
            return None;
        }
    };
    kept.into_iter()
        .map(|k| match k {
            Some(item) => Some(Arg::Kept(item)),
            None => values.next().map(Arg::Read),
        })
        .collect()
}

/// The body `task.spawn` was handed, which the region takes, answering what the operation's own
/// type parameter is. `None` with the context failed.
unsafe fn body_of(ctx: *mut Ctx, w: Word, op: &PlyOp) -> Option<Body> {
    let f = Block::of(w);
    match op.vars().first() {
        Some(ty) => Some(Body(f, ty)),
        None => {
            unsafe { f.let_go_function(ctx) };
            unsafe { &mut *ctx }.fail(err_crossing(
                "`task.spawn` performed with no type for what its task answers",
            ));
            None
        }
    }
}

/// `task.select`'s request: each arm's channel beside what it sends, and whether to wait. Takes
/// the arguments; `None` with the context failed.
unsafe fn select_of(ctx: *mut Ctx, args: &[Word], op: &PlyOp) -> Option<Request> {
    let site = unsafe { &*ctx }.site();
    let what = "`task.select`";
    let params = op.params();
    let arms = lowering_of(ctx, args[0], params[0]).and_then(|(bytes, beside)| {
        let arms = arms_read(ctx, &bytes, &beside, site);
        released(ctx, beside);
        arms
    });
    let wait = lowered_values(ctx, &args[1..2], &params[1..2]);
    for (w, ty) in args.iter().zip(params) {
        unsafe { Carried::new(Handed::of(*w), *ty) }.release(ctx);
    }
    let read = match (arms, wait) {
        (Ok(arms), Ok(wait)) => match wait.as_slice() {
            [v] => v.as_bool(site, what).map(|wait| (arms, wait)),
            _ => Err(err_crossing("`task.select` performed without its `wait`")),
        },
        (Err(why), _) | (_, Err(why)) => Err(err_crossing(&why)),
    };
    let c = unsafe { &mut *ctx };
    match read {
        Ok((arms, wait)) => Some(Request::Select(arms, wait)),
        Err(d) => {
            c.fail(d);
            None
        }
    }
}

/// `task.select`'s arms as emitted code lowers them: a list of pairs, each a channel at `_0`, a
/// runtime object, and at `_1` `None` to receive from it or `Some` of a value to send on it, lent
/// with its type, of which the region keeps a count.
fn arms_read(
    ctx: *mut Ctx,
    bytes: &[u8],
    beside: &[Beside],
    span: Span,
) -> Result<Vec<(ChanHandle, Option<Item>)>, String> {
    let mut r = Reader::new(bytes)?;
    r.expect(tag::LIST)?;
    let n = r.count()?;
    let mut arms = Vec::with_capacity(n);
    for _ in 0..n {
        r.expect(tag::RECORD)?;
        if r.count()? != 2 {
            return Err("an arm of `task.select` that is no pair".into());
        }
        field(&mut r, "_0")?;
        let chan = match handle_at(&mut r, beside)? {
            Beside::Runtime(v) => v.as_chan(span, "`task.select`").map_err(|d| d.message)?,
            Beside::Carried(_) => return Err("an arm's channel kept with a type".into()),
        };
        field(&mut r, "_1")?;
        r.expect(tag::CTOR)?;
        let name = r.symbol()?;
        let send = match (name.as_str(), r.count()?) {
            ("None", 0) => None,
            ("Some", 1) => match handle_at(&mut r, beside)? {
                Beside::Carried(v) => Some(Item::carried(v.dup(ctx))),
                Beside::Runtime(_) => {
                    return Err("what an arm sends read as a runtime object".into());
                }
            },
            (name, n) => return Err(format!("`{name}` of {n} where an arm's `Option` was read")),
        };
        arms.push((chan, send));
    }
    r.end()?;
    Ok(arms)
}

/// A record field's name, which must be `name`.
fn field(r: &mut Reader<&[u8]>, name: &str) -> Result<(), String> {
    let read = r.symbol()?;
    if read.as_str() != name {
        return Err(format!("field `{read}` where `{name}` was read"));
    }
    Ok(())
}

/// What stands at the handle read next.
fn handle_at<'b>(r: &mut Reader<&[u8]>, beside: &'b [Beside]) -> Result<&'b Beside, String> {
    r.expect(tag::HANDLE)?;
    let at = r.varint()?;
    usize::try_from(at)
        .ok()
        .and_then(|at| beside.get(at))
        .ok_or_else(|| format!("handle {at}, which was not kept"))
}

/// What a task's operation was answered, as the code that performed it holds it: the reply lifted
/// through the operation's answer.
unsafe fn heard(ctx: *mut Ctx, reply: Reply<Item>, op: &PlyOp) -> Word {
    let c = unsafe { &mut *ctx };
    let mut w = Writer::new();
    let mut beside = Vec::new();
    write_reply(c, &mut w, &mut beside, reply);
    lifted_from(ctx, w.finish(), beside, op.answer()).map_or(0, Handed::word)
}

/// `reply` written as emitted code lifts it, `beside` taking each value the bytes cannot hold: a
/// runtime object made for a handle, a value kept with its type as itself.
fn write_reply(c: &mut Ctx, w: &mut Writer, beside: &mut Vec<Lifted>, reply: Reply<Item>) {
    match reply {
        Reply::Value(v) => write_value(c, w, beside, &v),
        Reply::Item(i) => write_item(c, w, beside, i),
        Reply::Maybe(m) => write_maybe(c, w, beside, m),
        Reply::Chosen(None) => w.ctor(b"None", 0),
        Reply::Chosen(Some((at, got))) => {
            w.ctor(b"Some", 1);
            w.count(tag::RECORD, 2);
            w.name(b"_0");
            w.int(at as i64);
            w.name(b"_1");
            write_maybe(c, w, beside, got);
        }
    }
}

fn write_maybe(c: &mut Ctx, w: &mut Writer, beside: &mut Vec<Lifted>, m: Option<Item>) {
    match m {
        Some(i) => {
            w.ctor(b"Some", 1);
            write_item(c, w, beside, i);
        }
        None => w.ctor(b"None", 0),
    }
}

fn write_item(c: &mut Ctx, w: &mut Writer, beside: &mut Vec<Lifted>, i: Item) {
    match i.take() {
        Passed::Value(v) => write_value(c, w, beside, &v),
        Passed::Carried(v) => {
            w.handle(beside.len() as u64);
            beside.push(Lifted::Carried(v));
        }
    }
}

fn write_value(c: &mut Ctx, w: &mut Writer, beside: &mut Vec<Lifted>, v: &Value) {
    codec::write_value(w, v, &mut |x| {
        beside.push(Lifted::Runtime(c.heap.bridge(x.clone())));
        Ok(beside.len() as u64 - 1)
    })
    .expect("a value is written whole but for what is kept beside it");
}

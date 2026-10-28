//! The stacks a region's tasks run on, and the switch between each task and the region's loop.
//! Which task runs next and what a request means are the scheduler's, written in Ply
//! (`std.internal.schedule`); a region here holds what Ply cannot: each task's stack and where it
//! stands, where the loop stands, the brackets each task stands in, and how a body that did not
//! answer left the context. Then the objects the scheduler's natives make (a value of any type
//! kept with its type, a region's stacks, a task's body, a task's and a channel's handle) and the
//! natives themselves, and the production region's waits: the run's tickets and its clock.

use crate::emitted::{Block, Carried, Handed, Handle, PlyKept, PlyType};
use crate::flat::Reader;
use crate::heap::{self, Native, Word, native_of};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{
    Ctx, FAILED_CANCELLED, FAILED_PENDING, FAILED_TARGET, Unanswered, lent, no_stack_to_take,
    switch_keeping,
};
use crate::stack::{Stack, switch};
use ply_core::host::Pending;
use ply_core::sim::TaskId;
use ply_core::{Diagnostic, Span, codes};
use ply_host::pool::Run;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

/// What [`resume`] answers where the task's body ended rather than asked: it answered; it unwound
/// after a cancel; a raise nothing in it answered left it; it failed at run time. A request's own
/// code is never below 0.
pub const FINISHED: i64 = -1;
pub const UNWOUND: i64 = -2;
pub const RAISED: i64 = -3;
pub const FAILED: i64 = -4;

/// How a task's body runs: `run` runs it to its end, leaving the context failed as the body left
/// it; `release` lets go of what `arg` holds where the body never runs.
pub struct Body {
    pub run: unsafe fn(*mut Ctx, usize),
    pub release: unsafe fn(*mut Ctx, usize),
    pub arg: usize,
}

/// What passes at a switch between a task and its region's loop: a code, two numbers, a word, and
/// where the task stood, a root's id and offsets as the context stores a site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub code: i64,
    pub numbers: [i64; 2],
    pub word: Word,
    pub site: [i64; 3],
}

/// The brackets a task stands in, and whether a cancel waits for it to leave them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shields {
    pub acquiring: u32,
    pub releasing: u32,
    pub stopping: bool,
}

/// A bracket's `acquire`, where a cancel takes no answer, or its `release`, where it takes no wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shield {
    Acquire,
    Release,
}

/// How a body that did not answer left the context, held while the loop's Ply runs.
struct Stashed {
    failed: i64,
    diagnostic: Option<Diagnostic>,
    pending: Option<Unanswered>,
    target: Word,
}

impl Stashed {
    fn release(self) {
        if let Some(p) = self.pending {
            p.release();
        }
    }
}

#[derive(Default)]
struct Slot {
    stack: Option<Stack>,
    sp: usize,
    body: Option<Body>,
    /// The task's stack's index in [`Ctx::stacks`], once it has started.
    stack_index: Option<usize>,
    shields: Shields,
    stashed: Option<Stashed>,
    /// What `Ctx.task` says while it runs ([`task_number`]); none for the region's body, which runs
    /// as the task that opened the region.
    runs_as: Option<i64>,
}

/// One open region's tasks. Boxed, so a task's stack can name it while it runs.
pub struct Region {
    ctx: *mut Ctx,
    seeded: bool,
    /// The index in [`Ctx::stacks`] of the stack the region was opened on.
    opened_on: usize,
    /// The calls a task may nest as it starts: as many as were left where the region was opened,
    /// however deep the loop that starts it stands.
    fuel: i64,
    loop_sp: usize,
    running: Option<u64>,
    slots: HashMap<u64, Box<Slot>>,
    /// Stacks the region's ended tasks gave back, for its next ones: at most as many as it ever ran
    /// at once.
    spare: Vec<Stack>,
    live: usize,
    peak: usize,
    message: Message,
    /// The raise the loop left for the task it resumes raising.
    raising: Option<Diagnostic>,
}

impl Region {
    /// A region opened on the running stack of `ctx`, `seeded` for `simulate`.
    pub fn open(ctx: *mut Ctx, seeded: bool) -> Box<Region> {
        Box::new(Region {
            ctx,
            seeded,
            opened_on: unsafe { (*ctx).current },
            fuel: unsafe { (*ctx).fuel },
            loop_sp: 0,
            running: None,
            slots: HashMap::new(),
            spare: Vec::new(),
            live: 0,
            peak: 0,
            message: Message::default(),
            raising: None,
        })
    }

    pub fn is_seeded(&self) -> bool {
        self.seeded
    }

    /// Whether a seeded step runs: one of the region's tasks, which what it touches is recorded for.
    pub fn is_stepping(&self) -> bool {
        self.seeded && self.running.is_some()
    }

    /// The task the loop resumed, while it runs.
    pub fn running(&self) -> Option<u64> {
        self.running
    }

    /// The tasks whose stacks the region holds, started or not.
    pub fn tasks(&self) -> usize {
        self.slots.len()
    }

    /// The stacks its tasks run on and those it keeps for its next ones.
    pub fn stacks(&self) -> usize {
        self.live + self.spare.len()
    }

    /// Task `id`, to run `body` the first time the loop resumes it; task 0 is the region's body.
    pub fn start(&mut self, id: u64, body: Body) {
        let slot = Slot {
            body: Some(body),
            runs_as: (id != 0).then(task_number),
            ..Slot::default()
        };
        if let Some(old) = self.slots.insert(id, Box::new(slot)) {
            unsafe { self.freed(*old) };
        }
    }

    /// The brackets task `id` stands in; none for a task the region does not hold.
    pub fn shields(&self, id: u64) -> Shields {
        self.slots.get(&id).map(|s| s.shields).unwrap_or_default()
    }

    /// Every task standing in a bracket, or held back for one, ascending.
    pub fn shielded(&self) -> Vec<(u64, Shields)> {
        let mut out: Vec<(u64, Shields)> = (self.slots.iter())
            .filter(|(_, s)| s.shields != Shields::default())
            .map(|(id, s)| (*id, s.shields))
            .collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    /// Whether a cancel of task `id` waits for it to leave the brackets it stands in.
    pub fn hold_back(&mut self, id: u64, stopping: bool) {
        if let Some(slot) = self.slots.get_mut(&id) {
            slot.shields.stopping = stopping;
        }
    }

    /// The running task enters a bracket's `acquire` or `release`: the task, where one runs.
    pub fn shield(&mut self, shield: Shield) -> Option<u64> {
        let id = self.running?;
        let slot = self.slots.get_mut(&id)?;
        match shield {
            Shield::Acquire => slot.shields.acquiring += 1,
            Shield::Release => slot.shields.releasing += 1,
        }
        Some(id)
    }

    /// Task `id` leaves what [`Region::shield`] entered. Answers whether a cancel held back for it
    /// lands now: the task is to unwind from here.
    pub fn unshield(&mut self, id: u64, shield: Shield) -> bool {
        let Some(slot) = self.slots.get_mut(&id) else {
            return false;
        };
        let s = &mut slot.shields;
        match shield {
            Shield::Acquire => s.acquiring = s.acquiring.saturating_sub(1),
            Shield::Release => s.releasing = s.releasing.saturating_sub(1),
        }
        let lands = s.stopping && s.acquiring == 0 && s.releasing == 0;
        if lands {
            s.stopping = false;
        }
        lands
    }

    /// Puts back how task `id`'s body left the context, which [`resume`] answered [`RAISED`] or
    /// [`FAILED`] for, with `note` where one is given: the context is failed as the body left it.
    /// `false` where the task left nothing.
    pub fn restore(&mut self, id: u64, note: Option<String>) -> bool {
        let Some(s) = self.slots.get_mut(&id).and_then(|slot| slot.stashed.take()) else {
            return false;
        };
        let c = unsafe { &mut *self.ctx };
        c.failed = s.failed;
        c.diagnostic = match (s.diagnostic, note) {
            (Some(d), Some(n)) => Some(d.note(n)),
            (d, _) => d,
        };
        c.pending = s.pending;
        c.target = s.target;
        true
    }

    /// Lets go of task `id`, its stack back to the region and a body it never ran let go.
    pub fn free(&mut self, id: u64) {
        if let Some(slot) = self.slots.remove(&id) {
            unsafe { self.freed(*slot) };
        }
    }

    unsafe fn freed(&mut self, slot: Slot) {
        let c = unsafe { &mut *self.ctx };
        if let Some(body) = slot.body {
            unsafe { (body.release)(self.ctx, body.arg) };
        }
        if let Some(s) = slot.stashed {
            s.release();
        }
        if let Some(index) = slot.stack_index {
            c.close_stack(index);
        }
        if let Some(stack) = slot.stack {
            self.live = self.live.saturating_sub(1);
            if self.spare.len() < self.peak {
                self.spare.push(stack);
            }
        }
    }
}

/// A region closing lets go of every task it still holds.
impl Drop for Region {
    fn drop(&mut self) {
        let ids: Vec<u64> = self.slots.keys().copied().collect();
        for id in ids {
            self.free(id);
        }
    }
}

/// Runs task `id` of `region` until it asks something, handing it `answer`: what it asked, or how
/// its body ended. A task that has not run starts its body on a stack of its own, the region's
/// spare one where it has one. One the region does not hold, or that no stack can be had for, is
/// answered [`FAILED`] with the context failed.
///
/// # Safety
/// `region` is open on the running stack of its context, and nothing else runs there.
pub unsafe fn resume(region: *mut Region, id: u64, answer: Message) -> Message {
    let r = unsafe { &mut *region };
    let ctx = r.ctx;
    let c = unsafe { &mut *ctx };
    let Some(slot) = r.slots.get_mut(&id) else {
        c.fail(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the region was asked to run @{id}, which it does not hold"),
        ));
        return ended(FAILED);
    };
    if slot.stack.is_none() {
        let Some(stack) = r.spare.pop().or_else(Stack::take) else {
            no_stack_to_take(c);
            return ended(FAILED);
        };
        slot.sp = stack.prepare(task_entry, region as usize);
        slot.stack = Some(stack);
        slot.stack_index = Some(c.open_stack());
        r.live += 1;
        r.peak = r.peak.max(r.live);
    }
    let to = slot.sp;
    c.current = slot.stack_index.unwrap_or(r.opened_on);
    r.message = answer;
    r.running = Some(id);
    unsafe { switch_keeping(ctx, &raw mut r.loop_sp, to) };
    let r = unsafe { &mut *region };
    r.running = None;
    unsafe { (*ctx).current = r.opened_on };
    r.message
}

/// A number for `Ctx.task` no other task of any region in the process is given: below 0, so
/// apart from 0, which no region's task runs as.
pub(crate) fn task_number() -> i64 {
    static NEXT: AtomicI64 = AtomicI64::new(-1);
    NEXT.fetch_sub(1, Ordering::Relaxed)
}

/// The running task of `region` hands its loop `request` and waits to run again: answers what the
/// loop resumed it with.
///
/// # Safety
/// The running stack is the task's, which `region`'s loop resumed.
pub unsafe fn to_loop(region: *mut Region, request: Message) -> Message {
    let r = unsafe { &mut *region };
    let Some(id) = r.running else {
        let c = unsafe { &mut *r.ctx };
        c.fail(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "a task asked its region something while no task of it was running",
        ));
        return ended(FAILED);
    };
    let slot = r.slots.get_mut(&id).expect("a running task has its slot");
    r.message = request;
    let from = &raw mut slot.sp;
    unsafe { switch_keeping(r.ctx, from, r.loop_sp) };
    unsafe { (*region).message }
}

/// Whether the task `region`'s loop resumed with `heard` runs on: `false`, its context failed, where
/// it is to unwind after a cancel or to raise what the loop left for it, or the switch failed.
///
/// # Safety
/// `region` is open on the running task's context.
unsafe fn runs_on(region: *mut Region, heard: &Message) -> bool {
    let r = unsafe { &mut *region };
    let c = unsafe { &mut *r.ctx };
    match heard.code {
        RESUME_UNWIND => {
            c.failed = FAILED_CANCELLED;
            false
        }
        RESUME_RAISE => {
            let d = r.raising.take().unwrap_or_else(|| {
                Diagnostic::error(codes::INTERNAL_ERROR, "a task was resumed to raise nothing")
            });
            let message = d.message.clone();
            c.pend(d, message);
            false
        }
        _ => c.failed == 0,
    }
}

/// The codes the runtime itself asks the run's region's loop with, as `std.internal.schedule`
/// reads them: a sleep of some nanoseconds, and a wait on one of the run's tickets.
pub const ASK_SLEEP: i64 = 11;
pub const ASK_TICKET: i64 = 15;

/// The innermost region open on `c`, where it is the run's and one of its tasks runs.
fn production_running(c: &Ctx) -> Option<*mut Region> {
    let (region, _) = c.regions.last()?;
    let r = unsafe { &**region };
    (!r.is_seeded() && r.running().is_some()).then_some(*region)
}

/// The task of the run's region running on `c`, where one runs: who performs on the machine, and a
/// wait on the host waits there.
pub(crate) fn production_task(c: &Ctx) -> Option<TaskId> {
    let region = production_running(c)?;
    unsafe { (*region).running() }.map(TaskId)
}

/// The running task of the run's region asks its loop `code` about `n` and waits while the region's
/// other tasks run: whether it runs on, as [`runs_on`] says. `None` where no task of the run's
/// region runs on `c`, which then waits itself.
pub(crate) fn waited(c: &mut Ctx, code: i64, n: i64) -> Option<bool> {
    let region = production_running(c)?;
    let request = Message {
        code,
        numbers: [n, 0],
        word: 0,
        site: [c.performed_root, c.performed_start, c.performed_end],
    };
    let was = c.scheduler_entered();
    let ctx: *mut Ctx = c;
    let heard = unsafe { to_loop(region, request) };
    if !was {
        unsafe { &mut *ctx }.program_entered();
    }
    Some(unsafe { runs_on(region, &heard) })
}

fn ended(code: i64) -> Message {
    Message {
        code,
        ..Message::default()
    }
}

/// Where a task's stack begins: runs its body, then tells the loop how the body ended, and never
/// runs again.
extern "C" fn task_entry(arg: usize) {
    let region = arg as *mut Region;
    let (ctx, id, body) = {
        let r = unsafe { &mut *region };
        let id = r.running.expect("a task starts as its loop resumes it");
        let slot = r.slots.get_mut(&id).expect("a starting task has its slot");
        let c = unsafe { &mut *r.ctx };
        let own = slot.stack.as_ref().expect("a task starts on its own stack");
        c.stack_floor = unsafe { own.enter() };
        c.frames = std::ptr::null_mut();
        c.fuel = r.fuel;
        if let Some(n) = slot.runs_as {
            c.task = n;
        }
        (r.ctx, id, slot.body.take())
    };
    if let Some(body) = body {
        unsafe { (body.run)(ctx, body.arg) };
    }
    let c = unsafe { &mut *ctx };
    // A body that failed left without saying the program's work ended.
    c.scheduler_entered();
    let site = [c.site_root, c.site_start, c.site_end];
    let (code, stashed) = match c.failed {
        0 => (FINISHED, None),
        FAILED_CANCELLED => {
            c.failed = 0;
            (UNWOUND, None)
        }
        failed => {
            let raised = matches!(failed, FAILED_TARGET | FAILED_PENDING);
            let s = Stashed {
                failed,
                diagnostic: c.diagnostic.take(),
                pending: c.pending.take(),
                target: std::mem::take(&mut c.target),
            };
            c.failed = 0;
            (if raised { RAISED } else { FAILED }, Some(s))
        }
    };
    let r = unsafe { &mut *region };
    let slot = r.slots.get_mut(&id).expect("an ending task has its slot");
    slot.stashed = stashed;
    r.message = Message {
        code,
        site,
        ..Message::default()
    };
    let from = &raw mut slot.sp;
    unsafe { switch(&mut *from, r.loop_sp) };
    std::process::abort();
}

// --- The scheduler's objects -----------------------------------------------------------------

/// Every native `std.internal.schedule` declares, under the name it declares it by, then those
/// compiled code calls itself where a perform hands a program's clause a kept value.
pub fn natives() -> Vec<crate::c::MachineNative> {
    use crate::c::machine_native as n;
    vec![
        n("keep", rt_keep as unsafe extern "C" fn(_, _, _) -> _),
        n("open", rt_open as unsafe extern "C" fn(_, _, _) -> _),
        n(
            "?keep",
            rt_keep_compared as unsafe extern "C" fn(_, _, _) -> _,
        ),
        n("?open", rt_open as unsafe extern "C" fn(_, _, _) -> _),
        n(
            "?kept_equal",
            rt_kept_equal as unsafe extern "C" fn(_, _, _) -> _,
        ),
        n(
            "?kept_order",
            rt_kept_order as unsafe extern "C" fn(_, _, _) -> _,
        ),
        n(
            "stacks_opened",
            rt_stacks_opened as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        n(
            "stacks_region",
            rt_stacks_region as unsafe extern "C" fn(_, _) -> _,
        ),
        n(
            "stacks_outer",
            rt_stacks_outer as unsafe extern "C" fn(_, _, _) -> _,
        ),
        n("task_as", rt_handle_as as unsafe extern "C" fn(_, _) -> _),
        n(
            "program_resumed",
            rt_program_resumed as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        n(
            "program_paused",
            rt_program_paused as unsafe extern "C" fn(_, _, _) -> _,
        ),
        n("chan_as", rt_handle_as as unsafe extern "C" fn(_, _) -> _),
        n(
            "trail_entered",
            rt_trail_entered as unsafe extern "C" fn(_) -> _,
        ),
        n("seed_root", rt_seed_root as unsafe extern "C" fn(_, _) -> _),
        n("seed_path", rt_seed_path as unsafe extern "C" fn(_) -> _),
        n(
            "seeded_steps",
            rt_seeded_steps as unsafe extern "C" fn(_) -> _,
        ),
        n(
            "trail_left",
            rt_trail_left as unsafe extern "C" fn(_, _, _, _, _),
        ),
        n(
            "site_placed",
            rt_site_placed as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        n(
            "footprint_taken",
            rt_footprint_taken as unsafe extern "C" fn(_) -> _,
        ),
        n(
            "footprint_held",
            rt_footprint_held as unsafe extern "C" fn(_) -> _,
        ),
        n(
            "task_body",
            rt_task_body as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        n(
            "task_started",
            rt_task_started as unsafe extern "C" fn(_, _, _, _),
        ),
        n(
            "task_resumed",
            rt_task_resumed as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        n(
            "task_resumed_kept",
            rt_task_resumed_kept as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        n(
            "task_resumed_raising",
            rt_task_resumed_raising as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        n(
            "asked_number",
            rt_asked_number as unsafe extern "C" fn(_, _, _) -> _,
        ),
        n(
            "asked_site",
            rt_asked_site as unsafe extern "C" fn(_, _, _) -> _,
        ),
        n(
            "asked_kept",
            rt_asked_kept as unsafe extern "C" fn(_, _) -> _,
        ),
        n(
            "task_yielded",
            rt_task_yielded as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        n(
            "task_yielded_kept",
            rt_task_yielded_kept as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        n(
            "heard_number",
            rt_heard_number as unsafe extern "C" fn(_, _) -> _,
        ),
        n(
            "heard_kept",
            rt_heard_kept as unsafe extern "C" fn(_, _) -> _,
        ),
        n("task_freed", rt_task_freed as unsafe extern "C" fn(_, _, _)),
        n(
            "task_restored",
            rt_task_restored as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        n(
            "task_shields",
            rt_task_shields as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        n(
            "task_held_back",
            rt_task_held_back as unsafe extern "C" fn(_, _, _, _),
        ),
        n(
            "task_settled",
            rt_task_settled as unsafe extern "C" fn(_, _, _),
        ),
        n("task_made", rt_task_made as unsafe extern "C" fn(_, _) -> _),
        n(
            "task_said",
            rt_task_said as unsafe extern "C" fn(_, _, _) -> _,
        ),
        n(
            "task_answer",
            rt_task_answer as unsafe extern "C" fn(_, _) -> _,
        ),
        n(
            "chan_made",
            rt_chan_made as unsafe extern "C" fn(_, _, _) -> _,
        ),
        n(
            "chan_said",
            rt_chan_said as unsafe extern "C" fn(_, _, _) -> _,
        ),
        n(
            "region_failed",
            rt_region_failed as unsafe extern "C" fn(_, _, _),
        ),
        n(
            "ticket_watch",
            rt_ticket_watch as unsafe extern "C" fn(_, _) -> _,
        ),
        n(
            "tickets_answered",
            rt_tickets_answered as unsafe extern "C" fn(_) -> _,
        ),
        n(
            "tickets_park",
            rt_tickets_park as unsafe extern "C" fn(_, _) -> _,
        ),
        n("host_now", rt_host_now as unsafe extern "C" fn(_) -> _),
        n(
            "root_returned",
            rt_root_returned as unsafe extern "C" fn(_, _, _),
        ),
        n(
            "run_stopping",
            rt_run_stopping as unsafe extern "C" fn(_) -> _,
        ),
        n(
            "run_drained",
            rt_run_drained as unsafe extern "C" fn(_) -> _,
        ),
    ]
}

/// How the loop resumes a task that is to unwind after a cancel, or to raise what the loop left
/// for it ([`rt_task_resumed_raising`]); a task's own codes are never below 0.
pub const RESUME_UNWIND: i64 = -1;
pub const RESUME_RAISE: i64 = -2;

/// How a task's handle says the task ended: not yet, answering, or cancelled.
pub const LIVE: i64 = 0;
pub const ANSWERED: i64 = 1;
pub const CANCELLED: i64 = 2;

/// A value of any type held beside its type's descriptor: `std.internal.schedule`'s `Kept`, what a
/// value of an operation's own type parameter crosses as between a perform and its clause, and a
/// region passes between its tasks. One a perform hands a program's clause holds its `kept#T`
/// too, through which two of them compare.
pub struct Kept {
    value: Option<Carried>,
    compared: Option<&'static PlyKept>,
}

impl Native for Kept {
    fn release(&mut self, ctx: *mut Ctx) {
        if let Some(v) = self.value.take() {
            v.release(ctx);
        }
    }
}

/// One region's tasks' stacks (`Stacks`): the region, its id among the entry's regions, the
/// message the loop last heard from a task and the one a task last heard from the loop, and the
/// handle each started task answers into.
pub struct Stacks {
    region: Option<Box<Region>>,
    id: i64,
    asked: Message,
    heard: Message,
    tasks: HashMap<u64, Word>,
}

impl Stacks {
    fn region(&mut self) -> *mut Region {
        match &mut self.region {
            Some(r) => &mut **r,
            None => std::ptr::null_mut(),
        }
    }
}

impl Native for Stacks {
    fn release(&mut self, ctx: *mut Ctx) {
        let region = self.region();
        let c = unsafe { &mut *ctx };
        c.regions.retain(|(r, _)| *r != region);
        c.rearm_recording();
        drop(self.region.take());
        for w in [self.asked.word, self.heard.word] {
            if w != 0 {
                heap::release_object(ctx, w);
            }
        }
        for (_, t) in self.tasks.drain() {
            heap::release_object(ctx, t);
        }
    }
}

/// At the entry's end, with the context going, the region's stacks are left to the process.
impl Drop for Stacks {
    fn drop(&mut self) {
        if let Some(r) = self.region.take() {
            std::mem::forget(r);
        }
    }
}

/// A task's body as `task_body` took it (`TaskBody`), until a region starts it.
pub struct TaskBody(Option<Started>);

/// A body to start: the function, the descriptor of its row and the words for it, which it is
/// handed as the runtime's other calls into code are, and the handle its answer goes to.
struct Started {
    body: Block,
    desc: Word,
    words: Vec<Word>,
    task: Word,
}

impl Started {
    fn release(self, ctx: *mut Ctx) {
        unsafe { self.body.let_go_function(ctx) };
        heap::release_object(ctx, self.task);
    }
}

impl Native for TaskBody {
    fn release(&mut self, ctx: *mut Ctx) {
        if let Some(s) = self.0.take() {
            s.release(ctx);
        }
    }
}

/// A region's task's handle (`Task`): its region, its id, how it ended and its answer.
pub struct RegionTask {
    region: i64,
    id: i64,
    ended: i64,
    answer: Option<Word>,
}

impl Native for RegionTask {
    fn release(&mut self, ctx: *mut Ctx) {
        if let Some(w) = self.answer.take() {
            heap::release_object(ctx, w);
        }
    }

    fn escapes(&self) -> Option<ply_core::escape::Handle> {
        Some(ply_core::escape::Handle::Task)
    }
}

/// A region's channel's handle (`Chan`): its region and its id.
pub struct RegionChan {
    region: i64,
    id: i64,
}

impl Native for RegionChan {
    fn release(&mut self, _ctx: *mut Ctx) {}

    fn escapes(&self) -> Option<ply_core::escape::Handle> {
        Some(ply_core::escape::Handle::Channel)
    }
}

fn internal(c: &mut Ctx, why: impl Into<String>) {
    c.fail(Diagnostic::error(codes::INTERNAL_ERROR, why));
}

fn spelled(t: &PlyType) -> String {
    let bytes = unsafe { std::slice::from_raw_parts(t.spelled, t.spelled_len as usize) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// The object of type `T` `h` is, failing the run where it is none.
fn object<'a, T: Native>(ctx: *mut Ctx, h: &Handle, what: &str) -> Option<&'a mut T> {
    let found = unsafe { native_of::<T>(h.word()) };
    if found.is_none() {
        internal(
            unsafe { &mut *ctx },
            format!("a native was handed {what} that is none"),
        );
    }
    found
}

/// A new holder of the runtime object `w`, which compiled code lent.
fn held(w: Word) -> Word {
    Block::of(w).dup();
    w
}

fn object_made(ctx: *mut Ctx, held: Box<dyn Native>) -> Handle {
    Handle::of(unsafe { (*ctx).heap.native(held) })
}

/// `x`, a value of the type `ty` describes, kept: it holds the count compiled code handed with it.
pub unsafe extern "C" fn rt_keep(ctx: *mut Ctx, x: Handed, ty: *const PlyType) -> Handle {
    let value = Some(unsafe { Carried::new(x, ty) });
    object_made(
        ctx,
        Box::new(Kept {
            value,
            compared: None,
        }),
    )
}

/// `x`, a value a perform hands a program's clause, kept beside `kept`, which describes its type
/// and compares two of its values: it holds the count compiled code handed with it.
pub unsafe extern "C" fn rt_keep_compared(
    ctx: *mut Ctx,
    x: Handed,
    kept: *const PlyKept,
) -> Handle {
    let kept = unsafe { &*kept };
    let value = Some(unsafe { Carried::new(x, kept.ty) });
    object_made(
        ctx,
        Box::new(Kept {
            value,
            compared: Some(kept),
        }),
    )
}

/// 1 where the kept values `a` and `b`, which it borrows, are equal as their type compares them,
/// else 0. The run fails where `==` is not built at their type, as for a function or a runtime
/// object, and where the two are of different types, which no program compares.
pub unsafe extern "C" fn rt_kept_equal(ctx: *mut Ctx, a: Handle, b: Handle) -> I64 {
    I64(i64::from(kept_compared(ctx, &a, &b, false) != 0))
}

/// The sign of the order of the kept values `a` and `b`, which it borrows, as their type orders
/// them: below, at or above zero. The run fails where `compare` is not built at their type, and
/// where the two are of different types.
pub unsafe extern "C" fn rt_kept_order(ctx: *mut Ctx, a: Handle, b: Handle) -> I64 {
    I64(kept_compared(ctx, &a, &b, true).signum())
}

/// What the entry of the kept values' type answers for them, its equality's or, where `order`, its
/// order's; 0 where the run fails.
fn kept_compared(ctx: *mut Ctx, a: &Handle, b: &Handle, order: bool) -> i64 {
    // Each read out before the next is found: `a` and `b` may be one object.
    let (Some(x), Some(y)) = (compared_of(ctx, a), compared_of(ctx, b)) else {
        return 0;
    };
    if x.ty.identity != y.ty.identity {
        let why = format!(
            "a kept `{}` was compared with a kept `{}`",
            spelled(x.ty),
            spelled(y.ty)
        );
        internal(unsafe { &mut *ctx }, why);
        return 0;
    }
    let entry = x.kept.and_then(|k| if order { k.order } else { k.equal });
    match entry {
        Some(f) => unsafe { f(ctx, x.word, y.word) },
        None => {
            let c = unsafe { &mut *ctx };
            let how = if order { "order" } else { "compare" };
            let what = if order { "" } else { " for equality" };
            c.fail(
                Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("cannot {how} values of `{}`{what}", spelled(x.ty)),
                )
                .primary(Span::DUMMY, "in compiled code"),
            );
            0
        }
    }
}

/// What a kept value is compared through: its word, its type, and the descriptor of its
/// comparisons where a perform kept it.
struct Compared {
    word: Word,
    ty: &'static PlyType,
    kept: Option<&'static PlyKept>,
}

/// What the kept value `h` is compared through, failing the run where it is none or let go of.
fn compared_of(ctx: *mut Ctx, h: &Handle) -> Option<Compared> {
    let kept = object::<Kept>(ctx, h, "a kept value")?;
    let compared = kept.compared;
    match &kept.value {
        Some(v) => Some(Compared {
            word: v.word(),
            ty: v.ty(),
            kept: compared,
        }),
        None => {
            internal(
                unsafe { &mut *ctx },
                "a kept value was compared after it was let go of",
            );
            None
        }
    }
}

/// What `k` keeps, held once more, where it is a value of the type `ty` describes; the run fails
/// otherwise.
pub unsafe extern "C" fn rt_open(ctx: *mut Ctx, k: Handle, ty: *const PlyType) -> Handed {
    let want = unsafe { &*ty };
    let Some(kept) = object::<Kept>(ctx, &k, "a kept value") else {
        return Handed::of(0);
    };
    match &kept.value {
        Some(v) if v.ty().identity == want.identity => v.dup(ctx).handed(),
        Some(v) => {
            let why = format!(
                "a kept `{}` was opened as `{}`",
                spelled(v.ty()),
                spelled(want)
            );
            internal(unsafe { &mut *ctx }, why);
            Handed::of(0)
        }
        None => {
            internal(
                unsafe { &mut *ctx },
                "a kept value was opened after it was let go of",
            );
            Handed::of(0)
        }
    }
}

/// A region's stacks, opened on the running stack, `seeded` (1) for `simulate`, the region written
/// at the site `root`, `start` and `end` name.
pub unsafe extern "C" fn rt_stacks_opened(
    ctx: *mut Ctx,
    seeded: I64,
    root: I64,
    start: I64,
    end: I64,
) -> Handle {
    let c = unsafe { &mut *ctx };
    let id = i64::from(c.entered_sims);
    c.entered_sims += 1;
    let mut region = Region::open(ctx, seeded.0 != 0);
    c.regions.push((&mut *region, [root.0, start.0, end.0]));
    c.rearm_recording();
    let stacks = Stacks {
        region: Some(region),
        id,
        asked: Message::default(),
        heard: Message::default(),
        tasks: HashMap::new(),
    };
    object_made(ctx, Box::new(stacks))
}

/// Where a seeded region open on this context before `s`'s was written: its root, start or end by
/// `which`; a root of -1 where there is none.
pub unsafe extern "C" fn rt_stacks_outer(ctx: *mut Ctx, s: Handle, which: I64) -> I64 {
    let c = unsafe { &*ctx };
    let region = object::<Stacks>(ctx, &s, "a region's stacks").map(|s| s.region());
    let outer = (c.regions.iter())
        .take_while(|(r, _)| Some(*r) != region)
        .filter(|(r, _)| unsafe { (**r).is_seeded() })
        .last()
        .map(|(_, site)| *site);
    I64(match (outer, usize::try_from(which.0)) {
        (Some(site), Ok(i)) if i < 3 => site[i],
        _ => -1,
    })
}

/// Where the innermost region `std.internal.schedule` opened on `c` was written, where it is
/// seeded, so a refusal of what one of its tasks performs can name it.
pub fn seeded_region(c: &Ctx) -> Option<Span> {
    let (region, [root, start, end]) = c.regions.last()?;
    unsafe { (**region).is_seeded() }.then(|| c.span_at(*root, *start, *end))
}

/// `body`, which it takes and hands back, the program's own work running from here: its costs go
/// on from where it left them. The two descriptors are of `body`'s type, as handed and as answered.
pub unsafe extern "C" fn rt_program_resumed(
    ctx: *mut Ctx,
    body: Handed,
    _handed: *const PlyType,
    _answered: *const PlyType,
) -> Handed {
    unsafe { &mut *ctx }.program_entered();
    body
}

/// `answer`, a value of the type `ty` describes, kept as [`rt_keep`] keeps it, the program's own
/// work having ended with it: the keeping is the scheduler's.
pub unsafe extern "C" fn rt_program_paused(
    ctx: *mut Ctx,
    answer: Handed,
    ty: *const PlyType,
) -> Handle {
    unsafe { &mut *ctx }.scheduler_entered();
    unsafe { rt_keep(ctx, answer, ty) }
}

/// A task's or a channel's handle as the same handle at another type, held once more.
pub unsafe extern "C" fn rt_handle_as(_ctx: *mut Ctx, h: Handle) -> Handle {
    Handle::of(held(h.word()))
}

/// A `bracket`'s `acquire` or `release` the running task of the innermost region stands in.
pub(crate) struct Bracketed {
    region: *mut Region,
    task: u64,
    shield: Shield,
}

/// Enters `shield` for the running task of the innermost region running one.
pub(crate) fn shielded(c: &mut Ctx, shield: Shield) -> Option<Bracketed> {
    let (region, _) = c.regions.last().copied()?;
    let task = unsafe { (*region).shield(shield) }?;
    Some(Bracketed {
        region,
        task,
        shield,
    })
}

/// Leaves what [`shielded`] entered: whether a cancel held back for it lands now.
pub(crate) fn unshielded(c: &mut Ctx, entered: Option<Bracketed>) -> bool {
    entered.is_some_and(|b| {
        c.regions.iter().any(|(r, _)| *r == b.region)
            && unsafe { (*b.region).unshield(b.task, b.shield) }
    })
}

// --- The entry's trail -----------------------------------------------------------------------

/// What a seeded step touched, as `std.internal.trail`'s `Access` holds it: an effect's mode atom,
/// its resource where it names one, or a hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Touched {
    Atom {
        effect: ply_core::Symbol,
        resource: Option<ply_core::Symbol>,
        write: bool,
    },
    Hold {
        id: u64,
        write: bool,
    },
}

/// The trail the entry's last seeded region left, as the scheduler wrote it; none before its first.
pub unsafe extern "C" fn rt_trail_entered(ctx: *mut Ctx) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let kept = c.trail_kept.clone().unwrap_or_default();
    c.given(kept)
}

/// The root of the seed the driver set: its high 32 bits for `which` 0, its low ones for 1.
pub unsafe extern "C" fn rt_seed_root(ctx: *mut Ctx, which: I64) -> I64 {
    let root = unsafe { &*ctx }.seed.root;
    I64(if which.0 == 0 {
        (root >> 32) as i64
    } else {
        (root & 0xFFFF_FFFF) as i64
    })
}

/// The seed's path, each choice in two bytes, the least significant first.
pub unsafe extern "C" fn rt_seed_path(ctx: *mut Ctx) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let path: Vec<u8> = c.seed.path.iter().flat_map(|p| p.to_le_bytes()).collect();
    c.given(path)
}

/// The scheduling points a seeded region of the entry may take, as the driver set them.
pub unsafe extern "C" fn rt_seeded_steps(ctx: *mut Ctx) -> I64 {
    I64(i64::from(unsafe { &*ctx }.sim_steps))
}

/// Keeps the trail the `n` bytes at `p` hold for the entry's next region and the interleaving the
/// `m` bytes at `q` hold as its outcome's, each as the scheduler wrote it, which this never reads.
pub unsafe extern "C" fn rt_trail_left(ctx: *mut Ctx, p: Addr, n: I64, q: Addr, m: I64) {
    let c = unsafe { &mut *ctx };
    c.trail_kept = Some(unsafe { lent(p, n) }.to_vec());
    c.interleaving = Some(unsafe { lent(q, m) }.to_vec());
}

/// The site `root`, `start` and `end` name as an outcome places it, as `std.internal.trail`'s
/// `Placed` holds it: the definition it lies in, and its root's name and its span there.
pub unsafe extern "C" fn rt_site_placed(
    ctx: *mut Ctx,
    root: I64,
    start: I64,
    end: I64,
) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let name = c.root_name(root.0).map(|n| n.as_str().to_string());
    let span = c.span_at(root.0, start.0, end.0);
    let mut w = crate::flat::Writer::new();
    w.option(name.as_deref(), |w, n| {
        w.str(n);
    });
    w.option(name.as_deref().filter(|_| !span.is_dummy()), |w, n| {
        w.str(n)
            .count(u64::from(span.start))
            .count(u64::from(span.end));
    });
    let bytes = w.finish();
    c.given(bytes)
}

/// How many accesses the running seeded step touched since its footprint was last taken.
pub unsafe extern "C" fn rt_footprint_held(ctx: *mut Ctx) -> I64 {
    I64(unsafe { &*ctx }.footprint.len() as i64)
}

/// What the running seeded step touched since this was last asked, each access beside where it was
/// made, as `std.internal.trail` reads a `List<Touch>`; what it held is let go.
pub unsafe extern "C" fn rt_footprint_taken(ctx: *mut Ctx) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let touched = std::mem::take(&mut c.footprint);
    let mut w = crate::flat::Writer::new();
    w.list(touched.iter(), |w, (t, site)| {
        match t {
            Touched::Atom {
                effect,
                resource,
                write,
            } => {
                w.tag(0).str(effect.as_str());
                match resource {
                    Some(r) => w.tag(0).str(r.as_str()),
                    None => w.tag(2),
                };
                w.tag(u64::from(*write)).tag(0);
            }
            Touched::Hold { id, write } => {
                w.tag(2).count(*id).tag(u64::from(*write));
            }
        }
        w.int(site[0]).int(site[1]).int(site[2]);
    });
    let bytes = w.finish();
    c.given(bytes)
}

/// The id of the region `s` holds the stacks of, among its entry's regions.
pub unsafe extern "C" fn rt_stacks_region(ctx: *mut Ctx, s: Handle) -> I64 {
    I64(object::<Stacks>(ctx, &s, "a region's stacks").map_or(-1, |s| s.id))
}

/// The body `body`, to answer into the handle `t`, with the descriptor of its row and the words for
/// it its caller holds, which it copies, kept until a region starts it.
pub unsafe extern "C" fn rt_task_body(
    ctx: *mut Ctx,
    t: Handle,
    body: Block,
    desc: *const Word,
    words: *const Word,
) -> Handle {
    let n = if desc.is_null() {
        0
    } else {
        described_words(desc)
    };
    let words = if n == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(words, n) }.to_vec()
    };
    body.dup();
    let started = Started {
        body,
        desc: desc as Word,
        words,
        task: held(t.word()),
    };
    object_made(ctx, Box::new(TaskBody(Some(started))))
}

/// The words a row's descriptor at `desc` takes: one per entry, and one more per label the
/// descriptor leaves to the call.
fn described_words(d: *const Word) -> usize {
    let n = usize::try_from(unsafe { *d }).unwrap_or(0);
    let labels = (0..n)
        .filter(|i| unsafe { *d.add(2 + 2 * i) } == -3)
        .count();
    n + labels
}

unsafe fn run_started(ctx: *mut Ctx, arg: usize) {
    let Started {
        body,
        desc,
        words,
        task,
    } = *unsafe { Box::from_raw(arg as *mut Started) };
    let c = unsafe { &mut *ctx };
    c.entering_desc = desc;
    c.entering_words = if words.is_empty() {
        0
    } else {
        words.as_ptr() as Word
    };
    let answer = unsafe { body.call(ctx, &[]) };
    if unsafe { (*ctx).failed } == 0
        && let Some(t) = unsafe { native_of::<RegionTask>(task) }
    {
        t.answer = Some(answer);
        t.ended = ANSWERED;
    }
    drop(words);
    unsafe { body.let_go_function(ctx) };
    heap::release_object(ctx, task);
}

unsafe fn release_started(ctx: *mut Ctx, arg: usize) {
    unsafe { Box::from_raw(arg as *mut Started) }.release(ctx);
}

/// Task `id` of the region `s` holds, to run what `b` holds the first time the loop resumes it.
pub unsafe extern "C" fn rt_task_started(ctx: *mut Ctx, s: Handle, id: I64, b: Handle) {
    let Some(started) = object::<TaskBody>(ctx, &b, "a task's body").and_then(|b| b.0.take())
    else {
        return;
    };
    let Some(stacks) = object::<Stacks>(ctx, &s, "a region's stacks") else {
        started.release(ctx);
        return;
    };
    let Ok(n) = u64::try_from(id.0) else {
        started.release(ctx);
        return internal(unsafe { &mut *ctx }, "a task was started under no id");
    };
    if let Some(t) = unsafe { native_of::<RegionTask>(started.task) } {
        t.id = id.0;
    }
    if let Some(old) = stacks.tasks.insert(n, held(started.task)) {
        heap::release_object(ctx, old);
    }
    let body = Body {
        run: run_started,
        release: release_started,
        arg: Box::into_raw(Box::new(started)) as usize,
    };
    match &mut stacks.region {
        Some(r) => r.start(n, body),
        None => unsafe { (body.release)(ctx, body.arg) },
    }
}

/// Runs task `id` of the region `s` holds until it asks its loop something, handing it `code` and
/// `n`, and `kept` where one is given: the code of what it asked, which the `asked_*` natives read
/// the rest of.
fn resumed(ctx: *mut Ctx, s: &Handle, id: I64, code: i64, n: i64, kept: Word) -> I64 {
    let Some(stacks) = object::<Stacks>(ctx, s, "a region's stacks") else {
        return I64(tasks_failed(ctx, kept));
    };
    let region = stacks.region();
    let Ok(task) = u64::try_from(id.0) else {
        internal(unsafe { &mut *ctx }, "a task was resumed under no id");
        return I64(tasks_failed(ctx, kept));
    };
    if region.is_null() {
        return I64(tasks_failed(ctx, kept));
    }
    let answer = Message {
        code,
        numbers: [n, 0],
        word: kept,
        site: [0; 3],
    };
    let asked = unsafe { resume(region, task, answer) };
    let stacks = object::<Stacks>(ctx, s, "a region's stacks").expect("the stacks it resumed");
    let left = std::mem::replace(&mut stacks.asked, asked);
    if left.word != 0 {
        heap::release_object(ctx, left.word);
    }
    I64(asked.code)
}

fn tasks_failed(ctx: *mut Ctx, kept: Word) -> i64 {
    if kept != 0 {
        heap::release_object(ctx, kept);
    }
    FAILED
}

pub unsafe extern "C" fn rt_task_resumed(
    ctx: *mut Ctx,
    s: Handle,
    id: I64,
    code: I64,
    n: I64,
) -> I64 {
    resumed(ctx, &s, id, code.0, n.0, 0)
}

/// [`rt_task_resumed`] handing the task `kept` as well, which it lends.
pub unsafe extern "C" fn rt_task_resumed_kept(
    ctx: *mut Ctx,
    s: Handle,
    id: I64,
    code: I64,
    n: I64,
    kept: Handle,
) -> I64 {
    resumed(ctx, &s, id, code.0, n.0, held(kept.word()))
}

/// Resumes task `id` to raise the failure the `n` bytes at `p` hold as the scheduler writes one: a
/// raise of `abort.raise` its perform's site answers.
pub unsafe extern "C" fn rt_task_resumed_raising(
    ctx: *mut Ctx,
    s: Handle,
    id: I64,
    p: Addr,
    n: I64,
) -> I64 {
    let c = unsafe { &mut *ctx };
    let d = failure_of(c, unsafe { lent(p, n) }).unwrap_or_else(|why| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the scheduler wrote a failure the runtime does not read: {why}"),
        )
    });
    if let Some(stacks) = object::<Stacks>(ctx, &s, "a region's stacks")
        && let Some(region) = stacks.region.as_mut()
    {
        region.raising = Some(d);
    }
    resumed(ctx, &s, id, RESUME_RAISE, 0, 0)
}

/// What the task the loop last resumed asked: its `i`th number.
pub unsafe extern "C" fn rt_asked_number(ctx: *mut Ctx, s: Handle, i: I64) -> I64 {
    I64(
        object::<Stacks>(ctx, &s, "a region's stacks").map_or(0, |s| {
            usize::try_from(i.0)
                .ok()
                .and_then(|i| s.asked.numbers.get(i).copied())
                .unwrap_or(0)
        }),
    )
}

/// Where the task the loop last resumed stood as it asked: its root, start or end by `i`.
pub unsafe extern "C" fn rt_asked_site(ctx: *mut Ctx, s: Handle, i: I64) -> I64 {
    I64(
        object::<Stacks>(ctx, &s, "a region's stacks").map_or(0, |s| {
            usize::try_from(i.0)
                .ok()
                .and_then(|i| s.asked.site.get(i).copied())
                .unwrap_or(0)
        }),
    )
}

/// The value the task the loop last resumed handed with what it asked, taken.
pub unsafe extern "C" fn rt_asked_kept(ctx: *mut Ctx, s: Handle) -> Handle {
    taken_word(ctx, &s, |s| &mut s.asked)
}

fn taken_word(ctx: *mut Ctx, s: &Handle, of: impl FnOnce(&mut Stacks) -> &mut Message) -> Handle {
    let Some(stacks) = object::<Stacks>(ctx, s, "a region's stacks") else {
        return Handle::of(0);
    };
    let w = std::mem::take(&mut of(stacks).word);
    if w == 0 {
        internal(
            unsafe { &mut *ctx },
            "a kept value was taken where none was handed",
        );
    }
    Handle::of(w)
}

/// The running task of the region `s` holds hands its loop `code`, `a` and `b`, and `kept` where
/// one is given, placed where the perform its clause answers stood, and waits: the code the loop
/// resumes it with, which `heard_*` read the rest of. A task resumed to unwind or to raise leaves
/// the context failed so, its clause never running on.
fn yielded(ctx: *mut Ctx, s: &Handle, code: i64, numbers: [i64; 2], kept: Word) -> I64 {
    let Some(stacks) = object::<Stacks>(ctx, s, "a region's stacks") else {
        return I64(tasks_failed(ctx, kept));
    };
    let region = stacks.region();
    if region.is_null() {
        return I64(tasks_failed(ctx, kept));
    }
    let c = unsafe { &*ctx };
    let request = Message {
        code,
        numbers,
        word: kept,
        site: [c.performed_root, c.performed_start, c.performed_end],
    };
    let heard = unsafe { to_loop(region, request) };
    let stacks = object::<Stacks>(ctx, s, "a region's stacks").expect("the stacks it ran under");
    let left = std::mem::replace(&mut stacks.heard, heard);
    if left.word != 0 {
        heap::release_object(ctx, left.word);
    }
    unsafe { runs_on(region, &heard) };
    I64(heard.code)
}

pub unsafe extern "C" fn rt_task_yielded(
    ctx: *mut Ctx,
    s: Handle,
    code: I64,
    a: I64,
    b: I64,
) -> I64 {
    yielded(ctx, &s, code.0, [a.0, b.0], 0)
}

/// [`rt_task_yielded`] handing the loop `kept` as well, which it lends.
pub unsafe extern "C" fn rt_task_yielded_kept(
    ctx: *mut Ctx,
    s: Handle,
    code: I64,
    a: I64,
    b: I64,
    kept: Handle,
) -> I64 {
    yielded(ctx, &s, code.0, [a.0, b.0], held(kept.word()))
}

/// The number the loop resumed the running task with.
pub unsafe extern "C" fn rt_heard_number(ctx: *mut Ctx, s: Handle) -> I64 {
    I64(object::<Stacks>(ctx, &s, "a region's stacks").map_or(0, |s| s.heard.numbers[0]))
}

/// The value the loop resumed the running task with, taken.
pub unsafe extern "C" fn rt_heard_kept(ctx: *mut Ctx, s: Handle) -> Handle {
    taken_word(ctx, &s, |s| &mut s.heard)
}

/// Lets go of task `id`: its stack back to the region, a body it never ran let go, and its handle.
pub unsafe extern "C" fn rt_task_freed(ctx: *mut Ctx, s: Handle, id: I64) {
    let Some(stacks) = object::<Stacks>(ctx, &s, "a region's stacks") else {
        return;
    };
    let Ok(n) = u64::try_from(id.0) else {
        return;
    };
    let mut production = false;
    if let Some(r) = &mut stacks.region {
        r.free(n);
        production = !r.is_seeded();
    }
    if let Some(t) = stacks.tasks.remove(&n) {
        heap::release_object(ctx, t);
    }
    // A task of the run's region may hold spans of the run's trace open, which close as it ends.
    if production {
        let c = unsafe { &mut *ctx };
        c.host_runtime().end_task(c.id, TaskId(n));
    }
}

/// Puts back how task `id`'s body left the context where it did not answer, with the note the `n`
/// bytes at `p` hold where there are any: 1, or 0 where the task left nothing.
pub unsafe extern "C" fn rt_task_restored(
    ctx: *mut Ctx,
    s: Handle,
    id: I64,
    p: Addr,
    n: I64,
) -> I64 {
    let note = (n.0 > 0).then(|| String::from_utf8_lossy(unsafe { lent(p, n) }).into_owned());
    let restored = object::<Stacks>(ctx, &s, "a region's stacks").is_some_and(|stacks| {
        let Ok(task) = u64::try_from(id.0) else {
            return false;
        };
        stacks
            .region
            .as_mut()
            .is_some_and(|r| r.restore(task, note))
    });
    I64(i64::from(restored))
}

/// The brackets task `id` stands in: how many `acquire`s, how many `release`s, and whether a
/// cancel waits for it to leave them (1), by `which`.
pub unsafe extern "C" fn rt_task_shields(ctx: *mut Ctx, s: Handle, id: I64, which: I64) -> I64 {
    let shields = object::<Stacks>(ctx, &s, "a region's stacks").map_or(Shields::default(), |s| {
        let task = u64::try_from(id.0).unwrap_or(u64::MAX);
        s.region
            .as_ref()
            .map_or(Shields::default(), |r| r.shields(task))
    });
    I64(match which.0 {
        0 => i64::from(shields.acquiring),
        1 => i64::from(shields.releasing),
        _ => i64::from(shields.stopping),
    })
}

/// Whether a cancel of task `id` waits for it to leave the brackets it stands in.
pub unsafe extern "C" fn rt_task_held_back(ctx: *mut Ctx, s: Handle, id: I64, stopping: I64) {
    if let Some(stacks) = object::<Stacks>(ctx, &s, "a region's stacks")
        && let (Ok(task), Some(r)) = (u64::try_from(id.0), &mut stacks.region)
    {
        r.hold_back(task, stopping.0 != 0);
    }
}

/// Records on task `id`'s handle that it ended cancelled.
pub unsafe extern "C" fn rt_task_settled(ctx: *mut Ctx, s: Handle, id: I64) {
    let Some(stacks) = object::<Stacks>(ctx, &s, "a region's stacks") else {
        return;
    };
    if let Some(t) = u64::try_from(id.0)
        .ok()
        .and_then(|n| stacks.tasks.get(&n).copied())
        && let Some(t) = unsafe { native_of::<RegionTask>(t) }
    {
        t.ended = CANCELLED;
    }
}

/// A handle of a task of region `region`, its id set as the region starts it.
pub unsafe extern "C" fn rt_task_made(ctx: *mut Ctx, region: I64) -> Handle {
    let t = RegionTask {
        region: region.0,
        id: -1,
        ended: LIVE,
        answer: None,
    };
    object_made(ctx, Box::new(t))
}

/// What the task's handle `t` says, by `which`: its region, its id, or how it ended.
pub unsafe extern "C" fn rt_task_said(ctx: *mut Ctx, t: Handle, which: I64) -> I64 {
    I64(
        object::<RegionTask>(ctx, &t, "a task's handle").map_or(-1, |t| match which.0 {
            0 => t.region,
            1 => t.id,
            _ => t.ended,
        }),
    )
}

/// What the task `t` names answered, held once more; the run fails where it has not answered.
pub unsafe extern "C" fn rt_task_answer(ctx: *mut Ctx, t: Handle) -> Handle {
    match object::<RegionTask>(ctx, &t, "a task's handle").and_then(|t| t.answer) {
        Some(w) => Handle::of(held(w)),
        None => {
            internal(
                unsafe { &mut *ctx },
                "a task's answer was read before it answered",
            );
            Handle::of(0)
        }
    }
}

/// A handle of channel `id` of region `region`.
pub unsafe extern "C" fn rt_chan_made(ctx: *mut Ctx, region: I64, id: I64) -> Handle {
    object_made(
        ctx,
        Box::new(RegionChan {
            region: region.0,
            id: id.0,
        }),
    )
}

/// What the channel's handle `c` says, by `which`: its region or its id.
pub unsafe extern "C" fn rt_chan_said(ctx: *mut Ctx, c: Handle, which: I64) -> I64 {
    I64(
        object::<RegionChan>(ctx, &c, "a channel's handle").map_or(-1, |c| match which.0 {
            0 => c.region,
            _ => c.id,
        }),
    )
}

// --- A region's own failures -----------------------------------------------------------

/// The failure `bytes` hold as the scheduler writes one (`std.internal.schedule`'s `Failure`): its
/// code and message, each label's site, whether it is the primary one and its text, then its notes.
pub fn failure_of(c: &Ctx, bytes: &[u8]) -> Result<Diagnostic, String> {
    let mut r = Reader::new(bytes);
    let code = r.str()?;
    let message = r.str()?;
    let labels = r.list(|r| {
        let site = [r.int()?, r.int()?, r.int()?];
        Ok((site, r.bool()?, r.str()?))
    })?;
    let notes = r.list(|r| r.str())?;
    r.end()?;
    let failure = match code.as_str() {
        c if c == codes::TASK_ESCAPES_SCOPE => {
            Diagnostic::error(codes::TASK_ESCAPES_SCOPE, message)
        }
        c if c == codes::DEADLOCK => Diagnostic::error(codes::DEADLOCK, message),
        c if c == codes::SIMULATION_DIVERGENCE => {
            Diagnostic::error(codes::SIMULATION_DIVERGENCE, message)
        }
        c if c == codes::NESTED_SIMULATION => Diagnostic::error(codes::NESTED_SIMULATION, message),
        c if c == codes::RUNTIME_ERROR => Diagnostic::error(codes::RUNTIME_ERROR, message),
        c if c == codes::INTERNAL_ERROR => Diagnostic::error(codes::INTERNAL_ERROR, message),
        other => return Err(format!("`{other}` is no code a region fails with")),
    };
    let labelled = labels
        .into_iter()
        .fold(failure, |d, ([root, start, end], primary, text)| {
            let span = c.span_at(root, start, end);
            if primary {
                d.primary(span, text)
            } else {
                d.secondary(span, text)
            }
        });
    Ok(notes.into_iter().fold(labelled, |d, n| d.note(n)))
}

/// Fails the run with the region's failure the bytes at `p` hold.
pub unsafe extern "C" fn rt_region_failed(ctx: *mut Ctx, p: Addr, n: I64) {
    let c = unsafe { &mut *ctx };
    let d = failure_of(c, unsafe { lent(p, n) }).unwrap_or_else(|why| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the scheduler wrote a failure the runtime does not read: {why}"),
        )
    });
    c.fail(d);
}

// --- The production region's waits ---------------------------------------------------

/// The ticket `token` names, as the run that answers it holds it.
fn ticket(token: I64) -> Pending {
    Pending {
        token: token.0 as u64,
        label: "a native's job",
    }
}

/// What `f` answers of the context's run, which holds its tickets; `refused`, with the run failed,
/// where no host made the context's runtime.
fn with_run<R>(ctx: *mut Ctx, refused: R, f: impl FnOnce(&mut Ctx, &Run) -> R) -> R {
    let c = unsafe { &mut *ctx };
    let runtime = c.host_runtime();
    match ply_host::registry::run_of(&*runtime) {
        Ok(run) => f(c, run),
        Err(d) => {
            c.fail(d);
            refused
        }
    }
}

/// Has `tickets_answered` say once `token` is answered: 1, or 0 with the run failed for a ticket
/// the run did not hand out.
pub unsafe extern "C" fn rt_ticket_watch(ctx: *mut Ctx, token: I64) -> I64 {
    with_run(ctx, I64(0), |c, run| match run.watch(&ticket(token)) {
        Ok(()) => I64(1),
        Err(d) => {
            c.fail(d);
            I64(0)
        }
    })
}

/// The watched tickets answered and not yet taken, ascending, each token eight bytes, least
/// significant first; their answers stay for the task that waited on each.
pub unsafe extern "C" fn rt_tickets_answered(ctx: *mut Ctx) -> TakenBytes {
    with_run(ctx, TakenBytes(0), |c, run| {
        let bytes: Vec<u8> = (run.answered().iter())
            .flat_map(|t| t.to_le_bytes())
            .collect();
        c.given(bytes)
    })
}

/// The bytes ticket `token` was answered, taken: a job that failed fails the run, naming what
/// asked for it, as does a ticket not yet answered.
pub fn ticket_taken(ctx: *mut Ctx, token: I64) -> TakenBytes {
    with_run(ctx, TakenBytes(0), |c, run| taken(c, run, token))
}

fn taken(c: &mut Ctx, run: &Run, token: I64) -> TakenBytes {
    let answer = match run.taken(&ticket(token)) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            c.fail(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("ticket {} was taken before it was answered", token.0),
            ));
            return TakenBytes(0);
        }
        Err(d) => {
            c.fail(d);
            return TakenBytes(0);
        }
    };
    c.given(answer)
}

/// Blocks until a watched ticket is answered, the run is stopped, or the run's clock reaches
/// `deadline` where it is not below 0: 1, or 0 with the run failed where it watches nothing and has
/// no deadline, which no answer would end.
pub unsafe extern "C" fn rt_tickets_park(ctx: *mut Ctx, deadline: I64) -> I64 {
    let c = unsafe { &mut *ctx };
    let runtime = c.host_runtime();
    let parked = if deadline.0 < 0 {
        runtime.park()
    } else {
        runtime.park_until(deadline.0)
    };
    match parked {
        Ok(()) => I64(1),
        Err(d) => {
            c.fail(d);
            I64(0)
        }
    }
}

/// Notes what the run held as its root's body returned, before the region `s` holds the stacks of
/// drains its other tasks: `scheduled` tasks the region still holds, and the stacks it keeps.
pub unsafe extern "C" fn rt_root_returned(ctx: *mut Ctx, s: Handle, scheduled: I64) {
    let Some(stacks) = object::<Stacks>(ctx, &s, "a region's stacks") else {
        return;
    };
    let task_stacks = stacks.region.as_ref().map_or(0, |r| r.stacks());
    let c = unsafe { &mut *ctx };
    c.held_at_root = Some(crate::entry::Held {
        native_objects: c.heap.natives() as u64,
        scheduled: u64::try_from(scheduled.0).unwrap_or(0),
        task_stacks: task_stacks as u64,
        frame_tables: c.stack_slots() as u64,
    });
}

/// The run's clock: nanoseconds since it began. 0, with the run failed, where it has none.
pub unsafe extern "C" fn rt_host_now(ctx: *mut Ctx) -> I64 {
    let c = unsafe { &mut *ctx };
    match c.host_runtime().now() {
        Ok(now) => I64(now),
        Err(d) => {
            c.fail(d);
            I64(0)
        }
    }
}

/// 1 where the run has been told to stop, else 0.
pub unsafe extern "C" fn rt_run_stopping(ctx: *mut Ctx) -> I64 {
    let c = unsafe { &mut *ctx };
    I64(i64::from(c.host_runtime().stopping()))
}

/// 1, with the run failed saying so, where the run's drain after a stop has run out of time; 0
/// otherwise.
pub unsafe extern "C" fn rt_run_drained(ctx: *mut Ctx) -> I64 {
    let c = unsafe { &mut *ctx };
    match c.host_runtime().drain_expired() {
        Some(d) => {
            c.fail(d);
            I64(1)
        }
        None => I64(0),
    }
}

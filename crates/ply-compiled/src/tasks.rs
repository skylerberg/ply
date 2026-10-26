//! The stacks a region's tasks run on, and the switch between each task and the region's loop.
//! Which task runs next and what a request means are the scheduler's, written in Ply
//! (`std.internal.schedule`); a region here holds what Ply cannot: each task's stack and where it
//! stands, where the loop stands, the brackets each task stands in, and how a body that did not
//! answer left the context. Then the production region's waits: the run's tickets and its clock.

use crate::flat::Reader;
use crate::heap::Word;
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{
    Aborting, Ctx, FAILED_ABORT, FAILED_CANCELLED, FAILED_PENDING, FAILED_TARGET, FAILED_UNWIND,
    drop_frame, lent, no_stack_to_take, switch_keeping,
};
use crate::stack::{Stack, switch};
use ply_core::host::Pending;
use ply_core::{Diagnostic, Span, codes};
use ply_host::pool::Run;
use std::collections::HashMap;

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
    pending: Option<String>,
    unwind: Option<(usize, usize, Word)>,
    aborting: Option<Aborting>,
    target: Word,
}

impl Stashed {
    fn release(self) {
        if let Some(a) = self.aborting {
            a.args.release();
        }
    }
}

#[derive(Default)]
struct Slot {
    stack: Option<Stack>,
    sp: usize,
    body: Option<Body>,
    /// The task's frame stack, chained to the region's, once it has started.
    frames: Option<usize>,
    shields: Shields,
    stashed: Option<Stashed>,
}

/// One open region's tasks. Boxed, so a task's stack can name it while it runs.
pub struct Region {
    ctx: *mut Ctx,
    seeded: bool,
    site: Span,
    /// The frame stack the region was opened on, which each task's chains to.
    frames: usize,
    loop_sp: usize,
    running: Option<u64>,
    slots: HashMap<u64, Box<Slot>>,
    /// Stacks the region's ended tasks gave back, for its next ones: at most as many as it ever ran
    /// at once.
    spare: Vec<Stack>,
    live: usize,
    peak: usize,
    message: Message,
}

impl Region {
    /// A region opened on the running stack of `ctx`, `seeded` for `simulate`.
    pub fn open(ctx: *mut Ctx, seeded: bool, site: Span) -> Box<Region> {
        Box::new(Region {
            ctx,
            seeded,
            site,
            frames: unsafe { (*ctx).current },
            loop_sp: 0,
            running: None,
            slots: HashMap::new(),
            spare: Vec::new(),
            live: 0,
            peak: 0,
            message: Message::default(),
        })
    }

    pub fn is_seeded(&self) -> bool {
        self.seeded
    }

    pub fn site(&self) -> Span {
        self.site
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

    /// Task `id`, to run `body` the first time the loop resumes it.
    pub fn start(&mut self, id: u64, body: Body) {
        let slot = Slot {
            body: Some(body),
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
        c.unwind = s.unwind;
        c.aborting = s.aborting;
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
        if let Some(frames) = slot.frames {
            for f in std::mem::take(&mut c.stacks[frames].list) {
                drop_frame(f);
            }
            c.close_stack(frames);
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
        let frames = c.open_stack(Some(r.frames));
        c.stacks[frames].entered_from = Some(r.frames);
        slot.frames = Some(frames);
        r.live += 1;
        r.peak = r.peak.max(r.live);
    }
    let to = slot.sp;
    c.current = slot.frames.unwrap_or(r.frames);
    r.message = answer;
    r.running = Some(id);
    unsafe { switch_keeping(ctx, &raw mut r.loop_sp, to) };
    let r = unsafe { &mut *region };
    r.running = None;
    unsafe { (*ctx).current = r.frames };
    r.message
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
        (r.ctx, id, slot.body.take())
    };
    if let Some(body) = body {
        unsafe { (body.run)(ctx, body.arg) };
    }
    let c = unsafe { &mut *ctx };
    let site = [c.site_root, c.site_start, c.site_end];
    let (code, stashed) = match c.failed {
        0 => (FINISHED, None),
        FAILED_CANCELLED => {
            c.failed = 0;
            (UNWOUND, None)
        }
        failed => {
            let raised = matches!(
                failed,
                FAILED_ABORT | FAILED_UNWIND | FAILED_TARGET | FAILED_PENDING
            );
            let s = Stashed {
                failed,
                diagnostic: c.diagnostic.take(),
                pending: c.pending.take(),
                unwind: c.unwind.take(),
                aborting: c.aborting.take(),
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
/// significant first; their answers stay for `ticket_taken`.
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
pub unsafe extern "C" fn rt_ticket_taken(ctx: *mut Ctx, token: I64) -> TakenBytes {
    with_run(ctx, TakenBytes(0), |c, run| taken(c, run, token))
}

fn taken(c: &mut Ctx, run: &Run, token: I64) -> TakenBytes {
    let answer = match run.poll(&ticket(token)) {
        Ok(Some(ply_core::Value::Bytes(ref bytes))) => bytes.to_vec(),
        Ok(_) => {
            c.fail(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!(
                    "ticket {} was taken before it was answered with bytes",
                    token.0
                ),
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

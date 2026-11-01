use crate::emitted::{Block, Carried, Handle, OwnedWord, PlyKept, PlyType};
use crate::flat::Reader;
use crate::heap::{self, Native, Word, native_of};
use crate::native::{Address, I64, TakenBytes};
use crate::rt::{
    Context, FAILED_CANCELLED, FAILED_PENDING_RAISE, FAILED_UNWOUND_TO_HANDLER, Unhandled,
    borrowed, fail_out_of_stack, switch_stack_preserving_context,
};
use crate::stack::{Stack, switch};
use ply_core::host::Pending;
use ply_core::sim::TaskId;
use ply_core::{Diagnostic, Span, codes};
use ply_host::pool::Run;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

pub const FINISHED: i64 = -1;
pub const UNWOUND: i64 = -2;
pub const RAISED: i64 = -3;
pub const FAILED: i64 = -4;

pub struct Body {
    pub run: unsafe fn(*mut Context, usize),
    pub release: unsafe fn(*mut Context, usize),
    pub arg: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub code: i64,
    pub numbers: [i64; 2],
    pub word: Word,
    pub site: [i64; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shields {
    pub acquiring: u32,
    pub releasing: u32,
    pub stopping: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BracketPhase {
    Acquire,
    Release,
}

struct SavedFailure {
    failed: i64,
    diagnostic: Option<Diagnostic>,
    pending: Option<Unhandled>,
    target: Word,
}

impl SavedFailure {
    fn release(self) {
        if let Some(p) = self.pending {
            p.release();
        }
    }
}

#[derive(Default)]
struct Slot {
    stack: Option<Stack>,
    stack_pointer: usize,
    body: Option<Body>,
    stack_index: Option<usize>,
    shields: Shields,
    saved_failure: Option<SavedFailure>,
    context_task_number: Option<i64>,
}

pub struct Scheduler {
    context: *mut Context,
    seeded: bool,
    opening_stack_index: usize,
    loop_stack_pointer: usize,
    running: Option<u64>,
    slots: HashMap<u64, Box<Slot>>,
    spare: Vec<Stack>,
    live: usize,
    peak: usize,
    message: Message,
    raising: Option<Diagnostic>,
}

impl Scheduler {
    pub fn open(context: *mut Context, seeded: bool) -> Box<Scheduler> {
        Box::new(Scheduler {
            context,
            seeded,
            opening_stack_index: unsafe { (*context).current },
            loop_stack_pointer: 0,
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

    pub fn is_stepping(&self) -> bool {
        self.seeded && self.running.is_some()
    }

    pub fn running(&self) -> Option<u64> {
        self.running
    }

    pub fn tasks(&self) -> usize {
        self.slots.len()
    }

    pub fn stacks(&self) -> usize {
        self.live + self.spare.len()
    }

    pub fn start(&mut self, id: u64, body: Body) {
        let slot = Slot {
            body: Some(body),
            context_task_number: (id != 0).then(task_number),
            ..Slot::default()
        };
        if let Some(old) = self.slots.insert(id, Box::new(slot)) {
            unsafe { self.freed(*old) };
        }
    }

    pub fn shields(&self, id: u64) -> Shields {
        self.slots.get(&id).map(|s| s.shields).unwrap_or_default()
    }

    pub fn shielded(&self) -> Vec<(u64, Shields)> {
        let mut out: Vec<(u64, Shields)> = (self.slots.iter())
            .filter(|(_, s)| s.shields != Shields::default())
            .map(|(id, s)| (*id, s.shields))
            .collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    pub fn set_stopping(&mut self, id: u64, stopping: bool) {
        if let Some(slot) = self.slots.get_mut(&id) {
            slot.shields.stopping = stopping;
        }
    }

    pub fn shield(&mut self, shield: BracketPhase) -> Option<u64> {
        let id = self.running?;
        let slot = self.slots.get_mut(&id)?;
        match shield {
            BracketPhase::Acquire => slot.shields.acquiring += 1,
            BracketPhase::Release => slot.shields.releasing += 1,
        }
        Some(id)
    }

    pub fn leave_phase(&mut self, id: u64, shield: BracketPhase) -> bool {
        let Some(slot) = self.slots.get_mut(&id) else {
            return false;
        };
        let s = &mut slot.shields;
        match shield {
            BracketPhase::Acquire => s.acquiring = s.acquiring.saturating_sub(1),
            BracketPhase::Release => s.releasing = s.releasing.saturating_sub(1),
        }
        let lands = s.stopping && s.acquiring == 0 && s.releasing == 0;
        if lands {
            s.stopping = false;
        }
        lands
    }

    pub fn restore(&mut self, id: u64, note: Option<String>) -> bool {
        let Some(s) = self
            .slots
            .get_mut(&id)
            .and_then(|slot| slot.saved_failure.take())
        else {
            return false;
        };
        let c = unsafe { &mut *self.context };
        c.failed = s.failed;
        c.diagnostic = match (s.diagnostic, note) {
            (Some(d), Some(n)) => Some(d.note(n)),
            (d, _) => d,
        };
        c.pending = s.pending;
        c.target = s.target;
        true
    }

    pub fn free(&mut self, id: u64) {
        if let Some(slot) = self.slots.remove(&id) {
            unsafe { self.freed(*slot) };
        }
    }

    unsafe fn freed(&mut self, slot: Slot) {
        let c = unsafe { &mut *self.context };
        if let Some(body) = slot.body {
            unsafe { (body.release)(self.context, body.arg) };
        }
        if let Some(s) = slot.saved_failure {
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

impl Drop for Scheduler {
    fn drop(&mut self) {
        let ids: Vec<u64> = self.slots.keys().copied().collect();
        for id in ids {
            self.free(id);
        }
    }
}

pub unsafe fn resume(scheduler: *mut Scheduler, id: u64, result: Message) -> Message {
    let r = unsafe { &mut *scheduler };
    let context = r.context;
    let c = unsafe { &mut *context };
    let Some(slot) = r.slots.get_mut(&id) else {
        c.fail(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the scheduler was asked to run @{id}, which it does not hold"),
        ));
        return ended(FAILED);
    };
    if slot.stack.is_none() {
        let Some(stack) = r.spare.pop().or_else(Stack::take) else {
            fail_out_of_stack(c);
            return ended(FAILED);
        };
        slot.stack_pointer = stack.prepare(task_entry, scheduler as usize);
        slot.stack = Some(stack);
        slot.stack_index = Some(c.open_stack());
        r.live += 1;
        r.peak = r.peak.max(r.live);
    }
    let to = slot.stack_pointer;
    c.current = slot.stack_index.unwrap_or(r.opening_stack_index);
    r.message = result;
    r.running = Some(id);
    unsafe { switch_stack_preserving_context(context, &raw mut r.loop_stack_pointer, to) };
    let r = unsafe { &mut *scheduler };
    r.running = None;
    unsafe { (*context).current = r.opening_stack_index };
    r.message
}

pub(crate) fn task_number() -> i64 {
    static NEXT: AtomicI64 = AtomicI64::new(-1);
    NEXT.fetch_sub(1, Ordering::Relaxed)
}

pub unsafe fn switch_to_loop(scheduler: *mut Scheduler, request: Message) -> Message {
    let r = unsafe { &mut *scheduler };
    let Some(id) = r.running else {
        let c = unsafe { &mut *r.context };
        c.fail(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "a task asked its scheduler something while no task of it was running",
        ));
        return ended(FAILED);
    };
    let slot = r.slots.get_mut(&id).expect("a running task has its slot");
    r.message = request;
    let from = &raw mut slot.stack_pointer;
    unsafe { switch_stack_preserving_context(r.context, from, r.loop_stack_pointer) };
    unsafe { (*scheduler).message }
}

unsafe fn continues_running(scheduler: *mut Scheduler, heard: &Message) -> bool {
    let r = unsafe { &mut *scheduler };
    let c = unsafe { &mut *r.context };
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
            c.fail_unhandled_abort(d, message);
            false
        }
        _ => c.failed == 0,
    }
}

pub const REQUEST_SLEEP: i64 = 11;
pub const REQUEST_JOB_RESULT: i64 = 15;

fn running_unseeded_scheduler(c: &Context) -> Option<*mut Scheduler> {
    let (scheduler, _) = c.schedulers.last()?;
    let r = unsafe { &**scheduler };
    (!r.is_seeded() && r.running().is_some()).then_some(*scheduler)
}

pub(crate) fn running_unseeded_task(c: &Context) -> Option<TaskId> {
    let scheduler = running_unseeded_scheduler(c)?;
    unsafe { (*scheduler).running() }.map(TaskId)
}

pub(crate) fn waited(c: &mut Context, code: i64, n: i64) -> Option<bool> {
    let scheduler = running_unseeded_scheduler(c)?;
    let request = Message {
        code,
        numbers: [n, 0],
        word: 0,
        site: [c.performed_root, c.performed_start, c.performed_end],
    };
    let was = c.begin_scheduling();
    let context: *mut Context = c;
    let heard = unsafe { switch_to_loop(scheduler, request) };
    if !was {
        unsafe { &mut *context }.program_entered();
    }
    Some(unsafe { continues_running(scheduler, &heard) })
}

fn ended(code: i64) -> Message {
    Message {
        code,
        ..Message::default()
    }
}

extern "C" fn task_entry(arg: usize) {
    let scheduler = arg as *mut Scheduler;
    let (context, id, body) = {
        let r = unsafe { &mut *scheduler };
        let id = r.running.expect("a task starts as its loop resumes it");
        let slot = r.slots.get_mut(&id).expect("a starting task has its slot");
        let c = unsafe { &mut *r.context };
        let own = slot.stack.as_ref().expect("a task starts on its own stack");
        c.stack_floor
            .store(unsafe { own.enter() }, Ordering::Relaxed);
        c.frames = std::ptr::null_mut();
        if let Some(n) = slot.context_task_number {
            c.task = n;
        }
        (r.context, id, slot.body.take())
    };
    if let Some(body) = body {
        unsafe { (body.run)(context, body.arg) };
    }
    let c = unsafe { &mut *context };
    c.begin_scheduling();
    let site = [c.site_root, c.site_start, c.site_end];
    let (code, saved_failure) = match c.failed {
        0 => (FINISHED, None),
        FAILED_CANCELLED => {
            c.failed = 0;
            (UNWOUND, None)
        }
        failed => {
            let raised = matches!(failed, FAILED_UNWOUND_TO_HANDLER | FAILED_PENDING_RAISE);
            let s = SavedFailure {
                failed,
                diagnostic: c.diagnostic.take(),
                pending: c.pending.take(),
                target: std::mem::take(&mut c.target),
            };
            c.failed = 0;
            (if raised { RAISED } else { FAILED }, Some(s))
        }
    };
    let r = unsafe { &mut *scheduler };
    let slot = r.slots.get_mut(&id).expect("an ending task has its slot");
    slot.saved_failure = saved_failure;
    r.message = Message {
        code,
        site,
        ..Message::default()
    };
    let from = &raw mut slot.stack_pointer;
    unsafe { switch(&mut *from, r.loop_stack_pointer) };
    std::process::abort();
}

pub fn natives() -> Vec<crate::c::MachineNative> {
    use crate::c::runtime_native as n;
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

pub const RESUME_UNWIND: i64 = -1;
pub const RESUME_RAISE: i64 = -2;

pub const LIVE: i64 = 0;
pub const RETURNED: i64 = 1;
pub const CANCELLED: i64 = 2;

pub struct StoredValue {
    value: Option<Carried>,
    compared: Option<&'static PlyKept>,
}

impl Native for StoredValue {
    fn release(&mut self, context: *mut Context) {
        if let Some(v) = self.value.take() {
            v.release(context);
        }
    }
}

pub struct Stacks {
    scheduler: Option<Box<Scheduler>>,
    id: i64,
    request: Message,
    response: Message,
    tasks: HashMap<u64, Word>,
}

impl Stacks {
    fn scheduler(&mut self) -> *mut Scheduler {
        match &mut self.scheduler {
            Some(r) => &mut **r,
            None => std::ptr::null_mut(),
        }
    }
}

impl Native for Stacks {
    fn release(&mut self, context: *mut Context) {
        let scheduler = self.scheduler();
        let c = unsafe { &mut *context };
        c.schedulers.retain(|(r, _)| *r != scheduler);
        c.refresh_recording_flag();
        drop(self.scheduler.take());
        for w in [self.request.word, self.response.word] {
            if w != 0 {
                heap::release_object(context, w);
            }
        }
        for (_, t) in self.tasks.drain() {
            heap::release_object(context, t);
        }
    }
}

impl Drop for Stacks {
    fn drop(&mut self) {
        if let Some(r) = self.scheduler.take() {
            std::mem::forget(r);
        }
    }
}

pub struct TaskBody(Option<Started>);

struct Started {
    body: Block,
    descriptor: Word,
    words: Vec<Word>,
    task: Word,
}

impl Started {
    fn release(self, context: *mut Context) {
        unsafe { self.body.release_function_if_last(context) };
        heap::release_object(context, self.task);
    }
}

impl Native for TaskBody {
    fn release(&mut self, context: *mut Context) {
        if let Some(s) = self.0.take() {
            s.release(context);
        }
    }
}

pub struct SchedulerTask {
    scheduler: i64,
    id: i64,
    ended: i64,
    result: Option<Word>,
}

impl Native for SchedulerTask {
    fn release(&mut self, context: *mut Context) {
        if let Some(w) = self.result.take() {
            heap::release_object(context, w);
        }
    }

    fn escapes(&self) -> Option<ply_core::escape::Handle> {
        Some(ply_core::escape::Handle::Task)
    }
}

pub struct SchedulerChannel {
    scheduler: i64,
    id: i64,
}

impl Native for SchedulerChannel {
    fn release(&mut self, _context: *mut Context) {}

    fn escapes(&self) -> Option<ply_core::escape::Handle> {
        Some(ply_core::escape::Handle::Channel)
    }
}

fn internal(c: &mut Context, reason: impl Into<String>) {
    c.fail(Diagnostic::error(codes::INTERNAL_ERROR, reason));
}

fn spelled(t: &PlyType) -> String {
    let bytes = unsafe { std::slice::from_raw_parts(t.spelled, t.spelled_len as usize) };
    String::from_utf8_lossy(bytes).into_owned()
}

fn object<'a, T: Native>(context: *mut Context, h: &Handle, what: &str) -> Option<&'a mut T> {
    let found = unsafe { native_of::<T>(h.word()) };
    if found.is_none() {
        internal(
            unsafe { &mut *context },
            format!("a native was handed {what} that is none"),
        );
    }
    found
}

fn duplicated(w: Word) -> Word {
    Block::of(w).dup();
    w
}

fn object_made(context: *mut Context, held: Box<dyn Native>) -> Handle {
    Handle::of(unsafe { (*context).heap.native(held) })
}

pub unsafe extern "C" fn rt_keep(
    context: *mut Context,
    x: OwnedWord,
    ty: *const PlyType,
) -> Handle {
    let value = Some(unsafe { Carried::new(x, ty) });
    object_made(
        context,
        Box::new(StoredValue {
            value,
            compared: None,
        }),
    )
}

pub unsafe extern "C" fn rt_keep_compared(
    context: *mut Context,
    x: OwnedWord,
    kept: *const PlyKept,
) -> Handle {
    let kept = unsafe { &*kept };
    let value = Some(unsafe { Carried::new(x, kept.ty) });
    object_made(
        context,
        Box::new(StoredValue {
            value,
            compared: Some(kept),
        }),
    )
}

pub unsafe extern "C" fn rt_kept_equal(context: *mut Context, a: Handle, b: Handle) -> I64 {
    I64(i64::from(
        compare_stored_values(context, &a, &b, false) != 0,
    ))
}

pub unsafe extern "C" fn rt_kept_order(context: *mut Context, a: Handle, b: Handle) -> I64 {
    I64(compare_stored_values(context, &a, &b, true).signum())
}

fn compare_stored_values(context: *mut Context, a: &Handle, b: &Handle, order: bool) -> i64 {
    let (Some(x), Some(y)) = (
        comparison_operand(context, a),
        comparison_operand(context, b),
    ) else {
        return 0;
    };
    if x.ty.identity != y.ty.identity {
        let reason = format!(
            "a kept `{}` was compared with a kept `{}`",
            spelled(x.ty),
            spelled(y.ty)
        );
        internal(unsafe { &mut *context }, reason);
        return 0;
    }
    let entry = x.kept.and_then(|k| if order { k.order } else { k.equal });
    match entry {
        Some(f) => unsafe { f(context, x.word, y.word) },
        None => {
            let c = unsafe { &mut *context };
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

struct ComparisonOperand {
    word: Word,
    ty: &'static PlyType,
    kept: Option<&'static PlyKept>,
}

fn comparison_operand(context: *mut Context, h: &Handle) -> Option<ComparisonOperand> {
    let kept = object::<StoredValue>(context, h, "a kept value")?;
    let compared = kept.compared;
    match &kept.value {
        Some(v) => Some(ComparisonOperand {
            word: v.word(),
            ty: v.ty(),
            kept: compared,
        }),
        None => {
            internal(
                unsafe { &mut *context },
                "a kept value was compared after it was let go of",
            );
            None
        }
    }
}

pub unsafe extern "C" fn rt_open(
    context: *mut Context,
    k: Handle,
    ty: *const PlyType,
) -> OwnedWord {
    let want = unsafe { &*ty };
    let Some(kept) = object::<StoredValue>(context, &k, "a kept value") else {
        return OwnedWord::of(0);
    };
    match &kept.value {
        Some(v) if v.ty().identity == want.identity => v.dup(context).into_owned_word(),
        Some(v) => {
            let reason = format!(
                "a kept `{}` was opened as `{}`",
                spelled(v.ty()),
                spelled(want)
            );
            internal(unsafe { &mut *context }, reason);
            OwnedWord::of(0)
        }
        None => {
            internal(
                unsafe { &mut *context },
                "a kept value was opened after it was let go of",
            );
            OwnedWord::of(0)
        }
    }
}

pub unsafe extern "C" fn rt_stacks_opened(
    context: *mut Context,
    seeded: I64,
    root: I64,
    start: I64,
    end: I64,
) -> Handle {
    let c = unsafe { &mut *context };
    let id = i64::from(c.entered_sims);
    c.entered_sims += 1;
    let mut scheduler = Scheduler::open(context, seeded.0 != 0);
    c.schedulers
        .push((&mut *scheduler, [root.0, start.0, end.0]));
    c.refresh_recording_flag();
    let stacks = Stacks {
        scheduler: Some(scheduler),
        id,
        request: Message::default(),
        response: Message::default(),
        tasks: HashMap::new(),
    };
    object_made(context, Box::new(stacks))
}

pub unsafe extern "C" fn rt_stacks_outer(context: *mut Context, s: Handle, which: I64) -> I64 {
    let c = unsafe { &*context };
    let scheduler = object::<Stacks>(context, &s, "a scheduler's stacks").map(|s| s.scheduler());
    let outer = (c.schedulers.iter())
        .take_while(|(r, _)| Some(*r) != scheduler)
        .filter(|(r, _)| unsafe { (**r).is_seeded() })
        .last()
        .map(|(_, site)| *site);
    I64(match (outer, usize::try_from(which.0)) {
        (Some(site), Ok(i)) if i < 3 => site[i],
        _ => -1,
    })
}

pub fn seeded_scheduler(c: &Context) -> Option<Span> {
    let (scheduler, [root, start, end]) = c.schedulers.last()?;
    unsafe { (**scheduler).is_seeded() }.then(|| c.span_at(*root, *start, *end))
}

pub unsafe extern "C" fn rt_program_resumed(
    context: *mut Context,
    body: OwnedWord,
    _body_value_type: *const PlyType,
    _returned: *const PlyType,
) -> OwnedWord {
    unsafe { &mut *context }.program_entered();
    body
}

pub unsafe extern "C" fn rt_program_paused(
    context: *mut Context,
    result: OwnedWord,
    ty: *const PlyType,
) -> Handle {
    unsafe { &mut *context }.begin_scheduling();
    unsafe { rt_keep(context, result, ty) }
}

pub unsafe extern "C" fn rt_handle_as(_context: *mut Context, h: Handle) -> Handle {
    Handle::of(duplicated(h.word()))
}

pub(crate) struct OpenBracket {
    scheduler: *mut Scheduler,
    task: u64,
    shield: BracketPhase,
}

pub(crate) fn shielded(c: &mut Context, shield: BracketPhase) -> Option<OpenBracket> {
    let (scheduler, _) = c.schedulers.last().copied()?;
    let task = unsafe { (*scheduler).shield(shield) }?;
    Some(OpenBracket {
        scheduler,
        task,
        shield,
    })
}

pub(crate) fn leave_bracket(c: &mut Context, entered: Option<OpenBracket>) -> bool {
    entered.is_some_and(|b| {
        c.schedulers.iter().any(|(r, _)| *r == b.scheduler)
            && unsafe { (*b.scheduler).leave_phase(b.task, b.shield) }
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Effect {
        effect: ply_core::Symbol,
        resource: Option<ply_core::Symbol>,
        write: bool,
    },
    ScopedValue {
        id: u64,
        write: bool,
    },
}

pub unsafe extern "C" fn rt_trail_entered(context: *mut Context) -> TakenBytes {
    let c = unsafe { &mut *context };
    let kept = c.last_trail.clone().unwrap_or_default();
    c.given(kept)
}

pub unsafe extern "C" fn rt_seed_root(context: *mut Context, which: I64) -> I64 {
    let root = unsafe { &*context }.seed.root;
    I64(if which.0 == 0 {
        (root >> 32) as i64
    } else {
        (root & 0xFFFF_FFFF) as i64
    })
}

pub unsafe extern "C" fn rt_seed_path(context: *mut Context) -> TakenBytes {
    let c = unsafe { &mut *context };
    let path: Vec<u8> = c.seed.path.iter().flat_map(|p| p.to_le_bytes()).collect();
    c.given(path)
}

pub unsafe extern "C" fn rt_seeded_steps(context: *mut Context) -> I64 {
    I64(i64::from(unsafe { &*context }.sim_steps))
}

pub unsafe extern "C" fn rt_trail_left(
    context: *mut Context,
    p: Address,
    n: I64,
    q: Address,
    m: I64,
) {
    let c = unsafe { &mut *context };
    c.last_trail = Some(unsafe { borrowed(p, n) }.to_vec());
    c.interleaving = Some(unsafe { borrowed(q, m) }.to_vec());
}

pub unsafe extern "C" fn rt_site_placed(
    context: *mut Context,
    root: I64,
    start: I64,
    end: I64,
) -> TakenBytes {
    let c = unsafe { &mut *context };
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

pub unsafe extern "C" fn rt_footprint_held(context: *mut Context) -> I64 {
    I64(unsafe { &*context }.accesses.len() as i64)
}

pub unsafe extern "C" fn rt_footprint_taken(context: *mut Context) -> TakenBytes {
    let c = unsafe { &mut *context };
    let touched = std::mem::take(&mut c.accesses);
    let mut w = crate::flat::Writer::new();
    w.list(touched.iter(), |w, (t, site)| {
        match t {
            Access::Effect {
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
            Access::ScopedValue { id, write } => {
                w.tag(2).count(*id).tag(u64::from(*write));
            }
        }
        w.int(site[0]).int(site[1]).int(site[2]);
    });
    let bytes = w.finish();
    c.given(bytes)
}

pub unsafe extern "C" fn rt_stacks_region(context: *mut Context, s: Handle) -> I64 {
    I64(object::<Stacks>(context, &s, "a scheduler's stacks").map_or(-1, |s| s.id))
}

pub unsafe extern "C" fn rt_task_body(
    context: *mut Context,
    t: Handle,
    body: Block,
    descriptor: *const Word,
    words: *const Word,
) -> Handle {
    let n = if descriptor.is_null() {
        0
    } else {
        effect_word_count(descriptor)
    };
    let words = if n == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(words, n) }.to_vec()
    };
    body.dup();
    let started = Started {
        body,
        descriptor: descriptor as Word,
        words,
        task: duplicated(t.word()),
    };
    object_made(context, Box::new(TaskBody(Some(started))))
}

fn effect_word_count(d: *const Word) -> usize {
    let n = usize::try_from(unsafe { *d }).unwrap_or(0);
    let labels = (0..n)
        .filter(|i| unsafe { *d.add(2 + 2 * i) } == -3)
        .count();
    n + labels
}

unsafe fn run_started(context: *mut Context, arg: usize) {
    let Started {
        body,
        descriptor,
        words,
        task,
    } = *unsafe { Box::from_raw(arg as *mut Started) };
    let c = unsafe { &mut *context };
    c.entering_descriptor = descriptor;
    c.entering_words = if words.is_empty() {
        0
    } else {
        words.as_ptr() as Word
    };
    let result = unsafe { body.call(context, &[]) };
    if unsafe { (*context).failed } == 0
        && let Some(t) = unsafe { native_of::<SchedulerTask>(task) }
    {
        t.result = Some(result);
        t.ended = RETURNED;
    }
    drop(words);
    unsafe { body.release_function_if_last(context) };
    heap::release_object(context, task);
}

unsafe fn discard_started_task(context: *mut Context, arg: usize) {
    unsafe { Box::from_raw(arg as *mut Started) }.release(context);
}

pub unsafe extern "C" fn rt_task_started(context: *mut Context, s: Handle, id: I64, b: Handle) {
    let Some(started) = object::<TaskBody>(context, &b, "a task's body").and_then(|b| b.0.take())
    else {
        return;
    };
    let Some(stacks) = object::<Stacks>(context, &s, "a scheduler's stacks") else {
        started.release(context);
        return;
    };
    let Ok(n) = u64::try_from(id.0) else {
        started.release(context);
        return internal(unsafe { &mut *context }, "a task was started under no id");
    };
    if let Some(t) = unsafe { native_of::<SchedulerTask>(started.task) } {
        t.id = id.0;
    }
    if let Some(old) = stacks.tasks.insert(n, duplicated(started.task)) {
        heap::release_object(context, old);
    }
    let body = Body {
        run: run_started,
        release: discard_started_task,
        arg: Box::into_raw(Box::new(started)) as usize,
    };
    match &mut stacks.scheduler {
        Some(r) => r.start(n, body),
        None => unsafe { (body.release)(context, body.arg) },
    }
}

fn resumed(context: *mut Context, s: &Handle, id: I64, code: i64, n: i64, kept: Word) -> I64 {
    let Some(stacks) = object::<Stacks>(context, s, "a scheduler's stacks") else {
        return I64(release_and_fail_task(context, kept));
    };
    let scheduler = stacks.scheduler();
    let Ok(task) = u64::try_from(id.0) else {
        internal(unsafe { &mut *context }, "a task was resumed under no id");
        return I64(release_and_fail_task(context, kept));
    };
    if scheduler.is_null() {
        return I64(release_and_fail_task(context, kept));
    }
    let result = Message {
        code,
        numbers: [n, 0],
        word: kept,
        site: [0; 3],
    };
    let asked = unsafe { resume(scheduler, task, result) };
    let stacks =
        object::<Stacks>(context, s, "a scheduler's stacks").expect("the stacks it resumed");
    let left = std::mem::replace(&mut stacks.request, asked);
    if left.word != 0 {
        heap::release_object(context, left.word);
    }
    I64(asked.code)
}

fn release_and_fail_task(context: *mut Context, kept: Word) -> i64 {
    if kept != 0 {
        heap::release_object(context, kept);
    }
    FAILED
}

pub unsafe extern "C" fn rt_task_resumed(
    context: *mut Context,
    s: Handle,
    id: I64,
    code: I64,
    n: I64,
) -> I64 {
    resumed(context, &s, id, code.0, n.0, 0)
}

pub unsafe extern "C" fn rt_task_resumed_kept(
    context: *mut Context,
    s: Handle,
    id: I64,
    code: I64,
    n: I64,
    kept: Handle,
) -> I64 {
    resumed(context, &s, id, code.0, n.0, duplicated(kept.word()))
}

pub unsafe extern "C" fn rt_task_resumed_raising(
    context: *mut Context,
    s: Handle,
    id: I64,
    p: Address,
    n: I64,
) -> I64 {
    let c = unsafe { &mut *context };
    let d = failure_of(c, unsafe { borrowed(p, n) }).unwrap_or_else(|reason| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the scheduler wrote a failure the runtime does not read: {reason}"),
        )
    });
    if let Some(stacks) = object::<Stacks>(context, &s, "a scheduler's stacks")
        && let Some(scheduler) = stacks.scheduler.as_mut()
    {
        scheduler.raising = Some(d);
    }
    resumed(context, &s, id, RESUME_RAISE, 0, 0)
}

pub unsafe extern "C" fn rt_asked_number(context: *mut Context, s: Handle, i: I64) -> I64 {
    I64(
        object::<Stacks>(context, &s, "a scheduler's stacks").map_or(0, |s| {
            usize::try_from(i.0)
                .ok()
                .and_then(|i| s.request.numbers.get(i).copied())
                .unwrap_or(0)
        }),
    )
}

pub unsafe extern "C" fn rt_asked_site(context: *mut Context, s: Handle, i: I64) -> I64 {
    I64(
        object::<Stacks>(context, &s, "a scheduler's stacks").map_or(0, |s| {
            usize::try_from(i.0)
                .ok()
                .and_then(|i| s.request.site.get(i).copied())
                .unwrap_or(0)
        }),
    )
}

pub unsafe extern "C" fn rt_asked_kept(context: *mut Context, s: Handle) -> Handle {
    take_message_word(context, &s, |s| &mut s.request)
}

fn take_message_word(
    context: *mut Context,
    s: &Handle,
    of: impl FnOnce(&mut Stacks) -> &mut Message,
) -> Handle {
    let Some(stacks) = object::<Stacks>(context, s, "a scheduler's stacks") else {
        return Handle::of(0);
    };
    let w = std::mem::take(&mut of(stacks).word);
    if w == 0 {
        internal(
            unsafe { &mut *context },
            "a kept value was taken where none was handed",
        );
    }
    Handle::of(w)
}

fn yielded(context: *mut Context, s: &Handle, code: i64, numbers: [i64; 2], kept: Word) -> I64 {
    let Some(stacks) = object::<Stacks>(context, s, "a scheduler's stacks") else {
        return I64(release_and_fail_task(context, kept));
    };
    let scheduler = stacks.scheduler();
    if scheduler.is_null() {
        return I64(release_and_fail_task(context, kept));
    }
    let c = unsafe { &*context };
    let request = Message {
        code,
        numbers,
        word: kept,
        site: [c.performed_root, c.performed_start, c.performed_end],
    };
    let heard = unsafe { switch_to_loop(scheduler, request) };
    let stacks =
        object::<Stacks>(context, s, "a scheduler's stacks").expect("the stacks it ran under");
    let left = std::mem::replace(&mut stacks.response, heard);
    if left.word != 0 {
        heap::release_object(context, left.word);
    }
    unsafe { continues_running(scheduler, &heard) };
    I64(heard.code)
}

pub unsafe extern "C" fn rt_task_yielded(
    context: *mut Context,
    s: Handle,
    code: I64,
    a: I64,
    b: I64,
) -> I64 {
    yielded(context, &s, code.0, [a.0, b.0], 0)
}

pub unsafe extern "C" fn rt_task_yielded_kept(
    context: *mut Context,
    s: Handle,
    code: I64,
    a: I64,
    b: I64,
    kept: Handle,
) -> I64 {
    yielded(context, &s, code.0, [a.0, b.0], duplicated(kept.word()))
}

pub unsafe extern "C" fn rt_heard_number(context: *mut Context, s: Handle) -> I64 {
    I64(object::<Stacks>(context, &s, "a scheduler's stacks").map_or(0, |s| s.response.numbers[0]))
}

pub unsafe extern "C" fn rt_heard_kept(context: *mut Context, s: Handle) -> Handle {
    take_message_word(context, &s, |s| &mut s.response)
}

pub unsafe extern "C" fn rt_task_freed(context: *mut Context, s: Handle, id: I64) {
    let Some(stacks) = object::<Stacks>(context, &s, "a scheduler's stacks") else {
        return;
    };
    let Ok(n) = u64::try_from(id.0) else {
        return;
    };
    let mut production = false;
    if let Some(r) = &mut stacks.scheduler {
        r.free(n);
        production = !r.is_seeded();
    }
    if let Some(t) = stacks.tasks.remove(&n) {
        heap::release_object(context, t);
    }
    if production {
        let c = unsafe { &mut *context };
        c.host_runtime().end_task(c.id, TaskId(n));
    }
}

pub unsafe extern "C" fn rt_task_restored(
    context: *mut Context,
    s: Handle,
    id: I64,
    p: Address,
    n: I64,
) -> I64 {
    let note = (n.0 > 0).then(|| String::from_utf8_lossy(unsafe { borrowed(p, n) }).into_owned());
    let restored = object::<Stacks>(context, &s, "a scheduler's stacks").is_some_and(|stacks| {
        let Ok(task) = u64::try_from(id.0) else {
            return false;
        };
        stacks
            .scheduler
            .as_mut()
            .is_some_and(|r| r.restore(task, note))
    });
    I64(i64::from(restored))
}

pub unsafe extern "C" fn rt_task_shields(
    context: *mut Context,
    s: Handle,
    id: I64,
    which: I64,
) -> I64 {
    let shields =
        object::<Stacks>(context, &s, "a scheduler's stacks").map_or(Shields::default(), |s| {
            let task = u64::try_from(id.0).unwrap_or(u64::MAX);
            s.scheduler
                .as_ref()
                .map_or(Shields::default(), |r| r.shields(task))
        });
    I64(match which.0 {
        0 => i64::from(shields.acquiring),
        1 => i64::from(shields.releasing),
        _ => i64::from(shields.stopping),
    })
}

pub unsafe extern "C" fn rt_task_held_back(
    context: *mut Context,
    s: Handle,
    id: I64,
    stopping: I64,
) {
    if let Some(stacks) = object::<Stacks>(context, &s, "a scheduler's stacks")
        && let (Ok(task), Some(r)) = (u64::try_from(id.0), &mut stacks.scheduler)
    {
        r.set_stopping(task, stopping.0 != 0);
    }
}

pub unsafe extern "C" fn rt_task_settled(context: *mut Context, s: Handle, id: I64) {
    let Some(stacks) = object::<Stacks>(context, &s, "a scheduler's stacks") else {
        return;
    };
    if let Some(t) = u64::try_from(id.0)
        .ok()
        .and_then(|n| stacks.tasks.get(&n).copied())
        && let Some(t) = unsafe { native_of::<SchedulerTask>(t) }
    {
        t.ended = CANCELLED;
    }
}

pub unsafe extern "C" fn rt_task_made(context: *mut Context, scheduler: I64) -> Handle {
    let t = SchedulerTask {
        scheduler: scheduler.0,
        id: -1,
        ended: LIVE,
        result: None,
    };
    object_made(context, Box::new(t))
}

pub unsafe extern "C" fn rt_task_said(context: *mut Context, t: Handle, which: I64) -> I64 {
    I64(
        object::<SchedulerTask>(context, &t, "a task's handle").map_or(-1, |t| match which.0 {
            0 => t.scheduler,
            1 => t.id,
            _ => t.ended,
        }),
    )
}

pub unsafe extern "C" fn rt_task_answer(context: *mut Context, t: Handle) -> Handle {
    match object::<SchedulerTask>(context, &t, "a task's handle").and_then(|t| t.result) {
        Some(w) => Handle::of(duplicated(w)),
        None => {
            internal(
                unsafe { &mut *context },
                "a task's result was read before it returned",
            );
            Handle::of(0)
        }
    }
}

pub unsafe extern "C" fn rt_chan_made(context: *mut Context, scheduler: I64, id: I64) -> Handle {
    object_made(
        context,
        Box::new(SchedulerChannel {
            scheduler: scheduler.0,
            id: id.0,
        }),
    )
}

pub unsafe extern "C" fn rt_chan_said(context: *mut Context, c: Handle, which: I64) -> I64 {
    I64(
        object::<SchedulerChannel>(context, &c, "a channel's handle").map_or(-1, |c| {
            match which.0 {
                0 => c.scheduler,
                _ => c.id,
            }
        }),
    )
}

pub fn failure_of(c: &Context, bytes: &[u8]) -> Result<Diagnostic, String> {
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
        other => return Err(format!("`{other}` is no code a scheduler fails with")),
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

pub unsafe extern "C" fn rt_region_failed(context: *mut Context, p: Address, n: I64) {
    let c = unsafe { &mut *context };
    let d = failure_of(c, unsafe { borrowed(p, n) }).unwrap_or_else(|reason| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the scheduler wrote a failure the runtime does not read: {reason}"),
        )
    });
    c.fail(d);
}

fn ticket(token: I64) -> Pending {
    Pending {
        token: token.0 as u64,
        label: "a native's job",
    }
}

fn with_run<R>(context: *mut Context, rejected: R, f: impl FnOnce(&mut Context, &Run) -> R) -> R {
    let c = unsafe { &mut *context };
    let runtime = c.host_runtime();
    match ply_host::registry::run_of(&*runtime) {
        Ok(run) => f(c, run),
        Err(d) => {
            c.fail(d);
            rejected
        }
    }
}

pub unsafe extern "C" fn rt_ticket_watch(context: *mut Context, token: I64) -> I64 {
    with_run(context, I64(0), |c, run| match run.watch(&ticket(token)) {
        Ok(()) => I64(1),
        Err(d) => {
            c.fail(d);
            I64(0)
        }
    })
}

pub unsafe extern "C" fn rt_tickets_answered(context: *mut Context) -> TakenBytes {
    with_run(context, TakenBytes(0), |c, run| {
        let bytes: Vec<u8> = (run.returned().iter())
            .flat_map(|t| t.to_le_bytes())
            .collect();
        c.given(bytes)
    })
}

pub fn take_ticket_bytes(context: *mut Context, token: I64) -> TakenBytes {
    with_run(context, TakenBytes(0), |c, run| taken(c, run, token))
}

fn taken(c: &mut Context, run: &Run, token: I64) -> TakenBytes {
    let result = match run.taken(&ticket(token)) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            c.fail(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("ticket {} was taken before it was resolved", token.0),
            ));
            return TakenBytes(0);
        }
        Err(d) => {
            c.fail(d);
            return TakenBytes(0);
        }
    };
    c.given(result)
}

pub unsafe extern "C" fn rt_tickets_park(context: *mut Context, deadline: I64) -> I64 {
    let c = unsafe { &mut *context };
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

pub unsafe extern "C" fn rt_root_returned(context: *mut Context, s: Handle, scheduled: I64) {
    let Some(stacks) = object::<Stacks>(context, &s, "a scheduler's stacks") else {
        return;
    };
    let task_stacks = stacks.scheduler.as_ref().map_or(0, |r| r.stacks());
    let c = unsafe { &mut *context };
    c.retained_at_root = Some(crate::entry::Retained {
        native_objects: c.heap.natives() as u64,
        scheduled: u64::try_from(scheduled.0).unwrap_or(0),
        task_stacks: task_stacks as u64,
        frame_tables: c.stack_slots() as u64,
    });
}

pub unsafe extern "C" fn rt_host_now(context: *mut Context) -> I64 {
    let c = unsafe { &mut *context };
    match c.host_runtime().now() {
        Ok(now) => I64(now),
        Err(d) => {
            c.fail(d);
            I64(0)
        }
    }
}

pub unsafe extern "C" fn rt_run_stopping(context: *mut Context) -> I64 {
    let c = unsafe { &mut *context };
    I64(i64::from(c.host_runtime().stopping()))
}

pub unsafe extern "C" fn rt_run_drained(context: *mut Context) -> I64 {
    let c = unsafe { &mut *context };
    match c.host_runtime().drain_expired() {
        Some(d) => {
            c.fail(d);
            I64(1)
        }
        None => I64(0),
    }
}

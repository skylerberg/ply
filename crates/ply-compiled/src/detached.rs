//! A `handle` whose clause calls `resume` off the tail: the body runs on its own stack and `k`
//! switches into it. A stop a clause may resume again is snapshotted, so a finished body can be
//! resumed again (multi-shot); one a clause resumes at most once is not. When a continuation's last
//! holder lets it go, its snapshot is dropped, and the body with it where it waits at that stop.

use crate::frames::{self, PlyFrame, Snapshot};
use crate::heap::{self, CLOSURE_CAPTURES, CLOSURE_CODE, KIND_CLOSURE, Obj, Word, word_at};
use crate::rt::{
    Args, Ctx, FAILED_ABORT, FAILED_UNWIND, FrameClause, HandlerFrame, StackCopy, call_value,
    drop_frame, no_stack_to_take, release_vars, switch_keeping,
};
use crate::stack::{Stack, switch};
use ply_core::{Diagnostic, Symbol, codes};

pub struct Detached {
    stack: Option<Stack>,
    /// The body's own frames, the handle's at the bottom; its parent is where `k` was last called.
    frames: usize,
    /// The body's saved stack pointer, and the frames current when it stopped.
    sp: usize,
    saved_current: usize,
    /// The chain of frame records on the stack the body last stopped from, as it stopped.
    head: *mut PlyFrame,
    /// The stack that entered or resumed the body, to come back to when it stops.
    resumer_sp: usize,
    resumer_current: usize,
    request: Option<Stopped>,
    answer: Word,
    /// Owned here, not by the entry frame, which every restored snapshot would release again.
    body: Word,
    ret: Word,
    starting: Option<Word>,
    state: State,
    captures: Vec<Capture>,
    live: Option<usize>,
}

/// What a stop leaves behind.
struct Capture {
    /// What a restore puts back: the stack's bytes with their records' slots held, and its frames,
    /// what its runtime frames kept and the variables its running clauses hold. `None` for a stop
    /// resumed at most once, and for one from a stack other than the body's own.
    kept: Option<(Snapshot, StackCopy)>,
    /// The entry's count of at-most-once host operations when this stop was captured.
    born: u64,
    resumes: u32,
}

#[derive(PartialEq, Eq)]
enum State {
    Fresh,
    Running,
    Suspended,
    Done,
}

enum Stopped {
    /// `again`: the clause may resume the stop more than once.
    Performed {
        closure: Word,
        args: Vec<Word>,
        again: bool,
    },
    Finished(Word),
    Failed,
}

const ONE_SHOT: &str = "the `handle` this continuation was captured in has already ended";

/// `rt_handle_detached`: runs the body on a fresh stack until it stops; answers the handle's value.
pub(crate) unsafe fn open(ctx: *mut Ctx, clauses: Vec<FrameClause>, ret: Word, body: Word) -> Word {
    drain(ctx);
    let c = unsafe { &mut *ctx };
    let Some(stack) = Stack::take() else {
        clauses.into_iter().for_each(FrameClause::release);
        heap::dec(ret);
        heap::dec(body);
        return no_stack_to_take(c);
    };
    let id = c.detached.len();
    let opener = c.current;
    let fuel = c.fuel;
    let frames = c.open_stack(Some(opener));
    c.stacks[frames].body = Some(id);
    let handler = c.next_handler();
    c.stacks[frames]
        .list
        .push(HandlerFrame::detached(clauses, id, fuel, handler));
    let sp = stack.prepare(entry, ctx as usize);
    c.detached.push(Detached {
        stack: Some(stack),
        frames,
        sp,
        saved_current: frames,
        head: std::ptr::null_mut(),
        resumer_sp: 0,
        resumer_current: 0,
        request: None,
        answer: 0,
        body,
        ret,
        starting: Some(body),
        state: State::Fresh,
        captures: Vec::new(),
        live: None,
    });
    unsafe { resume(ctx, id, None, None) }
}

/// Runs the body from where it stopped, `answer` returned from its `perform`, until it stops again.
pub(crate) unsafe fn resume(
    ctx: *mut Ctx,
    id: usize,
    capture: Option<usize>,
    answer: Option<Word>,
) -> Word {
    let c = unsafe { &mut *ctx };
    let d = &mut c.detached[id];
    match (&d.state, capture) {
        (State::Fresh, _) => {}
        (State::Running, _) => {
            let dg = Diagnostic::error(
                codes::RUNTIME_ERROR,
                "a continuation was resumed while its earlier resumption is still running",
            );
            return c.fail(dg);
        }
        (State::Suspended, Some(k)) if d.live == Some(k) => {
            if let Some(dg) = replayed(c, id, k) {
                return c.fail(dg);
            }
        }
        (State::Suspended, _) => {
            let dg = Diagnostic::error(
                codes::RUNTIME_ERROR,
                "a continuation was resumed while a later stop of its body is still suspended",
            )
            .note("the C backend runs a body on one stack, and restoring an earlier point would overwrite the frames the later stop waits to return into");
            return c.fail(dg);
        }
        (State::Done, Some(k)) => {
            if let Some(dg) = replayed(c, id, k) {
                return c.fail(dg);
            }
            if !restore(ctx, id, k) {
                let dg = Diagnostic::error(codes::TASK_ESCAPES_SCOPE, ONE_SHOT);
                return unsafe { &mut *ctx }.fail(dg);
            }
        }
        (State::Done, None) => {
            let dg = Diagnostic::error(
                codes::INTERNAL_ERROR,
                "a finished body was entered again without a capture",
            );
            return c.fail(dg);
        }
    }
    let c = unsafe { &mut *ctx };
    // A resumption continues with the variables of the clause of its handler that resumes it, and
    // with those the body had where it stopped when nothing of its handler's runs.
    if capture.is_some() {
        let frames = c.detached[id].frames;
        let values = c.stacks[frames]
            .list
            .first()
            .and_then(|f| c.held_by(f.handler));
        if let Some(v) = values {
            if let Some(f) = c.stacks[frames].list.first_mut() {
                f.set_vars(&v.borrow());
            }
            release_vars(v);
        }
    }
    let d = &mut c.detached[id];
    if let Some(a) = answer {
        d.answer = a;
    }
    d.resumer_current = c.current;
    d.state = State::Running;
    let (frames, current, sp) = (d.frames, d.saved_current, d.sp);
    c.stacks[frames].parent = Some(c.current);
    c.current = current;
    if d.state == State::Running && d.starting.is_some() {
        c.starting_detached = Some(id);
    }
    let from = &mut c.detached[id].resumer_sp as *mut usize;
    unsafe { switch_keeping(ctx, from, sp) };

    let c = unsafe { &mut *ctx };
    let d = &mut c.detached[id];
    c.current = d.resumer_current;
    match d.request.take() {
        Some(Stopped::Performed {
            closure,
            mut args,
            again,
        }) => {
            d.state = State::Suspended;
            let k = capture_stop(ctx, id, again);
            let c = unsafe { &mut *ctx };
            args.push(token(c, id, k));
            // The clause works on its own copy of the variables, so each resumption it makes starts
            // from what the clause holds, whatever an earlier one did to the body's.
            let frames = c.detached[id].frames;
            let held = c.stacks[frames].list.first().and_then(|f| f.vars_copied());
            let call = Args::of(c, args);
            let r = Ctx::holding(ctx, held, || call_value(ctx, closure, call.words()));
            call.release();
            drain(ctx);
            r
        }
        Some(Stopped::Finished(v)) => {
            finish(c, id);
            v
        }
        Some(Stopped::Failed) => {
            finish(c, id);
            0
        }
        None => {
            d.state = State::Done;
            let dg = Diagnostic::error(
                codes::INTERNAL_ERROR,
                "a detached body gave control back without stopping at anything",
            );
            c.fail(dg)
        }
    }
}

fn finish(c: &mut Ctx, id: usize) {
    let d = &mut c.detached[id];
    d.state = State::Done;
    d.live = None;
    give_back(c, id);
}

/// A finished body keeps its stack and closures only while a snapshot could be restored onto it.
fn give_back(c: &mut Ctx, id: usize) {
    let d = &mut c.detached[id];
    if d.captures.iter().any(|k| k.kept.is_some()) {
        return;
    }
    d.stack = None;
    heap::dec(std::mem::take(&mut d.body));
    heap::dec(std::mem::take(&mut d.ret));
}

/// Records the stop the body just made, snapshotted where the clause may resume it `again`, and
/// answers its capture's index.
fn capture_stop(ctx: *mut Ctx, id: usize, again: bool) -> usize {
    let c = unsafe { &mut *ctx };
    let d = &c.detached[id];
    let (sp, own, head) = (d.sp, d.frames, d.head);
    // A stop from a task's stack or a nested body's has none to copy: the frames waiting there are
    // not the ones a restore would write back.
    let kept = match d.stack.as_ref() {
        Some(stack) if again && d.saved_current == own => {
            let stack: *const Stack = stack;
            unsafe { Snapshot::take(ctx, &*stack, sp, head) }.map(|snapshot| {
                let s = &unsafe { &*ctx }.stacks[own];
                (snapshot, StackCopy::of(&s.list, &s.kept, &s.held))
            })
        }
        _ => None,
    };
    let c = unsafe { &mut *ctx };
    let born = c.host_ops;
    let d = &mut c.detached[id];
    d.captures.push(Capture {
        kept,
        born,
        resumes: 0,
    });
    let k = d.captures.len() - 1;
    d.live = Some(k);
    k
}

/// Counts a resumption of capture `k`; a second across an at-most-once host op is `E0426`.
fn replayed(c: &mut Ctx, id: usize, k: usize) -> Option<Diagnostic> {
    let host_ops = c.host_ops;
    let cap = &mut c.detached[id].captures[k];
    cap.resumes = cap.resumes.saturating_add(1);
    if cap.resumes > 1 && host_ops > cap.born {
        let resumes = cap.resumes;
        let site = c.site();
        return Some(crate::host::err_continuation_resumed(
            site,
            resumes,
            c.last_linear.as_ref(),
        ));
    }
    None
}

/// Puts capture `k`'s snapshot back on the body's stack; `false` when it has none.
fn restore(ctx: *mut Ctx, id: usize, k: usize) -> bool {
    let d = &unsafe { &*ctx }.detached[id];
    let Some((snapshot, captured)) = d.captures[k].kept.as_ref() else {
        return false;
    };
    let stack: *const Stack = d
        .stack
        .as_ref()
        .expect("a body with a snapshot keeps its stack");
    unsafe { snapshot.restore(ctx, &*stack) };
    let copy = StackCopy::of(&captured.list, &captured.kept, &captured.held);
    let (own, sp, head) = (d.frames, snapshot.sp(), snapshot.head());
    let c = unsafe { &mut *ctx };
    let s = &mut c.stacks[own];
    StackCopy {
        list: std::mem::replace(&mut s.list, copy.list),
        kept: std::mem::replace(&mut s.kept, copy.kept),
        held: std::mem::replace(&mut s.held, copy.held),
    }
    .release();
    let d = &mut c.detached[id];
    d.sp = sp;
    d.saved_current = own;
    d.head = head;
    d.state = State::Suspended;
    d.live = Some(k);
    true
}

/// The body's `perform`: hands the clause to whoever resumed the body, returns what `k` got. The
/// clause may resume it `again`. Nothing uncounted may be owned across the switch (each restore
/// would free it again), so `owned` is dropped first.
pub(crate) unsafe fn stop(
    ctx: *mut Ctx,
    id: usize,
    closure: Word,
    args: Vec<Word>,
    again: bool,
    owned: (Symbol, Symbol, Option<Symbol>),
) -> Word {
    drop(owned);
    let c = unsafe { &mut *ctx };
    let head = c.frames;
    let d = &mut c.detached[id];
    d.request = Some(Stopped::Performed {
        closure,
        args,
        again,
    });
    d.saved_current = c.current;
    d.head = head;
    let to = d.resumer_sp;
    let from = &mut d.sp as *mut usize;
    unsafe { switch_keeping(ctx, from, to) };
    let c = unsafe { &mut *ctx };
    std::mem::take(&mut c.detached[id].answer)
}

extern "C" fn entry(arg: usize) {
    let ctx = arg as *mut Ctx;
    let (id, body) = {
        let c = unsafe { &mut *ctx };
        let id = c
            .starting_detached
            .take()
            .expect("a detached body starts with its id set");
        let body = c.detached[id]
            .starting
            .take()
            .expect("a detached body starts with its closure");
        let own = c.detached[id].stack.as_ref();
        c.stack_floor = unsafe { own.expect("a detached body starts on its stack").enter() };
        c.frames = frames::base();
        (id, body)
    };
    let mut r = call_value(ctx, body, &[]);
    let c = unsafe { &mut *ctx };
    let frames = c.detached[id].frames;
    let mut frame = c.stacks[frames].list.pop();
    if let Some(f) = &frame {
        f.land(c);
    }
    let mut held = frame.as_ref().and_then(HandlerFrame::held);
    // A zero-shot clause of this frame unwinds to it, and a raise its clause answers lands here;
    // `return` is applied to neither. What the body left open goes back in `finish`.
    let mut answered = false;
    if c.failed == FAILED_UNWIND
        && let Some((stack, depth, v)) = c.unwind.take()
    {
        if stack == frames && depth == 0 {
            c.failed = 0;
            r = v;
            answered = true;
        } else {
            c.unwind = Some((stack, depth, v));
        }
    }
    if c.failed == FAILED_ABORT
        && let Some(a) = c.aborting.take_if(|a| a.stack == frames && a.depth == 0)
        && let Some(f) = frame.as_mut()
    {
        c.failed = 0;
        let closure = f.take_clause(a.clause);
        r = Ctx::holding(ctx, held.take(), || call_value(ctx, closure, &a.args));
        heap::dec(closure);
        answered = true;
    }
    let c = unsafe { &mut *ctx };
    let ret = c.detached[id].ret;
    if c.failed == 0 && ret != 0 && !answered {
        r = Ctx::holding(ctx, held.take(), || call_value(ctx, ret, &[r]));
    }
    if let Some((_, v)) = held {
        release_vars(v);
    }
    let c = unsafe { &mut *ctx };
    if let Some(f) = frame {
        drop_frame(f);
    }
    let d = &mut c.detached[id];
    d.request = Some(if c.failed != 0 {
        Stopped::Failed
    } else {
        Stopped::Finished(r)
    });
    d.saved_current = c.current;
    let to = d.resumer_sp;
    let from = &mut d.sp as *mut usize;
    unsafe { switch(&mut *from, to) };
    std::process::abort();
}

/// Whether `code` is a continuation's: the closure names a handler or a body that only the entry
/// which captured it holds, so it means nothing to any other.
pub(crate) fn is_continuation(code: usize) -> bool {
    code == rt_resume_detached_entry as *const () as usize
        || code == crate::rt::rt_resume_entry as *const () as usize
}

/// The `k` a clause off the tail is handed: a closure resuming the body from its capture, and the
/// entry it was captured in.
fn token(c: &mut Ctx, id: usize, capture: usize) -> Word {
    let entry = c.entry as i64;
    crate::rt::closure_of(
        c,
        rt_resume_detached_entry as *const () as usize,
        &[((id << 32) | capture) as i64, entry],
    )
}

/// The body and capture a token's packed word names.
fn unpacked(packed: i64) -> (usize, usize) {
    let packed = packed as usize;
    (packed >> 32, packed & 0xffff_ffff)
}

unsafe extern "C" fn rt_resume_detached_entry(ctx: *mut Ctx, args: *const i64) -> i64 {
    let (packed, entry, v) = unsafe {
        (
            heap::imm_value(*args),
            heap::imm_value(*args.add(1)),
            *args.add(2),
        )
    };
    let (id, k) = unpacked(packed);
    let c = unsafe { &mut *ctx };
    if !captured_here(c, entry, id, k) {
        return c.fail(err_stale_continuation());
    }
    unsafe { resume(ctx, id, Some(k), Some(v)) }
}

/// Whether this entry captured stop `k` of body `id`. A token from another entry may carry an index
/// this entry uses for a body of its own, or one past every body it has.
fn captured_here(c: &Ctx, entry: i64, id: usize, k: usize) -> bool {
    entry == c.entry as i64 && c.detached.get(id).is_some_and(|d| k < d.captures.len())
}

/// Notes that the last holder of `o`, a runtime handle, let it go: where it is a continuation this
/// entry captured, the stop it resumes is dropped at the next drain.
///
/// # Safety
/// `o` is a live runtime handle, and `ctx` the running entry's.
pub(crate) unsafe fn died(ctx: *mut Ctx, o: *mut Obj) {
    let resumes_a_body = rt_resume_detached_entry as *const () as Word;
    if unsafe { (*o).kind != KIND_CLOSURE || word_at(o, CLOSURE_CODE) != resumes_a_body } {
        return;
    }
    let (packed, entry) = unsafe {
        (
            heap::imm_value(word_at(o, CLOSURE_CAPTURES)),
            heap::imm_value(word_at(o, CLOSURE_CAPTURES + 1)),
        )
    };
    let (id, k) = unpacked(packed);
    let c = unsafe { &mut *ctx };
    if captured_here(c, entry, id, k) {
        c.continuations_died.push((id, k));
    }
}

/// Drops every stop whose continuation died since the last drain, and every stop that dropping
/// them let die in turn.
pub(crate) fn drain(ctx: *mut Ctx) {
    while let Some((id, k)) = unsafe { (*ctx).continuations_died.pop() } {
        drop_stop(ctx, id, k);
    }
}

/// Drops stop `k` of body `id`, which nothing can resume any longer: its snapshot, and the body
/// itself where it waits at that stop.
fn drop_stop(ctx: *mut Ctx, id: usize, k: usize) {
    let d = &mut unsafe { &mut *ctx }.detached[id];
    let kept = d.captures[k].kept.take();
    let waiting = d.state == State::Suspended && d.live == Some(k);
    if let Some((snapshot, stack)) = kept {
        unsafe { snapshot.release(ctx) };
        stack.release();
    }
    if waiting {
        abandon(ctx, id);
    }
    let c = unsafe { &mut *ctx };
    if c.detached[id].state == State::Done {
        give_back(c, id);
    }
}

/// Lets go of what a body suspended at a stop that will never be resumed holds: its frames'
/// records on the stack it stopped from, and its stack's handler state. It runs no program code: a
/// `bracket` the body was inside does not release.
fn abandon(ctx: *mut Ctx, id: usize) {
    let d = &mut unsafe { &mut *ctx }.detached[id];
    d.state = State::Done;
    d.live = None;
    let (head, own) = (
        std::mem::replace(&mut d.head, std::ptr::null_mut()),
        d.frames,
    );
    unsafe { frames::release_live(ctx, head) };
    let_go_of_handlers(ctx, own);
}

fn let_go_of_handlers(ctx: *mut Ctx, own: usize) {
    if let Some(s) = unsafe { &mut *ctx }.stacks.get_mut(own) {
        StackCopy {
            list: std::mem::take(&mut s.list),
            kept: std::mem::take(&mut s.kept),
            held: std::mem::take(&mut s.held),
        }
        .release();
    }
}

/// Gives back what the entry's bodies still hold at its end: every snapshot, every suspended body's
/// frames and handler state, and every stack and closure. Runs before the entry's heap ends.
pub(crate) fn teardown(ctx: *mut Ctx) {
    drain(ctx);
    let bodies = unsafe { &*ctx }.detached.len();
    for id in (0..bodies).rev() {
        let (suspended, done, own) = {
            let d = &unsafe { &*ctx }.detached[id];
            (
                d.state == State::Suspended,
                d.state == State::Done,
                d.frames,
            )
        };
        if suspended {
            abandon(ctx, id);
        } else if !done {
            let_go_of_handlers(ctx, own);
        }
        for k in 0..unsafe { &*ctx }.detached[id].captures.len() {
            let kept = unsafe { &mut *ctx }.detached[id].captures[k].kept.take();
            if let Some((snapshot, stack)) = kept {
                unsafe { snapshot.release(ctx) };
                stack.release();
            }
        }
        let c = unsafe { &mut *ctx };
        let d = &mut c.detached[id];
        d.state = State::Done;
        d.live = None;
        give_back(c, id);
    }
    let c = unsafe { &mut *ctx };
    c.continuations_died.clear();
    c.detached.clear();
}

/// `E0505`: nothing a program writes carries a continuation out of the entry that captured it.
#[cold]
#[inline(never)]
fn err_stale_continuation() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a continuation was resumed outside the entry that captured it",
    )
    .note("the body it would resume lived only as long as that entry, so nothing is left to run")
    .note("this is Ply's fault, not the program's")
}

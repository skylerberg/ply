//! A `handle` whose clause binds `resume` and calls it off the tail: the body runs on a stack of
//! its own, the clause on the stack that was running when the body stopped, and `k` is a switch
//! from wherever it is called into the body's stack. When the body stops again the switch comes
//! back to whoever called `k`, which is what makes the resumed body run inside the clause that
//! resumed it, as a deep handler's does. A stop on the body's own stack is captured: the live
//! range of the stack, the body's frames and the heap objects the range references, held once
//! more each. Resuming a captured continuation after the body has finished restores the snapshot
//! in place and runs it again, which is multi-shot resumption; resuming one while a later stop
//! of the same body is still suspended is refused, since the restore would overwrite the frames
//! that stop is waiting to return into.

use crate::heap::{self, Word};
use crate::rt::{Ctx, FAILED_UNWIND, FrameClause, HandlerFrame, call_value, drop_frame};
use crate::stack::{Stack, switch};
use ply_span::{Diagnostic, Symbol, codes};

pub struct Detached {
    stack: Option<Stack>,
    /// The body's own frames, holding the handle's frame at the bottom; its parent is the stack
    /// `k` was last called from.
    frames: usize,
    /// The body's saved stack pointer, the frames and the floor current when it stopped.
    sp: usize,
    saved_current: usize,
    saved_floor: usize,
    /// The stack that entered or resumed the body, to come back to when it stops.
    resumer_sp: usize,
    resumer_current: usize,
    resumer_floor: usize,
    request: Option<Stopped>,
    answer: Word,
    /// The body's closure and the `return` clause's, owned here rather than by the entry frame
    /// on the body's stack: that frame comes back with every restored snapshot and would
    /// release them once per run.
    body: Word,
    ret: Word,
    starting: Option<Word>,
    state: State,
    captures: Vec<Capture>,
    /// The capture the body is suspended at, while it is.
    live: Option<usize>,
}

/// What a stop on the body's own stack leaves behind, so the body can be run from it again.
struct Capture {
    sp: usize,
    floor: usize,
    /// The bytes from `sp` to the top of the body's stack. `None` when the stop came from a stack
    /// running under the body, a task's, whose region will have ended by the time a second
    /// resumption could ask for it.
    bytes: Option<Vec<u8>>,
    frames: Vec<HandlerFrame>,
    /// Every word in the snapshot that is a live heap object, held once for the snapshot and
    /// once more per restore, since the run consumes at most one reference to each.
    pins: Vec<Word>,
}

#[derive(PartialEq, Eq)]
enum State {
    Fresh,
    Running,
    Suspended,
    Done,
}

enum Stopped {
    Performed { closure: Word, args: Vec<Word> },
    Finished(Word),
    Failed,
}

const ONE_SHOT: &str = "the region this continuation was captured in has already ended";

/// `rt_handle_detached`: pushes the frame at the bottom of a fresh stack's frames, prepares the
/// stack, and runs the body until it stops. Answers the handle's value.
pub(crate) unsafe fn open(ctx: *mut Ctx, clauses: Vec<FrameClause>, ret: Word, body: Word) -> Word {
    let c = unsafe { &mut *ctx };
    let id = c.detached.len();
    let frames = c.open_stack(Some(c.current));
    c.stacks[frames]
        .list
        .push(HandlerFrame::detached(clauses, id));
    let stack = Stack::new();
    let sp = stack.prepare(entry, ctx as usize);
    let floor = stack.floor();
    c.detached.push(Detached {
        stack: Some(stack),
        frames,
        sp,
        saved_current: frames,
        saved_floor: floor,
        resumer_sp: 0,
        resumer_current: 0,
        resumer_floor: 0,
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

/// Runs the body from where it stopped, with `answer` as what its `perform` gets back, until it
/// stops again; then the clause it stopped at, or the value it finished with, is this call's
/// answer.
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
        (State::Suspended, Some(k)) if d.live == Some(k) => {}
        (State::Suspended, _) => {
            let dg = Diagnostic::error(
                codes::RUNTIME_ERROR,
                "a continuation was resumed while a later stop of its body is still suspended",
            )
            .note("the compiled tier runs a body on one stack, and restoring an earlier point would overwrite the frames the later stop waits to return into");
            return c.fail(dg);
        }
        (State::Done, Some(k)) => {
            if !restore(c, id, k) {
                let dg = Diagnostic::error(codes::TASK_ESCAPES_SCOPE, ONE_SHOT);
                return c.fail(dg);
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
    let d = &mut c.detached[id];
    if let Some(a) = answer {
        d.answer = a;
    }
    d.resumer_current = c.current;
    d.resumer_floor = c.stack_floor;
    d.state = State::Running;
    let (frames, current, floor, sp) = (d.frames, d.saved_current, d.saved_floor, d.sp);
    c.stacks[frames].parent = Some(c.current);
    c.current = current;
    c.stack_floor = floor;
    if d.state == State::Running && d.starting.is_some() {
        c.starting_detached = Some(id);
    }
    let from = &mut c.detached[id].resumer_sp as *mut usize;
    unsafe { switch(&mut *from, sp) };

    let c = unsafe { &mut *ctx };
    let d = &mut c.detached[id];
    c.current = d.resumer_current;
    c.stack_floor = d.resumer_floor;
    match d.request.take() {
        Some(Stopped::Performed { closure, mut args }) => {
            d.state = State::Suspended;
            let k = capture_stop(c, id);
            args.push(token(c, id, k));
            call_value(ctx, closure, &args)
        }
        Some(Stopped::Finished(v)) => {
            finish(d);
            v
        }
        Some(Stopped::Failed) => {
            finish(d);
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

/// A body that stopped for good keeps its stack, and the closures a restored run would call
/// again, only while a snapshot could be restored onto it.
fn finish(d: &mut Detached) {
    d.state = State::Done;
    d.live = None;
    if d.captures.iter().all(|k| k.bytes.is_none()) {
        d.stack = None;
        if d.body != 0 {
            heap::dec(d.body);
            d.body = 0;
        }
        if d.ret != 0 {
            heap::dec(d.ret);
            d.ret = 0;
        }
    }
}

/// Records the stop the body just made and answers its capture's index. The snapshot is taken
/// only when the stop came from the body's own stack.
fn capture_stop(c: &mut Ctx, id: usize) -> usize {
    let d = &c.detached[id];
    let own = d.saved_current == d.frames;
    let (sp, floor) = (d.sp, d.saved_floor);
    let (bytes, frames, pins) = if own {
        let stack = d.stack.as_ref().expect("a suspended body has a stack");
        let bytes = stack.live(sp).to_vec();
        let mut pins = Vec::new();
        for chunk in bytes.chunks_exact(8) {
            let w = Word::from_ne_bytes(chunk.try_into().expect("eight bytes"));
            if c.heap.is_object(w) {
                heap::inc(w);
                pins.push(w);
            }
        }
        let frames = crate::rt::clone_frames(&c.stacks[d.frames].list);
        (Some(bytes), frames, pins)
    } else {
        (None, Vec::new(), Vec::new())
    };
    let d = &mut c.detached[id];
    d.captures.push(Capture {
        sp,
        floor,
        bytes,
        frames,
        pins,
    });
    let k = d.captures.len() - 1;
    d.live = Some(k);
    k
}

/// Puts capture `k`'s snapshot back on the body's stack, so the next switch in runs from it.
/// `false` when the stop had no snapshot to restore.
fn restore(c: &mut Ctx, id: usize, k: usize) -> bool {
    let d = &c.detached[id];
    let Some(bytes) = d.captures[k].bytes.as_ref() else {
        return false;
    };
    let stack = d
        .stack
        .as_ref()
        .expect("a body with a snapshot keeps its stack");
    unsafe { stack.restore(d.captures[k].sp, bytes) };
    for w in &d.captures[k].pins {
        heap::inc(*w);
    }
    let frames = crate::rt::clone_frames(&d.captures[k].frames);
    let (own, sp, floor) = (d.frames, d.captures[k].sp, d.captures[k].floor);
    for f in std::mem::replace(&mut c.stacks[own].list, frames) {
        drop_frame(f);
    }
    let d = &mut c.detached[id];
    d.sp = sp;
    d.saved_current = own;
    d.saved_floor = floor;
    d.state = State::Suspended;
    d.live = Some(k);
    true
}

/// From the body's side: the `perform` a clause off the tail answers. Hands the clause and its
/// arguments to whoever resumed the body and comes back with what `k` was called with.
///
/// Nothing on the stack below the switch may own memory the heap does not count: the frames
/// from the body's entry to here come back with every restored snapshot, and a `Vec` or an
/// `Arc` held across the switch would be released once per restore. `owned` is what the
/// caller still held; it is dropped here, before the switch.
pub(crate) unsafe fn stop(
    ctx: *mut Ctx,
    id: usize,
    closure: Word,
    args: &[Word],
    owned: (Symbol, Symbol, Option<Symbol>),
) -> Word {
    drop(owned);
    let c = unsafe { &mut *ctx };
    let d = &mut c.detached[id];
    d.request = Some(Stopped::Performed {
        closure,
        args: args.to_vec(),
    });
    d.saved_current = c.current;
    d.saved_floor = c.stack_floor;
    let to = d.resumer_sp;
    let from = &mut d.sp as *mut usize;
    unsafe { switch(&mut *from, to) };
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
        (id, body)
    };
    let mut r = call_value(ctx, body, &[]);
    let c = unsafe { &mut *ctx };
    let frames = c.detached[id].frames;
    let frame = c.stacks[frames].list.pop();
    // The zero-shot clause of this very frame unwinds to it: the clause's value is the
    // handle's, and the `return` clause is not applied, as at an inline landing.
    if c.failed == FAILED_UNWIND
        && let Some((stack, depth, v)) = c.unwind.take()
    {
        if stack == frames && depth == 0 {
            c.failed = 0;
            r = v;
        } else {
            c.unwind = Some((stack, depth, v));
        }
    }
    let ret = c.detached[id].ret;
    if c.failed == 0 && ret != 0 {
        r = call_value(ctx, ret, &[r]);
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

/// The `k` a clause off the tail is handed: a closure whose entry resumes the body from the
/// capture the stop made.
fn token(c: &mut Ctx, id: usize, capture: usize) -> Word {
    crate::rt::closure_of(
        c,
        rt_resume_detached_entry as *const () as usize,
        ((id << 32) | capture) as i64,
    )
}

unsafe extern "C" fn rt_resume_detached_entry(ctx: *mut Ctx, args: *const i64) -> i64 {
    let (packed, v) = unsafe { (heap::imm_value(*args) as usize, *args.add(1)) };
    unsafe { resume(ctx, packed >> 32, Some(packed & 0xffff_ffff), Some(v)) }
}

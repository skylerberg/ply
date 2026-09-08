//! A `handle` whose clause binds `resume` and calls it off the tail: the body runs on a stack of
//! its own, the clause on the stack that was running when the body stopped, and `k` is a switch
//! from wherever it is called into the body's stack. When the body stops again the switch comes
//! back to whoever called `k`, which is what makes the resumed body run inside the clause that
//! resumed it, as a deep handler's does. One shot: a second resumption is refused until the
//! record's third stage restores a snapshot.

use crate::heap::{self, Word};
use crate::rt::{Ctx, FAILED_UNWIND, FrameClause, HandlerFrame, call_value, drop_frame};
use crate::stack::{Stack, switch};
use ply_span::{Diagnostic, codes};

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
    ret: Word,
    starting: Option<Word>,
    state: State,
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
        ret,
        starting: Some(body),
        state: State::Fresh,
    });
    unsafe { resume(ctx, id, None) }
}

/// Runs the body from where it stopped, with `answer` as what its `perform` gets back, until it
/// stops again; then the clause it stopped at, or the value it finished with, is this call's
/// answer.
pub(crate) unsafe fn resume(ctx: *mut Ctx, id: usize, answer: Option<Word>) -> Word {
    let c = unsafe { &mut *ctx };
    let d = &mut c.detached[id];
    match d.state {
        State::Done => {
            let dg = Diagnostic::error(
                codes::RUNTIME_ERROR,
                "a continuation was resumed after its body finished",
            )
            .note("the compiled tier resumes a continuation once; the machine's multi-shot resumption is the record's third stage");
            return c.fail(dg);
        }
        State::Running => {
            let dg = Diagnostic::error(
                codes::RUNTIME_ERROR,
                "a continuation was resumed while its earlier resumption is still running",
            );
            return c.fail(dg);
        }
        State::Fresh | State::Suspended => {}
    }
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
            args.push(token(c, id));
            call_value(ctx, closure, &args)
        }
        Some(Stopped::Finished(v)) => {
            d.state = State::Done;
            d.stack = None;
            v
        }
        Some(Stopped::Failed) => {
            d.state = State::Done;
            d.stack = None;
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

/// From the body's side: the `perform` a clause off the tail answers. Hands the clause and its
/// arguments to whoever resumed the body and comes back with what `k` was called with.
pub(crate) unsafe fn stop(ctx: *mut Ctx, id: usize, closure: Word, args: &[Word]) -> Word {
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
    heap::dec(body);
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
    if ret != 0 {
        heap::dec(ret);
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

/// The `k` a clause off the tail is handed: a closure whose entry resumes the body.
fn token(c: &mut Ctx, id: usize) -> Word {
    crate::rt::closure_of(c, rt_resume_detached_entry as *const () as usize, id as i64)
}

unsafe extern "C" fn rt_resume_detached_entry(ctx: *mut Ctx, args: *const i64) -> i64 {
    let (id, v) = unsafe { (heap::imm_value(*args), *args.add(1)) };
    unsafe { resume(ctx, id as usize, Some(v)) }
}

//! A `handle` whose clause calls `resume` off the tail: the body runs on its own stack and `k`
//! switches into it; stops are snapshotted so a finished body can be resumed again (multi-shot).

use crate::heap::{self, Word};
use crate::rt::{
    Ctx, FAILED_ABORT, FAILED_UNWIND, FrameClause, HandlerFrame, call_value, drop_frame,
};
use crate::stack::{Stack, switch};
use ply_eval::arena::{Owner, Pin, RegionId};
use ply_eval::{Diagnostic, Symbol, codes};

pub struct Detached {
    stack: Option<Stack>,
    /// The body's own frames, the handle's at the bottom; its parent is where `k` was last called.
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
    /// Owned here, not by the entry frame, which every restored snapshot would release again.
    body: Word,
    ret: Word,
    starting: Option<Word>,
    state: State,
    captures: Vec<Capture>,
    live: Option<usize>,
    /// The regions around the `handle` whose cells the body reaches, from [`pin_enclosing`]:
    /// pinned for as long as the body's own stack can run again.
    enclosing: Vec<Pin>,
}

/// What a stop on the body's own stack leaves behind, so the body can be run from it again.
struct Capture {
    sp: usize,
    floor: usize,
    /// The bytes from `sp` to the top of the body's stack; `None` for a stop from a task's stack.
    bytes: Option<Vec<u8>>,
    frames: Vec<HandlerFrame>,
    /// The snapshot's live heap words, held once for it and once more per restore.
    pins: Vec<Word>,
    /// With a snapshot, the regions open on the body's own stack at the stop, outermost first,
    /// which a restore runs inside again: pinned, so their closes keep their cells.
    regions: Vec<Pin>,
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
    Performed { closure: Word, args: Vec<Word> },
    Finished(Word),
    Failed,
}

const ONE_SHOT: &str = "the region this continuation was captured in has already ended";

/// `rt_handle_detached`: runs the body on a fresh stack until it stops; answers the handle's value.
pub(crate) unsafe fn open(ctx: *mut Ctx, clauses: Vec<FrameClause>, ret: Word, body: Word) -> Word {
    let c = unsafe { &mut *ctx };
    let id = c.detached.len();
    let opener = c.current;
    let enclosing = pin_enclosing(c, opener);
    let frames = c.open_stack(Some(opener));
    c.stacks[frames].body = Some(id);
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
        enclosing,
    });
    let answer = unsafe { resume(ctx, id, None, None) };
    let c = unsafe { &mut *ctx };
    // A failure or an unwind leaving the `handle` abandons its body wherever the body stood.
    if c.failed != 0 {
        let frames = c.detached[id].frames;
        c.release_regions(frames);
    }
    answer
}

/// Once the entry has returned no body can be resumed, so none holds a region any longer and none
/// pins one.
pub(crate) fn release_all(c: &mut Ctx) {
    for id in 0..c.detached.len() {
        let frames = c.detached[id].frames;
        c.release_regions(frames);
        let d = &mut c.detached[id];
        for capture in &mut d.captures {
            for pin in capture.regions.drain(..) {
                c.cells.unpin(pin);
            }
        }
        for pin in d.enclosing.drain(..) {
            c.cells.unpin(pin);
        }
    }
}

/// Pins every region whose cells code written on `stack` reaches: those open on it, then, for a
/// detached body's stack, the regions that body pins around its own `handle`, and for a task's,
/// those around the region it runs in.
fn pin_enclosing(c: &mut Ctx, stack: usize) -> Vec<Pin> {
    let mut pins = Vec::new();
    let mut at = stack;
    loop {
        pins.extend(pin_open(c, at));
        if at == Owner::ENTRY.0 {
            break;
        }
        if let Some(body) = c.stacks[at].body {
            // Its own open walked on from here, and its regions may have parked since.
            let held = &c.detached[body].enclosing;
            pins.extend(held.iter().map(|pin| c.cells.repin(pin)));
            break;
        }
        match c.stacks[at].entered_from {
            Some(outer) => at = outer,
            None => break,
        }
    }
    pins
}

/// Pins every region `stack` holds open, outermost first.
fn pin_open(c: &mut Ctx, stack: usize) -> Vec<Pin> {
    let open: Vec<RegionId> = c.cells.nesting(Owner(stack)).collect();
    open.into_iter()
        .rev()
        .map(|region| {
            c.cells
                .pin(region)
                .expect("a region on its owner's nesting is open")
        })
        .collect()
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

/// A finished body gives back the regions it left open, those its snapshots pin staying parked for
/// a restore, and keeps its stack, closures and enclosing regions only while a snapshot could be
/// restored onto it.
fn finish(c: &mut Ctx, id: usize) {
    let d = &mut c.detached[id];
    d.state = State::Done;
    d.live = None;
    let frames = d.frames;
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
        for pin in d.enclosing.drain(..) {
            c.cells.unpin(pin);
        }
    }
    c.release_regions(frames);
}

/// Records the stop the body just made and answers its capture's index.
fn capture_stop(c: &mut Ctx, id: usize) -> usize {
    let d = &c.detached[id];
    let (sp, floor, own) = (d.sp, d.saved_floor, d.frames);
    // A stop from a task's stack has none to copy, and so has one the body grew onto: the frames
    // waiting there are not the ones a restore would write back.
    let live = if d.saved_current == own {
        d.stack.as_ref().and_then(|stack| stack.live(sp))
    } else {
        None
    };
    let (bytes, frames, pins, regions) = if let Some(live) = live {
        let bytes = live.to_vec();
        let mut pins = Vec::new();
        for chunk in bytes.chunks_exact(8) {
            let w = Word::from_ne_bytes(chunk.try_into().expect("eight bytes"));
            if c.heap.is_object(w) {
                heap::inc(w);
                pins.push(w);
            }
        }
        let frames = crate::rt::clone_frames(&c.stacks[own].list);
        (Some(bytes), frames, pins, pin_open(c, own))
    } else {
        (None, Vec::new(), Vec::new(), Vec::new())
    };
    let born = c.host_ops;
    let d = &mut c.detached[id];
    d.captures.push(Capture {
        sp,
        floor,
        bytes,
        frames,
        pins,
        regions,
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

/// Puts capture `k`'s snapshot back on the body's stack; `false` when it has none. The cells stay
/// as they are: a restored body reads what the runs before it wrote.
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
    // The restored frames close the regions open at the stop, so those go back on, over nothing
    // the stack still holds.
    c.release_regions(own);
    for pin in &c.detached[id].captures[k].regions {
        let reopened = c.cells.reopen(pin, Owner(own));
        debug_assert!(
            reopened,
            "a capture's region is parked once its stack lets it go"
        );
    }
    let d = &mut c.detached[id];
    d.sp = sp;
    d.saved_current = own;
    d.saved_floor = floor;
    d.state = State::Suspended;
    d.live = Some(k);
    true
}

/// The body's `perform`: hands the clause to whoever resumed the body, returns what `k` got.
/// Nothing uncounted may be owned across the switch (each restore would free it again), so
/// `owned` is dropped first.
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
    let mut frame = c.stacks[frames].list.pop();
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
        r = call_value(ctx, closure, &a.args);
        heap::dec(closure);
        answered = true;
    }
    let c = unsafe { &mut *ctx };
    let ret = c.detached[id].ret;
    if c.failed == 0 && ret != 0 && !answered {
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

unsafe extern "C" fn rt_resume_detached_entry(ctx: *mut Ctx, args: *const i64) -> i64 {
    let (packed, entry, v) = unsafe {
        (
            heap::imm_value(*args) as usize,
            heap::imm_value(*args.add(1)),
            *args.add(2),
        )
    };
    let (id, k) = (packed >> 32, packed & 0xffff_ffff);
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

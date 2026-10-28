//! A `handle` whose clause calls `resume` off the tail: the body runs on its own stack and `k`
//! switches into it. A stop a clause may resume again is snapshotted, so a finished body can be
//! resumed again (multi-shot); one a clause resumes at most once is not. When a continuation's last
//! holder lets it go, its snapshot is dropped, and the body with it where it waits at that stop.

use crate::emitted::{Block, Handed, PlyFnDesc, PlyType};
use crate::frames::{self, PlyFrame, Snapshot};
use crate::heap::{HEADER, Word};
use crate::rt::{
    Ctx, FAILED_ABORT, FAILED_UNWIND, FrameClause, HandlerFrame, Kept, StackCopy, call_clause,
    call_fun, drop_frame, no_stack_to_take, release_fun, release_vars, switch_keeping,
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
    /// The block whose objects the body, clauses and `return` clause hold for what their rows take
    /// around the `handle`, kept as the closures are.
    forwards: Option<Word>,
    starting: Option<Word>,
    state: State,
    captures: Vec<Capture>,
    live: Option<usize>,
    /// The counted block of the `handle`'s instance where its objects enter its clauses, which hold
    /// no frame: what the body holds of it once more, and where its variables are.
    instance: Option<Word>,
}

/// A heap instance's block as emitted code lays it out, past its header, in words: how many its
/// head holds before the instance, where in the head the body it is the `handle` of is (which
/// [`open_over`] writes), and where the place of its first variable is, their count after it.
pub(crate) const INSTANCE_HEAD: usize = 7;
pub(crate) const INSTANCE_DETACHED: usize = 4;
pub(crate) const INSTANCE_VARS: usize = 5;

/// A clause's record in an instance: its closure, the instance, its place, and the descriptor of
/// `String` where it is handed one.
pub(crate) const RECORD_CLOSURE: usize = 0;
pub(crate) const RECORD_INSTANCE: usize = 1;
pub(crate) const RECORD_STRING: usize = 3;

/// A detached `handle`'s forwarding block, as emitted code lays it out past its header, in words:
/// the handler (which [`open`] writes), the unit's word mapping, the static descriptor of the words
/// it forwards, how many those are (`m`), how many words the block holds, then the `m` words each
/// forwards to, the `m` tasks that handed them, the labels the descriptor leaves to them, and the
/// evidence objects. An object that forwards has its slot's environment point at a record: the
/// address of the word it forwards to, the operation's place and the address of that word's task,
/// which the call it forwards runs as. Every call of a continuation sets those `m` words to what its
/// caller hands for the same, their tasks to the caller's.
const FORWARDS_HANDLER: usize = 0;
const FORWARDS_MAPPED: usize = 1;
const FORWARDS_OWN: usize = 2;
const FORWARDS_COUNT: usize = 3;
const FORWARDS_WORDS: usize = 4;
const FORWARDS_TARGETS: usize = 5;

/// `void ply_words_mapped(PlyCtx *, const Word *own, const Word *labels, const Word *site, const
/// Word *words, Word *out)`, the head's.
type Mapped =
    unsafe extern "C" fn(*mut Ctx, *const Word, *const Word, *const Word, *const Word, *mut Word);

fn forwards_words(block: Word) -> *mut Word {
    unsafe { (block as *mut Word).byte_add(HEADER) }
}

fn forwards_bytes(block: Word) -> Word {
    let words = unsafe { *forwards_words(block).add(FORWARDS_WORDS) };
    HEADER as Word + 8 * words
}

/// Points body `id`'s forwarding words at the words the call resuming it handed for the same, as
/// `ctx`'s `entering_*` name them, the frame bridge where it handed none, each as the caller's task;
/// a word that is one of the block's own objects, which the body's own clause hands back, leaves its
/// word and task as they were.
fn retargeted(ctx: *mut Ctx, id: usize) {
    let c = unsafe { &mut *ctx };
    let site = std::mem::take(&mut c.entering_desc);
    let words = std::mem::take(&mut c.entering_words);
    let task = c.task;
    let Some(block) = c.detached[id].forwards else {
        return;
    };
    let b = forwards_words(block);
    let (m, size) = unsafe {
        (
            *b.add(FORWARDS_COUNT) as usize,
            *b.add(FORWARDS_WORDS) as usize,
        )
    };
    let targets = unsafe { b.add(FORWARDS_TARGETS) };
    let old: Vec<Word> = (0..m).map(|i| unsafe { *targets.add(i) }).collect();
    let mapped: Mapped = unsafe { std::mem::transmute(*b.add(FORWARDS_MAPPED)) };
    unsafe {
        mapped(
            ctx,
            *b.add(FORWARDS_OWN) as *const Word,
            targets.add(2 * m),
            site as *const Word,
            words as *const Word,
            targets,
        );
    }
    let own = (b as Word)..(unsafe { b.add(size) } as Word);
    for (i, was) in old.into_iter().enumerate() {
        let now = unsafe { targets.add(i) };
        if own.contains(unsafe { &*now }) {
            unsafe { now.write(was) };
        } else {
            unsafe { targets.add(m + i).write(task as Word) };
        }
    }
}

/// The tasks of forwarding block `block`'s words, each `task`, which handed them as the body opened.
fn tasks_handed(block: Word, task: i64) {
    let b = forwards_words(block);
    let m = unsafe { *b.add(FORWARDS_COUNT) } as usize;
    for i in 0..m {
        unsafe { b.add(FORWARDS_TARGETS + m + i).write(task as Word) };
    }
}

/// What a stop leaves behind.
struct Capture {
    /// What a restore puts back: the stack's bytes with their records' slots held, and its frames,
    /// what its runtime frames kept and the variables its running clauses hold. `None` for a stop
    /// resumed at most once, and for one from a stack other than the body's own.
    kept: Option<(Snapshot, StackCopy)>,
    /// The values of an instance's variables as the body stood at this stop, each held once more
    /// beside its type's descriptor: what a resumption from outside a clause of the `handle`
    /// continues with.
    vars: Vec<(Word, Word)>,
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

/// `rt_handle_install_detached` and `rt_handle_forwarded`: runs the body on a fresh stack until it
/// stops; answers the handle's value. Takes `forwards`, the body's forwarding block where it has one,
/// whose handler word this sets.
pub(crate) unsafe fn open(
    ctx: *mut Ctx,
    clauses: Vec<FrameClause>,
    ret: Word,
    body: Word,
    forwards: Option<Word>,
) -> Word {
    drain(ctx);
    let c = unsafe { &mut *ctx };
    let Some(stack) = Stack::take() else {
        clauses.into_iter().for_each(FrameClause::release);
        if ret != 0 {
            release_fun(ret);
        }
        release_fun(body);
        if let Some(block) = forwards {
            unsafe { Block::of(block).free(&raw mut c.heap, forwards_bytes(block)) };
        }
        return no_stack_to_take(c);
    };
    let id = c.detached.len();
    let opener = c.current;
    let fuel = c.fuel;
    let frames = c.open_stack(Some(opener));
    c.stacks[frames].body = Some(id);
    let handler = c.next_handler();
    if let Some(block) = forwards {
        unsafe {
            forwards_words(block)
                .add(FORWARDS_HANDLER)
                .write(handler as Word)
        };
        tasks_handed(block, c.task);
    }
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
        forwards,
        starting: Some(body),
        state: State::Fresh,
        captures: Vec::new(),
        live: None,
        instance: None,
    });
    unsafe { resume(ctx, id, None, None) }
}

/// `rt_detached_open`: runs the body of a `handle` whose objects enter its clauses on a fresh stack
/// until it stops or ends; answers the `handle`'s value, its landing the body's own. Takes `body`
/// and `forwards`, the body's forwarding block where it has one; holds `instance`, the counted
/// block of the `handle`'s instance, once more, writing the body's place into its head.
pub(crate) unsafe fn open_over(
    ctx: *mut Ctx,
    body: Word,
    forwards: Option<Word>,
    instance: Word,
) -> Word {
    drain(ctx);
    let c = unsafe { &mut *ctx };
    let Some(stack) = Stack::take() else {
        release_fun(body);
        if let Some(block) = forwards {
            unsafe { Block::of(block).free(&raw mut c.heap, forwards_bytes(block)) };
        }
        return no_stack_to_take(c);
    };
    let id = c.detached.len();
    let frames = c.open_stack(Some(c.current));
    c.stacks[frames].body = Some(id);
    Block::of(instance).dup();
    unsafe {
        instance_head(instance)
            .add(INSTANCE_DETACHED)
            .write(id as Word)
    };
    if let Some(block) = forwards {
        tasks_handed(block, c.task);
    }
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
        ret: 0,
        forwards,
        starting: Some(body),
        state: State::Fresh,
        captures: Vec::new(),
        live: None,
        instance: Some(instance),
    });
    unsafe { resume(ctx, id, None, None) }
}

/// The words of a heap instance's block past its header: its head, then the instance.
fn instance_head(block: Word) -> *mut Word {
    unsafe { (block as *mut Word).byte_add(HEADER) }
}

/// Where a heap instance's variables are, as its head names them: each a value and its type's
/// descriptor.
fn instance_vars(block: Word) -> (*mut Word, usize) {
    let head = instance_head(block);
    let (place, count) = unsafe {
        (
            *head.add(INSTANCE_VARS) as usize,
            *head.add(INSTANCE_VARS + 1) as usize,
        )
    };
    (unsafe { head.add(INSTANCE_HEAD + place) }, count)
}

/// The values of the variables of the instance in `block`, each held once more.
fn vars_kept(ctx: *mut Ctx, block: Word) -> Vec<Word> {
    let (vars, count) = instance_vars(block);
    (0..count)
        .map(|j| unsafe {
            let (v, ty) = (*vars.add(2 * j), *vars.add(2 * j + 1));
            if ty != 0 {
                ((*(ty as *const PlyType)).inc)(ctx, v);
            }
            v
        })
        .collect()
}

/// The variables of the instance in `block` given `values`, which this takes; what they held let
/// go.
fn vars_put(ctx: *mut Ctx, block: Word, values: Vec<Word>) {
    let (vars, _) = instance_vars(block);
    for (j, v) in values.into_iter().enumerate() {
        unsafe {
            let (old, ty) = (*vars.add(2 * j), *vars.add(2 * j + 1));
            if ty != 0 {
                ((*(ty as *const PlyType)).dec)(ctx, old);
            }
            vars.add(2 * j).write(v);
        }
    }
}

/// The values of the variables of the instance in `block`, each held once more, beside its type's
/// descriptor, so they can be let go of whatever becomes of the instance.
fn vars_typed(ctx: *mut Ctx, block: Word) -> Vec<(Word, Word)> {
    let (vars, count) = instance_vars(block);
    (0..count)
        .map(|j| unsafe {
            let (v, ty) = (*vars.add(2 * j), *vars.add(2 * j + 1));
            if ty != 0 {
                ((*(ty as *const PlyType)).inc)(ctx, v);
            }
            (v, ty)
        })
        .collect()
}

/// `values`, each held once more.
fn vars_held(ctx: *mut Ctx, values: &[(Word, Word)]) -> Vec<Word> {
    values
        .iter()
        .map(|&(v, ty)| {
            if ty != 0 {
                unsafe { ((*(ty as *const PlyType)).inc)(ctx, v) };
            }
            v
        })
        .collect()
}

/// Lets go of `values`, each through its type's descriptor.
fn vars_let_go(ctx: *mut Ctx, values: Vec<(Word, Word)>) {
    for (v, ty) in values {
        if ty != 0 {
            unsafe { ((*(ty as *const PlyType)).dec)(ctx, v) };
        }
    }
}

/// `rt_detached_stop`: the body of the `handle` the instance of clause record `record` belongs to
/// stops at a perform the clause answers off its tail, `args` its words and `resource` its label's
/// field-table entry; the clause runs where the body was entered or resumed from, which may resume
/// it `again`. Answers what `k` is resumed with.
pub(crate) unsafe fn stop_over(
    ctx: *mut Ctx,
    record: *const Word,
    args: Vec<Word>,
    resource: i64,
    again: bool,
) -> Word {
    let (closure, instance, string) = unsafe {
        (
            *record.add(RECORD_CLOSURE),
            *record.add(RECORD_INSTANCE) as *const Word,
            *record.add(RECORD_STRING),
        )
    };
    let id = unsafe { *instance.sub(INSTANCE_HEAD).add(INSTANCE_DETACHED) } as usize;
    let mut bound = args;
    // A clause written `[*t]` reads the label its call site named, before the continuation does.
    if string != 0 {
        let c = unsafe { &*ctx };
        let label = usize::try_from(resource)
            .ok()
            .and_then(|i| c.tables.fields.get(i))
            .cloned();
        if let Some(label) = label {
            bound.push(crate::rt::label_word(
                ctx,
                unsafe { &*(string as *const PlyType) },
                &label,
            ));
        }
    }
    unsafe { stopped(ctx, id, closure, bound, again) }
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
    // Over an instance, a resumption from a clause of the `handle` continues with that clause's
    // variables, which it gets back as they were once the body stops again; one from anywhere else
    // continues with those the body had at its stop.
    let instance = c.detached[id].instance;
    let mut clause_vars = None;
    if let (Some(block), Some(k)) = (instance, capture) {
        if clause_running(c, id) {
            clause_vars = Some(vars_kept(ctx, block));
        } else {
            let at_stop = vars_held(ctx, &unsafe { &*ctx }.detached[id].captures[k].vars);
            vars_put(ctx, block, at_stop);
        }
    }
    let c = unsafe { &mut *ctx };
    if capture.is_some() && instance.is_none() {
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
    let r = match d.request.take() {
        Some(Stopped::Performed {
            closure,
            mut args,
            again,
        }) => {
            d.state = State::Suspended;
            let k = capture_stop(ctx, id, again);
            if let Some(block) = instance {
                let kept = vars_typed(ctx, block);
                unsafe { &mut *ctx }.detached[id].captures[k].vars = kept;
            }
            let c = unsafe { &mut *ctx };
            args.push(token(c, id, k));
            // The clause works on its own copy of the variables, so each resumption it makes starts
            // from what the clause holds, whatever an earlier one did to the body's.
            let frames = c.detached[id].frames;
            let held = c.stacks[frames].list.first().and_then(|f| f.vars_copied());
            let here = c.keep(Kept::Clause(id));
            let r = call_clause(ctx, closure, args, held);
            let c = unsafe { &mut *ctx };
            c.kept_back(here);
            settle(c, id);
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
    };
    if let (Some(block), Some(values)) = (instance, clause_vars) {
        vars_put(ctx, block, values);
    }
    r
}

/// A finished body over an instance lets go of it once no clause of its `handle` runs, since a
/// running clause reads its variables through it.
fn settle(c: &mut Ctx, id: usize) {
    let running = clause_running(c, id);
    let d = &mut c.detached[id];
    if d.state == State::Done
        && !running
        && d.stack.is_none()
        && let Some(block) = d.instance.take()
    {
        release_fun(block);
    }
}

/// Whether a clause of body `id`'s `handle` runs on any stack. Each stack keeps what its running
/// clauses are, so a stack a snapshot is restored onto runs again the clauses running when it was
/// taken, and a stack let go of runs none.
fn clause_running(c: &Ctx, id: usize) -> bool {
    c.stacks.iter().any(|s| {
        s.kept
            .iter()
            .any(|k| matches!(k, Kept::Clause(i) if *i == id))
    })
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
    for f in [std::mem::take(&mut d.body), std::mem::take(&mut d.ret)] {
        if f != 0 {
            release_fun(f);
        }
    }
    if let Some(block) = c.detached[id].forwards.take() {
        unsafe { Block::of(block).free(&raw mut c.heap, forwards_bytes(block)) };
    }
    settle(c, id);
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
        vars: Vec::new(),
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
    unsafe { stopped(ctx, id, closure, args, again) }
}

/// [`stop`] with nothing uncounted owned across the switch.
unsafe fn stopped(ctx: *mut Ctx, id: usize, closure: Word, args: Vec<Word>, again: bool) -> Word {
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
    let mut r = call_fun(ctx, body, &[]);
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
        let clause = f.take_clause(a.clause);
        r = call_clause(ctx, clause.closure, a.args.handed(), held.take());
        clause.release();
        answered = true;
    }
    let c = unsafe { &mut *ctx };
    let ret = c.detached[id].ret;
    if c.failed == 0 && ret != 0 && !answered {
        r = Ctx::holding(ctx, held.take(), || call_fun(ctx, ret, &[r]));
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

/// The `k` a clause off the tail is handed: a block resuming the body from its capture in the entry
/// it was captured in, which the clause calls and lets go of through [`TOKEN`].
fn token(c: &mut Ctx, id: usize, capture: usize) -> Word {
    let packed = ((id << 32) | capture) as Word;
    let entry = c.entry as Word;
    let block = Block::alloc(&mut c.heap, TOKEN_BYTES).word();
    unsafe {
        let words = (block as *mut Word).byte_add(HEADER);
        words.write((&raw const TOKEN) as Word);
        words.add(1).write(packed);
        words.add(2).write(entry);
    }
    block
}

/// A continuation's block emitted code holds: its header, then [`TOKEN`], the body and capture it
/// resumes packed in one word, and the entry that captured them.
const TOKEN_BYTES: Word = (HEADER + 3 * size_of::<Word>()) as Word;

/// The descriptor of a continuation emitted code holds: it is resumed through `uniform` and released
/// through `release`, a native entry that queues its stop's drop; it has no typed entry, and crosses
/// no thread and outlives no entry.
static TOKEN: PlyFnDesc = PlyFnDesc {
    uniform: token_resumed,
    typed: None,
    convention: 0,
    release: token_released,
    share: crosses_nowhere,
    keep: crosses_nowhere,
};

/// The packed body and capture, and the entry, a continuation's block holds.
///
/// # Safety
/// `k` is a live block [`token`] laid out.
unsafe fn token_words(k: Word) -> (i64, i64) {
    let words = unsafe { (k as *const Word).byte_add(HEADER) };
    unsafe { (*words.add(1), *words.add(2)) }
}

unsafe extern "C" fn token_resumed(ctx: *mut Ctx, k: Word, args: *const Word) -> Word {
    let (packed, entry) = unsafe { token_words(k) };
    let (id, capture) = unpacked(packed);
    let v = unsafe { *args };
    let c = unsafe { &mut *ctx };
    if !captured_here(c, entry, id, capture) {
        return c.fail(err_stale_continuation());
    }
    retargeted(ctx, id);
    unsafe { resume(ctx, id, Some(capture), Some(v)) }
}

/// The last holder let go of a continuation: its stop is dropped at the next drain, and its block
/// given back.
unsafe extern "C" fn token_released(ctx: *mut Ctx, k: Handed) -> Word {
    let k = k.word();
    let (packed, entry) = unsafe { token_words(k) };
    let (id, capture) = unpacked(packed);
    let c = unsafe { &mut *ctx };
    if captured_here(c, entry, id, capture) {
        c.continuations_died.push((id, capture));
    }
    unsafe { Block::of(k).free(&raw mut c.heap, TOKEN_BYTES) };
    0
}

/// A continuation is its entry's: it crosses to no other thread and is kept past no entry.
unsafe extern "C" fn crosses_nowhere(_ctx: *mut Ctx, _k: Word) -> Word {
    0
}

/// The body and capture a token's packed word names.
fn unpacked(packed: i64) -> (usize, usize) {
    let packed = packed as usize;
    (packed >> 32, packed & 0xffff_ffff)
}

/// Whether this entry captured stop `k` of body `id`. A token from another entry may carry an index
/// this entry uses for a body of its own, or one past every body it has.
fn captured_here(c: &Ctx, entry: i64, id: usize, k: usize) -> bool {
    entry == c.entry as i64 && c.detached.get(id).is_some_and(|d| k < d.captures.len())
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
    let vars = std::mem::take(&mut d.captures[k].vars);
    vars_let_go(ctx, vars);
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
            let vars = std::mem::take(&mut unsafe { &mut *ctx }.detached[id].captures[k].vars);
            vars_let_go(ctx, vars);
        }
        let c = unsafe { &mut *ctx };
        let d = &mut c.detached[id];
        d.state = State::Done;
        d.live = None;
        give_back(c, id);
        // A clause of an outer body abandoned here may still be kept as running.
        if let Some(block) = c.detached[id].instance.take() {
            release_fun(block);
        }
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

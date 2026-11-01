use crate::emitted::{Block, Handed, PlyFnDesc, PlyType};
use crate::frames::{self, PlyFrame};
use crate::heap::{HEADER, Word};
use crate::rt::{
    Ctx, SavedState, call_clause, call_fun, fail_out_of_stack, release_fun,
    switch_stack_preserving_context,
};
use crate::stack::{Stack, switch};
use ply_core::{Diagnostic, codes};

pub struct Detached {
    stack: Option<Stack>,
    stack_index: usize,
    sp: usize,
    saved_current: usize,
    head: *mut PlyFrame,
    caller_stack_pointer: usize,
    resumer_stack_index: usize,
    request: Option<Stopped>,
    result: Word,
    body: Word,
    forwards: Option<Word>,
    starting: Option<Word>,
    state: State,
    captures: Vec<Capture>,
    live: Option<usize>,
    instance: Option<Word>,
}

pub const INSTANCE_HEADER_WORDS: usize = 7;
pub const INSTANCE_DETACHED: usize = 4;
pub const INSTANCE_VARS: usize = 5;

pub const RECORD_CLOSURE: usize = 0;
pub const RECORD_INSTANCE: usize = 1;
pub const RECORD_STRING: usize = 3;

const FORWARDS_MAP_FUNCTION: usize = 1;
const FORWARDS_DESCRIPTOR: usize = 2;
const FORWARDS_COUNT: usize = 3;
const FORWARDS_WORDS: usize = 4;
const FORWARDS_TARGETS: usize = 5;

type MapFunction =
    unsafe extern "C" fn(*mut Ctx, *const Word, *const Word, *const Word, *const Word, *mut Word);

fn forwards_words(block: Word) -> *mut Word {
    unsafe { (block as *mut Word).byte_add(HEADER) }
}

fn forwards_bytes(block: Word) -> Word {
    let words = unsafe { *forwards_words(block).add(FORWARDS_WORDS) };
    HEADER as Word + 8 * words
}

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
    let mapped: MapFunction = unsafe { std::mem::transmute(*b.add(FORWARDS_MAP_FUNCTION)) };
    unsafe {
        mapped(
            ctx,
            *b.add(FORWARDS_DESCRIPTOR) as *const Word,
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

fn assign_forwarded_task(block: Word, task: i64) {
    let b = forwards_words(block);
    let m = unsafe { *b.add(FORWARDS_COUNT) } as usize;
    for i in 0..m {
        unsafe { b.add(FORWARDS_TARGETS + m + i).write(task as Word) };
    }
}

struct Capture {
    vars: Vec<(Word, Word)>,
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

pub(crate) unsafe fn open_detached(
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
        return fail_out_of_stack(c);
    };
    let id = c.detached.len();
    let stack_index = c.open_stack();
    Block::of(instance).dup();
    unsafe {
        instance_head(instance)
            .add(INSTANCE_DETACHED)
            .write(id as Word)
    };
    if let Some(block) = forwards {
        assign_forwarded_task(block, c.task);
    }
    let sp = stack.prepare(entry, ctx as usize);
    c.detached.push(Detached {
        stack: Some(stack),
        stack_index,
        sp,
        saved_current: stack_index,
        head: std::ptr::null_mut(),
        caller_stack_pointer: 0,
        resumer_stack_index: 0,
        request: None,
        result: 0,
        body,
        forwards,
        starting: Some(body),
        state: State::Fresh,
        captures: Vec::new(),
        live: None,
        instance: Some(instance),
    });
    unsafe { resume(ctx, id, None, None) }
}

fn instance_head(block: Word) -> *mut Word {
    unsafe { (block as *mut Word).byte_add(HEADER) }
}

fn instance_vars(block: Word) -> (*mut Word, usize) {
    let head = instance_head(block);
    let (place, count) = unsafe {
        (
            *head.add(INSTANCE_VARS) as usize,
            *head.add(INSTANCE_VARS + 1) as usize,
        )
    };
    (unsafe { head.add(INSTANCE_HEADER_WORDS + place) }, count)
}

fn copy_instance_vars(ctx: *mut Ctx, block: Word) -> Vec<Word> {
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

fn retain_vars(ctx: *mut Ctx, values: &[(Word, Word)]) -> Vec<Word> {
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

fn release_vars(ctx: *mut Ctx, values: Vec<(Word, Word)>) {
    for (v, ty) in values {
        if ty != 0 {
            unsafe { ((*(ty as *const PlyType)).dec)(ctx, v) };
        }
    }
}

pub(crate) unsafe fn stop_detached(
    ctx: *mut Ctx,
    record: *const Word,
    args: Vec<Word>,
    resource: i64,
) -> Word {
    let (closure, instance, string) = unsafe {
        (
            *record.add(RECORD_CLOSURE),
            *record.add(RECORD_INSTANCE) as *const Word,
            *record.add(RECORD_STRING),
        )
    };
    let id = unsafe { *instance.sub(INSTANCE_HEADER_WORDS).add(INSTANCE_DETACHED) } as usize;
    let mut bound = args;
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
    unsafe { stopped(ctx, id, closure, bound) }
}

pub(crate) unsafe fn resume(
    ctx: *mut Ctx,
    id: usize,
    capture: Option<usize>,
    result: Option<Word>,
) -> Word {
    let c = unsafe { &mut *ctx };
    let d = &mut c.detached[id];
    match (&d.state, capture) {
        (State::Fresh, _) => {}
        (State::Suspended, Some(k)) if d.live == Some(k) => {}
        (State::Running | State::Suspended, _) | (State::Done, Some(_)) => {
            return c.fail(err_resumed_again());
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
    let instance = c.detached[id].instance;
    let mut clause_vars = None;
    if let (Some(block), Some(k)) = (instance, capture) {
        if clause_running(c, id) {
            clause_vars = Some(copy_instance_vars(ctx, block));
        } else {
            let stop_counts = retain_vars(ctx, &unsafe { &*ctx }.detached[id].captures[k].vars);
            vars_put(ctx, block, stop_counts);
        }
    }
    let c = unsafe { &mut *ctx };
    let d = &mut c.detached[id];
    if let Some(a) = result {
        d.result = a;
    }
    d.resumer_stack_index = c.current;
    d.state = State::Running;
    let (current, sp) = (d.saved_current, d.sp);
    c.current = current;
    if d.state == State::Running && d.starting.is_some() {
        c.starting_detached = Some(id);
    }
    let from = &mut c.detached[id].caller_stack_pointer as *mut usize;
    unsafe { switch_stack_preserving_context(ctx, from, sp) };

    let c = unsafe { &mut *ctx };
    let d = &mut c.detached[id];
    c.current = d.resumer_stack_index;
    let r = match d.request.take() {
        Some(Stopped::Performed { closure, mut args }) => {
            d.state = State::Suspended;
            let k = new_capture(ctx, id);
            if let Some(block) = instance {
                let kept = vars_typed(ctx, block);
                unsafe { &mut *ctx }.detached[id].captures[k].vars = kept;
            }
            let c = unsafe { &mut *ctx };
            args.push(token(c, id, k));
            let here = c.keep(SavedState::Clause(id));
            let r = call_clause(ctx, closure, args);
            let c = unsafe { &mut *ctx };
            c.pop_saved(here);
            release_if_finished(c, id);
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

fn release_if_finished(c: &mut Ctx, id: usize) {
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

fn clause_running(c: &Ctx, id: usize) -> bool {
    c.stacks
        .iter()
        .any(|s| s.iter().any(|k| matches!(k, SavedState::Clause(i) if *i == id)))
}

fn finish(c: &mut Ctx, id: usize) {
    let d = &mut c.detached[id];
    d.state = State::Done;
    d.live = None;
    give_back(c, id);
}

fn give_back(c: &mut Ctx, id: usize) {
    let d = &mut c.detached[id];
    d.stack = None;
    let body = std::mem::take(&mut d.body);
    if body != 0 {
        release_fun(body);
    }
    if let Some(block) = c.detached[id].forwards.take() {
        unsafe { Block::of(block).free(&raw mut c.heap, forwards_bytes(block)) };
    }
    release_if_finished(c, id);
}

fn new_capture(ctx: *mut Ctx, id: usize) -> usize {
    let d = &mut unsafe { &mut *ctx }.detached[id];
    d.captures.push(Capture { vars: Vec::new() });
    let k = d.captures.len() - 1;
    d.live = Some(k);
    k
}

unsafe fn stopped(ctx: *mut Ctx, id: usize, closure: Word, args: Vec<Word>) -> Word {
    let c = unsafe { &mut *ctx };
    let head = c.frames;
    let d = &mut c.detached[id];
    d.request = Some(Stopped::Performed { closure, args });
    d.saved_current = c.current;
    d.head = head;
    let to = d.caller_stack_pointer;
    let from = &mut d.sp as *mut usize;
    unsafe { switch_stack_preserving_context(ctx, from, to) };
    let c = unsafe { &mut *ctx };
    std::mem::take(&mut c.detached[id].result)
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
        let floor = unsafe { own.expect("a detached body starts on its stack").enter() };
        c.stack_floor
            .store(floor, std::sync::atomic::Ordering::Relaxed);
        c.frames = frames::base();
        (id, body)
    };
    let r = call_fun(ctx, body, &[]);
    let c = unsafe { &mut *ctx };
    let d = &mut c.detached[id];
    d.request = Some(if c.failed != 0 {
        Stopped::Failed
    } else {
        Stopped::Finished(r)
    });
    d.saved_current = c.current;
    let to = d.caller_stack_pointer;
    let from = &mut d.sp as *mut usize;
    unsafe { switch(&mut *from, to) };
    std::process::abort();
}

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

const TOKEN_BYTES: Word = (HEADER + 3 * size_of::<Word>()) as Word;

static TOKEN: PlyFnDesc = PlyFnDesc {
    uniform: token_resumed,
    typed: None,
    convention: 0,
    release: token_released,
    share: inert_continuation,
    keep: inert_continuation,
};

unsafe fn token_words(k: Word) -> (i64, i64) {
    let words = unsafe { (k as *const Word).byte_add(HEADER) };
    unsafe { (*words.add(1), *words.add(2)) }
}

unsafe extern "C" fn token_resumed(ctx: *mut Ctx, k: Word, args: *const Word) -> Word {
    let (packed, entry) = unsafe { token_words(k) };
    let (id, capture) = unpacked(packed);
    let v = unsafe { *args };
    let c = unsafe { &mut *ctx };
    if !capture_exists(c, entry, id, capture) {
        return c.fail(err_stale_continuation());
    }
    retargeted(ctx, id);
    unsafe { resume(ctx, id, Some(capture), Some(v)) }
}

unsafe extern "C" fn token_released(ctx: *mut Ctx, k: Handed) -> Word {
    let k = k.word();
    let (packed, entry) = unsafe { token_words(k) };
    let (id, capture) = unpacked(packed);
    let c = unsafe { &mut *ctx };
    if capture_exists(c, entry, id, capture) {
        c.continuations_died.push((id, capture));
    }
    unsafe { Block::of(k).free(&raw mut c.heap, TOKEN_BYTES) };
    0
}

unsafe extern "C" fn inert_continuation(_ctx: *mut Ctx, _k: Word) -> Word {
    0
}

fn unpacked(packed: i64) -> (usize, usize) {
    let packed = packed as usize;
    (packed >> 32, packed & 0xffff_ffff)
}

fn capture_exists(c: &Ctx, entry: i64, id: usize, k: usize) -> bool {
    entry == c.entry as i64 && c.detached.get(id).is_some_and(|d| k < d.captures.len())
}

pub(crate) fn drain(ctx: *mut Ctx) {
    while let Some((id, k)) = unsafe { (*ctx).continuations_died.pop() } {
        drop_stop(ctx, id, k);
    }
}

fn drop_stop(ctx: *mut Ctx, id: usize, k: usize) {
    let d = &mut unsafe { &mut *ctx }.detached[id];
    let vars = std::mem::take(&mut d.captures[k].vars);
    let waiting = d.state == State::Suspended && d.live == Some(k);
    release_vars(ctx, vars);
    if waiting {
        abandon(ctx, id);
    }
    let c = unsafe { &mut *ctx };
    if c.detached[id].state == State::Done {
        give_back(c, id);
    }
}

fn abandon(ctx: *mut Ctx, id: usize) {
    let d = &mut unsafe { &mut *ctx }.detached[id];
    d.state = State::Done;
    d.live = None;
    let (head, own) = (
        std::mem::replace(&mut d.head, std::ptr::null_mut()),
        d.stack_index,
    );
    unsafe { frames::release_frames(ctx, head) };
    release_stack(ctx, own);
}

fn release_stack(ctx: *mut Ctx, own: usize) {
    if let Some(s) = unsafe { &mut *ctx }.stacks.get_mut(own) {
        release_saved_states(std::mem::take(s));
    }
}

fn release_saved_states(kept: Vec<SavedState>) {
    kept.into_iter().for_each(SavedState::release);
}

pub(crate) fn teardown(ctx: *mut Ctx) {
    drain(ctx);
    let bodies = unsafe { &*ctx }.detached.len();
    for id in (0..bodies).rev() {
        let (suspended, done, own) = {
            let d = &unsafe { &*ctx }.detached[id];
            (
                d.state == State::Suspended,
                d.state == State::Done,
                d.stack_index,
            )
        };
        if suspended {
            abandon(ctx, id);
        } else if !done {
            release_stack(ctx, own);
        }
        for k in 0..unsafe { &*ctx }.detached[id].captures.len() {
            let vars = std::mem::take(&mut unsafe { &mut *ctx }.detached[id].captures[k].vars);
            release_vars(ctx, vars);
        }
        let c = unsafe { &mut *ctx };
        let d = &mut c.detached[id];
        d.state = State::Done;
        d.live = None;
        give_back(c, id);
        if let Some(block) = c.detached[id].instance.take() {
            release_fun(block);
        }
    }
    let c = unsafe { &mut *ctx };
    c.continuations_died.clear();
    c.detached.clear();
}

#[cold]
#[inline(never)]
fn err_resumed_again() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a continuation was resumed a second time",
    )
    .note("a continuation is a `once` function, so the check should have rejected a second call")
    .note("this is Ply's fault, not the program's")
}

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

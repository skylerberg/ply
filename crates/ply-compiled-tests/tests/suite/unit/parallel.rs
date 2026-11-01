use crate::bare::{context, helper};
use ply_compiled::emitted::{Handed, PlyFnDesc};
use ply_compiled::heap::{HEADER, SHARED, Word};
use ply_compiled::rt::Ctx;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Run = unsafe extern "C" fn(*mut Ctx, *mut Word, Word);

const BLOCK: Word = HEADER as Word + 8;

const PAIR: Word = HEADER as Word + 16;

fn alloc(ctx: *mut Ctx, bytes: Word, words: &[Word]) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, bytes) };
    for (i, word) in words.iter().enumerate() {
        unsafe { *(w as *mut Word).byte_add(HEADER).add(i) = *word };
    }
    w
}

fn block(ctx: *mut Ctx, word: Word) -> Word {
    alloc(ctx, BLOCK, &[word])
}

fn held(w: Word) -> Word {
    unsafe { *(w as *const Word).byte_add(HEADER) }
}

fn free(ctx: *mut Ctx, w: Word) {
    unsafe { helper::<Free>("rt_free")(ctx, w, BLOCK) };
}

unsafe extern "C" fn release(ctx: *mut Ctx, f: Handed) -> Word {
    free(ctx, unsafe { std::mem::transmute::<Handed, Word>(f) });
    0
}

unsafe extern "C" fn release_pair(ctx: *mut Ctx, f: Handed) -> Word {
    let w = unsafe { std::mem::transmute::<Handed, Word>(f) };
    unsafe { helper::<Free>("rt_free")(ctx, w, PAIR) };
    0
}

unsafe extern "C" fn crosses(_ctx: *mut Ctx, _f: Word) -> Word {
    1
}

unsafe extern "C" fn stays(_ctx: *mut Ctx, _f: Word) -> Word {
    0
}

static WALKED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn walks_then_marks(_ctx: *mut Ctx, f: Word) -> Word {
    let count = unsafe { &*(f as *const AtomicU32) };
    if count.load(Ordering::Relaxed) < SHARED {
        WALKED.store(true, Ordering::SeqCst);
        count.fetch_or(SHARED, Ordering::Relaxed);
    }
    1
}

static SKIPPED_BRANCH_STARTED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn first(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    block(ctx, 1)
}

unsafe extern "C" fn second(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    block(ctx, 2)
}

unsafe extern "C" fn unrun(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    SKIPPED_BRANCH_STARTED.store(true, Ordering::SeqCst);
    block(ctx, 3)
}

unsafe extern "C" fn fails(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    unsafe { (*ctx).failed = 1 };
    0
}

unsafe extern "C" fn apart(ctx: *mut Ctx, f: Word, _args: *const Word) -> Word {
    let from = unsafe { *(f as *const Word).byte_add(HEADER).add(1) };
    block(ctx, Word::from(from != ctx as Word))
}

unsafe extern "C" fn enters(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    let mut inner = context();
    block(ctx, left_apart(&mut inner))
}

const fn desc(
    uniform: unsafe extern "C" fn(*mut Ctx, Word, *const Word) -> Word,
    share: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
) -> PlyFnDesc {
    PlyFnDesc {
        uniform,
        typed: None,
        convention: 0,
        release,
        share,
        keep: stays,
    }
}

static FIRST: PlyFnDesc = desc(first, crosses);
static SECOND: PlyFnDesc = desc(second, crosses);
static FAILS: PlyFnDesc = desc(fails, crosses);
static FAILS_HERE: PlyFnDesc = desc(fails, stays);
static UNRUN: PlyFnDesc = desc(unrun, crosses);
static WALKS: PlyFnDesc = desc(first, walks_then_marks);
static ENTERS: PlyFnDesc = desc(enters, crosses);
static BRANCH_REPORTING_OTHER_THREAD: PlyFnDesc = PlyFnDesc {
    release: release_pair,
    ..desc(apart, crosses)
};

fn multicore() -> bool {
    std::thread::available_parallelism().map_or(1, |n| n.get()) >= 2
}

fn branches(ctx: &mut Ctx, descs: [&'static PlyFnDesc; 2]) -> [Word; 2] {
    let c: *mut Ctx = ctx;
    let mut slots = descs.map(|d| block(c, d as *const PlyFnDesc as Word));
    unsafe { helper::<Run>("rt_branches")(c, slots.as_mut_ptr(), 2) };
    slots
}

fn left_apart(ctx: &mut Ctx) -> Word {
    let c: *mut Ctx = ctx;
    ctx.begin();
    let mut slots = [0; 2].map(|_| {
        alloc(
            c,
            PAIR,
            &[&raw const BRANCH_REPORTING_OTHER_THREAD as Word, c as Word],
        )
    });
    unsafe { helper::<Run>("rt_branches")(c, slots.as_mut_ptr(), 2) };
    assert_eq!(ctx.failed, 0);
    let left = held(slots[0]) + held(slots[1]);
    for w in slots {
        free(c, w);
    }
    assert_eq!(ctx.heap.live(), 0, "a branch was not let go");
    ctx.end();
    left
}

#[test]
fn a_block_entered_on_a_parallel_branch_runs_on_its_own_thread_without_touching_the_outer_blocks() {
    if !multicore() {
        return;
    }
    assert_eq!(
        left_apart(&mut context()),
        2,
        "a block entered on a thread of its own"
    );
    let mut ctx = context();
    ctx.begin();
    let [a, b] = branches(&mut ctx, [&ENTERS, &ENTERS]);
    assert_eq!(
        held(a) + held(b),
        0,
        "the two blocks entered on the outer block's branches"
    );
    free(&mut ctx, a);
    free(&mut ctx, b);
    ctx.end();
}

#[test]
fn parallel_branches_store_their_results_in_their_slots_and_free_their_closures() {
    let mut ctx = context();
    ctx.begin();
    let [a, b] = branches(&mut ctx, [&FIRST, &SECOND]);
    assert_eq!(ctx.failed, 0);
    assert_eq!((held(a), held(b)), (1, 2));
    free(&mut ctx, a);
    free(&mut ctx, b);
    assert_eq!(ctx.heap.live(), 0, "a branch's closure was not let go");
    ctx.end();
}

#[test]
fn a_closure_sent_to_another_thread_has_its_captured_values_found_through_its_layout_table() {
    if !multicore() {
        return;
    }
    let mut ctx = context();
    ctx.begin();
    let [a, b] = branches(&mut ctx, [&WALKS, &SECOND]);
    assert!(
        WALKED.load(Ordering::SeqCst),
        "the closure was marked before its descriptor walked it"
    );
    assert_eq!((held(a), held(b)), (1, 2));
    free(&mut ctx, a);
    free(&mut ctx, b);
    assert_eq!(ctx.heap.live(), 0);
    ctx.end();
}

#[test]
fn when_a_parallel_branch_fails_its_result_is_0_and_the_later_branches_are_freed_without_running() {
    let mut ctx = context();
    ctx.begin();
    let slots = branches(&mut ctx, [&FAILS_HERE, &UNRUN]);
    assert_eq!(ctx.failed, 1);
    assert_eq!(slots, [0, 0]);
    assert!(!SKIPPED_BRANCH_STARTED.load(Ordering::SeqCst));
    assert_eq!(ctx.heap.live(), 0);
    ctx.failed = 0;
    ctx.end();
}

#[test]
fn a_branch_that_returns_after_another_branch_failed_still_returns_its_result_in_its_slot() {
    if !multicore() {
        return;
    }
    let mut ctx = context();
    ctx.begin();
    let [failed, returned] = branches(&mut ctx, [&FAILS, &SECOND]);
    assert_eq!((ctx.failed, failed), (1, 0));
    assert_eq!(held(returned), 2);
    free(&mut ctx, returned);
    assert_eq!(ctx.heap.live(), 0);
    ctx.failed = 0;
    ctx.end();
}

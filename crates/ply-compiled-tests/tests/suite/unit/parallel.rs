use crate::bare::{helper, new_context};
use ply_compiled::emitted::{OwnedWord, PlyFunctionDescriptor};
use ply_compiled::heap::{HEADER, SHARED, Word};
use ply_compiled::rt::Context;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

type Alloc = unsafe extern "C" fn(*mut Context, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Context, Word, Word);
type Run = unsafe extern "C" fn(*mut Context, *mut Word, Word);

const BLOCK: Word = HEADER as Word + 8;

const PAIR: Word = HEADER as Word + 16;

fn alloc(context: *mut Context, bytes: Word, words: &[Word]) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(context, bytes) };
    for (i, word) in words.iter().enumerate() {
        unsafe { *(w as *mut Word).byte_add(HEADER).add(i) = *word };
    }
    w
}

fn block(context: *mut Context, word: Word) -> Word {
    alloc(context, BLOCK, &[word])
}

fn first_word(w: Word) -> Word {
    unsafe { *(w as *const Word).byte_add(HEADER) }
}

fn free(context: *mut Context, w: Word) {
    unsafe { helper::<Free>("rt_free")(context, w, BLOCK) };
}

unsafe extern "C" fn release(context: *mut Context, f: OwnedWord) -> Word {
    free(context, unsafe {
        std::mem::transmute::<OwnedWord, Word>(f)
    });
    0
}

unsafe extern "C" fn release_pair(context: *mut Context, f: OwnedWord) -> Word {
    let w = unsafe { std::mem::transmute::<OwnedWord, Word>(f) };
    unsafe { helper::<Free>("rt_free")(context, w, PAIR) };
    0
}

unsafe extern "C" fn crosses(_context: *mut Context, _f: Word) -> Word {
    1
}

unsafe extern "C" fn stays(_context: *mut Context, _f: Word) -> Word {
    0
}

static WALKED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn walks_then_marks(_context: *mut Context, f: Word) -> Word {
    let count = unsafe { &*(f as *const AtomicU32) };
    if count.load(Ordering::Relaxed) < SHARED {
        WALKED.store(true, Ordering::SeqCst);
        count.fetch_or(SHARED, Ordering::Relaxed);
    }
    1
}

static SKIPPED_BRANCH_STARTED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn first(context: *mut Context, _f: Word, _args: *const Word) -> Word {
    block(context, 1)
}

unsafe extern "C" fn second(context: *mut Context, _f: Word, _args: *const Word) -> Word {
    block(context, 2)
}

unsafe extern "C" fn unrun(context: *mut Context, _f: Word, _args: *const Word) -> Word {
    SKIPPED_BRANCH_STARTED.store(true, Ordering::SeqCst);
    block(context, 3)
}

unsafe extern "C" fn fails(context: *mut Context, _f: Word, _args: *const Word) -> Word {
    unsafe { (*context).failed = 1 };
    0
}

unsafe extern "C" fn apart(context: *mut Context, f: Word, _args: *const Word) -> Word {
    let from = unsafe { *(f as *const Word).byte_add(HEADER).add(1) };
    block(context, Word::from(from != context as Word))
}

unsafe extern "C" fn enters(context: *mut Context, _f: Word, _args: *const Word) -> Word {
    let mut inner = new_context();
    block(context, left_apart(&mut inner))
}

const fn descriptor(
    uniform: unsafe extern "C" fn(*mut Context, Word, *const Word) -> Word,
    share: unsafe extern "C" fn(*mut Context, Word) -> Word,
) -> PlyFunctionDescriptor {
    PlyFunctionDescriptor {
        uniform,
        typed: None,
        convention: 0,
        release,
        share,
        keep: stays,
    }
}

static FIRST: PlyFunctionDescriptor = descriptor(first, crosses);
static SECOND: PlyFunctionDescriptor = descriptor(second, crosses);
static FAILS: PlyFunctionDescriptor = descriptor(fails, crosses);
static FAILS_HERE: PlyFunctionDescriptor = descriptor(fails, stays);
static UNRUN: PlyFunctionDescriptor = descriptor(unrun, crosses);
static WALKS: PlyFunctionDescriptor = descriptor(first, walks_then_marks);
static ENTERS: PlyFunctionDescriptor = descriptor(enters, crosses);
static BRANCH_REPORTING_OTHER_THREAD: PlyFunctionDescriptor = PlyFunctionDescriptor {
    release: release_pair,
    ..descriptor(apart, crosses)
};

fn multicore() -> bool {
    std::thread::available_parallelism().map_or(1, |n| n.get()) >= 2
}

fn branches(context: &mut Context, descs: [&'static PlyFunctionDescriptor; 2]) -> [Word; 2] {
    let c: *mut Context = context;
    let mut slots = descs.map(|d| block(c, d as *const PlyFunctionDescriptor as Word));
    unsafe { helper::<Run>("rt_branches")(c, slots.as_mut_ptr(), 2) };
    slots
}

fn left_apart(context: &mut Context) -> Word {
    let c: *mut Context = context;
    context.begin();
    let mut slots = [0; 2].map(|_| {
        alloc(
            c,
            PAIR,
            &[&raw const BRANCH_REPORTING_OTHER_THREAD as Word, c as Word],
        )
    });
    unsafe { helper::<Run>("rt_branches")(c, slots.as_mut_ptr(), 2) };
    assert_eq!(context.failed, 0);
    let left = first_word(slots[0]) + first_word(slots[1]);
    for w in slots {
        free(c, w);
    }
    assert_eq!(context.heap.live(), 0, "a branch was not let go");
    context.end();
    left
}

#[test]
fn a_block_entered_on_a_parallel_branch_runs_on_its_own_thread_without_touching_the_outer_blocks() {
    if !multicore() {
        return;
    }
    assert_eq!(
        left_apart(&mut new_context()),
        2,
        "a block entered on a thread of its own"
    );
    let mut context = new_context();
    context.begin();
    let [a, b] = branches(&mut context, [&ENTERS, &ENTERS]);
    assert_eq!(
        first_word(a) + first_word(b),
        0,
        "the two blocks entered on the outer block's branches"
    );
    free(&mut context, a);
    free(&mut context, b);
    context.end();
}

#[test]
fn parallel_branches_store_their_results_in_their_slots_and_free_their_closures() {
    let mut context = new_context();
    context.begin();
    let [a, b] = branches(&mut context, [&FIRST, &SECOND]);
    assert_eq!(context.failed, 0);
    assert_eq!((first_word(a), first_word(b)), (1, 2));
    free(&mut context, a);
    free(&mut context, b);
    assert_eq!(context.heap.live(), 0, "a branch's closure was not let go");
    context.end();
}

#[test]
fn a_closure_sent_to_another_thread_has_its_captured_values_found_through_its_layout_table() {
    if !multicore() {
        return;
    }
    let mut context = new_context();
    context.begin();
    let [a, b] = branches(&mut context, [&WALKS, &SECOND]);
    assert!(
        WALKED.load(Ordering::SeqCst),
        "the closure was marked before its descriptor walked it"
    );
    assert_eq!((first_word(a), first_word(b)), (1, 2));
    free(&mut context, a);
    free(&mut context, b);
    assert_eq!(context.heap.live(), 0);
    context.end();
}

#[test]
fn when_a_parallel_branch_fails_its_result_is_0_and_the_later_branches_are_freed_without_running() {
    let mut context = new_context();
    context.begin();
    let slots = branches(&mut context, [&FAILS_HERE, &UNRUN]);
    assert_eq!(context.failed, 1);
    assert_eq!(slots, [0, 0]);
    assert!(!SKIPPED_BRANCH_STARTED.load(Ordering::SeqCst));
    assert_eq!(context.heap.live(), 0);
    context.failed = 0;
    context.end();
}

#[test]
fn a_branch_that_returns_after_another_branch_failed_still_returns_its_result_in_its_slot() {
    if !multicore() {
        return;
    }
    let mut context = new_context();
    context.begin();
    let [failed, returned] = branches(&mut context, [&FAILS, &SECOND]);
    assert_eq!((context.failed, failed), (1, 0));
    assert_eq!(first_word(returned), 2);
    free(&mut context, returned);
    assert_eq!(context.heap.live(), 0);
    context.failed = 0;
    context.end();
}

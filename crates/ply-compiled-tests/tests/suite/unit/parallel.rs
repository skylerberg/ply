//! `parallel` blocks as emitted code runs them: closure blocks laid out as it lays them, whether
//! their branches leave this thread decided by what the block's site says they may perform, held
//! against each clause installed above it.

use crate::bare::{context, context_of, helper};
use ply_compiled::emitted::{Handed, PlyFnDesc};
use ply_compiled::heap::{HEADER, IMMORTAL, SHARED, Word};
use ply_compiled::rt::Ctx;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Run = unsafe extern "C" fn(*mut Ctx, *mut Word, Word);
type RunPast = unsafe extern "C" fn(*mut Ctx, *mut Word, Word, *const Word, Word);
type Install = unsafe extern "C" fn(*mut Ctx, *const Word, Word, Word) -> Word;
type Land = unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word;

/// A box of one word, the number it holds, and a closure block of its descriptor alone.
const BLOCK: Word = HEADER as Word + 8;

/// A closure block of its descriptor and the context of the machine whose block it is a branch of.
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

/// Captures nothing that cannot cross threads.
unsafe extern "C" fn crosses(_ctx: *mut Ctx, _f: Word) -> Word {
    1
}

/// Captures what may not leave this thread.
unsafe extern "C" fn stays(_ctx: *mut Ctx, _f: Word) -> Word {
    0
}

static WALKED: AtomicBool = AtomicBool::new(false);

/// Shares as emitted code's roots do: what the block captures is walked only where nothing has
/// marked the block yet, and the block marked after it.
unsafe extern "C" fn walks_then_marks(_ctx: *mut Ctx, f: Word) -> Word {
    let count = unsafe { &*(f as *const AtomicU32) };
    if count.load(Ordering::Relaxed) < SHARED {
        WALKED.store(true, Ordering::SeqCst);
        count.fetch_or(SHARED, Ordering::Relaxed);
    }
    1
}

static UNRUN_RAN: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn first(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    block(ctx, 1)
}

unsafe extern "C" fn second(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    block(ctx, 2)
}

/// The branch after one that fails in turn, which no other test runs.
unsafe extern "C" fn unrun(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    UNRUN_RAN.store(true, Ordering::SeqCst);
    block(ctx, 3)
}

unsafe extern "C" fn fails(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    unsafe { (*ctx).failed = 1 };
    0
}

/// Whether the branch ran in a context other than that of the block it is a branch of, which is
/// what a branch on another thread is given.
unsafe extern "C" fn apart(ctx: *mut Ctx, f: Word, _args: *const Word) -> Word {
    let from = unsafe { *(f as *const Word).byte_add(HEADER).add(1) };
    block(ctx, Word::from(from != ctx as Word))
}

/// Enters a machine of its own, which runs a block of two [`APART`] branches, and answers how many
/// of them left the thread it entered on.
unsafe extern "C" fn enters(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    let mut inner = context();
    block(ctx, apart_in(&mut inner, None, None))
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
static APART: PlyFnDesc = PlyFnDesc {
    release: release_pair,
    ..desc(apart, crosses)
};

fn multicore() -> bool {
    std::thread::available_parallelism().map_or(1, |n| n.get()) >= 2
}

/// A block of a branch for each of `descs`, run: its slots as `rt_branches` leaves them.
fn branches(ctx: &mut Ctx, descs: [&'static PlyFnDesc; 2]) -> [Word; 2] {
    let c: *mut Ctx = ctx;
    let mut slots = descs.map(|d| block(c, d as *const PlyFnDesc as Word));
    unsafe { helper::<Run>("rt_branches")(c, slots.as_mut_ptr(), 2) };
    slots
}

/// Under a handler of one clause where `clause` names an effect and its resource, a block of two
/// [`APART`] branches whose site says they may perform `reached` where it names one: how many of
/// them left the thread `ctx` runs on.
fn apart_in(ctx: &mut Ctx, clause: Option<(Word, Word)>, reached: Option<(Word, Word)>) -> Word {
    let clauses: Vec<Clause> = clause
        .map(|(effect, resource)| (effect, resource, None, 0))
        .into_iter()
        .collect();
    apart_under(ctx, &clauses, reached)
}

/// A clause of an effect and a resource whose closure is the one named, or else a block of
/// [`FIRST`] made where it is installed, and which resumes as a `handle` site's table says.
type Clause = (Word, Word, Option<Word>, Word);

/// [`apart_in`] under a handler of each of `clauses`, the first outermost.
fn apart_under(ctx: &mut Ctx, clauses: &[Clause], reached: Option<(Word, Word)>) -> Word {
    let c: *mut Ctx = ctx;
    ctx.begin(1_000);
    let depths: Vec<Word> = (clauses.iter())
        .map(|&(effect, resource, closure, resumes)| {
            let answers = closure.unwrap_or_else(|| block(c, &raw const FIRST as Word));
            let row: [Word; 6] = [effect, resource, effect, answers, resumes, 0];
            unsafe { helper::<Install>("rt_handle_install")(c, row.as_ptr(), 1, 0) }
        })
        .collect();
    let mut slots = [0; 2].map(|_| alloc(c, PAIR, &[&raw const APART as Word, c as Word]));
    match reached {
        None => unsafe { helper::<Run>("rt_branches")(c, slots.as_mut_ptr(), 2) },
        Some((effect, resource)) => {
            let reach = [effect, resource];
            unsafe {
                helper::<RunPast>("rt_branches_past")(c, slots.as_mut_ptr(), 2, reach.as_ptr(), 1)
            }
        }
    }
    assert_eq!(ctx.failed, 0);
    let left = held(slots[0]) + held(slots[1]);
    for w in slots {
        free(c, w);
    }
    for depth in depths.into_iter().rev() {
        unsafe { helper::<Land>("rt_handle_land")(c, depth, 0) };
    }
    assert_eq!(ctx.heap.live(), 0, "a branch or a clause was not let go");
    ctx.end();
    left
}

const HELD: Word = 0;
const OTHER: Word = 1;
const CONN: Word = 2;
const WIRE: Word = 3;
/// A clause on every resource, or a site that names none.
const ANY: Word = -1;

#[test]
fn a_block_leaves_this_thread_past_every_handler_its_site_says_it_cannot_reach() {
    // On one core every block runs its branches in turn, and there is nothing to tell apart.
    if !multicore() {
        return;
    }
    let apart = |clause, reached| {
        let mut ctx = context_of(&["held", "other", "conn", "wire"], Vec::new());
        apart_in(&mut ctx, clause, reached)
    };
    // The clause's effect and resource, the reached effect and resource, and how many branches left.
    for (clause, reached, left, when) in [
        (None, None, 2, "under no handler"),
        (
            None,
            Some((OTHER, ANY)),
            2,
            "under no handler, its reach said",
        ),
        (
            Some((HELD, ANY)),
            Some((OTHER, ANY)),
            2,
            "past a handler of another effect",
        ),
        (
            Some((HELD, ANY)),
            Some((HELD, ANY)),
            0,
            "under a handler it reaches",
        ),
        (
            Some((HELD, ANY)),
            None,
            0,
            "under any handler, reach unsaid",
        ),
        (
            Some((HELD, CONN)),
            Some((HELD, WIRE)),
            2,
            "past another resource's clause",
        ),
        (
            Some((HELD, CONN)),
            Some((HELD, CONN)),
            0,
            "under a clause on its resource",
        ),
        (
            Some((HELD, CONN)),
            Some((HELD, ANY)),
            0,
            "under a clause it may name",
        ),
        (
            Some((HELD, ANY)),
            Some((HELD, WIRE)),
            0,
            "under a clause on every resource",
        ),
    ] {
        assert_eq!(apart(clause, reached), left, "{when}");
    }
}

/// A closure block no count is kept of, as emitted code lays out one that captures nothing.
#[repr(C, align(8))]
struct Immortal {
    rc: u32,
    desc: &'static PlyFnDesc,
}

static ANSWERS_ANYWHERE: Immortal = Immortal {
    rc: IMMORTAL,
    desc: &FIRST,
};

/// A clause that answers with its value, of a frame holding no variables, whose closure is
/// immortal, answers a branch on another thread from a copy of its frame, so a block reaching it
/// leaves this thread; one that resumes `k`, which may unwind to its `handle` on a path that does
/// not, or whose closure is counted, keeps the block here, and so does every frame above one that
/// does not cross, which answers what those frames' clauses perform and raise.
#[test]
fn a_block_leaves_this_thread_under_clauses_that_answer_from_any_thread() {
    if !multicore() {
        return;
    }
    let immortal = &raw const ANSWERS_ANYWHERE as Word;
    let answering = (HELD, ANY, Some(immortal), 0);
    for (clauses, reached, left, when) in [
        (vec![answering], Some((HELD, ANY)), 2, "reaching it"),
        (vec![answering], None, 2, "reach unsaid"),
        (
            vec![(HELD, ANY, Some(immortal), 1)],
            Some((HELD, ANY)),
            0,
            "under one that resumes, which may unwind",
        ),
        (
            vec![(HELD, ANY, None, 0)],
            Some((HELD, ANY)),
            0,
            "under one whose closure is counted",
        ),
        (
            vec![(OTHER, ANY, None, 0), answering],
            Some((HELD, ANY)),
            0,
            "above a frame that does not cross",
        ),
        (
            vec![(OTHER, ANY, Some(immortal), 0), answering],
            Some((HELD, ANY)),
            2,
            "above a frame that crosses",
        ),
    ] {
        let mut ctx = context_of(&["held", "other", "conn", "wire"], Vec::new());
        assert_eq!(apart_under(&mut ctx, &clauses, reached), left, "{when}");
    }
}

/// A thread waiting on a block takes up other branches of the pool, so a machine that waited inside
/// another machine's branch would carry that machine's other work on its own stack.
#[test]
fn a_machine_entered_on_a_branch_runs_its_own_blocks_in_turn() {
    if !multicore() {
        return;
    }
    assert_eq!(
        apart_in(&mut context(), None, None),
        2,
        "a block entered on a thread of its own"
    );
    let mut ctx = context();
    ctx.begin(1_000);
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

/// Each branch's closure block is taken and let go once it has run, and its answer written
/// over it, at once on threads of their own where the closures may cross.
#[test]
fn branches_emitted_code_made_answer_in_their_slots_and_are_let_go() {
    let mut ctx = context();
    ctx.begin(1_000);
    let [a, b] = branches(&mut ctx, [&FIRST, &SECOND]);
    assert_eq!(ctx.failed, 0);
    assert_eq!((held(a), held(b)), (1, 2));
    free(&mut ctx, a);
    free(&mut ctx, b);
    assert_eq!(ctx.heap.live(), 0, "a branch's closure was not let go");
    ctx.end();
}

/// A branch's closure is marked shared by its descriptor, which walks what it captures on the
/// way: one the runtime marked first would cross with its captures counted as this thread's.
#[test]
fn a_branch_crossing_threads_is_marked_by_its_descriptor() {
    if !multicore() {
        return;
    }
    let mut ctx = context();
    ctx.begin(1_000);
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

/// In turn, a branch that fails leaves 0, and those after it are let go unrun.
#[test]
fn a_failing_branch_in_turn_leaves_0_and_those_after_it_are_let_go_unrun() {
    let mut ctx = context();
    ctx.begin(1_000);
    let slots = branches(&mut ctx, [&FAILS_HERE, &UNRUN]);
    assert_eq!(ctx.failed, 1);
    assert_eq!(slots, [0, 0]);
    assert!(!UNRUN_RAN.load(Ordering::SeqCst));
    assert_eq!(ctx.heap.live(), 0);
    ctx.failed = 0;
    ctx.end();
}

/// At once, a branch that answered after one before it failed leaves its answer in its slot
/// for emitted code, which knows its type, to let go.
#[test]
fn a_branch_that_answered_past_one_that_failed_is_left_for_emitted_code() {
    if !multicore() {
        return;
    }
    let mut ctx = context();
    ctx.begin(1_000);
    let [failed, answered] = branches(&mut ctx, [&FAILS, &SECOND]);
    assert_eq!((ctx.failed, failed), (1, 0));
    assert_eq!(held(answered), 2);
    free(&mut ctx, answered);
    assert_eq!(ctx.heap.live(), 0);
    ctx.failed = 0;
    ctx.end();
}

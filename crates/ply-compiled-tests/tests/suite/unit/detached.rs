//! Detached bodies run as compiled code runs them, the code written by hand: a body holding values
//! across `amb.flip()` in records of its own frames and of a runtime frame between them, a value of
//! every kind each, and clauses that resume its continuation twice, never, or leave it parked.

use super::frames::{boxed, count, dup, every_kind, freed, release_box};
use crate::bare::{context_of, helper};
use ply_compiled::emitted::{Block, Handed, PlyFnDesc, PlyOp};
use ply_compiled::frames::{PlyFrame, held_across};
use ply_compiled::heap::{KIND_REFUSED, Word, imm, imm_value, kind};
use ply_compiled::native::{Answered, I64};
use ply_compiled::rt::{Ctx, rt_dispatch, rt_handle_install_detached};
use std::cell::RefCell;
use std::sync::Arc;

/// The unit's fields, by index.
const AMB: i64 = 0;
const FLIP: i64 = 1;

/// Resumes at most once, and may resume again.
const ONCE: i64 = 2;
const AGAIN: i64 = 4;

/// What each block the body holds is told apart by: its own box, the closure and the box it
/// captures, the carried box, and the box the frame it calls holds.
const HELD: [Word; 5] = [1, 2, 3, 4, 5];

thread_local! {
    static TEXT: Arc<str> = Arc::from("held across a stop");
    /// What the body's frames counted of what they held, each run once past the stop.
    static SEEN: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    /// Whether each clause ran on a stack whose chain was null.
    static CLAUSES_UNRECORDED: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    /// What each continuation a clause was handed says of itself: whether its block is of the
    /// refused kind, whether its descriptor has no typed entry, and whether sharing and keeping it
    /// answer 0.
    static TOKENS: RefCell<Vec<(bool, bool, bool)>> = const { RefCell::new(Vec::new()) };
}

fn seen(n: u32) {
    SEEN.with(|s| s.borrow_mut().push(n));
}

fn taken<T>(cell: &'static std::thread::LocalKey<RefCell<Vec<T>>>) -> Vec<T> {
    cell.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

fn text_holders() -> usize {
    TEXT.with(Arc::strong_count)
}

/// A frame record of one box, as compiled code links one around a call that may stop.
#[repr(C)]
struct Linked {
    h: PlyFrame,
    slot: Word,
}

unsafe extern "C" fn box_each(ctx: *mut Ctx, slots: *mut Word, d: i64) {
    let w = unsafe { *slots };
    if d > 0 { dup(w) } else { release_box(ctx, w) }
}

/// Runs `call` with a box told apart by `marker` held in a record of its frame, then counts it and
/// lets it go, as a compiled frame's scope does.
unsafe fn holding_a_box(ctx: *mut Ctx, marker: Word, call: impl FnOnce() -> Word) -> Word {
    let mut frame = Linked {
        h: PlyFrame {
            prev: std::ptr::null_mut(),
            each: box_each,
        },
        slot: boxed(ctx, marker),
    };
    let record = (&raw mut frame).cast::<PlyFrame>();
    unsafe {
        if !(*ctx).frames.is_null() {
            (*record).prev = (*ctx).frames;
            (*ctx).frames = record;
        }
        let r = call();
        if (*ctx).frames == record {
            (*ctx).frames = (*record).prev;
        }
        let b = (*record.cast::<Linked>()).slot;
        if (*ctx).failed == 0 {
            seen(count(b));
        }
        release_box(ctx, b);
        r
    }
}

/// `amb.flip()`, as emitted code describes it: no parameters, and a `Bool` the clause resumes with.
static FLIP_OP: PlyOp = PlyOp {
    effect: AMB,
    op: FLIP,
    answer: std::ptr::null(),
    own: 0,
    params_len: 0,
    params: std::ptr::null(),
    vars_len: 0,
    vars: std::ptr::null(),
};

/// The body: its own box held across a runtime frame holding a value of every kind, across a
/// compiled frame holding a box, across `amb.flip()`. Answers 1 for `true` and 2 for `false`.
unsafe extern "C" fn body(ctx: *mut Ctx, _block: Word, _args: *const Word) -> Word {
    unsafe { holding_a_box(ctx, HELD[0], || through_a_runtime_frame(ctx)) }
}

unsafe fn through_a_runtime_frame(ctx: *mut Ctx) -> Word {
    let (held, words) = TEXT.with(|t| every_kind(ctx, [HELD[1], HELD[2], HELD[3]], t));
    let (r, back) = unsafe {
        held_across(ctx, held, || {
            holding_a_box(ctx, HELD[4], || {
                let flip = rt_dispatch(ctx, &FLIP_OP, I64(-1), I64(0), std::ptr::null());
                let flip = std::mem::transmute::<Handed, Word>(flip);
                if (*ctx).failed != 0 {
                    0
                } else {
                    imm(if imm_value(flip) == 1 { 1 } else { 2 })
                }
            })
        })
    };
    unsafe {
        if (*ctx).failed == 0 {
            for w in words {
                seen(count(w));
            }
        }
        for one in back {
            one.release(ctx);
        }
    }
    r
}

/// Notes the clause ran, and what continuation `k` says of itself through its block and
/// descriptor.
unsafe fn clause_ran(ctx: *mut Ctx, k: Word) {
    let unrecorded = unsafe { (*ctx).frames.is_null() };
    CLAUSES_UNRECORDED.with(|c| c.borrow_mut().push(unrecorded));
    let desc = unsafe { &**(k as *const *const PlyFnDesc).byte_add(16) };
    let crosses = unsafe { (desc.share)(ctx, k) != 0 || (desc.keep)(ctx, k) != 0 };
    let seen = (kind(k) == KIND_REFUSED, desc.typed.is_none(), !crosses);
    TOKENS.with(|t| t.borrow_mut().push(seen));
}

/// `amb.flip() resume k -> k(true) * 10 + k(false)`, `k` applied and let go of as emitted code
/// applies and lets go of a function value: through its descriptor.
unsafe extern "C" fn twice(ctx: *mut Ctx, _block: Word, args: *const Word) -> Word {
    unsafe {
        clause_ran(ctx, *args);
        let k = Block::of(*args);
        let first = k.call(ctx, &[imm(1)]);
        let second = if (*ctx).failed == 0 {
            k.call(ctx, &[imm(0)])
        } else {
            0
        };
        k.let_go_function(ctx);
        if (*ctx).failed != 0 {
            return 0;
        }
        imm(imm_value(first) * 10 + imm_value(second))
    }
}

/// `amb.flip() resume k -> 42`, `k` let go of through its descriptor.
unsafe extern "C" fn never(ctx: *mut Ctx, _block: Word, args: *const Word) -> Word {
    unsafe {
        clause_ran(ctx, *args);
        Block::of(*args).let_go_function(ctx);
    }
    imm(42)
}

/// `amb.flip() resume k -> Parked(k)`, the continuation the handle's answer.
unsafe extern "C" fn parks(ctx: *mut Ctx, _block: Word, args: *const Word) -> Word {
    unsafe {
        clause_ran(ctx, *args);
        *args
    }
}

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);

/// A closure over no captures: a block of its descriptor alone.
const CLOSURE: Word = 16 + 8;

fn closure(ctx: *mut Ctx, desc: &'static PlyFnDesc) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, CLOSURE) };
    unsafe { *(w as *mut Word).add(2) = (&raw const *desc) as Word };
    w
}

unsafe extern "C" fn released(ctx: *mut Ctx, h: Handed) -> Word {
    let w = unsafe { std::mem::transmute::<Handed, Word>(h) };
    unsafe { helper::<Free>("rt_free")(ctx, w, CLOSURE) };
    0
}

unsafe extern "C" fn stays_here(_ctx: *mut Ctx, _w: Word) -> Word {
    0
}

const fn desc(uniform: unsafe extern "C" fn(*mut Ctx, Word, *const Word) -> Word) -> PlyFnDesc {
    PlyFnDesc {
        uniform,
        typed: None,
        convention: 0,
        release: released,
        share: stays_here,
        keep: stays_here,
    }
}

static BODY: PlyFnDesc = desc(body);
static TWICE: PlyFnDesc = desc(twice);
static NEVER: PlyFnDesc = desc(never);
static PARKS: PlyFnDesc = desc(parks);

fn unit() -> Ctx {
    context_of(&["amb", "flip"], Vec::new())
}

/// `handle { body() } with { amb.flip() resume k -> <clause> }`, the clause resuming as `resumes`
/// says.
unsafe fn handled(ctx: *mut Ctx, clause: &'static PlyFnDesc, resumes: i64) -> Word {
    let body = closure(ctx, &BODY);
    let clause = closure(ctx, clause);
    let row: [Word; 6] = [AMB, -1, FLIP, clause, resumes, 0];
    let answer = unsafe {
        rt_handle_install_detached(ctx, row.as_ptr(), I64(1), Handed::of(0), Block::of(body))
    };
    unsafe { std::mem::transmute::<Handed, Word>(answer) }
}

fn failure(ctx: *mut Ctx) -> String {
    unsafe { (*ctx).take_failure() }.map_or_else(String::new, |d| d.message)
}

/// What the continuation a clause is handed says of itself: a block of the refused kind with no
/// typed entry, crossing nowhere.
const TOKEN: (bool, bool, bool) = (true, true, true);

/// Each run past the stop finds everything its frames held held once more for the snapshot, so a
/// release in the first cannot free what the second reads; when the clause lets go of the
/// continuation, the snapshot's holds go too, and everything comes back once.
#[test]
fn a_stop_resumed_twice_finds_every_value_held_for_each_run_and_everything_comes_back_once() {
    let mut owned = unit();
    let ctx = &raw mut owned;
    unsafe {
        (*ctx).begin(1_000);
        let answer = handled(ctx, &TWICE, AGAIN);
        assert_eq!((*ctx).failed, 0, "{}", failure(ctx));
        assert_eq!(imm_value(answer), 12);
        assert_eq!(taken(&SEEN), [2; 10]);
        assert_eq!(taken(&CLAUSES_UNRECORDED), [true]);
        assert_eq!(taken(&TOKENS), [TOKEN]);
        assert_eq!(freed(), HELD);
        assert_eq!(text_holders(), 1);
        assert_eq!((*ctx).heap.live(), 0);
        (*ctx).end();
    }
}

#[test]
fn a_stop_resumed_at_most_once_is_not_snapshotted_and_a_second_resumption_is_refused() {
    let mut owned = unit();
    let ctx = &raw mut owned;
    unsafe {
        (*ctx).begin(1_000);
        handled(ctx, &TWICE, ONCE);
        assert_ne!((*ctx).failed, 0);
        let said = failure(ctx);
        assert!(said.contains("has already ended"), "{said}");
        assert_eq!(taken(&SEEN), [1; 5]);
        assert_eq!(freed(), HELD);
        assert_eq!(text_holders(), 1);
        assert_eq!((*ctx).heap.live(), 0);
        (*ctx).end();
    }
}

/// A clause that lets go of its continuation without resuming it abandons the body: what the
/// body's frames held, and what its snapshot held, comes back once, and no frame runs on.
#[test]
fn a_body_whose_continuation_dies_unresumed_gives_back_what_it_held_once() {
    let mut owned = unit();
    let ctx = &raw mut owned;
    for resumes in [ONCE, AGAIN] {
        unsafe {
            (*ctx).begin(1_000);
            let answer = handled(ctx, &NEVER, resumes);
            assert_eq!((*ctx).failed, 0, "{}", failure(ctx));
            assert_eq!(imm_value(answer), 42);
            assert!(taken(&SEEN).is_empty(), "a frame ran past the stop");
            assert_eq!(taken(&TOKENS), [TOKEN]);
            assert_eq!(freed(), HELD, "resumes {resumes}");
            assert_eq!(text_holders(), 1);
            assert_eq!((*ctx).heap.live(), 0, "resumes {resumes}");
            (*ctx).end();
        }
    }
}

/// An entry that ends with a body suspended and its continuation still held gives back what the
/// body and its snapshot held before its heap goes, each once.
#[test]
fn an_entry_ending_with_a_body_suspended_gives_back_what_it_held_once() {
    let mut owned = unit();
    let ctx = &raw mut owned;
    for resumes in [ONCE, AGAIN] {
        unsafe {
            (*ctx).begin(1_000);
            handled(ctx, &PARKS, resumes);
            assert_eq!((*ctx).failed, 0, "{}", failure(ctx));
            assert!(freed().is_empty());
            assert!(text_holders() > 1);
            (*ctx).end();
            assert!(taken(&SEEN).is_empty(), "a frame ran past the stop");
            assert_eq!(freed(), HELD, "resumes {resumes}");
            assert_eq!(text_holders(), 1);
        }
    }
}

//! Detached bodies run as compiled code runs them, the code written by hand: a body holding values
//! across `amb.flip()` in records of its own frames and of a runtime frame between them, a value of
//! every kind each, and clauses that resume its continuation twice, never, or leave it parked.

use super::frames::{boxed, count, dup, every_kind, freed, release_box};
use crate::bare::{context_of, helper};
use ply_compiled::emitted::Block;
use ply_compiled::frames::{PlyFrame, held_across};
use ply_compiled::heap::{Word, imm, imm_value, inc};
use ply_compiled::native::Answered;
use ply_compiled::rt::{Ctx, rt_call, rt_closure, rt_handle_detached, rt_perform};
use std::cell::RefCell;
use std::sync::Arc;

type Entry = unsafe extern "C" fn(*mut Ctx, *const i64) -> i64;
type Given = unsafe extern "C" fn(*mut Ctx, Word);

/// The unit's fields and functions, by index.
const AMB: i64 = 0;
const FLIP: i64 = 1;
const BODY: i64 = 0;
const TWICE: i64 = 1;
const NEVER: i64 = 2;
const PARKS: i64 = 3;

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

/// The body: its own box held across a runtime frame holding a value of every kind, across a
/// compiled frame holding a box, across `amb.flip()`. Answers 1 for `true` and 2 for `false`.
unsafe extern "C" fn body(ctx: *mut Ctx, _args: *const i64) -> i64 {
    unsafe { holding_a_box(ctx, HELD[0], || through_a_runtime_frame(ctx)) }
}

unsafe fn through_a_runtime_frame(ctx: *mut Ctx) -> Word {
    let (held, words) = TEXT.with(|t| every_kind(ctx, [HELD[1], HELD[2], HELD[3]], t));
    let (r, back) = unsafe {
        held_across(ctx, held, || {
            holding_a_box(ctx, HELD[4], || {
                let none: [i64; 0] = [];
                let flip = rt_perform(ctx, AMB, FLIP, -1, 0, none.as_ptr(), 0);
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

/// `k(v)`, the clause keeping its own count of `k`.
unsafe fn resumed(ctx: *mut Ctx, k: Word, v: i64) -> Word {
    inc(k);
    let arg = [imm(v)];
    unsafe { rt_call(ctx, k, arg.as_ptr(), 1) }
}

/// Lets go of `k` as emitted code lets go of a continuation.
unsafe fn let_go(ctx: *mut Ctx, k: Word) {
    if unsafe { Block::of(k).last() } {
        unsafe { helper::<Given>("rt_handle_free")(ctx, k) };
    }
}

fn clause_ran(ctx: *mut Ctx) {
    let unrecorded = unsafe { (*ctx).frames.is_null() };
    CLAUSES_UNRECORDED.with(|c| c.borrow_mut().push(unrecorded));
}

/// `amb.flip() resume k -> k(true) * 10 + k(false)`.
unsafe extern "C" fn twice(ctx: *mut Ctx, args: *const i64) -> i64 {
    unsafe {
        let k = *args;
        clause_ran(ctx);
        let first = resumed(ctx, k, 1);
        let second = if (*ctx).failed == 0 {
            resumed(ctx, k, 0)
        } else {
            0
        };
        let_go(ctx, k);
        if (*ctx).failed != 0 {
            return 0;
        }
        imm(imm_value(first) * 10 + imm_value(second))
    }
}

/// `amb.flip() resume k -> 42`.
unsafe extern "C" fn never(ctx: *mut Ctx, args: *const i64) -> i64 {
    unsafe {
        clause_ran(ctx);
        let_go(ctx, *args);
    }
    imm(42)
}

/// `amb.flip() resume k -> Parked(k)`, the continuation the handle's answer.
unsafe extern "C" fn parks(ctx: *mut Ctx, args: *const i64) -> i64 {
    clause_ran(ctx);
    unsafe { *args }
}

fn unit() -> Ctx {
    let code = |f: Entry| f as usize;
    context_of(
        &["amb", "flip"],
        vec![code(body), code(twice), code(never), code(parks)],
    )
}

/// `handle { body() } with { amb.flip() resume k -> <clause> }`, the clause resuming as `resumes`
/// says.
unsafe fn handled(ctx: *mut Ctx, clause: i64, resumes: i64) -> Word {
    let none: [i64; 0] = [];
    unsafe {
        let body = rt_closure(ctx, BODY, 0, none.as_ptr(), 0);
        let clause = rt_closure(ctx, clause, 1, none.as_ptr(), 0);
        let row = [AMB, -1, FLIP, clause, resumes, 0];
        rt_handle_detached(ctx, row.as_ptr(), 1, 0, body)
    }
}

fn failure(ctx: *mut Ctx) -> String {
    unsafe { (*ctx).take_failure() }.map_or_else(String::new, |d| d.message)
}

/// Each run past the stop finds everything its frames held held once more for the snapshot, so a
/// release in the first cannot free what the second reads; when the clause lets go of the
/// continuation, the snapshot's holds go too, and everything comes back once.
#[test]
fn a_stop_resumed_twice_finds_every_value_held_for_each_run_and_everything_comes_back_once() {
    let mut owned = unit();
    let ctx = &raw mut owned;
    unsafe {
        (*ctx).begin(1_000);
        let answer = handled(ctx, TWICE, AGAIN);
        assert_eq!((*ctx).failed, 0, "{}", failure(ctx));
        assert_eq!(imm_value(answer), 12);
        assert_eq!(taken(&SEEN), [2; 10]);
        assert_eq!(taken(&CLAUSES_UNRECORDED), [true]);
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
        handled(ctx, TWICE, ONCE);
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
            let answer = handled(ctx, NEVER, resumes);
            assert_eq!((*ctx).failed, 0, "{}", failure(ctx));
            assert_eq!(imm_value(answer), 42);
            assert!(taken(&SEEN).is_empty(), "a frame ran past the stop");
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
            handled(ctx, PARKS, resumes);
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

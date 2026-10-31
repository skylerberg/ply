//! Detached bodies run as compiled code runs them, the code written by hand: a body holding values
//! across `amb.flip()` in records of its own frames and of a runtime frame between them, a value of
//! every kind each, and clauses that resume its continuation once, twice, never, or leave it parked.

use super::frames::{boxed, count, every_kind, freed, release_box};
use crate::bare::{context_of, helper};
use ply_compiled::detached::{INSTANCE_DETACHED, INSTANCE_HEAD, INSTANCE_VARS};
use ply_compiled::emitted::{Block, Handed, PlyFnDesc, PlyOp};
use ply_compiled::frames::{PlyFrame, held_across};
use ply_compiled::heap::{HEADER, Word, imm, imm_value};
use ply_compiled::native::{I64, Returned};
use ply_compiled::rt::{Ctx, rt_detached_open, rt_detached_perform};
use std::cell::{Cell, RefCell};
use std::sync::Arc;

/// The unit's fields, by index.
const AMB: i64 = 0;
const FLIP: i64 = 1;

/// What each block the body holds is told apart by: its own box, the closure and the box it
/// captures, the carried box, and the box the frame it calls holds.
const HELD: [Word; 5] = [1, 2, 3, 4, 5];

thread_local! {
    static TEXT: Arc<str> = Arc::from("held across a stop");
    /// What the body's frames counted of what they held, each run once past the stop.
    static SEEN: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    /// Whether each clause ran on a stack whose chain was null.
    static CLAUSES_UNRECORDED: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    /// What each continuation a clause was handed says of itself: whether its descriptor has no
    /// typed entry, and whether sharing and keeping it return 0.
    static TOKENS: RefCell<Vec<(bool, bool)>> = const { RefCell::new(Vec::new()) };
    /// The clause's record in the running `handle`'s instance.
    static STOP: Cell<*const Word> = const { Cell::new(std::ptr::null()) };
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

unsafe extern "C" fn box_each(ctx: *mut Ctx, slots: *mut Word, _d: i64) {
    release_box(ctx, unsafe { *slots });
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
    result: std::ptr::null(),
    own: 0,
    params_len: 0,
    params: std::ptr::null(),
    vars_len: 0,
    vars: std::ptr::null(),
};

/// The body: its own box held across a runtime frame holding a value of every kind, across a
/// compiled frame holding a box, across `amb.flip()`. Returns 1 for `true` and 2 for `false`.
unsafe extern "C" fn body(ctx: *mut Ctx, _block: Word, _args: *const Word) -> Word {
    unsafe { holding_a_box(ctx, HELD[0], || through_a_runtime_frame(ctx)) }
}

unsafe fn through_a_runtime_frame(ctx: *mut Ctx) -> Word {
    let (held, words) = TEXT.with(|t| every_kind(ctx, [HELD[1], HELD[2], HELD[3]], t));
    let (r, back) = unsafe {
        held_across(ctx, held, || {
            holding_a_box(ctx, HELD[4], || {
                let flip =
                    rt_detached_perform(ctx, STOP.get(), &FLIP_OP, I64(-1), std::ptr::null());
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
    let desc = unsafe { &**(k as *const *const PlyFnDesc).byte_add(HEADER) };
    let crosses = unsafe { (desc.share)(ctx, k) != 0 || (desc.keep)(ctx, k) != 0 };
    let seen = (desc.typed.is_none(), !crosses);
    TOKENS.with(|t| t.borrow_mut().push(seen));
}

/// `amb.flip() resume k -> k(true) * 10`, `k` applied and let go of as emitted code applies and
/// lets go of a function value: through its descriptor.
unsafe extern "C" fn once(ctx: *mut Ctx, _block: Word, args: *const Word) -> Word {
    unsafe {
        clause_ran(ctx, *args);
        let k = Block::of(*args);
        let first = k.call(ctx, &[imm(1)]);
        k.let_go_function(ctx);
        if (*ctx).failed != 0 {
            return 0;
        }
        imm(imm_value(first) * 10)
    }
}

/// `amb.flip() resume k -> k(true) * 10 + k(false)`, as a copy of `k` kept in a value may be called.
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

/// `amb.flip() resume k -> Parked(k)`, the continuation the handle's result.
unsafe extern "C" fn parks(ctx: *mut Ctx, _block: Word, args: *const Word) -> Word {
    unsafe {
        clause_ran(ctx, *args);
        *args
    }
}

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);

/// A closure over no captures: a block of its descriptor alone.
const CLOSURE: Word = (HEADER + 8) as Word;

fn closure(ctx: *mut Ctx, desc: &'static PlyFnDesc) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, CLOSURE) };
    unsafe { *(w as *mut Word).byte_add(HEADER) = (&raw const *desc) as Word };
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
static ONCE: PlyFnDesc = desc(once);
static TWICE: PlyFnDesc = desc(twice);
static NEVER: PlyFnDesc = desc(never);
static PARKS: PlyFnDesc = desc(parks);

fn unit() -> Ctx {
    context_of(&["amb", "flip"], Vec::new())
}

/// A `handle`'s instance on the heap: its head, then the clause's record, which holds its closure,
/// the instance, its place and no `String` descriptor.
const RECORD: usize = 4;
const INSTANCE: Word = (HEADER + 8 * (INSTANCE_HEAD + RECORD)) as Word;

unsafe extern "C" fn instance_released(ctx: *mut Ctx, h: Handed) -> Word {
    let w = unsafe { std::mem::transmute::<Handed, Word>(h) };
    unsafe {
        let clause = *(w as *const Word).byte_add(HEADER).add(INSTANCE_HEAD);
        Block::of(clause).let_go_function(ctx);
        helper::<Free>("rt_free")(ctx, w, INSTANCE);
    }
    0
}

static INSTANCE_DESC: PlyFnDesc = PlyFnDesc {
    release: instance_released,
    ..desc(body)
};

/// The counted block of an instance of a `handle` whose one clause is `clause`, as emitted code
/// lays one out, and the clause's record in it.
fn instance(ctx: *mut Ctx, clause: Word) -> (Word, *const Word) {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, INSTANCE) };
    let head = unsafe { (w as *mut Word).byte_add(HEADER) };
    let record = unsafe { head.add(INSTANCE_HEAD) };
    let mut words = [0; INSTANCE_HEAD + RECORD];
    words[0] = (&raw const INSTANCE_DESC) as Word;
    words[1] = RECORD as Word;
    words[INSTANCE_DETACHED] = -1;
    words[INSTANCE_VARS] = RECORD as Word;
    words[INSTANCE_HEAD..].copy_from_slice(&[clause, record as Word, 0, 0]);
    unsafe { std::ptr::copy_nonoverlapping(words.as_ptr(), head, words.len()) };
    (w, record)
}

/// `handle { body() } with { amb.flip() resume k -> <clause> }`.
unsafe fn handled(ctx: *mut Ctx, clause: &'static PlyFnDesc) -> Word {
    let body = closure(ctx, &BODY);
    let (instance, record) = instance(ctx, closure(ctx, clause));
    STOP.set(record);
    let result =
        unsafe { rt_detached_open(ctx, Block::of(body), Block::of(0), Block::of(instance)) };
    unsafe { Block::of(instance).let_go_function(ctx) };
    unsafe { std::mem::transmute::<Handed, Word>(result) }
}

fn failure(ctx: *mut Ctx) -> String {
    unsafe { (*ctx).take_failure() }.map_or_else(String::new, |d| d.message)
}

/// What the continuation a clause is handed says of itself: a block with no typed entry, marshalled
/// nowhere.
const TOKEN: (bool, bool) = (true, true);

/// The run past the stop finds everything its frames held held once, and everything comes back
/// once.
#[test]
fn a_stop_resumed_once_runs_on_and_everything_comes_back_once() {
    let mut owned = unit();
    let ctx = &raw mut owned;
    unsafe {
        (*ctx).begin();
        let result = handled(ctx, &ONCE);
        assert_eq!((*ctx).failed, 0, "{}", failure(ctx));
        assert_eq!(imm_value(result), 10);
        assert_eq!(taken(&SEEN), [1; 5]);
        assert_eq!(taken(&CLAUSES_UNRECORDED), [true]);
        assert_eq!(taken(&TOKENS), [TOKEN]);
        assert_eq!(freed(), HELD);
        assert_eq!(text_holders(), 1);
        assert_eq!((*ctx).heap.live(), 0);
        (*ctx).end();
    }
}

#[test]
fn a_second_resumption_of_a_stop_is_rejected_and_everything_comes_back_once() {
    let mut owned = unit();
    let ctx = &raw mut owned;
    unsafe {
        (*ctx).begin();
        handled(ctx, &TWICE);
        assert_ne!((*ctx).failed, 0);
        let said = failure(ctx);
        assert!(said.contains("resumed a second time"), "{said}");
        assert_eq!(taken(&SEEN), [1; 5]);
        assert_eq!(freed(), HELD);
        assert_eq!(text_holders(), 1);
        assert_eq!((*ctx).heap.live(), 0);
        (*ctx).end();
    }
}

/// A clause that lets go of its continuation without resuming it abandons the body: what the
/// body's frames held comes back once, and no frame runs on.
#[test]
fn a_body_whose_continuation_dies_unresumed_gives_back_what_it_held_once() {
    let mut owned = unit();
    let ctx = &raw mut owned;
    unsafe {
        (*ctx).begin();
        let result = handled(ctx, &NEVER);
        assert_eq!((*ctx).failed, 0, "{}", failure(ctx));
        assert_eq!(imm_value(result), 42);
        assert!(taken(&SEEN).is_empty(), "a frame ran past the stop");
        assert_eq!(taken(&TOKENS), [TOKEN]);
        assert_eq!(freed(), HELD);
        assert_eq!(text_holders(), 1);
        assert_eq!((*ctx).heap.live(), 0);
        (*ctx).end();
    }
}

/// An entry that ends with a body suspended and its continuation still held gives back what the
/// body held before its heap goes, each once.
#[test]
fn an_entry_ending_with_a_body_suspended_gives_back_what_it_held_once() {
    let mut owned = unit();
    let ctx = &raw mut owned;
    unsafe {
        (*ctx).begin();
        handled(ctx, &PARKS);
        assert_eq!((*ctx).failed, 0, "{}", failure(ctx));
        assert!(freed().is_empty());
        assert!(text_holders() > 1);
        (*ctx).end();
        assert!(taken(&SEEN).is_empty(), "a frame ran past the stop");
        assert_eq!(freed(), HELD);
        assert_eq!(text_holders(), 1);
    }
}

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

const AMB: i64 = 0;
const FLIP: i64 = 1;

const HELD: [Word; 5] = [1, 2, 3, 4, 5];

thread_local! {
    static TEXT: Arc<str> = Arc::from("held across a stop");
    static SEEN: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static CLAUSES_UNRECORDED: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    static TOKENS: RefCell<Vec<(bool, bool)>> = const { RefCell::new(Vec::new()) };
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

#[repr(C)]
struct Linked {
    h: PlyFrame,
    slot: Word,
}

unsafe extern "C" fn box_each(ctx: *mut Ctx, slots: *mut Word, _d: i64) {
    release_box(ctx, unsafe { *slots });
}

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

unsafe fn clause_ran(ctx: *mut Ctx, k: Word) {
    let unrecorded = unsafe { (*ctx).frames.is_null() };
    CLAUSES_UNRECORDED.with(|c| c.borrow_mut().push(unrecorded));
    let desc = unsafe { &**(k as *const *const PlyFnDesc).byte_add(HEADER) };
    let crosses = unsafe { (desc.share)(ctx, k) != 0 || (desc.keep)(ctx, k) != 0 };
    let seen = (desc.typed.is_none(), !crosses);
    TOKENS.with(|t| t.borrow_mut().push(seen));
}

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

unsafe extern "C" fn never(ctx: *mut Ctx, _block: Word, args: *const Word) -> Word {
    unsafe {
        clause_ran(ctx, *args);
        Block::of(*args).let_go_function(ctx);
    }
    imm(42)
}

unsafe extern "C" fn parks(ctx: *mut Ctx, _block: Word, args: *const Word) -> Word {
    unsafe {
        clause_ran(ctx, *args);
        *args
    }
}

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);

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

const TOKEN: (bool, bool) = (true, true);

#[test]
fn resuming_a_suspended_computation_once_lets_it_run_to_the_end_and_releases_every_resource_exactly_once()
 {
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
fn resuming_a_suspended_computation_a_second_time_fails_and_every_resource_is_still_released_exactly_once()
 {
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

#[test]
fn dropping_a_suspended_computation_without_resuming_it_releases_its_resources_exactly_once() {
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

#[test]
fn ending_an_entry_call_while_a_computation_is_still_suspended_releases_the_resources_of_that_computation_exactly_once()
 {
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

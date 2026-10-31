//! What the runtime does with values emitted code hands it beside their types: a perform's
//! arguments, a raise's payload, a label and a bracket's closures, each counted, crossed and let go
//! through the descriptors emitted code lays out. The blocks here are laid out
//! as emitted code lays one: a count at offset 0, freed by size.

use crate::bare::{context, context_of, helper};

/// A context over a unit whose field table holds `fields`, which name effects and operations.
fn naming(fields: &[&str]) -> Ctx {
    context_of(fields, Vec::new())
}
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::{Handed, PlyFnDesc, PlyOp, PlyType};
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::native::I64;
use ply_compiled::rt::{BRIDGE, Ctx, FAILED_TARGET};
use ply_core::{Effect, Mode, Resource};
use std::cell::Cell;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);

/// A value of `m.Box`: a block of one word, the number it holds.
const BOX: Word = HEADER as Word + 8;

fn boxed(ctx: &mut Ctx, n: Word) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, BOX) };
    unsafe { *(w as *mut Word).byte_add(HEADER) = n };
    w
}

unsafe extern "C" fn box_inc(_ctx: *mut Ctx, w: Word) -> Word {
    unsafe { *(w as *mut u32) += 1 };
    0
}

unsafe extern "C" fn box_dec(ctx: *mut Ctx, w: Word) -> Word {
    let rc = w as *mut u32;
    unsafe {
        if *rc > 1 {
            *rc -= 1;
        } else {
            helper::<Free>("rt_free")(ctx, w, BOX);
        }
    }
    0
}

unsafe extern "C" fn box_lower(ctx: *mut Ctx, w: Word) -> Word {
    let out: unsafe extern "C" fn(*mut Ctx, Word) = helper(&builtin_helper_name("?out_int"));
    unsafe { out(ctx, *(w as *const Word).byte_add(HEADER)) };
    0
}

unsafe extern "C" fn unread(_ctx: *mut Ctx, _w: Word) -> Word {
    0
}

unsafe extern "C" fn nothing(_ctx: *mut Ctx) -> Word {
    0
}

static BOX_T: PlyType = PlyType {
    identity: 1,
    spelled: b"m.Box".as_ptr(),
    spelled_len: 5,
    inc: box_inc,
    dec: box_dec,
    lower: box_lower,
    lift: nothing,
};

/// A `String` whose word is the address of a Rust string's bytes, which it lowers as.
unsafe extern "C" fn text_lower(ctx: *mut Ctx, w: Word) -> Word {
    let out: unsafe extern "C" fn(*mut Ctx, Word, Word) = helper(&builtin_helper_name("?out_str"));
    let s: &&str = unsafe { &*(w as *const &str) };
    unsafe { out(ctx, s.as_ptr() as Word, s.len() as Word) };
    0
}

static TEXT_T: PlyType = PlyType {
    identity: 2,
    spelled: b"String".as_ptr(),
    spelled_len: 6,
    inc: unread,
    dec: unread,
    lower: text_lower,
    lift: nothing,
};

struct Params([*const PlyType; 1]);
unsafe impl Sync for Params {}

static BOXES: Params = Params([&raw const BOX_T]);
static TEXTS: Params = Params([&raw const TEXT_T]);

/// An operation of the unit's fields 0 and 1, of one parameter of the types `params` holds.
fn op(params: &'static Params) -> PlyOp {
    PlyOp {
        effect: 0,
        op: 1,
        answer: &raw const TEXT_T,
        own: 0,
        params_len: 1,
        params: params.0.as_ptr(),
        vars_len: 0,
        vars: std::ptr::null(),
    }
}

/// A perform through the bridge, as emitted code performs where nothing between handles it.
fn performed(ctx: &mut Ctx, op: &PlyOp, mode: Word, args: &[Word]) -> Word {
    let entry = BRIDGE.slot.entry;
    let result = unsafe { entry(ctx, 0, op, I64(mode), I64(-1), args.as_ptr().cast()) };
    unsafe { std::mem::transmute::<Handed, Word>(result) }
}

#[test]
fn an_unhandled_raise_shows_its_payload_lowered_through_its_type_and_lets_it_go() {
    let mut ctx = naming(&["m", "boom"]);
    ctx.begin();
    let payload = boxed(&mut ctx, 42);
    performed(&mut ctx, &op(&BOXES), 2, &[payload]);
    let d = ctx.take_failure().expect("the raise failed the entry");
    assert!(
        d.message.contains("`m.boom` was raised with"),
        "{}",
        d.message
    );
    let mut shown = ply_core::codec::Writer::new();
    shown.int(42);
    assert_eq!(d.values.iter().collect::<Vec<_>>(), [&shown.finish()]);
    assert_eq!(ctx.heap.live(), 0, "the payload was not let go");
    ctx.end();
}

type PendingAt = unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word;

#[test]
fn an_unhandled_raise_waits_for_the_code_whose_clause_handles_its_operation() {
    let mut ctx = naming(&["m", "boom", "other"]);
    ctx.begin();
    let payload = boxed(&mut ctx, 42);
    performed(&mut ctx, &op(&BOXES), 2, &[payload]);
    assert_ne!(ctx.failed, 0, "the raise left the entry failed");
    let at = helper::<PendingAt>("rt_pending_raised_at");
    // A clause of another operation does not take it.
    assert_eq!(unsafe { at(&mut ctx, 0, 2) }, 0);
    assert_ne!(ctx.failed, 0);
    // The clause of its own takes what it carries, the entry no longer failed.
    let words = unsafe { at(&mut ctx, 0, 1) } as *const Word;
    assert!(!words.is_null());
    assert_eq!(ctx.failed, 0);
    assert_eq!(unsafe { *words }, payload);
    unsafe { box_dec(&mut ctx, payload) };
    assert_eq!(ctx.heap.live(), 0);
    ctx.end();
}

#[test]
fn an_unhandled_panic_says_its_message_lowered_through_its_type() {
    let mut ctx = naming(&["abort", "raise"]);
    ctx.begin();
    let message: &str = "it broke";
    performed(&mut ctx, &op(&TEXTS), 0, &[&raw const message as Word]);
    let d = ctx.take_failure().expect("the panic failed the entry");
    assert_eq!(d.message, "panic: it broke");
    ctx.end();
}

/// A host operation's arguments cross as their types lower them, and are let go through those
/// types whatever the host makes of them: here, nothing handles them.
#[test]
fn an_operation_nothing_handles_is_handed_its_arguments_lowered_and_lets_them_go() {
    let mut ctx = naming(&["m", "ask"]);
    ctx.begin();
    let arg = boxed(&mut ctx, 7);
    performed(&mut ctx, &op(&BOXES), 0, &[arg]);
    let d = ctx.take_failure().expect("nothing handles `m.ask`");
    assert!(d.message.contains("m.ask"), "{}", d.message);
    assert_eq!(ctx.heap.live(), 0, "the argument was not let go");
    assert_eq!(
        ctx.performed,
        [Effect::operation(
            "m",
            Resource::Singleton,
            Mode::Read,
            "ask"
        )]
    );
    ctx.end();
}

thread_local! {
    static CLEANED: Cell<Word> = const { Cell::new(0) };
}

/// A closure block as emitted code lays one: a count, then the descriptor's address.
const CLOSURE: Word = HEADER as Word + 8;

unsafe extern "C" fn closure_release(ctx: *mut Ctx, block: Handed) -> Word {
    let w = unsafe { std::mem::transmute::<Handed, Word>(block) };
    unsafe { helper::<Free>("rt_free")(ctx, w, CLOSURE) };
    0
}

/// The closures here cross to no other thread and outlive no entry.
unsafe extern "C" fn stays_in_its_entry(_ctx: *mut Ctx, _block: Word) -> Word {
    0
}

const fn desc(uniform: unsafe extern "C" fn(*mut Ctx, Word, *const Word) -> Word) -> PlyFnDesc {
    PlyFnDesc {
        uniform,
        typed: None,
        convention: 0,
        release: closure_release,
        share: stays_in_its_entry,
        keep: stays_in_its_entry,
    }
}

static ACQUIRE: PlyFnDesc = desc(acquire);
static ACQUIRE_SIX: PlyFnDesc = desc(acquire_six);
static BODY: PlyFnDesc = desc(body);
static CLEANUP: PlyFnDesc = desc(cleanup);

unsafe extern "C" fn acquire(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    boxed(unsafe { &mut *ctx }, 5)
}

/// Takes what was acquired and lets it go; returns 7, or unwinds where it was acquired as 6.
unsafe extern "C" fn body(ctx: *mut Ctx, _f: Word, args: *const Word) -> Word {
    let held = unsafe { *args };
    let n = unsafe { *(held as *const Word).byte_add(HEADER) };
    unsafe { box_dec(ctx, held) };
    if n == 6 {
        unsafe { (*ctx).failed = FAILED_TARGET };
        return 0;
    }
    7
}

unsafe extern "C" fn cleanup(ctx: *mut Ctx, _f: Word, args: *const Word) -> Word {
    let held = unsafe { *args };
    CLEANED.with(|c| c.set(unsafe { *(held as *const Word).byte_add(HEADER) }));
    unsafe { box_dec(ctx, held) };
    0
}

fn closure(ctx: &mut Ctx, d: &'static PlyFnDesc) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, CLOSURE) };
    unsafe { *(w as *mut *const PlyFnDesc).byte_add(HEADER) = d };
    w
}

type WithCleanup =
    unsafe extern "C" fn(*mut Ctx, Word, Word, Word, *const PlyType, *const PlyType) -> Word;

fn bracket(ctx: &mut Ctx, acquire_desc: &'static PlyFnDesc) -> Word {
    let with: WithCleanup = helper("rt_with_cleanup");
    let (a, b, c) = (
        closure(ctx, acquire_desc),
        closure(ctx, &BODY),
        closure(ctx, &CLEANUP),
    );
    unsafe { with(ctx, a, b, c, &BOX_T, &TEXT_T) }
}

#[test]
fn a_bracket_of_emitted_closures_cleans_up_what_it_acquired_and_lets_every_closure_go() {
    let mut ctx = context();
    ctx.begin();
    CLEANED.with(|c| c.set(0));
    let result = bracket(&mut ctx, &ACQUIRE);
    assert_eq!((result, ctx.failed), (7, 0));
    assert_eq!(CLEANED.with(Cell::get), 5);
    assert_eq!(ctx.heap.live(), 0);
    ctx.end();
}

unsafe extern "C" fn acquire_six(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    boxed(unsafe { &mut *ctx }, 6)
}

/// A body a clause unwinds through is cleaned up after, and the unwind goes on.
#[test]
fn a_bracket_cleans_up_after_a_body_that_unwinds() {
    let mut ctx = context();
    ctx.begin();
    CLEANED.with(|c| c.set(0));
    let result = bracket(&mut ctx, &ACQUIRE_SIX);
    assert_eq!((result, ctx.failed), (0, FAILED_TARGET));
    assert_eq!(CLEANED.with(Cell::get), 6);
    assert_eq!(ctx.heap.live(), 0);
    ctx.failed = 0;
    ctx.end();
}

thread_local! {
    static LIFTED: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// A `String` as emitted code lays one, which here is a box standing for the text, the text kept
/// aside: what the runtime lifts a label through.
unsafe extern "C" fn string_lift(ctx: *mut Ctx) -> Word {
    let text: unsafe extern "C" fn(*mut Ctx) -> Word = helper(&builtin_helper_name("?in_str"));
    let taken: unsafe extern "C" fn(*mut Ctx, *mut u8, Word) =
        helper(&builtin_helper_name("?taken"));
    let n = unsafe { text(ctx) };
    let mut b = vec![0u8; n as usize];
    unsafe { taken(ctx, b.as_mut_ptr(), n) };
    LIFTED.with(|l| *l.borrow_mut() = String::from_utf8(b).expect("UTF-8"));
    boxed(unsafe { &mut *ctx }, n)
}

static STRING_T: PlyType = PlyType {
    identity: 3,
    spelled: b"String".as_ptr(),
    spelled_len: 6,
    inc: box_inc,
    dec: box_dec,
    lower: unread,
    lift: string_lift,
};

type LabelString = unsafe extern "C" fn(*mut Ctx, Word, *const PlyType) -> Word;

/// A clause written `[*t]` reads its label as emitted code lays a `String`, lifted through the type
/// its site handed, never as a string the runtime made.
#[test]
fn a_label_a_clause_reads_is_lifted_through_the_string_type_its_site_handed() {
    let mut ctx = naming(&["m", "ask", "*", "files"]);
    ctx.begin();
    LIFTED.with(|l| l.borrow_mut().clear());
    let label = unsafe { helper::<LabelString>("rt_label_string")(&mut ctx, 3, &STRING_T) };
    super::marshalling::unfailed(&mut ctx);
    assert_eq!(LIFTED.with(|l| l.borrow().clone()), "files");
    unsafe { box_dec(&mut ctx, label) };
    assert_eq!(ctx.heap.live(), 0, "the label was not let go");
    ctx.end();
}

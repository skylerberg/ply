//! What the runtime does with values emitted code hands it beside their types: a perform's
//! arguments, a raise's payload, a handler's variables and a bracket's closures, each counted,
//! crossed and let go through the descriptors emitted code lays out. The blocks here are laid out
//! as emitted code lays one: a count at offset 0, freed by size.

use crate::bare::{context, helper, naming};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::{Handed, PlyFnDesc, PlyOp, PlyType};
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::rt::{Ctx, FAILED_UNWIND};
use ply_core::{EffectAtom, Mode, Plain, Resource};
use std::cell::Cell;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Dispatch = unsafe extern "C" fn(*mut Ctx, *const PlyOp, Word, Word, *const Word) -> Word;

/// A value of `m.Box`: a block of one word, the number it holds.
const BOX: Word = HEADER as Word + 8;

fn boxed(ctx: &mut Ctx, n: Word) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, BOX) };
    unsafe { *(w as *mut Word).add(2) = n };
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
    unsafe { out(ctx, *(w as *const Word).add(2)) };
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

fn dispatch(ctx: &mut Ctx, op: &PlyOp, mode: Word, args: &[Word]) -> Word {
    unsafe { helper::<Dispatch>("rt_dispatch")(ctx, op, -1, mode, args.as_ptr()) }
}

#[test]
fn a_raise_nothing_answers_shows_its_payload_lowered_through_its_type_and_lets_it_go() {
    let mut ctx = naming(&["m", "boom"]);
    ctx.begin(1_000);
    let payload = boxed(&mut ctx, 42);
    dispatch(&mut ctx, &op(&BOXES), 2, &[payload]);
    let d = ctx.take_failure().expect("the raise failed the entry");
    assert!(
        d.message.contains("`m.boom` was raised with"),
        "{}",
        d.message
    );
    assert_eq!(d.values.iter().collect::<Vec<_>>(), [&Plain::Int(42)]);
    assert_eq!(ctx.heap.live(), 0, "the payload was not let go");
    ctx.end();
}

#[test]
fn a_panic_nothing_answers_says_its_message_lowered_through_its_type() {
    let mut ctx = naming(&["abort", "raise"]);
    ctx.begin(1_000);
    let message: &str = "it broke";
    dispatch(&mut ctx, &op(&TEXTS), 0, &[&raw const message as Word]);
    let d = ctx.take_failure().expect("the panic failed the entry");
    assert_eq!(d.message, "panic: it broke");
    ctx.end();
}

/// A host operation's arguments cross as their types lower them, and are let go through those
/// types whatever the host makes of them: here, nothing answers.
#[test]
fn an_operation_nothing_handles_is_handed_its_arguments_lowered_and_lets_them_go() {
    let mut ctx = naming(&["m", "ask"]);
    ctx.begin(1_000);
    let arg = boxed(&mut ctx, 7);
    dispatch(&mut ctx, &op(&BOXES), 0, &[arg]);
    let d = ctx.take_failure().expect("nothing handles `m.ask`");
    assert!(d.message.contains("m.ask"), "{}", d.message);
    assert_eq!(ctx.heap.live(), 0, "the argument was not let go");
    assert_eq!(
        ctx.performed,
        [EffectAtom::operation(
            "m",
            Resource::Singleton,
            Mode::Read,
            "ask"
        )]
    );
    ctx.end();
}

/// A handler's variable emitted code gave a type is let go through it when its `handle` lands.
#[test]
fn a_handlers_typed_variable_is_let_go_through_its_type_when_its_handle_ends() {
    let mut ctx = context();
    ctx.begin(1_000);
    let install: unsafe extern "C" fn(*mut Ctx, *const Word, Word, Word) -> Word =
        helper("rt_handle_install");
    let typed: unsafe extern "C" fn(*mut Ctx, Word, Word, *const PlyType) =
        helper(&builtin_helper_name("?handler_typed"));
    let land: unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word = helper("rt_handle_land");
    let v = boxed(&mut ctx, 3);
    unsafe {
        let depth = install(&mut ctx, std::ptr::null(), 0, 0);
        typed(&mut ctx, 0, v, &BOX_T);
        assert_eq!(ctx.heap.live(), 1);
        assert_eq!(land(&mut ctx, depth, 9), 9);
    }
    assert_eq!(ctx.failed, 0);
    assert_eq!(ctx.heap.live(), 0, "the variable was not let go");
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

const fn desc(uniform: unsafe extern "C" fn(*mut Ctx, Word, *const Word) -> Word) -> PlyFnDesc {
    PlyFnDesc {
        uniform,
        typed: std::ptr::null(),
        convention: 0,
        release: closure_release,
        share: std::ptr::null(),
        keep: std::ptr::null(),
    }
}

static ACQUIRE: PlyFnDesc = desc(acquire);
static ACQUIRE_SIX: PlyFnDesc = desc(acquire_six);
static BODY: PlyFnDesc = desc(body);
static CLEANUP: PlyFnDesc = desc(cleanup);

unsafe extern "C" fn acquire(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    boxed(unsafe { &mut *ctx }, 5)
}

/// Takes what was acquired and lets it go; answers 7, or unwinds where it was acquired as 6.
unsafe extern "C" fn body(ctx: *mut Ctx, _f: Word, args: *const Word) -> Word {
    let held = unsafe { *args };
    let n = unsafe { *(held as *const Word).add(2) };
    unsafe { box_dec(ctx, held) };
    if n == 6 {
        unsafe { (*ctx).failed = FAILED_UNWIND };
        return 0;
    }
    7
}

unsafe extern "C" fn cleanup(ctx: *mut Ctx, _f: Word, args: *const Word) -> Word {
    let held = unsafe { *args };
    CLEANED.with(|c| c.set(unsafe { *(held as *const Word).add(2) }));
    unsafe { box_dec(ctx, held) };
    0
}

fn closure(ctx: &mut Ctx, d: &'static PlyFnDesc) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, CLOSURE) };
    unsafe { *(w as *mut *const PlyFnDesc).add(2) = d };
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
    ctx.begin(1_000);
    CLEANED.with(|c| c.set(0));
    let answer = bracket(&mut ctx, &ACQUIRE);
    assert_eq!((answer, ctx.failed), (7, 0));
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
    ctx.begin(1_000);
    CLEANED.with(|c| c.set(0));
    let answer = bracket(&mut ctx, &ACQUIRE_SIX);
    assert_eq!((answer, ctx.failed), (0, FAILED_UNWIND));
    assert_eq!(CLEANED.with(Cell::get), 6);
    assert_eq!(ctx.heap.live(), 0);
    ctx.failed = 0;
    ctx.end();
}

/// What a stretch of an entry cost, read as `metered` reads it: calls, allocations, bytes, live
/// objects, and each atom performed with its count, in the atoms' order.
#[test]
fn what_an_entry_spent_since_a_mark_is_read_back_from_it() {
    let mut ctx = context();
    ctx.begin(1_000);
    let mark: unsafe extern "C" fn(*mut Ctx, *mut Word) =
        helper(&builtin_helper_name("?cost_mark"));
    let spent: unsafe extern "C" fn(*mut Ctx, *mut Word) =
        helper(&builtin_helper_name("?cost_spent"));
    let mut m = [0 as Word; 5];
    unsafe { mark(&mut ctx, m.as_mut_ptr()) };
    let kept = boxed(&mut ctx, 1);
    let gone = boxed(&mut ctx, 2);
    unsafe { box_dec(&mut ctx, gone) };
    let atom = |op: &str| EffectAtom::operation("m", Resource::Singleton, Mode::Read, op);
    ctx.performed.extend([atom("b"), atom("a"), atom("b")]);
    unsafe { spent(&mut ctx, m.as_mut_ptr()) };
    assert_eq!(m[..4], [0, 2, 2 * BOX, 1]);
    let since: unsafe extern "C" fn(*mut Ctx, Word) =
        helper(&builtin_helper_name("?performs_since"));
    let count: unsafe extern "C" fn(*mut Ctx, Word) -> Word =
        helper(&builtin_helper_name("?in_count"));
    let int: unsafe extern "C" fn(*mut Ctx) -> Word = helper(&builtin_helper_name("?in_int"));
    let text: unsafe extern "C" fn(*mut Ctx) -> Word = helper(&builtin_helper_name("?in_str"));
    let taken: unsafe extern "C" fn(*mut Ctx, *mut u8, Word) =
        helper(&builtin_helper_name("?taken"));
    let close: unsafe extern "C" fn(*mut Ctx) = helper(&builtin_helper_name("?lift_close"));
    let mut read = Vec::new();
    unsafe {
        since(&mut ctx, m[4]);
        assert_eq!(count(&mut ctx, ply_core::codec::tag::LIST as Word), 4);
        for _ in 0..2 {
            let n = text(&mut ctx);
            let mut b = vec![0u8; n as usize];
            taken(&mut ctx, b.as_mut_ptr(), n);
            read.push((String::from_utf8(b).unwrap(), int(&mut ctx)));
        }
        close(&mut ctx);
    }
    super::crossing::unfailed(&mut ctx);
    let named = |op: &str| atom(op).to_string();
    assert_eq!(read, [(named("a"), 1), (named("b"), 2)]);
    unsafe { box_dec(&mut ctx, kept) };
    ctx.end();
}

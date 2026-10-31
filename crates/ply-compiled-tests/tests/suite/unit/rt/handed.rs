use crate::bare::{context, context_of, helper};

fn naming(fields: &[&str]) -> Ctx {
    context_of(fields, Vec::new())
}
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::{Handed, PlyFnDesc, PlyOp, PlyType};
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::native::I64;
use ply_compiled::rt::{Ctx, FAILED_UNWOUND_TO_HANDLER, NO_HANDLER_OBJECT};
use ply_core::{Effect, Mode, Resource};
use std::cell::Cell;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);

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

fn performed(ctx: &mut Ctx, op: &PlyOp, mode: Word, args: &[Word]) -> Word {
    let entry = NO_HANDLER_OBJECT.slot.entry;
    let result = unsafe { entry(ctx, 0, op, I64(mode), I64(-1), args.as_ptr().cast()) };
    unsafe { std::mem::transmute::<Handed, Word>(result) }
}

#[test]
fn an_effect_with_no_handler_fails_the_run_with_its_value_in_the_error_and_frees_that_value() {
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

type TakeRaisedPayload = unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word;

#[test]
fn a_raised_effect_is_caught_only_by_a_handler_for_the_same_operation() {
    let mut ctx = naming(&["m", "boom", "other"]);
    ctx.begin();
    let payload = boxed(&mut ctx, 42);
    performed(&mut ctx, &op(&BOXES), 2, &[payload]);
    assert_ne!(ctx.failed, 0, "the raise left the entry failed");
    let at = helper::<TakeRaisedPayload>("rt_pending_raised_at");
    assert_eq!(unsafe { at(&mut ctx, 0, 2) }, 0);
    assert_ne!(ctx.failed, 0);
    let words = unsafe { at(&mut ctx, 0, 1) } as *const Word;
    assert!(!words.is_null());
    assert_eq!(ctx.failed, 0);
    assert_eq!(unsafe { *words }, payload);
    unsafe { box_dec(&mut ctx, payload) };
    assert_eq!(ctx.heap.live(), 0);
    ctx.end();
}

#[test]
fn an_unhandled_panic_fails_the_entry_with_its_message_text() {
    let mut ctx = naming(&["abort", "raise"]);
    ctx.begin();
    let message: &str = "it broke";
    performed(&mut ctx, &op(&TEXTS), 0, &[&raw const message as Word]);
    let d = ctx.take_failure().expect("the panic failed the entry");
    assert_eq!(d.message, "panic: it broke");
    ctx.end();
}

#[test]
fn performing_an_operation_with_no_handler_fails_naming_it_and_frees_its_arguments() {
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

const CLOSURE: Word = HEADER as Word + 8;

unsafe extern "C" fn closure_release(ctx: *mut Ctx, block: Handed) -> Word {
    let w = unsafe { std::mem::transmute::<Handed, Word>(block) };
    unsafe { helper::<Free>("rt_free")(ctx, w, CLOSURE) };
    0
}

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

unsafe extern "C" fn body(ctx: *mut Ctx, _f: Word, args: *const Word) -> Word {
    let held = unsafe { *args };
    let n = unsafe { *(held as *const Word).byte_add(HEADER) };
    unsafe { box_dec(ctx, held) };
    if n == 6 {
        unsafe { (*ctx).failed = FAILED_UNWOUND_TO_HANDLER };
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
fn bracket_runs_its_cleanup_after_acquiring_and_frees_every_closure_the_compiler_generated() {
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

#[test]
fn bracket_cleans_up_what_it_acquired_after_a_body_that_unwinds() {
    let mut ctx = context();
    ctx.begin();
    CLEANED.with(|c| c.set(0));
    let result = bracket(&mut ctx, &ACQUIRE_SIX);
    assert_eq!((result, ctx.failed), (0, FAILED_UNWOUND_TO_HANDLER));
    assert_eq!(CLEANED.with(Cell::get), 6);
    assert_eq!(ctx.heap.live(), 0);
    ctx.failed = 0;
    ctx.end();
}

thread_local! {
    static RECEIVED_STRING: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

unsafe extern "C" fn string_lift(ctx: *mut Ctx) -> Word {
    let text: unsafe extern "C" fn(*mut Ctx) -> Word = helper(&builtin_helper_name("?in_str"));
    let taken: unsafe extern "C" fn(*mut Ctx, *mut u8, Word) =
        helper(&builtin_helper_name("?taken"));
    let n = unsafe { text(ctx) };
    let mut b = vec![0u8; n as usize];
    unsafe { taken(ctx, b.as_mut_ptr(), n) };
    RECEIVED_STRING.with(|l| *l.borrow_mut() = String::from_utf8(b).expect("UTF-8"));
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

#[test]
fn the_operation_label_matched_by_a_handler_is_converted_to_a_string_and_then_freed() {
    let mut ctx = naming(&["m", "ask", "*", "files"]);
    ctx.begin();
    RECEIVED_STRING.with(|l| l.borrow_mut().clear());
    let label = unsafe { helper::<LabelString>("rt_label_string")(&mut ctx, 3, &STRING_T) };
    super::marshalling::unfailed(&mut ctx);
    assert_eq!(RECEIVED_STRING.with(|l| l.borrow().clone()), "files");
    unsafe { box_dec(&mut ctx, label) };
    assert_eq!(ctx.heap.live(), 0, "the label was not let go");
    ctx.end();
}

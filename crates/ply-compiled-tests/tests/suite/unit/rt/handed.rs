use crate::bare::{context_of, helper, new_context};

fn naming(fields: &[&str]) -> Context {
    context_of(fields, Vec::new())
}
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::{Handed, PlyFunctionDescriptor, PlyOp, PlyType};
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::native::I64;
use ply_compiled::rt::{Context, FAILED_UNWOUND_TO_HANDLER, NO_HANDLER_OBJECT};
use ply_core::{Effect, Mode, Resource};
use std::cell::Cell;

type Alloc = unsafe extern "C" fn(*mut Context, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Context, Word, Word);

const BOX: Word = HEADER as Word + 8;

fn boxed(context: &mut Context, n: Word) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(context, BOX) };
    unsafe { *(w as *mut Word).byte_add(HEADER) = n };
    w
}

unsafe extern "C" fn box_increment(_context: *mut Context, w: Word) -> Word {
    unsafe { *(w as *mut u32) += 1 };
    0
}

unsafe extern "C" fn box_decrement(context: *mut Context, w: Word) -> Word {
    let rc = w as *mut u32;
    unsafe {
        if *rc > 1 {
            *rc -= 1;
        } else {
            helper::<Free>("rt_free")(context, w, BOX);
        }
    }
    0
}

unsafe extern "C" fn box_lower(context: *mut Context, w: Word) -> Word {
    let out: unsafe extern "C" fn(*mut Context, Word) = helper(&builtin_helper_name("?out_int"));
    unsafe { out(context, *(w as *const Word).byte_add(HEADER)) };
    0
}

unsafe extern "C" fn unread(_context: *mut Context, _w: Word) -> Word {
    0
}

unsafe extern "C" fn nothing(_context: *mut Context) -> Word {
    0
}

static BOX_T: PlyType = PlyType {
    identity: 1,
    spelled: b"m.Box".as_ptr(),
    spelled_len: 5,
    inc: box_increment,
    dec: box_decrement,
    lower: box_lower,
    lift: nothing,
};

unsafe extern "C" fn text_lower(context: *mut Context, w: Word) -> Word {
    let out: unsafe extern "C" fn(*mut Context, Word, Word) =
        helper(&builtin_helper_name("?out_str"));
    let s: &&str = unsafe { &*(w as *const &str) };
    unsafe { out(context, s.as_ptr() as Word, s.len() as Word) };
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
        variables_length: 0,
        variables: std::ptr::null(),
    }
}

fn performed(context: &mut Context, op: &PlyOp, mode: Word, args: &[Word]) -> Word {
    let entry = NO_HANDLER_OBJECT.slot.entry;
    let result = unsafe { entry(context, 0, op, I64(mode), I64(-1), args.as_ptr().cast()) };
    unsafe { std::mem::transmute::<Handed, Word>(result) }
}

#[test]
fn an_effect_with_no_handler_fails_the_run_with_its_value_in_the_error_and_frees_that_value() {
    let mut context = naming(&["m", "boom"]);
    context.begin();
    let payload = boxed(&mut context, 42);
    performed(&mut context, &op(&BOXES), 2, &[payload]);
    let d = context.take_failure().expect("the raise failed the entry");
    assert!(
        d.message.contains("`m.boom` was raised with"),
        "{}",
        d.message
    );
    let mut shown = ply_core::codec::Writer::new();
    shown.int(42);
    assert_eq!(d.values.iter().collect::<Vec<_>>(), [&shown.finish()]);
    assert_eq!(context.heap.live(), 0, "the payload was not let go");
    context.end();
}

type TakeRaisedPayload = unsafe extern "C" fn(*mut Context, Word, Word) -> Word;

#[test]
fn a_raised_effect_is_caught_only_by_a_handler_for_the_same_operation() {
    let mut context = naming(&["m", "boom", "other"]);
    context.begin();
    let payload = boxed(&mut context, 42);
    performed(&mut context, &op(&BOXES), 2, &[payload]);
    assert_ne!(context.failed, 0, "the raise left the entry failed");
    let at = helper::<TakeRaisedPayload>("rt_pending_raised_at");
    assert_eq!(unsafe { at(&mut context, 0, 2) }, 0);
    assert_ne!(context.failed, 0);
    let words = unsafe { at(&mut context, 0, 1) } as *const Word;
    assert!(!words.is_null());
    assert_eq!(context.failed, 0);
    assert_eq!(unsafe { *words }, payload);
    unsafe { box_decrement(&mut context, payload) };
    assert_eq!(context.heap.live(), 0);
    context.end();
}

#[test]
fn an_unhandled_panic_fails_the_entry_with_its_message_text() {
    let mut context = naming(&["abort", "raise"]);
    context.begin();
    let message: &str = "it broke";
    performed(&mut context, &op(&TEXTS), 0, &[&raw const message as Word]);
    let d = context.take_failure().expect("the panic failed the entry");
    assert_eq!(d.message, "panic: it broke");
    context.end();
}

#[test]
fn performing_an_operation_with_no_handler_fails_naming_it_and_frees_its_arguments() {
    let mut context = naming(&["m", "ask"]);
    context.begin();
    let arg = boxed(&mut context, 7);
    performed(&mut context, &op(&BOXES), 0, &[arg]);
    let d = context.take_failure().expect("nothing handles `m.ask`");
    assert!(d.message.contains("m.ask"), "{}", d.message);
    assert_eq!(context.heap.live(), 0, "the argument was not let go");
    assert_eq!(
        context.performed,
        [Effect::operation(
            "m",
            Resource::Singleton,
            Mode::Read,
            "ask"
        )]
    );
    context.end();
}

thread_local! {
    static CLEANED: Cell<Word> = const { Cell::new(0) };
}

const CLOSURE: Word = HEADER as Word + 8;

unsafe extern "C" fn closure_release(context: *mut Context, block: Handed) -> Word {
    let w = unsafe { std::mem::transmute::<Handed, Word>(block) };
    unsafe { helper::<Free>("rt_free")(context, w, CLOSURE) };
    0
}

unsafe extern "C" fn stays_in_its_entry(_context: *mut Context, _block: Word) -> Word {
    0
}

const fn descriptor(
    uniform: unsafe extern "C" fn(*mut Context, Word, *const Word) -> Word,
) -> PlyFunctionDescriptor {
    PlyFunctionDescriptor {
        uniform,
        typed: None,
        convention: 0,
        release: closure_release,
        share: stays_in_its_entry,
        keep: stays_in_its_entry,
    }
}

static ACQUIRE: PlyFunctionDescriptor = descriptor(acquire);
static ACQUIRE_SIX: PlyFunctionDescriptor = descriptor(acquire_six);
static BODY: PlyFunctionDescriptor = descriptor(body);
static CLEANUP: PlyFunctionDescriptor = descriptor(cleanup);

unsafe extern "C" fn acquire(context: *mut Context, _f: Word, _args: *const Word) -> Word {
    boxed(unsafe { &mut *context }, 5)
}

unsafe extern "C" fn body(context: *mut Context, _f: Word, args: *const Word) -> Word {
    let held = unsafe { *args };
    let n = unsafe { *(held as *const Word).byte_add(HEADER) };
    unsafe { box_decrement(context, held) };
    if n == 6 {
        unsafe { (*context).failed = FAILED_UNWOUND_TO_HANDLER };
        return 0;
    }
    7
}

unsafe extern "C" fn cleanup(context: *mut Context, _f: Word, args: *const Word) -> Word {
    let held = unsafe { *args };
    CLEANED.with(|c| c.set(unsafe { *(held as *const Word).byte_add(HEADER) }));
    unsafe { box_decrement(context, held) };
    0
}

fn closure(context: &mut Context, d: &'static PlyFunctionDescriptor) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(context, CLOSURE) };
    unsafe { *(w as *mut *const PlyFunctionDescriptor).byte_add(HEADER) = d };
    w
}

type WithCleanup =
    unsafe extern "C" fn(*mut Context, Word, Word, Word, *const PlyType, *const PlyType) -> Word;

fn bracket(context: &mut Context, acquire_descriptor: &'static PlyFunctionDescriptor) -> Word {
    let with: WithCleanup = helper("rt_with_cleanup");
    let (a, b, c) = (
        closure(context, acquire_descriptor),
        closure(context, &BODY),
        closure(context, &CLEANUP),
    );
    unsafe { with(context, a, b, c, &BOX_T, &TEXT_T) }
}

#[test]
fn bracket_runs_its_cleanup_after_acquiring_and_frees_every_closure_the_compiler_generated() {
    let mut context = new_context();
    context.begin();
    CLEANED.with(|c| c.set(0));
    let result = bracket(&mut context, &ACQUIRE);
    assert_eq!((result, context.failed), (7, 0));
    assert_eq!(CLEANED.with(Cell::get), 5);
    assert_eq!(context.heap.live(), 0);
    context.end();
}

unsafe extern "C" fn acquire_six(context: *mut Context, _f: Word, _args: *const Word) -> Word {
    boxed(unsafe { &mut *context }, 6)
}

#[test]
fn bracket_cleans_up_what_it_acquired_after_a_body_that_unwinds() {
    let mut context = new_context();
    context.begin();
    CLEANED.with(|c| c.set(0));
    let result = bracket(&mut context, &ACQUIRE_SIX);
    assert_eq!((result, context.failed), (0, FAILED_UNWOUND_TO_HANDLER));
    assert_eq!(CLEANED.with(Cell::get), 6);
    assert_eq!(context.heap.live(), 0);
    context.failed = 0;
    context.end();
}

thread_local! {
    static RECEIVED_STRING: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

unsafe extern "C" fn string_lift(context: *mut Context) -> Word {
    let text: unsafe extern "C" fn(*mut Context) -> Word = helper(&builtin_helper_name("?in_str"));
    let taken: unsafe extern "C" fn(*mut Context, *mut u8, Word) =
        helper(&builtin_helper_name("?taken"));
    let n = unsafe { text(context) };
    let mut b = vec![0u8; n as usize];
    unsafe { taken(context, b.as_mut_ptr(), n) };
    RECEIVED_STRING.with(|l| *l.borrow_mut() = String::from_utf8(b).expect("UTF-8"));
    boxed(unsafe { &mut *context }, n)
}

static STRING_T: PlyType = PlyType {
    identity: 3,
    spelled: b"String".as_ptr(),
    spelled_len: 6,
    inc: box_increment,
    dec: box_decrement,
    lower: unread,
    lift: string_lift,
};

type LabelString = unsafe extern "C" fn(*mut Context, Word, *const PlyType) -> Word;

#[test]
fn the_operation_label_matched_by_a_handler_is_converted_to_a_string_and_then_freed() {
    let mut context = naming(&["m", "ask", "*", "files"]);
    context.begin();
    RECEIVED_STRING.with(|l| l.borrow_mut().clear());
    let label = unsafe { helper::<LabelString>("rt_label_string")(&mut context, 3, &STRING_T) };
    super::marshalling::unfailed(&mut context);
    assert_eq!(RECEIVED_STRING.with(|l| l.borrow().clone()), "files");
    unsafe { box_decrement(&mut context, label) };
    assert_eq!(context.heap.live(), 0, "the label was not let go");
    context.end();
}

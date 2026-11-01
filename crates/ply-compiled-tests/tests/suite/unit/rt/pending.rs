use crate::bare::{context_of, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::PlyType;
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::rt::{Context, FAILED_PENDING_RAISE};
use std::cell::RefCell;

type Alloc = unsafe extern "C" fn(*mut Context, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Context, Word, Word);
type Pending = unsafe extern "C" fn(*mut Context, *const PlyType, Word) -> Word;

const BOX: Word = HEADER as Word + 8;

thread_local! {
    static TEXTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn text_increment(_context: *mut Context, w: Word) -> Word {
    unsafe { *(w as *mut u32) += 1 };
    0
}

unsafe extern "C" fn text_decrement(context: *mut Context, w: Word) -> Word {
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

unsafe extern "C" fn text_lower(context: *mut Context, w: Word) -> Word {
    let out: unsafe extern "C" fn(*mut Context, Word, Word) =
        helper(&builtin_helper_name("?out_str"));
    let at = unsafe { *(w as *const Word).byte_add(HEADER) } as usize;
    let text = TEXTS.with(|t| t.borrow()[at].clone());
    unsafe { out(context, text.as_ptr() as Word, text.len() as Word) };
    0
}

unsafe extern "C" fn text_lift(context: *mut Context) -> Word {
    let text: unsafe extern "C" fn(*mut Context) -> Word = helper(&builtin_helper_name("?in_str"));
    let taken: unsafe extern "C" fn(*mut Context, *mut u8, Word) =
        helper(&builtin_helper_name("?taken"));
    let n = unsafe { text(context) };
    let mut b = vec![0u8; n as usize];
    unsafe { taken(context, b.as_mut_ptr(), n) };
    let at = TEXTS.with(|t| {
        let mut t = t.borrow_mut();
        t.push(String::from_utf8(b).expect("UTF-8"));
        t.len() - 1
    });
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(context, BOX) };
    unsafe { *(w as *mut Word).byte_add(HEADER) = at as Word };
    w
}

static STRING_T: PlyType = PlyType {
    identity: 3,
    spelled: b"String".as_ptr(),
    spelled_len: 6,
    inc: text_increment,
    dec: text_decrement,
    lower: text_lower,
    lift: text_lift,
};

fn failed(context: &mut Context, code: &[u8], message: &[u8], note: &[u8]) {
    let raised: unsafe extern "C" fn(*mut Context, Word, *const Word, Word, Word) =
        helper("rt_failed");
    let label = b"here";
    let words = [
        code.as_ptr() as Word,
        code.len() as Word,
        message.as_ptr() as Word,
        message.len() as Word,
        label.as_ptr() as Word,
        label.len() as Word,
        note.as_ptr() as Word,
        note.len() as Word,
    ];
    unsafe { raised(context, 1, words.as_ptr(), 1, 0) };
}

fn raised_on_keeps(context: &mut Context) {
    assert_eq!(
        context.failed, FAILED_PENDING_RAISE,
        "nothing handles the rejection"
    );
    let left = context
        .diagnostic
        .clone()
        .expect("a pending raise keeps its diagnostic");
    let message = unsafe { helper::<Pending>("rt_pending_raised")(context, &STRING_T, 1) };
    assert_eq!(message, 0, "nothing is handed over to the bridge");
    assert_eq!(
        context.failed, FAILED_PENDING_RAISE,
        "nothing handles it the second time"
    );
    let now = context.diagnostic.clone().expect("the entry fails");
    assert_eq!(
        (now.code, now.message.as_str(), &now.notes),
        (left.code, left.message.as_str(), &left.notes),
        "the raise came back as another diagnostic"
    );
    context.failed = 0;
    context.end();
}

#[test]
fn a_rejection_raised_again_from_a_stored_failure_keeps_its_explanatory_text() {
    let mut context = context_of(&["abort", "raise"], Vec::new());
    context.begin();
    failed(&mut context, b"E0502", b"it failed", b"why it failed");
    raised_on_keeps(&mut context);
}

#[test]
fn an_assertion_failure_raised_again_from_a_stored_failure_is_still_an_assertion_failure() {
    let mut context = context_of(&["abort", "raise"], Vec::new());
    context.begin();
    failed(
        &mut context,
        b"E0501",
        b"assertion failed",
        b"the sides differ",
    );
    assert_eq!(context.diagnostic.as_ref().map(|d| d.code), Some("E0501"));
    raised_on_keeps(&mut context);
}

#[test]
fn a_handler_receives_the_message_of_a_stored_raise_as_a_string() {
    let mut context = context_of(&["abort", "raise"], Vec::new());
    context.begin();
    failed(&mut context, b"E0502", b"it failed", b"why it failed");
    let message = unsafe { helper::<Pending>("rt_pending_raised")(&mut context, &STRING_T, 0) };
    assert_eq!(context.failed, 0, "the message was handed over");
    assert_ne!(message, 0, "the message is a `String`");
    unsafe { text_decrement(&mut context, message) };
    context.end();
}

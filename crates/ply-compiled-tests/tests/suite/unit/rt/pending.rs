use crate::bare::{context_of, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::PlyType;
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::rt::{Ctx, FAILED_PENDING};
use std::cell::RefCell;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Pending = unsafe extern "C" fn(*mut Ctx, *const PlyType, Word) -> Word;

const BOX: Word = HEADER as Word + 8;

thread_local! {
    static TEXTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn text_inc(_ctx: *mut Ctx, w: Word) -> Word {
    unsafe { *(w as *mut u32) += 1 };
    0
}

unsafe extern "C" fn text_dec(ctx: *mut Ctx, w: Word) -> Word {
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

unsafe extern "C" fn text_lower(ctx: *mut Ctx, w: Word) -> Word {
    let out: unsafe extern "C" fn(*mut Ctx, Word, Word) = helper(&builtin_helper_name("?out_str"));
    let at = unsafe { *(w as *const Word).byte_add(HEADER) } as usize;
    let text = TEXTS.with(|t| t.borrow()[at].clone());
    unsafe { out(ctx, text.as_ptr() as Word, text.len() as Word) };
    0
}

unsafe extern "C" fn text_lift(ctx: *mut Ctx) -> Word {
    let text: unsafe extern "C" fn(*mut Ctx) -> Word = helper(&builtin_helper_name("?in_str"));
    let taken: unsafe extern "C" fn(*mut Ctx, *mut u8, Word) =
        helper(&builtin_helper_name("?taken"));
    let n = unsafe { text(ctx) };
    let mut b = vec![0u8; n as usize];
    unsafe { taken(ctx, b.as_mut_ptr(), n) };
    let at = TEXTS.with(|t| {
        let mut t = t.borrow_mut();
        t.push(String::from_utf8(b).expect("UTF-8"));
        t.len() - 1
    });
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, BOX) };
    unsafe { *(w as *mut Word).byte_add(HEADER) = at as Word };
    w
}

static STRING_T: PlyType = PlyType {
    identity: 3,
    spelled: b"String".as_ptr(),
    spelled_len: 6,
    inc: text_inc,
    dec: text_dec,
    lower: text_lower,
    lift: text_lift,
};

fn rejected(ctx: &mut Ctx) {
    let rejected: unsafe extern "C" fn(*mut Ctx, Word, Word, Word, Word) =
        helper(&builtin_helper_name("?refused"));
    unsafe { rejected(ctx, 8, 5, 2, 0) };
}

fn raised_on_keeps(ctx: &mut Ctx) {
    assert_eq!(ctx.failed, FAILED_PENDING, "nothing handles the rejection");
    let left = ctx
        .diagnostic
        .clone()
        .expect("a pending raise keeps its diagnostic");
    let message = unsafe { helper::<Pending>("rt_pending_raised")(ctx, &STRING_T, 1) };
    assert_eq!(message, 0, "nothing is handed over to the bridge");
    assert_eq!(
        ctx.failed, FAILED_PENDING,
        "nothing handles it the second time"
    );
    let now = ctx.diagnostic.clone().expect("the entry fails");
    assert_eq!(
        (now.code, now.message.as_str(), &now.notes),
        (left.code, left.message.as_str(), &left.notes),
        "the raise came back as another diagnostic"
    );
    ctx.failed = 0;
    ctx.end();
}

#[test]
fn a_rejection_raised_again_from_a_stored_failure_keeps_its_explanatory_text() {
    let mut ctx = context_of(&["abort", "raise"], Vec::new());
    ctx.begin();
    rejected(&mut ctx);
    raised_on_keeps(&mut ctx);
}

#[test]
fn an_assertion_failure_raised_again_from_a_stored_failure_is_still_an_assertion_failure() {
    let mut ctx = context_of(&["abort", "raise"], Vec::new());
    ctx.begin();
    let said = "the sides differ";
    let rejected: unsafe extern "C" fn(*mut Ctx, Word, Word, Word, Word, Word) =
        helper(&builtin_helper_name("?refused_text"));
    unsafe {
        rejected(
            &mut ctx,
            21,
            said.as_ptr() as Word,
            said.len() as Word,
            0,
            0,
        )
    };
    assert_eq!(ctx.diagnostic.as_ref().map(|d| d.code), Some("E0501"));
    raised_on_keeps(&mut ctx);
}

#[test]
fn a_handler_receives_the_message_of_a_stored_raise_as_a_string() {
    let mut ctx = context_of(&["abort", "raise"], Vec::new());
    ctx.begin();
    rejected(&mut ctx);
    let message = unsafe { helper::<Pending>("rt_pending_raised")(&mut ctx, &STRING_T, 0) };
    assert_eq!(ctx.failed, 0, "the message was handed over");
    assert_ne!(message, 0, "the message is a `String`");
    unsafe { text_dec(&mut ctx, message) };
    ctx.end();
}

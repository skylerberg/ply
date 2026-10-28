//! A raise the runtime left pending, which a closure holding a word for `abort` raises through it as
//! it leaves: handed over as its message to a clause's slot, and through the bridge's raised on as
//! the raise it is, its diagnostic kept where nothing answers it the second time either.

use crate::bare::{context_of, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::PlyType;
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::rt::{Ctx, FAILED_PENDING};
use std::cell::RefCell;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Pending = unsafe extern "C" fn(*mut Ctx, *const PlyType, Word) -> Word;

/// A `String` as emitted code holds one here: a block of one word, the place of its text in `TEXTS`.
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

/// `Bytes::at` given index 5 of 2 bytes, left pending, numbered as `REFUSED` numbers it.
fn refused(ctx: &mut Ctx) {
    let refused: unsafe extern "C" fn(*mut Ctx, Word, Word, Word, Word) =
        helper(&builtin_helper_name("?refused"));
    unsafe { refused(ctx, 8, 5, 2, 0) };
}

/// Through the bridge, which answers no raise, the entry fails as the runtime left it failing:
/// `left`'s code, message and notes, not a `panic:` made of its text.
fn raised_on_keeps(ctx: &mut Ctx) {
    assert_eq!(ctx.failed, FAILED_PENDING, "nothing answers the refusal");
    let left = ctx
        .diagnostic
        .clone()
        .expect("a pending raise keeps its diagnostic");
    let message = unsafe { helper::<Pending>("rt_pending_raised")(ctx, &STRING_T, 1) };
    assert_eq!(message, 0, "nothing is handed over to the bridge");
    assert_eq!(
        ctx.failed, FAILED_PENDING,
        "nothing answers it the second time"
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
fn a_refusal_raised_on_through_the_bridge_keeps_its_note() {
    let mut ctx = context_of(&["abort", "raise"], Vec::new());
    ctx.begin(1_000);
    refused(&mut ctx);
    raised_on_keeps(&mut ctx);
}

/// A failed `assert` inside a lambda of a test is this: unanswered, it is `E0501` with its message.
#[test]
fn an_assertion_raised_on_through_the_bridge_stays_an_assertion() {
    let mut ctx = context_of(&["abort", "raise"], Vec::new());
    ctx.begin(1_000);
    let said = "the sides differ";
    // `assert` with a message, numbered as `REFUSED` numbers it.
    let refused: unsafe extern "C" fn(*mut Ctx, Word, Word, Word, Word, Word) =
        helper(&builtin_helper_name("?refused_text"));
    unsafe {
        refused(
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

/// A clause's slot is handed the message, and the entry fails no longer: the clause answers it.
#[test]
fn a_clause_is_handed_the_message_of_a_raise_left_pending() {
    let mut ctx = context_of(&["abort", "raise"], Vec::new());
    ctx.begin(1_000);
    refused(&mut ctx);
    let message = unsafe { helper::<Pending>("rt_pending_raised")(&mut ctx, &STRING_T, 0) };
    assert_eq!(ctx.failed, 0, "the message was handed over");
    assert_ne!(message, 0, "the message is a `String`");
    unsafe { text_dec(&mut ctx, message) };
    ctx.end();
}

//! Values emitted code hands the runtime with their type: counted, let go and handed back through
//! the type's descriptor alone, which here counts a block at its offset 0 as emitted code does.

use crate::bare::{context, helper};
use ply_compiled::emitted::{Carried, Handed, PlyType};
use ply_compiled::heap::{Word, kind};
use ply_compiled::native::Answered;
use ply_compiled::rt::Ctx;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);

/// A value of `m.Box`: a block of one word.
const BYTES: Word = 16 + 8;

unsafe extern "C" fn dup(_ctx: *mut Ctx, w: Word) -> Word {
    unsafe { *(w as *mut u32) += 1 };
    0
}

unsafe extern "C" fn release(ctx: *mut Ctx, w: Word) -> Word {
    let rc = w as *mut u32;
    unsafe {
        if *rc > 1 {
            *rc -= 1;
        } else {
            helper::<Free>("rt_free")(ctx, w, BYTES);
        }
    }
    0
}

unsafe extern "C" fn unread(_ctx: *mut Ctx, _w: Word) -> Word {
    0
}

unsafe extern "C" fn nothing(_ctx: *mut Ctx) -> Word {
    0
}

static BOX: PlyType = PlyType {
    identity: 1,
    spelled: b"m.Box".as_ptr(),
    spelled_len: 5,
    inc: dup,
    dec: release,
    lower: unread,
    lift: nothing,
};

fn count(w: Word) -> u32 {
    unsafe { *(w as *const u32) }
}

fn boxed(ctx: &mut Ctx) -> Word {
    unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, BYTES) }
}

#[test]
fn a_carried_value_is_counted_and_let_go_through_its_type() {
    let mut ctx = context();
    let w = boxed(&mut ctx);
    let held = unsafe { Carried::new(Handed::of(w), &BOX) };
    let again = held.dup(&mut ctx);
    assert_eq!(count(w), 2);
    held.release(&mut ctx);
    assert_eq!(count(w), 1);
    assert_eq!(ctx.heap.live(), 1);
    again.release(&mut ctx);
    assert_eq!(kind(w), ply_compiled::heap::KIND_DEAD);
    assert_eq!(ctx.heap.live(), 0);
}

/// Compiled code reads a handed word as the word it is.
#[test]
fn a_carried_value_handed_back_is_the_word_and_the_count_it_came_with() {
    let mut ctx = context();
    let w = boxed(&mut ctx);
    let held = unsafe { Carried::new(Handed::of(w), &BOX) };
    assert!(std::ptr::eq(held.ty(), &BOX));
    let back = unsafe { std::mem::transmute::<Handed, Word>(held.handed()) };
    assert_eq!((back, count(w)), (w, 1));
}

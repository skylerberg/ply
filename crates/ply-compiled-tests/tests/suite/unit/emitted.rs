use crate::bare::{context, helper};
use ply_compiled::emitted::{Carried, Handed, PlyType};
use ply_compiled::heap::{HEADER, Obj, Word};
use ply_compiled::native::Returned;
use ply_compiled::rt::Ctx;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);

const BYTES: Word = (HEADER + 8) as Word;

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
fn a_carried_value_adds_a_reference_when_duplicated_and_frees_its_block_when_the_last_reference_is_released()
 {
    let mut ctx = context();
    let w = boxed(&mut ctx);
    let held = unsafe { Carried::new(Handed::of(w), &BOX) };
    let again = held.dup(&mut ctx);
    assert_eq!(count(w), 2);
    held.release(&mut ctx);
    assert_eq!(count(w), 1);
    assert_eq!(ctx.heap.live(), 1);
    again.release(&mut ctx);
    assert_eq!(
        unsafe { (*(w as *mut Obj)).rc },
        0,
        "the last release gave the block back"
    );
    assert_eq!(ctx.heap.live(), 0);
}

#[test]
fn a_carried_value_converted_back_into_a_raw_word_gives_the_same_word_and_the_same_reference_count_it_was_created_with()
 {
    let mut ctx = context();
    let w = boxed(&mut ctx);
    let held = unsafe { Carried::new(Handed::of(w), &BOX) };
    assert!(std::ptr::eq(held.ty(), &BOX));
    let back = unsafe { std::mem::transmute::<Handed, Word>(held.handed()) };
    assert_eq!((back, count(w)), (w, 1));
}

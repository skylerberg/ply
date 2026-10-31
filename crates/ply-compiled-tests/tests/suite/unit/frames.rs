use super::heap::Holding;
use crate::bare::{context, helper};
use ply_compiled::emitted::{Block, Carried, Handed, PlyFnDesc, PlyType};
use ply_compiled::frames::{Held, PlyFrame, base, held_across, release_live};
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::native::Returned;
use ply_compiled::rt::Ctx;
use ply_compiled::stack::Stack;
use std::cell::RefCell;
use std::sync::Arc;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);

const BOX: Word = (HEADER + 8) as Word;

const CLOSURE: Word = (HEADER + 8 * 3) as Word;

thread_local! {
    static FREED: RefCell<Vec<Word>> = const { RefCell::new(Vec::new()) };
    static WALKED: RefCell<Vec<(Word, i64)>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn freed() -> Vec<Word> {
    let mut freed = FREED.with(|f| std::mem::take(&mut *f.borrow_mut()));
    freed.sort_unstable();
    freed
}

fn walked() -> Vec<(Word, i64)> {
    WALKED.with(|w| std::mem::take(&mut *w.borrow_mut()))
}

pub(super) fn count(w: Word) -> u32 {
    unsafe { *(w as *const u32) }
}

fn word(w: Word, at: usize) -> *mut Word {
    unsafe { (w as *mut Word).byte_add(HEADER).add(at) }
}

pub(super) fn boxed(ctx: *mut Ctx, marker: Word) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, BOX) };
    unsafe { *word(w, 0) = marker };
    w
}

pub(super) fn dup(w: Word) {
    unsafe { *(w as *mut u32) += 1 };
}

pub(super) fn release_box(ctx: *mut Ctx, w: Word) {
    unsafe {
        if count(w) > 1 {
            *(w as *mut u32) -= 1;
            return;
        }
        FREED.with(|f| f.borrow_mut().push(*word(w, 0)));
        helper::<Free>("rt_free")(ctx, w, BOX);
    }
}

unsafe extern "C" fn box_dup(_ctx: *mut Ctx, w: Word) -> Word {
    dup(w);
    0
}

unsafe extern "C" fn box_release(ctx: *mut Ctx, w: Word) -> Word {
    release_box(ctx, w);
    0
}

unsafe extern "C" fn unread(_ctx: *mut Ctx, _w: Word) -> Word {
    0
}

unsafe extern "C" fn nothing(_ctx: *mut Ctx) -> Word {
    0
}

static COUNTED: PlyType = PlyType {
    identity: 1,
    spelled: b"m.Box".as_ptr(),
    spelled_len: 5,
    inc: box_dup,
    dec: box_release,
    lower: unread,
    lift: nothing,
};

unsafe extern "C" fn uncalled(_ctx: *mut Ctx, _block: Word, _args: *const Word) -> Word {
    unreachable!("nothing here calls a closure")
}

unsafe extern "C" fn closure_released(ctx: *mut Ctx, h: Handed) -> Word {
    let w = unsafe { std::mem::transmute::<Handed, Word>(h) };
    unsafe {
        release_box(ctx, *word(w, 2));
        FREED.with(|f| f.borrow_mut().push(*word(w, 1)));
        helper::<Free>("rt_free")(ctx, w, CLOSURE);
    }
    0
}

static CLOSES_OVER_A_BOX: PlyFnDesc = PlyFnDesc {
    uniform: uncalled,
    typed: None,
    convention: 0,
    release: closure_released,
    share: unread,
    keep: unread,
};

fn closure(ctx: *mut Ctx, marker: Word, capture: Word) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, CLOSURE) };
    unsafe {
        *word(w, 0) = (&raw const CLOSES_OVER_A_BOX) as Word;
        *word(w, 1) = marker;
        *word(w, 2) = capture;
    }
    w
}

pub(super) fn every_kind(
    ctx: *mut Ctx,
    markers: [Word; 3],
    text: &Arc<str>,
) -> ([Held; 3], [Word; 3]) {
    let [f, capture, x] = markers;
    let f = closure(ctx, f, boxed(ctx, capture));
    let x = boxed(ctx, x);
    let h = unsafe { (*ctx).heap.native(Box::new(Holding(Arc::clone(text)))) };
    let held = unsafe {
        [
            Held::Closure(Block::of(f)),
            Held::Carried(Carried::new(Handed::of(x), &COUNTED)),
            Held::Handle(Block::of(h)),
        ]
    };
    (held, [f, x, h])
}

unsafe extern "C" fn logged(_ctx: *mut Ctx, slots: *mut Word, d: i64) {
    let slot = unsafe { *slots };
    WALKED.with(|w| w.borrow_mut().push((slot, d)));
}

#[repr(C)]
struct Record {
    h: PlyFrame,
    slot: Word,
}

fn laid(stack: &Stack, sp: usize) -> *mut PlyFrame {
    let top = stack.top();
    let mut below = base();
    for (slot, at) in [(1, top - 256), (2, top - 1024), (3, sp + 64)] {
        let record = at as *mut Record;
        unsafe {
            record.write(Record {
                h: PlyFrame {
                    prev: below,
                    each: logged,
                },
                slot,
            })
        };
        below = record.cast();
    }
    below
}

#[test]
fn the_chain_head_follows_the_eight_fields_every_unit_reads_and_a_record_is_two_words() {
    use std::mem::offset_of;
    assert_eq!(offset_of!(Ctx, frames), 64);
    assert_eq!(size_of::<PlyFrame>(), 16);
    assert_eq!(offset_of!(PlyFrame, prev), 0);
    assert_eq!(offset_of!(PlyFrame, each), 8);
}

#[test]
fn a_live_chain_released_lets_go_of_every_record_once() {
    let mut ctx = context();
    let stack = Stack::take().expect("the system maps a stack");
    let head = laid(&stack, stack.top() - 4096);
    unsafe { release_live(&mut ctx, head) };
    assert_eq!(walked(), [(3, -1), (2, -1), (1, -1)]);
}

#[test]
fn a_chain_with_no_record_walks_nothing() {
    let mut ctx = context();
    for head in [std::ptr::null_mut(), base()] {
        unsafe { release_live(&mut ctx, head) };
    }
    assert!(walked().is_empty());
}

#[test]
fn a_runtime_frames_record_lets_go_of_each_slot_as_its_kind_is_counted() {
    let mut owned = context();
    owned.frames = base();
    let ctx = &raw mut owned;
    let text: Arc<str> = Arc::from("held");
    let (held, words) = every_kind(ctx, [5, 2, 3], &text);
    words.iter().copied().for_each(dup);
    let (seen, back) = unsafe {
        held_across(ctx, held, || {
            let record = (*ctx).frames;
            let linked = record != base() && (*record).prev == base();
            ((*record).each)(ctx, record.add(1).cast::<Word>(), -1);
            (linked, words.map(count))
        })
    };
    assert_eq!(seen, (true, [1, 1, 1]));
    assert_eq!(
        unsafe { (*ctx).frames },
        base(),
        "the record outlived its call"
    );
    for one in back {
        unsafe { one.release(ctx) };
    }
    assert_eq!(freed(), [2, 3, 5]);
    assert_eq!(Arc::strong_count(&text), 1);
    assert_eq!(unsafe { (*ctx).heap.live() }, 0);
}

#[test]
fn a_runtime_frame_abandoned_under_its_call_lets_go_of_each_slot_once() {
    let mut owned = context();
    owned.frames = base();
    let ctx = &raw mut owned;
    let text: Arc<str> = Arc::from("held");
    let (held, _) = every_kind(ctx, [5, 2, 3], &text);
    let ((), _never_given_back) =
        unsafe { held_across(ctx, held, || release_live(ctx, (*ctx).frames)) };
    assert_eq!(freed(), [2, 3, 5]);
    assert_eq!(Arc::strong_count(&text), 1);
    assert_eq!(unsafe { (*ctx).heap.live() }, 0);
}

#[test]
fn a_runtime_frame_on_a_stack_that_records_nothing_links_nothing() {
    let mut owned = context();
    let ctx = &raw mut owned;
    let (linked, []) = unsafe { held_across(ctx, [], || !(*ctx).frames.is_null()) };
    assert!(!linked);
}

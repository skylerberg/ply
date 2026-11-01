use super::heap::Holding;
use crate::bare::{helper, new_context};
use ply_compiled::emitted::{Block, Carried, Handed, PlyFunctionDescriptor, PlyType};
use ply_compiled::frames::{FrameObject, PlyFrame, base, call_recording_slots, release_frames};
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::native::Returned;
use ply_compiled::rt::Context;
use ply_compiled::stack::Stack;
use std::cell::RefCell;
use std::sync::Arc;

type Alloc = unsafe extern "C" fn(*mut Context, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Context, Word, Word);

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

pub(super) fn boxed(context: *mut Context, marker: Word) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(context, BOX) };
    unsafe { *word(w, 0) = marker };
    w
}

pub(super) fn dup(w: Word) {
    unsafe { *(w as *mut u32) += 1 };
}

pub(super) fn release_box(context: *mut Context, w: Word) {
    unsafe {
        if count(w) > 1 {
            *(w as *mut u32) -= 1;
            return;
        }
        FREED.with(|f| f.borrow_mut().push(*word(w, 0)));
        helper::<Free>("rt_free")(context, w, BOX);
    }
}

unsafe extern "C" fn box_dup(_context: *mut Context, w: Word) -> Word {
    dup(w);
    0
}

unsafe extern "C" fn box_release(context: *mut Context, w: Word) -> Word {
    release_box(context, w);
    0
}

unsafe extern "C" fn unread(_context: *mut Context, _w: Word) -> Word {
    0
}

unsafe extern "C" fn nothing(_context: *mut Context) -> Word {
    0
}

static BOX_TYPE: PlyType = PlyType {
    identity: 1,
    spelled: b"m.Box".as_ptr(),
    spelled_len: 5,
    inc: box_dup,
    dec: box_release,
    lower: unread,
    lift: nothing,
};

unsafe extern "C" fn uncalled(_context: *mut Context, _block: Word, _args: *const Word) -> Word {
    unreachable!("nothing here calls a closure")
}

unsafe extern "C" fn closure_released(context: *mut Context, h: Handed) -> Word {
    let w = unsafe { std::mem::transmute::<Handed, Word>(h) };
    unsafe {
        release_box(context, *word(w, 2));
        FREED.with(|f| f.borrow_mut().push(*word(w, 1)));
        helper::<Free>("rt_free")(context, w, CLOSURE);
    }
    0
}

static CLOSES_OVER_A_BOX: PlyFunctionDescriptor = PlyFunctionDescriptor {
    uniform: uncalled,
    typed: None,
    convention: 0,
    release: closure_released,
    share: unread,
    keep: unread,
};

fn closure(context: *mut Context, marker: Word, capture: Word) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(context, CLOSURE) };
    unsafe {
        *word(w, 0) = (&raw const CLOSES_OVER_A_BOX) as Word;
        *word(w, 1) = marker;
        *word(w, 2) = capture;
    }
    w
}

pub(super) fn every_kind(
    context: *mut Context,
    markers: [Word; 3],
    text: &Arc<str>,
) -> ([FrameObject; 3], [Word; 3]) {
    let [f, capture, x] = markers;
    let f = closure(context, f, boxed(context, capture));
    let x = boxed(context, x);
    let h = unsafe { (*context).heap.native(Box::new(Holding(Arc::clone(text)))) };
    let held = unsafe {
        [
            FrameObject::Closure(Block::of(f)),
            FrameObject::Carried(Carried::new(Handed::of(x), &BOX_TYPE)),
            FrameObject::Handle(Block::of(h)),
        ]
    };
    (held, [f, x, h])
}

unsafe extern "C" fn logged(_context: *mut Context, slots: *mut Word, d: i64) {
    let slot = unsafe { *slots };
    WALKED.with(|w| w.borrow_mut().push((slot, d)));
}

#[repr(C)]
struct Record {
    h: PlyFrame,
    slot: Word,
}

fn laid(stack: &Stack, stack_pointer: usize) -> *mut PlyFrame {
    let top = stack.top();
    let mut below = base();
    for (slot, at) in [(1, top - 256), (2, top - 1024), (3, stack_pointer + 64)] {
        let record = at as *mut Record;
        unsafe {
            record.write(Record {
                h: PlyFrame {
                    previous: below,
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
fn the_frame_list_in_the_context_struct_is_at_byte_64_and_each_frame_struct_is_16_bytes() {
    use std::mem::offset_of;
    assert_eq!(offset_of!(Context, frames), 64);
    assert_eq!(size_of::<PlyFrame>(), 16);
    assert_eq!(offset_of!(PlyFrame, previous), 0);
    assert_eq!(offset_of!(PlyFrame, each), 8);
}

#[test]
fn releasing_a_live_frame_list_releases_every_frame_exactly_once() {
    let mut context = new_context();
    let stack = Stack::take().expect("the system maps a stack");
    let head = laid(&stack, stack.top() - 4096);
    unsafe { release_frames(&mut context, head) };
    assert_eq!(walked(), [(3, -1), (2, -1), (1, -1)]);
}

#[test]
fn releasing_frames_from_a_null_frame_list_or_an_empty_frame_list_releases_nothing() {
    let mut context = new_context();
    for head in [std::ptr::null_mut(), base()] {
        unsafe { release_frames(&mut context, head) };
    }
    assert!(walked().is_empty());
}

#[test]
fn a_runtime_frame_is_added_to_the_frame_list_during_its_call_and_releases_each_slot_by_its_type() {
    let mut owned = new_context();
    owned.frames = base();
    let context = &raw mut owned;
    let text: Arc<str> = Arc::from("held");
    let (held, words) = every_kind(context, [5, 2, 3], &text);
    words.iter().copied().for_each(dup);
    let (seen, back) = unsafe {
        call_recording_slots(context, held, || {
            let record = (*context).frames;
            let linked = record != base() && (*record).previous == base();
            ((*record).each)(context, record.add(1).cast::<Word>(), -1);
            (linked, words.map(count))
        })
    };
    assert_eq!(seen, (true, [1, 1, 1]));
    assert_eq!(
        unsafe { (*context).frames },
        base(),
        "the record outlived its call"
    );
    for one in back {
        unsafe { one.release(context) };
    }
    assert_eq!(freed(), [2, 3, 5]);
    assert_eq!(Arc::strong_count(&text), 1);
    assert_eq!(unsafe { (*context).heap.live() }, 0);
}

#[test]
fn releasing_the_live_frame_list_in_the_middle_of_a_call_releases_every_slot_once() {
    let mut owned = new_context();
    owned.frames = base();
    let context = &raw mut owned;
    let text: Arc<str> = Arc::from("held");
    let (held, _) = every_kind(context, [5, 2, 3], &text);
    let ((), _never_given_back) = unsafe {
        call_recording_slots(context, held, || release_frames(context, (*context).frames))
    };
    assert_eq!(freed(), [2, 3, 5]);
    assert_eq!(Arc::strong_count(&text), 1);
    assert_eq!(unsafe { (*context).heap.live() }, 0);
}

#[test]
fn a_call_with_no_slots_to_release_adds_no_runtime_frame_to_the_frame_list() {
    let mut owned = new_context();
    let context = &raw mut owned;
    let (linked, []) =
        unsafe { call_recording_slots(context, [], || !(*context).frames.is_null()) };
    assert!(!linked);
}

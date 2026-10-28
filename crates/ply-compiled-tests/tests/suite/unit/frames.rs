//! Frame records walked as the runtime walks them: records laid by hand where a stopped
//! computation's frames would be, between its stack pointer and its stack's top, and a runtime
//! frame's record holding a value of every kind it may hold.

use super::heap::Holding;
use crate::bare::{context, helper};
use ply_compiled::emitted::{Block, Carried, Handed, PlyFnDesc, PlyType};
use ply_compiled::frames::{Held, PlyFrame, Snapshot, base, held_across, release_live};
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::native::Answered;
use ply_compiled::rt::Ctx;
use ply_compiled::stack::Stack;
use std::cell::RefCell;
use std::sync::Arc;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);

/// A box: a block of one word, a marker the box is told apart by.
const BOX: Word = (HEADER + 8) as Word;

/// A closure: a block of its descriptor, a marker, and a box it captures.
const CLOSURE: Word = (HEADER + 8 * 3) as Word;

thread_local! {
    /// The markers of the blocks released, in the order they went.
    static FREED: RefCell<Vec<Word>> = const { RefCell::new(Vec::new()) };
    /// Each record's one slot and the `d` its `each` was called with.
    static WALKED: RefCell<Vec<(Word, i64)>> = const { RefCell::new(Vec::new()) };
}

/// The markers of the blocks released since the last call, least first.
pub(super) fn freed() -> Vec<Word> {
    let mut freed = FREED.with(|f| std::mem::take(&mut *f.borrow_mut()));
    freed.sort_unstable();
    freed
}

fn walked() -> Vec<(Word, i64)> {
    WALKED.with(|w| std::mem::take(&mut *w.borrow_mut()))
}

/// The count at a block's offset 0.
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

/// One holder fewer of a box, as a box's drop root counts one: the last frees it.
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

/// `type#m.Box`, which counts a box as emitted code does.
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

/// A closure's release entry: its captured box, then its block.
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

/// A closure told apart by `marker`, holding `capture`.
fn closure(ctx: *mut Ctx, marker: Word, capture: Word) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, CLOSURE) };
    unsafe {
        *word(w, 0) = (&raw const CLOSES_OVER_A_BOX) as Word;
        *word(w, 1) = marker;
        *word(w, 2) = capture;
    }
    w
}

/// A closure over a box, a carried box and a native object holding `text`, as a runtime frame holds
/// them, and their words, for the test to count.
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

/// A record and its one slot, as a compiled frame lays them out.
#[repr(C)]
struct Record {
    h: PlyFrame,
    slot: Word,
}

/// Lays three records on `stack` as three frames between `sp` and the top would hold them, slots
/// 1 to 3 from the outermost in, and answers the innermost.
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

/// A snapshot holds every record once more, innermost first, and stops at the base; a restore puts
/// the bytes back and holds them again; dropping the copy lets go of what the copy's own records
/// hold, whatever the stack holds since.
#[test]
fn a_snapshot_and_each_restore_hold_every_record_once_and_the_copy_lets_go_of_its_own() {
    let mut ctx = context();
    let stack = Stack::take().expect("the system maps a stack");
    let sp = stack.top() - 4096;
    let head = laid(&stack, sp);
    let snapshot =
        unsafe { Snapshot::take(&mut ctx, &stack, sp, head) }.expect("sp is on the stack");
    assert_eq!((snapshot.sp(), snapshot.head()), (sp, head));
    assert_eq!(walked(), [(3, 1), (2, 1), (1, 1)]);
    let scribbled = |byte| unsafe { std::ptr::write_bytes(sp as *mut u8, byte, stack.top() - sp) };
    scribbled(0xAB);
    unsafe { snapshot.restore(&mut ctx, &stack) };
    assert_eq!(walked(), [(3, 1), (2, 1), (1, 1)]);
    assert_eq!(unsafe { (*head.cast::<Record>()).slot }, 3);
    scribbled(0xCD);
    unsafe { snapshot.release(&mut ctx) };
    assert_eq!(walked(), [(3, -1), (2, -1), (1, -1)]);
}

/// A body abandoned at a stop lets go of what its live records hold, innermost first.
#[test]
fn a_live_chain_released_lets_go_of_every_record_once() {
    let mut ctx = context();
    let stack = Stack::take().expect("the system maps a stack");
    let head = laid(&stack, stack.top() - 4096);
    unsafe { release_live(&mut ctx, head) };
    assert_eq!(walked(), [(3, -1), (2, -1), (1, -1)]);
}

#[test]
fn a_chain_with_no_record_walks_nothing_and_a_stop_off_the_stack_takes_no_snapshot() {
    let mut ctx = context();
    let stack = Stack::take().expect("the system maps a stack");
    let other = Stack::take().expect("the system maps a stack");
    let sp = stack.top() - 256;
    for head in [std::ptr::null_mut(), base()] {
        let snapshot =
            unsafe { Snapshot::take(&mut ctx, &stack, sp, head) }.expect("sp is on the stack");
        unsafe {
            snapshot.restore(&mut ctx, &stack);
            snapshot.release(&mut ctx);
            release_live(&mut ctx, head);
        }
    }
    assert!(walked().is_empty());
    assert!(unsafe { Snapshot::take(&mut ctx, &stack, other.top() - 64, base()) }.is_none());
}

/// A runtime frame's record counts a closure through its descriptor, a carried value through its
/// type and a handle as a handle, and gives each back to the frame when its call returns.
#[test]
fn a_runtime_frames_record_counts_each_slot_as_its_kind_is_counted() {
    let mut owned = context();
    owned.frames = base();
    let ctx = &raw mut owned;
    let text: Arc<str> = Arc::from("held");
    let (held, words) = every_kind(ctx, [5, 2, 3], &text);
    let counts = || words.map(count);
    let (seen, back) = unsafe {
        held_across(ctx, held, || {
            let record = (*ctx).frames;
            let linked = record != base() && (*record).prev == base();
            let slots = record.add(1).cast::<Word>();
            ((*record).each)(ctx, slots, 1);
            let up = counts();
            ((*record).each)(ctx, slots, -1);
            (linked, up, counts())
        })
    };
    assert_eq!(seen, (true, [2, 2, 2], [1, 1, 1]));
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

/// A frame abandoned under its call never returns to let go of what it held: the walk does, as each
/// is released.
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

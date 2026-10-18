//! Frame records, how the runtime knows what a stopped computation's frames hold. A frame that may
//! stop links a [`PlyFrame`] into [`Ctx::frames`] around the call, its slots following the record,
//! and the record's `each` counts every slot once more (`d > 0`) or once fewer (`d < 0`). The
//! runtime reads no slot: it walks a chain to hold a stop for its snapshot, to hold it again for each
//! restore, and to let go of what a dropped snapshot or an abandoned body held. Only a detached
//! body's own stack records; on every other stack the head is null and nothing links.

use crate::emitted::{Block, Carried};
use crate::heap::Word;
use crate::rt::Ctx;
use crate::stack::Stack;

/// `void (*each)(PlyCtx *ctx, Word *slots, int64_t d)`: one holder more of every slot for `d > 0`,
/// one fewer for `d < 0`.
pub type Each = unsafe extern "C" fn(*mut Ctx, *mut Word, i64);

/// A record in a frame that may stop, as compiled code lays it out, its slots following it.
#[repr(C)]
pub struct PlyFrame {
    /// The record below on the same stack, or [`base`].
    pub prev: *mut PlyFrame,
    pub each: Each,
}

struct Base(PlyFrame);

// SAFETY: nothing writes the base, which only ends chains.
unsafe impl Sync for Base {}

unsafe extern "C" fn holds_nothing(_ctx: *mut Ctx, _slots: *mut Word, _d: i64) {}

static BASE: Base = Base(PlyFrame {
    prev: std::ptr::null_mut(),
    each: holds_nothing,
});

/// The record every chain on a stack that records ends at, and the head of one where no frame has
/// linked a record yet.
pub fn base() -> *mut PlyFrame {
    (&raw const BASE.0).cast_mut()
}

/// Calls `each(d)` on the slots of every record from `head` to the chain's end, innermost first,
/// each record read where `at` puts it.
unsafe fn walk(
    ctx: *mut Ctx,
    head: *mut PlyFrame,
    d: i64,
    at: impl Fn(*mut PlyFrame) -> *mut PlyFrame,
) {
    let mut record = head;
    while !record.is_null() && record != base() {
        let here = at(record);
        let (prev, each) = unsafe { ((*here).prev, (*here).each) };
        unsafe { each(ctx, here.add(1).cast::<Word>(), d) };
        record = prev;
    }
}

/// Whether a record at `record` lies whole in `[sp, top)`, where a stopped computation's live
/// frames are.
fn within(record: *mut PlyFrame, sp: usize, top: usize) -> bool {
    let at = record.addr();
    at >= sp && at.is_multiple_of(8) && at + size_of::<PlyFrame>() <= top
}

/// Lets go of what every record of a live chain holds: the frames of a computation that will never
/// run on.
///
/// # Safety
/// The records from `head` are live, in the frames of a stopped computation that never resumes.
pub unsafe fn release_live(ctx: *mut Ctx, head: *mut PlyFrame) {
    unsafe { walk(ctx, head, -1, |record| record) }
}

/// What a snapshot keeps of a computation stopped on a stack of the runtime's: the stack's bytes
/// from the stopped stack pointer to its top, and the head of the chain they hold, every slot of
/// which is held once for the copy.
pub struct Snapshot {
    sp: usize,
    words: Box<[Word]>,
    head: *mut PlyFrame,
}

impl Snapshot {
    /// The bytes live on `stack` with `sp` its stack pointer, each slot of the chain from `head`
    /// held once more for the copy; `None` when `sp` is not on `stack`.
    ///
    /// # Safety
    /// A computation stopped on `stack` at `sp`, every record of its chain from `head` in its live
    /// frames.
    pub unsafe fn take(
        ctx: *mut Ctx,
        stack: &Stack,
        sp: usize,
        head: *mut PlyFrame,
    ) -> Option<Snapshot> {
        let live = stack.live(sp)?;
        assert!(
            live.len().is_multiple_of(8),
            "a stack stopped off a word's alignment"
        );
        let words: Box<[Word]> = live
            .chunks_exact(8)
            .map(|w| Word::from_ne_bytes(w.try_into().expect("eight bytes")))
            .collect();
        let top = stack.top();
        unsafe { walk(ctx, head, 1, |record| on_stack(record, sp, top)) };
        Some(Snapshot { sp, words, head })
    }

    /// The stack pointer the computation stopped at, which a restore puts back.
    pub fn sp(&self) -> usize {
        self.sp
    }

    pub fn head(&self) -> *mut PlyFrame {
        self.head
    }

    fn top(&self) -> usize {
        self.sp + 8 * self.words.len()
    }

    /// Writes the copy back where it was taken from, each slot held once more for the run that goes
    /// on from it; the copy keeps its own.
    ///
    /// # Safety
    /// `stack` is the stack the copy was taken of, and nothing runs on it.
    pub unsafe fn restore(&self, ctx: *mut Ctx, stack: &Stack) {
        let bytes = unsafe {
            std::slice::from_raw_parts(self.words.as_ptr().cast::<u8>(), 8 * self.words.len())
        };
        unsafe { stack.restore(self.sp, bytes) };
        let (sp, top) = (self.sp, self.top());
        unsafe { walk(ctx, self.head, 1, |record| on_stack(record, sp, top)) };
    }

    /// Lets go of what the copy holds: each slot once, read from the copy, whatever the stack it was
    /// taken of holds now.
    ///
    /// # Safety
    /// `ctx` is the context the copy was taken under.
    pub unsafe fn release(mut self, ctx: *mut Ctx) {
        let (sp, top) = (self.sp, self.top());
        let copy = self.words.as_mut_ptr().cast::<u8>();
        let in_copy = |record: *mut PlyFrame| {
            assert!(
                within(record, sp, top),
                "a frame record at {record:p} lies outside the snapshot of {sp:#x}..{top:#x}"
            );
            unsafe { copy.add(record.addr() - sp).cast::<PlyFrame>() }
        };
        unsafe { walk(ctx, self.head, -1, in_copy) };
    }
}

fn on_stack(record: *mut PlyFrame, sp: usize, top: usize) -> *mut PlyFrame {
    debug_assert!(
        within(record, sp, top),
        "a frame record at {record:p} lies outside the stopped stack's {sp:#x}..{top:#x}"
    );
    record
}

/// A word a runtime frame holds across a call into compiled code, by how it is counted.
#[repr(C, u64)]
pub enum Held {
    /// A function value's block: counted at offset 0, released through its descriptor.
    Closure(Block),
    /// A runtime handle: counted at offset 0, given back as `rt_handle_free` gives one back.
    Handle(Block),
    /// A value with its type, counted through the type.
    Carried(Carried),
}

impl Held {
    fn dup(&self, ctx: *mut Ctx) {
        match self {
            Held::Closure(b) | Held::Handle(b) => b.dup(),
            Held::Carried(v) => {
                let _ = v.dup(ctx);
            }
        }
    }

    /// Lets go of the count this holds, the last holder releasing the value as its kind does.
    ///
    /// # Safety
    /// `ctx` is the running entry's.
    pub unsafe fn release(self, ctx: *mut Ctx) {
        unsafe {
            match self {
                Held::Closure(b) => {
                    if b.last() {
                        b.release_function(ctx);
                    }
                }
                Held::Handle(b) => {
                    if b.last() {
                        b.free_handle(ctx);
                    }
                }
                Held::Carried(v) => v.release(ctx),
            }
        }
    }
}

/// A runtime frame's record, laid out as compiled code lays out its own.
#[repr(C)]
struct RtFrame<const N: usize> {
    h: PlyFrame,
    held: [Held; N],
}

unsafe extern "C" fn held_each<const N: usize>(ctx: *mut Ctx, slots: *mut Word, d: i64) {
    let held = slots.cast::<Held>();
    for i in 0..N {
        let one = unsafe { held.add(i) };
        if d > 0 {
            unsafe { (*one).dup(ctx) };
        } else {
            unsafe { one.read().release(ctx) };
        }
    }
}

/// Runs `call`, which enters compiled code that may stop the body whose stack this frame is on,
/// with `held` in a record on that stack's chain: a snapshot of a stop under the call holds each once
/// more, every restore once more, and a body abandoned under the call lets go of them. Answers what
/// `call` answered and `held`, the caller's again. A stack that records nothing links nothing.
///
/// # Safety
/// `ctx` is the context running on this stack.
pub unsafe fn held_across<const N: usize, R>(
    ctx: *mut Ctx,
    held: [Held; N],
    call: impl FnOnce() -> R,
) -> (R, [Held; N]) {
    let below = unsafe { (*ctx).frames };
    let mut record = RtFrame {
        h: PlyFrame {
            prev: below,
            each: held_each::<N>,
        },
        held,
    };
    let me = (&raw mut record).cast::<PlyFrame>();
    if !below.is_null() {
        unsafe { (*ctx).frames = me };
    }
    let r = call();
    unsafe {
        if (*ctx).frames == me {
            (*ctx).frames = (*me).prev;
        }
    }
    (r, record.held)
}

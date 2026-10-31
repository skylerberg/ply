//! Frame records, how the runtime knows what a stopped computation's frames hold. A frame that may
//! stop links a [`PlyFrame`] into [`Ctx::frames`] around the call, its slots following the record,
//! and the record's `each` lets go of every slot. The runtime reads no slot: it walks a chain to let
//! go of what an abandoned body held. Only a detached body's own stack records; on every other
//! stack the head is null and nothing links.

use crate::emitted::{Block, Carried};
use crate::heap::Word;
use crate::rt::Ctx;

/// `void (*each)(PlyCtx *ctx, Word *slots, int64_t d)`: one holder fewer of every slot. The runtime
/// passes `d` as -1.
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

/// Lets go of what every record of a live chain holds, innermost first: the frames of a
/// computation that will never run on.
///
/// # Safety
/// The records from `head` are live, in the frames of a stopped computation that never resumes.
pub unsafe fn release_live(ctx: *mut Ctx, head: *mut PlyFrame) {
    let mut record = head;
    while !record.is_null() && record != base() {
        let (prev, each) = unsafe { ((*record).prev, (*record).each) };
        unsafe { each(ctx, record.add(1).cast::<Word>(), -1) };
        record = prev;
    }
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

unsafe extern "C" fn held_each<const N: usize>(ctx: *mut Ctx, slots: *mut Word, _d: i64) {
    let held = slots.cast::<Held>();
    for i in 0..N {
        unsafe { held.add(i).read().release(ctx) };
    }
}

/// Runs `call`, which enters compiled code that may stop the body whose stack this frame is on,
/// with `held` in a record on that stack's chain, so a body abandoned under the call lets go of
/// them. Returns what `call` returned and `held`, the caller's again. A stack that records nothing
/// links nothing.
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

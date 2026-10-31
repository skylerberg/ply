use crate::emitted::{Block, Carried};
use crate::heap::Word;
use crate::rt::Ctx;

pub type Each = unsafe extern "C" fn(*mut Ctx, *mut Word, i64);

#[repr(C)]
pub struct PlyFrame {
    pub prev: *mut PlyFrame,
    pub each: Each,
}

struct Base(PlyFrame);

unsafe impl Sync for Base {}

unsafe extern "C" fn release_nothing(_ctx: *mut Ctx, _slots: *mut Word, _d: i64) {}

static BASE: Base = Base(PlyFrame {
    prev: std::ptr::null_mut(),
    each: release_nothing,
});

pub fn base() -> *mut PlyFrame {
    (&raw const BASE.0).cast_mut()
}

pub unsafe fn release_frames(ctx: *mut Ctx, head: *mut PlyFrame) {
    let mut record = head;
    while !record.is_null() && record != base() {
        let (prev, each) = unsafe { ((*record).prev, (*record).each) };
        unsafe { each(ctx, record.add(1).cast::<Word>(), -1) };
        record = prev;
    }
}

#[repr(C, u64)]
pub enum Held {
    Closure(Block),
    Handle(Block),
    Carried(Carried),
}

impl Held {
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

#[repr(C)]
struct RtFrame<const N: usize> {
    h: PlyFrame,
    held: [Held; N],
}

unsafe extern "C" fn release_slots<const N: usize>(ctx: *mut Ctx, slots: *mut Word, _d: i64) {
    let held = slots.cast::<Held>();
    for i in 0..N {
        unsafe { held.add(i).read().release(ctx) };
    }
}

pub unsafe fn call_recording_slots<const N: usize, R>(
    ctx: *mut Ctx,
    held: [Held; N],
    call: impl FnOnce() -> R,
) -> (R, [Held; N]) {
    let below = unsafe { (*ctx).frames };
    let mut record = RtFrame {
        h: PlyFrame {
            prev: below,
            each: release_slots::<N>,
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

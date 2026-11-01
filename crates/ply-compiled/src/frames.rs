use crate::emitted::{Block, Carried};
use crate::heap::Word;
use crate::rt::Context;

pub type Each = unsafe extern "C" fn(*mut Context, *mut Word, i64);

#[repr(C)]
pub struct PlyFrame {
    pub previous: *mut PlyFrame,
    pub each: Each,
}

struct Base(PlyFrame);

unsafe impl Sync for Base {}

unsafe extern "C" fn release_nothing(_context: *mut Context, _slots: *mut Word, _d: i64) {}

static BASE: Base = Base(PlyFrame {
    previous: std::ptr::null_mut(),
    each: release_nothing,
});

pub fn base() -> *mut PlyFrame {
    (&raw const BASE.0).cast_mut()
}

pub unsafe fn release_frames(context: *mut Context, head: *mut PlyFrame) {
    let mut record = head;
    while !record.is_null() && record != base() {
        let (previous, each) = unsafe { ((*record).previous, (*record).each) };
        unsafe { each(context, record.add(1).cast::<Word>(), -1) };
        record = previous;
    }
}

#[repr(C, u64)]
pub enum FrameObject {
    Closure(Block),
    Handle(Block),
    Carried(Carried),
}

impl FrameObject {
    pub unsafe fn release(self, context: *mut Context) {
        unsafe {
            match self {
                FrameObject::Closure(b) => {
                    if b.last() {
                        b.release_function(context);
                    }
                }
                FrameObject::Handle(b) => {
                    if b.last() {
                        b.free_handle(context);
                    }
                }
                FrameObject::Carried(v) => v.release(context),
            }
        }
    }
}

#[repr(C)]
struct RtFrame<const N: usize> {
    h: PlyFrame,
    held: [FrameObject; N],
}

unsafe extern "C" fn release_slots<const N: usize>(
    context: *mut Context,
    slots: *mut Word,
    _d: i64,
) {
    let held = slots.cast::<FrameObject>();
    for i in 0..N {
        unsafe { held.add(i).read().release(context) };
    }
}

pub unsafe fn call_recording_slots<const N: usize, R>(
    context: *mut Context,
    held: [FrameObject; N],
    call: impl FnOnce() -> R,
) -> (R, [FrameObject; N]) {
    let below = unsafe { (*context).frames };
    let mut record = RtFrame {
        h: PlyFrame {
            previous: below,
            each: release_slots::<N>,
        },
        held,
    };
    let me = (&raw mut record).cast::<PlyFrame>();
    if !below.is_null() {
        unsafe { (*context).frames = me };
    }
    let r = call();
    unsafe {
        if (*context).frames == me {
            (*context).frames = (*me).previous;
        }
    }
    (r, record.held)
}

use super::*;
use std::sync::atomic::AtomicUsize;
use std::sync::{Condvar, Once, OnceLock, PoisonError};
use std::time::{Duration, Instant};

pub unsafe extern "C" fn rt_stack_more(context: *mut Context) -> Word {
    let here = 0u8;
    let at = std::ptr::from_ref(&here) as usize;
    let c = unsafe { &mut *context };
    if c.out_of_time() {
        return 0;
    }
    let floor = stack_floor();
    if at >= floor {
        c.stack_floor.store(floor, Relaxed);
        return 1;
    }
    if ply_core::limit::writable_stack_start(at).is_some() && ply_core::limit::grow_active_stack() {
        c.grown += 1;
        c.stack_floor.store(stack_floor(), Relaxed);
        return 1;
    }
    let d = Diagnostic::error(
        codes::RUNTIME_ERROR,
        "this call would nest past what the native stack holds",
    )
    .primary(c.site(), "the call that nested too deep");
    c.fail_with(FAILED_OUT_OF_STACK, d);
    0
}

pub unsafe fn switch_stack_preserving_context(context: *mut Context, from: *mut usize, to: usize) {
    let (frames, task) = unsafe { ((*context).frames, (*context).task) };
    let running = ply_core::limit::running();
    unsafe { switch(&mut *from, to) };
    unsafe {
        ply_core::limit::set_active_stack(running);
        (*context).frames = frames;
        (*context).task = task;
        (*context).stack_floor.store(stack_floor(), Relaxed);
    }
}

pub fn on_known_stack<F: FnOnce() -> Word>(context: *mut Context, call: F) -> Word {
    if let Some(floor) = floor_here() {
        unsafe { (*context).stack_floor.store(floor, Relaxed) };
        return call();
    }
    let Some(stack) = Stack::take() else {
        return fail_out_of_stack(unsafe { &mut *context });
    };
    let mut hop = Hop {
        context,
        stack: &stack,
        call: Some(call),
        result: 0,
        back: 0,
    };
    let handover = &raw mut hop;
    let stack_pointer = stack.prepare(run_on_new_stack::<F>, handover as usize);
    unsafe { switch_stack_preserving_context(context, &raw mut (*handover).back, stack_pointer) };
    unsafe { (*handover).result }
}

struct Hop<'a, F> {
    context: *mut Context,
    stack: &'a Stack,
    call: Option<F>,
    result: Word,
    back: usize,
}

extern "C" fn run_on_new_stack<F: FnOnce() -> Word>(arg: usize) {
    let hop = arg as *mut Hop<'_, F>;
    let call = unsafe {
        (*(*hop).context)
            .stack_floor
            .store((*hop).stack.enter(), Relaxed);
        (*(*hop).context).frames = std::ptr::null_mut();
        (*hop).call.take().expect("a hop runs its call once")
    };
    let result = call();
    let to = unsafe {
        (*hop).result = result;
        (*hop).back
    };
    let mut spent = 0;
    unsafe { switch(&mut spent, to) };
    std::process::abort();
}

pub(crate) fn fail_out_of_stack(c: &mut Context) -> Word {
    let d = error("the system would map no further stack").note(
        "each live task, and each `handle` body whose clause resumes off the tail, holds one until it ends",
    );
    c.fail_with(FAILED_OUT_OF_STACK, d)
}

const DEADLINE_RECHECK_INTERVAL: Duration = Duration::from_millis(10);

struct Watch {
    watched: Mutex<Vec<(usize, Instant)>>,
    changed: Condvar,
}

fn watch() -> &'static Watch {
    static DEADLINE_WATCHDOG: OnceLock<Watch> = OnceLock::new();
    static STARTED: Once = Once::new();
    let w = DEADLINE_WATCHDOG.get_or_init(|| Watch {
        watched: Mutex::new(Vec::new()),
        changed: Condvar::new(),
    });
    STARTED.call_once(|| {
        std::thread::Builder::new()
            .name("ply-deadlines".into())
            .spawn(move || w.run())
            .expect("the deadline watchdog's thread starts");
    });
    w
}

impl Watch {
    fn run(&self) -> ! {
        let mut watched = self.watched.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            let now = Instant::now();
            let mut wake: Option<Instant> = None;
            for &(floor, deadline) in watched.iter() {
                let next = if deadline <= now {
                    unsafe { (*(floor as *const AtomicUsize)).store(usize::MAX, Relaxed) };
                    now + DEADLINE_RECHECK_INTERVAL
                } else {
                    deadline
                };
                wake = Some(wake.map_or(next, |w| w.min(next)));
            }
            watched = match wake {
                Some(at) => {
                    let wait = at.saturating_duration_since(now);
                    let waited = self.changed.wait_timeout(watched, wait);
                    waited.unwrap_or_else(PoisonError::into_inner).0
                }
                None => (self.changed.wait(watched)).unwrap_or_else(PoisonError::into_inner),
            };
        }
    }
}

pub(crate) fn watched(floor: &AtomicUsize, deadline: Instant) {
    let w = watch();
    let at = std::ptr::from_ref(floor) as usize;
    (w.watched.lock().unwrap_or_else(PoisonError::into_inner)).push((at, deadline));
    w.changed.notify_one();
}

pub(crate) fn unwatched(floor: &AtomicUsize) {
    let at = std::ptr::from_ref(floor) as usize;
    let mut watched = watch()
        .watched
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    watched.retain(|&(f, _)| f != at);
}

pub const PROGRAM_STACK: usize = crate::stack::RESERVE;

pub const STACK_MARGIN: usize = 512 * 1024;

thread_local! {
    static OWN: (usize, usize) = stack_of_this_thread();
}

pub(crate) fn release_unused_stack_pages() {
    const KEPT: usize = 64 << 10;
    let here = 0u8;
    let at = std::ptr::from_ref(&here) as usize;
    OWN.with(|&(bottom, top)| {
        if !(bottom..top).contains(&at) {
            return;
        }
        let from = bottom.saturating_add(STACK_MARGIN).next_multiple_of(KEPT);
        let to = at.saturating_sub(KEPT) / KEPT * KEPT;
        if to > from {
            unsafe { crate::vm::let_go(from as *mut u8, to - from) };
        }
    });
}

pub(crate) fn floor_here() -> Option<usize> {
    let here = 0u8;
    let at = std::ptr::from_ref(&here) as usize;
    if let Some(low) = ply_core::limit::writable_stack_start(at) {
        return Some(low + STACK_MARGIN);
    }
    OWN.with(|&(bottom, top)| {
        (bottom..top)
            .contains(&at)
            .then(|| bottom.saturating_add(STACK_MARGIN))
    })
}

pub(crate) fn stack_floor() -> usize {
    floor_here().unwrap_or(usize::MAX)
}

#[cfg(target_os = "macos")]
pub(super) fn stack_of_this_thread() -> (usize, usize) {
    unsafe extern "C" {
        fn pthread_self() -> *mut std::ffi::c_void;
        fn pthread_get_stackaddr_np(thread: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        fn pthread_get_stacksize_np(thread: *mut std::ffi::c_void) -> usize;
    }
    let (top, size) = unsafe {
        let me = pthread_self();
        (
            pthread_get_stackaddr_np(me) as usize,
            pthread_get_stacksize_np(me),
        )
    };
    (top.saturating_sub(size), top)
}

#[cfg(target_os = "linux")]
pub(super) fn stack_of_this_thread() -> (usize, usize) {
    #[repr(C, align(8))]
    struct Attribute([u8; 64]);
    unsafe extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getattr_np(thread: usize, attribute: *mut Attribute) -> i32;
        fn pthread_attr_getstack(
            attribute: *const Attribute,
            address: *mut *mut std::ffi::c_void,
            size: *mut usize,
        ) -> i32;
        fn pthread_attr_destroy(attribute: *mut Attribute) -> i32;
    }
    let mut attribute = Attribute([0; 64]);
    let mut address: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut size = 0usize;
    let bottom = unsafe {
        if pthread_getattr_np(pthread_self(), &mut attribute) != 0 {
            return fallback_stack();
        }
        let ok = pthread_attr_getstack(&attribute, &mut address, &mut size) == 0;
        pthread_attr_destroy(&mut attribute);
        if !ok {
            return fallback_stack();
        }
        address as usize
    };
    (bottom, bottom.saturating_add(size))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn stack_of_this_thread() -> (usize, usize) {
    fallback_stack()
}

#[cfg(not(target_os = "macos"))]
pub(super) fn fallback_stack() -> (usize, usize) {
    let here = 0u8;
    let bottom = (std::ptr::from_ref(&here) as usize).saturating_sub((1 << 20) + STACK_MARGIN);
    (bottom, usize::MAX)
}

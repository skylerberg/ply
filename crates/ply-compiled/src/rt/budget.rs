//! Fuel, stacks, and the wall clock an entry runs within.

use super::*;
use std::sync::atomic::AtomicUsize;
use std::sync::{Condvar, Once, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// The prologue's refusal: this call would nest past the budget the machine handed the entry.
pub unsafe extern "C" fn rt_no_fuel(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let depth = ctx.depth_bound;
    let d = Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("recursion limit of {depth} nested calls exceeded"),
    )
    .primary(ctx.site(), "the call that overran it");
    ctx.fail_with(FAILED_OUT_OF_FUEL, d);
}

/// The slow path of a prologue or a loop's restart, for a frame below `ctx.stack_floor`: ends an
/// entry whose wall clock ran out, which the watchdog marks by raising its floor past every frame;
/// otherwise makes room below the floor of the stack it runs on and lowers the floor to match.
/// Answers 1, or 0 with the context failed for a spent clock, a call nesting past what the native
/// stack holds, on a thread's own stack, whose floor is final, on a runtime stack whose reservation
/// is spent, and on a stack the runtime does not know.
pub unsafe extern "C" fn rt_stack_more(ctx: *mut Ctx) -> Word {
    let here = 0u8;
    let at = std::ptr::from_ref(&here) as usize;
    let c = unsafe { &mut *ctx };
    if c.out_of_time() {
        return 0;
    }
    // A floor another context's growth or a switch of stacks left stale needs no system call.
    let floor = stack_floor();
    if at >= floor {
        c.stack_floor.store(floor, Relaxed);
        return 1;
    }
    if ply_core::limit::running_low(at).is_some() && ply_core::limit::extend_running() {
        c.grown += 1;
        c.stack_floor.store(stack_floor(), Relaxed);
        return 1;
    }
    let d = error("this call would nest past what the native stack holds");
    c.fail_with(FAILED_OUT_OF_STACK, d);
    0
}

/// [`switch`] between two computations, each nesting on a stack of its own: returns when something
/// switches back, with [`Ctx::fuel`], [`Ctx::frames`] and [`Ctx::task`] as this stack left them,
/// whatever ran meanwhile, this stack the running one again and the floor its own.
///
/// # Safety
/// As [`switch`], and `ctx` is the context both stacks run under.
pub unsafe fn switch_keeping(ctx: *mut Ctx, from: *mut usize, to: usize) {
    let (fuel, frames, task) = unsafe { ((*ctx).fuel, (*ctx).frames, (*ctx).task) };
    let running = ply_core::limit::running();
    unsafe { switch(&mut *from, to) };
    unsafe {
        ply_core::limit::set_running(running);
        (*ctx).fuel = fuel;
        (*ctx).frames = frames;
        (*ctx).task = task;
        (*ctx).stack_floor.store(stack_floor(), Relaxed);
    }
}

/// Runs `call`, which begins compiled code, where that code can make room as it nests: here, on a
/// stack the runtime knows, or else, on a segment a Rust recursion grew onto, on a runtime stack of
/// its own for the call's duration. Answers 0 with the context failed when no stack can be had.
pub fn on_known_stack<F: FnOnce() -> Word>(ctx: *mut Ctx, call: F) -> Word {
    if let Some(floor) = floor_here() {
        unsafe { (*ctx).stack_floor.store(floor, Relaxed) };
        return call();
    }
    let Some(stack) = Stack::take() else {
        return no_stack_to_take(unsafe { &mut *ctx });
    };
    let mut hop = Hop {
        ctx,
        stack: &stack,
        call: Some(call),
        answer: 0,
        back: 0,
    };
    let handover = &raw mut hop;
    let sp = stack.prepare(hopped::<F>, handover as usize);
    unsafe { switch_keeping(ctx, &raw mut (*handover).back, sp) };
    unsafe { (*handover).answer }
}

/// What [`on_known_stack`] hands the stack it hops onto, from its own frame, which outlives the hop.
struct Hop<'a, F> {
    ctx: *mut Ctx,
    stack: &'a Stack,
    call: Option<F>,
    answer: Word,
    back: usize,
}

extern "C" fn hopped<F: FnOnce() -> Word>(arg: usize) {
    let hop = arg as *mut Hop<'_, F>;
    let call = unsafe {
        (*(*hop).ctx)
            .stack_floor
            .store((*hop).stack.enter(), Relaxed);
        (*(*hop).ctx).frames = std::ptr::null_mut();
        (*hop).call.take().expect("a hop runs its call once")
    };
    let answer = call();
    let to = unsafe {
        (*hop).answer = answer;
        (*hop).back
    };
    let mut spent = 0;
    unsafe { switch(&mut spent, to) };
    std::process::abort();
}

/// Fails `c` for a stack the system would not map, as when too many bodies and tasks are suspended
/// at once.
pub(crate) fn no_stack_to_take(c: &mut Ctx) -> Word {
    let d = error("the system would map no further stack").note(
        "each live task, and each `handle` body whose clause resumes off the tail, holds one until it ends",
    );
    c.fail_with(FAILED_OUT_OF_STACK, d)
}

/// Nothing. The released compiler's code calls it once its counter reaches [`Ctx::next_tick`],
/// which this runtime never lets it do.
pub unsafe extern "C" fn rt_tick(_ctx: *mut Ctx) {}

/// How soon the watchdog raises an expired entry's floor again, since a stack switch lowers it.
const REWATCH: Duration = Duration::from_millis(10);

/// The floors of the contexts running against a deadline, each by its address, which a thread
/// raises past every frame once the deadline passes: the context's next call or loop restart then
/// reaches [`rt_stack_more`], which ends the entry. Calls themselves pay nothing for the clock.
struct Watch {
    watched: Mutex<Vec<(usize, Instant)>>,
    changed: Condvar,
}

fn watch() -> &'static Watch {
    static WATCH: OnceLock<Watch> = OnceLock::new();
    static STARTED: Once = Once::new();
    let w = WATCH.get_or_init(|| Watch {
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
                    // SAFETY: a context leaves the list before it moves or its memory goes.
                    unsafe { (*(floor as *const AtomicUsize)).store(usize::MAX, Relaxed) };
                    now + REWATCH
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

/// Has the watchdog raise `floor` once `deadline` passes, until [`unwatched`].
pub(crate) fn watched(floor: &AtomicUsize, deadline: Instant) {
    let w = watch();
    let at = std::ptr::from_ref(floor) as usize;
    (w.watched.lock().unwrap_or_else(PoisonError::into_inner)).push((at, deadline));
    w.changed.notify_one();
}

/// Takes `floor` off the watchdog's list, where it is on it.
pub(crate) fn unwatched(floor: &AtomicUsize) {
    let at = std::ptr::from_ref(floor) as usize;
    let mut watched = watch()
        .watched
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    watched.retain(|&(f, _)| f != at);
}

/// The stack of a thread that runs a program, as large as a runtime stack can grow: the runtime
/// recurses on the native stack, a compiled body honours its call bound there, and an unoptimised
/// frame runs to tens of kilobytes. Reserved, not committed. A `parallel` block's thread is one: a
/// run's lane enters a whole test on it.
pub const PROGRAM_STACK: usize = crate::stack::RESERVE;

/// Room below the floor for the runtime's frames and the deepest compiled frame itself.
pub const STACK_MARGIN: usize = 512 * 1024;

/// The floor compiled code honours on the stack this frame is on: the running runtime stack's, or
/// this thread's own. `None` on a stack the runtime does not know, a segment a Rust recursion grew
/// onto.
pub(crate) fn floor_here() -> Option<usize> {
    thread_local! {
        // Asked of the platform once per thread: it can be a `/proc` read.
        static OWN: (usize, usize) = stack_of_this_thread();
    }
    let here = 0u8;
    let at = std::ptr::from_ref(&here) as usize;
    if let Some(low) = ply_core::limit::running_low(at) {
        return Some(low + STACK_MARGIN);
    }
    OWN.with(|&(bottom, top)| {
        (bottom..top)
            .contains(&at)
            .then(|| bottom.saturating_add(STACK_MARGIN))
    })
}

/// [`floor_here`], or above every address on a stack the runtime does not know, where a probe then
/// asks [`rt_stack_more`], which refuses.
pub(crate) fn stack_floor() -> usize {
    floor_here().unwrap_or(usize::MAX)
}

/// This thread's stack, from its lowest address to its top.
#[cfg(target_os = "macos")]
pub(super) fn stack_of_this_thread() -> (usize, usize) {
    unsafe extern "C" {
        fn pthread_self() -> *mut std::ffi::c_void;
        fn pthread_get_stackaddr_np(thread: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        fn pthread_get_stacksize_np(thread: *mut std::ffi::c_void) -> usize;
    }
    // The address is the stack's *top*; the region runs down from it.
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
    // Room for the opaque `pthread_attr_t`, at most 56 bytes on 64-bit libcs.
    #[repr(C, align(8))]
    struct Attr([u8; 64]);
    unsafe extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getattr_np(thread: usize, attr: *mut Attr) -> i32;
        fn pthread_attr_getstack(
            attr: *const Attr,
            addr: *mut *mut std::ffi::c_void,
            size: *mut usize,
        ) -> i32;
        fn pthread_attr_destroy(attr: *mut Attr) -> i32;
    }
    let mut attr = Attr([0; 64]);
    let mut addr: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut size = 0usize;
    let bottom = unsafe {
        if pthread_getattr_np(pthread_self(), &mut attr) != 0 {
            return fallback_stack();
        }
        let ok = pthread_attr_getstack(&attr, &mut addr, &mut size) == 0;
        pthread_attr_destroy(&mut attr);
        if !ok {
            return fallback_stack();
        }
        addr as usize
    };
    (bottom, bottom.saturating_add(size))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn stack_of_this_thread() -> (usize, usize) {
    fallback_stack()
}

/// Assumes a spawned thread's default stack below this frame, and every address above that the
/// thread's: refusing early beats overflowing.
#[cfg(not(target_os = "macos"))]
pub(super) fn fallback_stack() -> (usize, usize) {
    let here = 0u8;
    let bottom = (std::ptr::from_ref(&here) as usize).saturating_sub((1 << 20) + STACK_MARGIN);
    (bottom, usize::MAX)
}

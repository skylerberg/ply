//! Fuel, stacks, ticks, and the step and time budgets an entry runs within.

use super::*;

/// The prologue's refusal: this call would nest past the budget the machine handed the entry.
pub unsafe extern "C" fn rt_no_fuel(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("this call would nest past the machine's own bound on nested calls");
    ctx.fail_with(FAILED_OUT_OF_FUEL, d);
}

/// What a growth that could not be given a stack raises: the one bound left is the platform's.
pub unsafe extern "C" fn rt_no_stack(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("this call would nest past what the native stack holds");
    ctx.fail_with(FAILED_OUT_OF_STACK, d);
}

/// The handover a grown call runs from: what to enter, with what, where its answer goes, and the
/// stack pointer to come back to. It lives in [`rt_grow`]'s frame, which the new stack outlives.
pub(super) struct Grown {
    pub(super) ctx: *mut Ctx,
    pub(super) entry: Entry,
    pub(super) args: *const i64,
    pub(super) answer: i64,
    pub(super) back: usize,
}

/// The new stack's first and only frame: the call, then back to whoever grew it. Nothing switches
/// into this stack again, so where its own pointer lands is spent with it.
pub(super) extern "C" fn run_grown(arg: usize) {
    let g = arg as *mut Grown;
    let (ctx, entry, args) = unsafe { ((*g).ctx, (*g).entry, (*g).args) };
    let answer = unsafe { entry(ctx, args) };
    let to = unsafe {
        (*g).answer = answer;
        (*g).back
    };
    let mut spent = 0;
    unsafe { switch(&mut spent, to) };
    std::process::abort();
}

/// The prologue's answer to a frame that would cross the floor: the call runs on a stack of its
/// own, so how deep a program nests is the fuel's answer and never this thread's. `entry` is the
/// callee's own `(ctx, args)` form and `args` its arguments, both still held by the caller's frame.
pub unsafe extern "C" fn rt_grow(ctx: *mut Ctx, entry: i64, args: i64) -> i64 {
    let Some(stack) = Stack::reserve() else {
        unsafe { rt_no_stack(ctx) };
        return 0;
    };
    let mut grown = Grown {
        ctx,
        // SAFETY: the emitter passes the address of a compiled function's own `(ctx, args)` entry.
        entry: unsafe { std::mem::transmute::<usize, Entry>(entry as usize) },
        args: args as usize as *const i64,
        answer: 0,
        back: 0,
    };
    let handover = &raw mut grown;
    let sp = stack.prepare(run_grown, handover as usize);
    let floor = {
        let c = unsafe { &mut *ctx };
        c.grown += 1;
        std::mem::replace(&mut c.stack_floor, stack.floor())
    };
    let from = unsafe { &mut (*handover).back };
    unsafe { switch(from, sp) };
    let c = unsafe { &mut *ctx };
    c.stack_floor = floor;
    unsafe { (*handover).answer }
}

/// [`switch`] between two computations, each nesting on a stack of its own: returns when something
/// switches back, with [`Ctx::fuel`] as this stack left it, whatever ran meanwhile.
///
/// # Safety
/// As [`switch`], and `ctx` is the context both stacks run under.
pub(crate) unsafe fn switch_keeping(ctx: *mut Ctx, from: *mut usize, to: usize) {
    let fuel = unsafe { (*ctx).fuel };
    unsafe { switch(&mut *from, to) };
    unsafe { (*ctx).fuel = fuel };
}

/// The callback the counter reaching [`Ctx::next_tick`] makes: the entry gets more work, or it
/// gets none.
pub unsafe extern "C" fn rt_tick(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    ctx.tick();
}

/// Calls between two callbacks while an entry is still within its bounds.
pub(super) const TICK_CHUNK: i64 = 4096;

thread_local! {
    /// The calls one entry may make; 0 is no bound.
    static THREAD_STEP_BUDGET: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// Runs `f` with entries on this thread bounded by `steps` rather than by the default budget.
pub fn with_step_budget<R>(steps: i64, f: impl FnOnce() -> R) -> R {
    let before = THREAD_STEP_BUDGET.with(|t| t.replace(Some(steps.max(0))));
    let out = f();
    THREAD_STEP_BUDGET.with(|t| t.set(before));
    out
}

pub fn step_budget() -> i64 {
    THREAD_STEP_BUDGET
        .with(|t| t.get())
        .unwrap_or(ply_core::DEFAULT_STEP_BUDGET)
}

/// Runs `f` with entries on this thread bounded by neither budget: the compiler's own work is
/// not the program's, so the program's bounds are not its.
pub fn unbounded<R>(f: impl FnOnce() -> R) -> R {
    with_step_budget(0, || with_time_budget(0, f))
}

thread_local! {
    /// The wall clock an entry may take, in milliseconds; 0 is none. It abandons a run rather than
    /// judging it, so only a harness that will say so sets one.
    static THREAD_TIME_BUDGET_MS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Runs `f` with entries on this thread bounded by `ms` rather than by none.
pub fn with_time_budget<R>(ms: u64, f: impl FnOnce() -> R) -> R {
    let before = THREAD_TIME_BUDGET_MS.with(|t| t.replace(Some(ms)));
    let out = f();
    THREAD_TIME_BUDGET_MS.with(|t| t.set(before));
    out
}

pub fn time_budget_ms() -> u64 {
    THREAD_TIME_BUDGET_MS.with(|t| t.get()).unwrap_or(0)
}

/// The stack of a thread that runs a program: the runtime recurses on the native stack, a compiled
/// body honours its call bound there, and an unoptimised frame runs to kilobytes. Reserved, not
/// committed. A `parallel` block's thread is one: a run's lane enters a whole test on it.
pub const PROGRAM_STACK: usize = 256 << 20;

/// Room below the floor for the runtime's frames and the deepest compiled frame itself.
pub(crate) const STACK_MARGIN: usize = 512 * 1024;

/// The floor a computation begun here runs under: this thread's, where it begins on the thread's
/// own stack. On any other, one a call grew onto and a host call or a waiting block then began
/// more work on, nothing here knows how deep that stack runs, so the floor is above every address
/// and the first compiled frame takes a stack of its own.
pub(crate) fn stack_floor() -> usize {
    thread_local! {
        // Asked of the platform once per thread: it can be a `/proc` read.
        static OWN: (usize, usize) = stack_of_this_thread();
    }
    let here = 0u8;
    let at = std::ptr::from_ref(&here) as usize;
    OWN.with(|&(bottom, top)| {
        if (bottom..top).contains(&at) {
            bottom.saturating_add(STACK_MARGIN)
        } else {
            usize::MAX
        }
    })
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

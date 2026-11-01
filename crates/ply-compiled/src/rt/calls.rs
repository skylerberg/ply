//! The constants a unit reads, the scheduler's own work, and calls of a function value.

use super::*;

impl Ctx {
    /// The scheduler's own work begins, where it had not: the blocks it makes are not the
    /// program's. Returns whether it had.
    pub(crate) fn scheduler_entered(&mut self) -> bool {
        self.heap.set_scheduling(true)
    }

    /// The program's own work runs again, where the scheduler's ran.
    pub(crate) fn program_entered(&mut self) {
        self.heap.set_scheduling(false);
    }
}

/// The scheduler's own work begins, entered from a perform or a program call: returns 1 where
/// it already ran, for [`rt_scheduler_left`].
pub unsafe extern "C" fn rt_scheduler_entered(ctx: *mut Ctx) -> Word {
    Word::from(unsafe { &mut *ctx }.scheduler_entered())
}

/// Leaves what [`rt_scheduler_entered`] entered: the program runs again where `was` is 0.
pub unsafe extern "C" fn rt_scheduler_left(ctx: *mut Ctx, was: Word) {
    if was == 0 {
        unsafe { &mut *ctx }.program_entered();
    }
}

/// Keeps `kept`, an immortal copy, as the constant at code-table `index`. Only the first copy kept
/// stays, so emitted code on any thread reads one value for good.
pub unsafe extern "C" fn rt_keep_constant(ctx: *mut Ctx, index: I64, kept: Word) {
    if let Ok(index) = usize::try_from(index.0) {
        unsafe { &*ctx }.tables.keep(index, kept);
    }
}

/// Ends the run on a raise that left a `const fn`'s body, so no handler around the read takes it:
/// the definition's effects say it raises nothing. Any other failure stands as it is.
pub unsafe extern "C" fn rt_const_failed(ctx: *mut Ctx) {
    let c = unsafe { &mut *ctx };
    if c.failed == FAILED_PENDING {
        if let Some(p) = c.pending.take() {
            p.release();
        }
        c.failed = FAILED;
        c.diagnostic = c.diagnostic.take().map(|d| {
            d.note(
                "a constant must return a value: a raise that leaves its body ends the run, \
                 and no handler around the read can catch it",
            )
        });
    }
}

/// A read of the pure nullary definition at `index` in the unit's code table, for code the release
/// compiled: its value's word form. The first read evaluates the definition and hands `keep`, the
/// `keep#T` root of its type, the value, whose immortal copy the unit then keeps for good; each
/// later read returns that copy. A value `keep` returns 0 for, one that may not outlive its entry,
/// is kept nowhere, and each read evaluates the definition again. Where the evaluation fails or
/// raises, this returns 0 with the context as the evaluation left it, and keeps nothing. Contexts
/// reading one definition at once, on `parallel` branches or other threads, may each evaluate it;
/// the first copy kept is the one every later read returns.
pub unsafe extern "C" fn rt_memo(ctx: *mut Ctx, index: I64, keep: KeepRoot) -> Handed {
    if let Some(w) = memoized(unsafe { &*ctx }, index) {
        return Handed::of(w);
    }
    unsafe { evaluated(ctx, index, keep) }
}

/// A read of the `const fn` at `index` in the unit's code table, as [`rt_memo`] reads a plain
/// definition, but a raise that leaves the body fails the entry, so no handler around the read
/// takes it: the definition's effects say it raises nothing.
pub unsafe extern "C" fn rt_const(ctx: *mut Ctx, index: I64, keep: KeepRoot) -> Handed {
    if let Some(w) = memoized(unsafe { &*ctx }, index) {
        return Handed::of(w);
    }
    let w = unsafe { evaluated(ctx, index, keep) };
    unsafe { rt_const_failed(ctx) };
    w
}

/// The value the unit kept of the definition at code-table index `index`, if it has.
fn memoized(c: &Ctx, index: I64) -> Option<Word> {
    c.tables.memoized(usize::try_from(index.0).ok()?)
}

/// The pure nullary definition at `index` evaluated, and its value kept for good through `keep`:
/// the value, or 0 with the context failed.
unsafe fn evaluated(ctx: *mut Ctx, index: I64, keep: KeepRoot) -> Handed {
    let c = unsafe { &mut *ctx };
    let tables = Arc::clone(&c.tables);
    let Some(&code) = usize::try_from(index.0)
        .ok()
        .and_then(|i| tables.functions.get(i))
    else {
        c.fail(err_marshalling(&format!(
            "definition {} of a code table of {}",
            index.0,
            tables.functions.len()
        )));
        return Handed::of(0);
    };
    let index = index.0 as usize;
    // SAFETY: as in `call_value`; a nullary function never reads the null argument pointer.
    let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(code) };
    let w = unsafe { f(ctx, std::ptr::null()) };
    if unsafe { (*ctx).failed } != 0 {
        return Handed::of(0);
    }
    let kept = unsafe { keep(ctx, w) };
    if kept != 0 && unsafe { (*ctx).failed } == 0 {
        tables.keep(index, kept);
    }
    Handed::of(w)
}

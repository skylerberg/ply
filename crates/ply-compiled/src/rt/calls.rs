//! What emitted code asks of the runtime to read a constant, and the scheduler's own work.

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

/// Ends the run on a raise that left a `const`'s value, so no handler around the read takes it: the
/// definition's effects say it raises nothing. Any other failure stands as it is.
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

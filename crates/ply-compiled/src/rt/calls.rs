use super::*;

impl Ctx {
    pub(crate) fn begin_scheduling(&mut self) -> bool {
        self.heap.replace_scheduler_active(true)
    }

    pub(crate) fn program_entered(&mut self) {
        self.heap.replace_scheduler_active(false);
    }
}

pub unsafe extern "C" fn rt_scheduler_entered(ctx: *mut Ctx) -> Word {
    Word::from(unsafe { &mut *ctx }.begin_scheduling())
}

pub unsafe extern "C" fn rt_scheduler_left(ctx: *mut Ctx, was: Word) {
    if was == 0 {
        unsafe { &mut *ctx }.program_entered();
    }
}

pub unsafe extern "C" fn rt_keep_constant(ctx: *mut Ctx, index: I64, kept: Word) {
    if let Ok(index) = usize::try_from(index.0) {
        unsafe { &*ctx }.tables.keep(index, kept);
    }
}

pub unsafe extern "C" fn rt_const_failed(ctx: *mut Ctx) {
    let c = unsafe { &mut *ctx };
    if c.failed == FAILED_PENDING_RAISE {
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

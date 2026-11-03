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

pub unsafe extern "C" fn rt_failed_call(ctx: *mut Ctx, root: I64, start: I64, end: I64) {
    let c = unsafe { &mut *ctx };
    let site = c.span_at(root.0, start.0, end.0);
    c.failed_through_call(site);
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

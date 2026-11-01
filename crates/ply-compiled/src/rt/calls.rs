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

pub unsafe extern "C" fn rt_memo(ctx: *mut Ctx, index: I64, keep: KeepRoot) -> Handed {
    if let Some(w) = memoized(unsafe { &*ctx }, index) {
        return Handed::of(w);
    }
    unsafe { evaluated(ctx, index, keep) }
}

pub unsafe extern "C" fn rt_const(ctx: *mut Ctx, index: I64, keep: KeepRoot) -> Handed {
    if let Some(w) = memoized(unsafe { &*ctx }, index) {
        return Handed::of(w);
    }
    let w = unsafe { evaluated(ctx, index, keep) };
    unsafe { rt_const_failed(ctx) };
    w
}

fn memoized(c: &Ctx, index: I64) -> Option<Word> {
    c.tables.memoized(usize::try_from(index.0).ok()?)
}

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

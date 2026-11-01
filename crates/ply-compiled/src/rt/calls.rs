use super::*;

impl Context {
    pub(crate) fn begin_scheduling(&mut self) -> bool {
        self.heap.replace_scheduler_active(true)
    }

    pub(crate) fn program_entered(&mut self) {
        self.heap.replace_scheduler_active(false);
    }
}

pub unsafe extern "C" fn rt_scheduler_entered(context: *mut Context) -> Word {
    Word::from(unsafe { &mut *context }.begin_scheduling())
}

pub unsafe extern "C" fn rt_scheduler_left(context: *mut Context, was: Word) {
    if was == 0 {
        unsafe { &mut *context }.program_entered();
    }
}

pub unsafe extern "C" fn rt_keep_constant(context: *mut Context, index: I64, kept: Word) {
    if let Ok(index) = usize::try_from(index.0) {
        unsafe { &*context }.tables.keep(index, kept);
    }
}

pub unsafe extern "C" fn rt_const_failed(context: *mut Context) {
    let c = unsafe { &mut *context };
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

pub unsafe extern "C" fn rt_memo(context: *mut Context, index: I64, keep: KeepRoot) -> Handed {
    if let Some(w) = memoized(unsafe { &*context }, index) {
        return Handed::of(w);
    }
    unsafe { evaluated(context, index, keep) }
}

pub unsafe extern "C" fn rt_const(context: *mut Context, index: I64, keep: KeepRoot) -> Handed {
    if let Some(w) = memoized(unsafe { &*context }, index) {
        return Handed::of(w);
    }
    let w = unsafe { evaluated(context, index, keep) };
    unsafe { rt_const_failed(context) };
    w
}

fn memoized(c: &Context, index: I64) -> Option<Word> {
    c.tables.memoized(usize::try_from(index.0).ok()?)
}

unsafe fn evaluated(context: *mut Context, index: I64, keep: KeepRoot) -> Handed {
    let c = unsafe { &mut *context };
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
    let w = unsafe { f(context, std::ptr::null()) };
    if unsafe { (*context).failed } != 0 {
        return Handed::of(0);
    }
    let kept = unsafe { keep(context, w) };
    if kept != 0 && unsafe { (*context).failed } == 0 {
        tables.keep(index, kept);
    }
    Handed::of(w)
}

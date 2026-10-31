use super::*;

pub const FAILED: i64 = 1;

pub const FAILED_OUT_OF_STACK: i64 = 3;

pub const FAILED_TIMED_OUT: i64 = 5;

pub const FAILED_CANCELLED: i64 = 7;

pub const FAILED_UNWOUND_TO_HANDLER: i64 = 9;

pub const FAILED_PENDING_RAISE: i64 = 10;

pub(super) const MODE_RAISE: i64 = 2;

pub(super) fn is_abort(effect: &str, op: &str) -> bool {
    effect == "abort" && op == "raise"
}

pub(crate) fn call_fun(ctx: *mut Ctx, f: Word, args: &[Word]) -> Word {
    unsafe { Block::of(f).call(ctx, args) }
}

pub(crate) fn release_fun(f: Word) {
    unsafe { Block::of(f).release_function_if_last(running()) };
}

pub(crate) enum Kept {
    Pending(Unhandled, Option<Diagnostic>),
    Args(Vec<Word>),
    Clause(usize),
}

impl Kept {
    pub(crate) fn release(self) {
        match self {
            Kept::Pending(raise, _) => raise.release(),
            Kept::Args(words) => drop(words),
            Kept::Clause(_) => {}
        }
    }
}

pub(crate) fn call_clause(ctx: *mut Ctx, clause: Word, args: Vec<Word>) -> Word {
    let c = unsafe { &mut *ctx };
    let at = args.as_ptr();
    let n = args.len();
    let here = c.keep(Kept::Args(args));
    let r = call_fun(ctx, clause, unsafe { std::slice::from_raw_parts(at, n) });
    unsafe { &mut *ctx }.pop_saved(here).release();
    r
}

pub(crate) struct Raised(pub(crate) Vec<Carried>);

impl Raised {
    pub(crate) fn release(self) {
        let ctx = running();
        self.0.into_iter().for_each(|v| v.release(ctx));
    }

    pub(crate) fn handed(self) -> Vec<Word> {
        self.0.into_iter().map(|v| v.handed().word()).collect()
    }
}

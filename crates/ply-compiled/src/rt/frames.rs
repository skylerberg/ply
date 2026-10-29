//! How an entry fails, and what the runtime's own frames keep while they call into the program.

use super::*;

/// `Ctx::failed` when the entry failed with the diagnostic it records.
pub const FAILED: i64 = 1;

/// A call would nest past what its stack holds, or a body or task could not be given a stack.
pub const FAILED_OUT_OF_STACK: i64 = 3;

/// The harness stopped the entry at its wall clock, which is not a verdict on the program.
pub const FAILED_ABANDONED: i64 = 5;

/// The task was cancelled: it unwinds to its entry, which reports it cancelled rather than failed.
pub const FAILED_CANCELLED: i64 = 7;

/// A clause unwinds to the `handle` instance `Ctx::target` names, whose landing reads what the clause
/// left in it.
pub const FAILED_TARGET: i64 = 9;

/// A raise no clause answers in place, which `Ctx::pending` keeps for the emitted code that holds
/// the word answering it: [`rt_pending_raised`] hands it over. One never handed over fails the
/// entry with the diagnostic it keeps.
pub const FAILED_PENDING: i64 = 10;

/// The mode a compiled `perform` of a `raise` operation passes, beside 0 for a read and 1 for a
/// write.
pub(super) const MODE_RAISE: i64 = 2;

/// `abort.raise`, which the runtime itself raises for what fails on a value.
pub(super) fn is_abort(effect: &str, op: &str) -> bool {
    effect == "abort" && op == "raise"
}

/// Calls a function value the runtime holds for a handler through its descriptor. Takes the
/// arguments.
pub(crate) fn call_fun(ctx: *mut Ctx, f: Word, args: &[Word]) -> Word {
    unsafe { Block::of(f).call(ctx, args) }
}

/// One holder fewer of a function value the runtime holds for a handler.
pub(crate) fn release_fun(f: Word) {
    unsafe { Block::of(f).let_go_function(running()) };
}

/// What a runtime frame holds while it calls into the program. A frame's locals are on a stack
/// that is never unwound where its body is abandoned, so what one owned would never be let go;
/// what is kept in [`Ctx::stacks`] is let go with the stack instead.
pub(crate) enum Kept {
    /// The pending raise a `bracket`'s release runs during, and the failure it carries.
    Pending(Unanswered, Option<Diagnostic>),
    /// The arguments a clause emitted code made is called with, which it takes as it is entered.
    Args(Vec<Word>),
    /// A clause of the `handle` of the detached body at this index running, which reads the
    /// variables of that body's instance.
    Clause(usize),
}

impl Kept {
    pub(crate) fn release(self) {
        match self {
            Kept::Pending(raise, _) => raise.release(),
            // The clause took the words; only their buffer goes.
            Kept::Args(words) => drop(words),
            Kept::Clause(_) => {}
        }
    }
}

/// `clause` called with `args`, which it takes as it is entered. Their buffer is kept rather than
/// owned by this frame, so a clause abandoned at a stop inside it still gives the buffer back.
pub(crate) fn call_clause(ctx: *mut Ctx, clause: Word, args: Vec<Word>) -> Word {
    let c = unsafe { &mut *ctx };
    let at = args.as_ptr();
    let n = args.len();
    let here = c.keep(Kept::Args(args));
    let r = call_fun(ctx, clause, unsafe { std::slice::from_raw_parts(at, n) });
    unsafe { &mut *ctx }.kept_back(here).release();
    r
}

/// What a raise carries: values held with their types.
pub(crate) struct Raised(pub(crate) Vec<Carried>);

impl Raised {
    pub(crate) fn release(self) {
        let ctx = running();
        self.0.into_iter().for_each(|v| v.release(ctx));
    }

    /// The words the clause the raise is bound for is called with, which it takes.
    pub(crate) fn handed(self) -> Vec<Word> {
        self.0.into_iter().map(|v| v.handed().word()).collect()
    }
}

//! `bracket`'s runtime part, and the rejections and notes compiled code over data raises or makes.

use super::*;
use crate::native::Addr;

/// `bracket(acquire, release, body)`: `body` given what `acquire` returned and `cleanup` given it
/// after, however `body` ends: by returning, by a raise or a clause that did not resume it unwinding
/// through, or by a cancel. It runs where the bracket stands, with the handlers around it, and a
/// failure in it replaces whatever was unwinding. Each closure is let go once called. `held` is the
/// type of what `acquire` returns, which both take a count of, and `result` the type of what `body`
/// returns, let go of where `cleanup` fails. A value a clause unwinds with is abandoned with the
/// unwind, uncounted: its type is not known here. What this holds across each call is in a record
/// on the stack's chain, so a body abandoned at a stop under the call lets it go.
pub unsafe extern "C" fn rt_with_cleanup(
    ctx: *mut Ctx,
    acquire: Block,
    body: Block,
    cleanup: Block,
    held: *const PlyType,
    result: *const PlyType,
) -> Handed {
    use crate::frames::{Held, held_across};
    let c = unsafe { &mut *ctx };
    // The three share the bracket's effects: each is handed the words its caller set for it.
    let words = (c.entering_desc, c.entering_words);
    let entering = move || {
        let c = unsafe { &mut *ctx };
        (c.entering_desc, c.entering_words) = words;
    };
    c.entering_desc = 0;
    c.entering_words = 0;
    let entered = crate::tasks::shielded(c, crate::tasks::Shield::Acquire);
    let f = acquire.word();
    entering();
    let holding = [
        Held::Closure(acquire),
        Held::Closure(body),
        Held::Closure(cleanup),
    ];
    let (acquired, back) = unsafe { held_across(ctx, holding, || Block::of(f).call(ctx, &[])) };
    let [
        Held::Closure(acquire),
        Held::Closure(body),
        Held::Closure(cleanup),
    ] = back
    else {
        unreachable!("a record gives back what it held");
    };
    unsafe { acquire.let_go_function(ctx) };
    let c = unsafe { &mut *ctx };
    let cancelled = crate::tasks::unshielded(c, entered);
    if c.failed != 0 {
        if cancelled {
            cancel_carried(c);
        }
        unsafe {
            cleanup.let_go_function(ctx);
            body.let_go_function(ctx);
        }
        return Handed::of(0);
    }
    let held = unsafe { Carried::new(Handed::of(acquired), held) };
    let (returned, body, cleanup, held) = if cancelled {
        c.failed = FAILED_CANCELLED;
        (None, body, cleanup, held)
    } else {
        let given = held.dup(ctx).handed().word();
        let f = body.word();
        let holding = [
            Held::Closure(body),
            Held::Closure(cleanup),
            Held::Carried(held),
        ];
        entering();
        let (r, back) = unsafe { held_across(ctx, holding, || Block::of(f).call(ctx, &[given])) };
        let [
            Held::Closure(body),
            Held::Closure(cleanup),
            Held::Carried(held),
        ] = back
        else {
            unreachable!("a record gives back what it held");
        };
        let r = unsafe { Carried::new(Handed::of(r), result) };
        (Some(r), body, cleanup, held)
    };
    unsafe { body.let_go_function(ctx) };
    let c = unsafe { &mut *ctx };
    let ending = c.failed;
    let Some(aside) = Unwinding::set_aside(c) else {
        held.release(ctx);
        unsafe { cleanup.let_go_function(ctx) };
        return Handed::of(0);
    };
    let returned = returned.filter(|_| ending == 0);
    let (f, given) = (cleanup.word(), held.handed().word());
    let call = || {
        entering();
        unsafe { Block::of(f).call(ctx, &[given]) }
    };
    let (cleanup, returned) = match returned {
        Some(a) => {
            let (_, back) =
                unsafe { held_across(ctx, [Held::Closure(cleanup), Held::Carried(a)], call) };
            let [Held::Closure(cleanup), Held::Carried(a)] = back else {
                unreachable!("a record gives back what it held");
            };
            (cleanup, Some(a))
        }
        None => {
            let (_, back) = unsafe { held_across(ctx, [Held::Closure(cleanup)], call) };
            let [Held::Closure(cleanup)] = back else {
                unreachable!("a record gives back what it held");
            };
            (cleanup, None)
        }
    };
    unsafe { cleanup.let_go_function(ctx) };
    if !aside.taken_up(unsafe { &mut *ctx }) {
        if let Some(a) = returned {
            a.release(ctx);
        }
        return Handed::of(0);
    }
    returned.map_or(Handed::of(0), Carried::handed)
}

/// What is going on where a release runs, set aside while it runs: a `bracket` holds it across its
/// cleanup, and emitted code letting go of a resource on a failure's way out in words of its frame.
#[repr(C)]
pub(crate) struct Unwinding {
    ending: i64,
    target: Word,
    pending: Option<usize>,
    entered: Option<crate::tasks::Bracketed>,
}

/// The words emitted code sets aside in its frame for an [`Unwinding`].
pub const UNWINDING_WORDS: usize = 16;
const _: () = assert!(std::mem::size_of::<Unwinding>() <= 8 * UNWINDING_WORDS);

impl Unwinding {
    /// Sets aside what is going on where a release runs during it (nothing, a raise, a clause that
    /// did not resume, a cancel), leaving the task in a release's shield; none where the failure is
    /// the run's, which releases nothing.
    fn set_aside(c: &mut Ctx) -> Option<Unwinding> {
        let ending = c.failed;
        if ![0, FAILED_CANCELLED, FAILED_TARGET, FAILED_PENDING].contains(&ending) {
            return None;
        }
        let pending = c.pending.take().map(|raise| {
            let failure = c.diagnostic.take();
            c.keep(Kept::Pending(raise, failure))
        });
        c.failed = 0;
        let entered = crate::tasks::shielded(c, crate::tasks::Shield::Release);
        Some(Unwinding {
            ending,
            target: c.target,
            pending,
            entered,
        })
    }

    /// What [`Unwinding::set_aside`] set aside going on again once the release ran: false where the
    /// release failed or was cancelled, whose failure replaces it.
    fn taken_up(self, c: &mut Ctx) -> bool {
        let pending = self.pending.map(|here| match c.kept_back(here) {
            Kept::Pending(raise, failure) => (raise, failure),
            _ => unreachable!("a release keeps the pending raise it runs during"),
        });
        let cancelled = crate::tasks::unshielded(c, self.entered);
        if c.failed != 0 || cancelled {
            if let Some((raise, _)) = pending {
                raise.release();
            }
            if c.failed == 0 {
                c.failed = FAILED_CANCELLED;
            } else if cancelled {
                cancel_carried(c);
            }
            return false;
        }
        c.failed = self.ending;
        c.target = self.target;
        if let Some((raise, diagnostic)) = pending {
            c.pending = Some(raise);
            c.diagnostic = diagnostic;
        }
        true
    }
}

/// Sets aside the failure going on, where it is one a release runs during (a raise, a clause that
/// did not resume, a cancel), so emitted code can let go of a resource on the failure's way out:
/// returns 1, the failure written into the words at `held`, or 0 where nothing is failing or the
/// failure is the run's, which releases nothing. The release runs where the failure stands, with the
/// handlers around it, as a `bracket`'s does.
pub unsafe extern "C" fn rt_unwind_hold(ctx: *mut Ctx, held: Word) -> Word {
    let c = unsafe { &mut *ctx };
    if c.failed == 0 {
        return 0;
    }
    let Some(aside) = Unwinding::set_aside(c) else {
        return 0;
    };
    // SAFETY: emitted code hands the address of `UNWINDING_WORDS` words of its frame, which hold
    // nothing until `rt_unwind_resume` reads them back.
    unsafe { (held as usize as *mut Unwinding).write(aside) };
    1
}

/// The failure [`rt_unwind_hold`] set aside at `held` going on again once the release ran, or
/// replaced where the release failed or was cancelled.
pub unsafe extern "C" fn rt_unwind_resume(ctx: *mut Ctx, held: Word) {
    // SAFETY: `rt_unwind_hold` wrote these words, and nothing read them since.
    let aside = unsafe { (held as usize as *mut Unwinding).read() };
    aside.taken_up(unsafe { &mut *ctx });
}

/// [`cancel_here`] where what a clause unwinds with is emitted code's, which is abandoned uncounted.
fn cancel_carried(c: &mut Ctx) {
    if ![FAILED_TARGET, FAILED_PENDING].contains(&c.failed) {
        return;
    }
    if let Some(raise) = c.pending.take() {
        raise.release();
        c.diagnostic = None;
    }
    c.failed = FAILED_CANCELLED;
}

/// Fails the run with a diagnostic compiled code wrote, or raises it for `abort.raise` to handle
/// where `raises` is not 0. `texts` holds an address and a byte count for each of: the code, the
/// message, the primary label, `notes` notes, then `shown` values as PLV1 bytes. The diagnostic is
/// placed at the site the body last stored.
pub unsafe extern "C" fn rt_failed(
    ctx: *mut Ctx,
    raises: I64,
    texts: *const Word,
    notes: I64,
    shown: I64,
) {
    let c = unsafe { &mut *ctx };
    let bytes = |at: usize| {
        let (address, count) = unsafe { (*texts.add(2 * at), *texts.add(2 * at + 1)) };
        unsafe { borrowed(Addr(address as *mut u8), I64(count)) }
    };
    let text = |at: usize| String::from_utf8_lossy(bytes(at)).into_owned();
    let (notes, shown) = (notes.0 as usize, shown.0 as usize);
    let code = ply_core::intern_code(&text(0));
    let mut d = Diagnostic::error(code, text(1)).primary(Span::DUMMY, text(2));
    for at in 3..3 + notes {
        d = d.note(text(at));
    }
    let values = (3 + notes..3 + notes + shown).map(|at| bytes(at).to_vec());
    let d = d.showing(values.collect());
    if raises.0 != 0 {
        raise_error(c, d);
    } else {
        c.fail(d);
    }
}

/// Does nothing: the release's emitter still calls it in the builder it makes.
pub unsafe extern "C" fn rt_note_update(_ctx: *mut Ctx, _in_place: i64, _copied: i64) {}

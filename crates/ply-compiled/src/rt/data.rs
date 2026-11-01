use super::*;
use crate::native::Addr;

pub unsafe extern "C" fn rt_with_cleanup(
    ctx: *mut Ctx,
    acquire: Block,
    body: Block,
    cleanup: Block,
    held: *const PlyType,
    result: *const PlyType,
) -> Handed {
    use crate::frames::{Held, call_recording_slots};
    let c = unsafe { &mut *ctx };
    let words = (c.entering_desc, c.entering_words);
    let entering = move || {
        let c = unsafe { &mut *ctx };
        (c.entering_desc, c.entering_words) = words;
    };
    c.entering_desc = 0;
    c.entering_words = 0;
    let entered = crate::tasks::shielded(c, crate::tasks::BracketPhase::Acquire);
    let f = acquire.word();
    entering();
    let holding = [
        Held::Closure(acquire),
        Held::Closure(body),
        Held::Closure(cleanup),
    ];
    let (acquired, back) =
        unsafe { call_recording_slots(ctx, holding, || Block::of(f).call(ctx, &[])) };
    let [
        Held::Closure(acquire),
        Held::Closure(body),
        Held::Closure(cleanup),
    ] = back
    else {
        unreachable!("a record gives back what it held");
    };
    unsafe { acquire.release_function_if_last(ctx) };
    let c = unsafe { &mut *ctx };
    let cancelled = crate::tasks::leave_bracket(c, entered);
    if c.failed != 0 {
        if cancelled {
            cancel_pending_failure(c);
        }
        unsafe {
            cleanup.release_function_if_last(ctx);
            body.release_function_if_last(ctx);
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
        let (r, back) =
            unsafe { call_recording_slots(ctx, holding, || Block::of(f).call(ctx, &[given])) };
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
    unsafe { body.release_function_if_last(ctx) };
    let c = unsafe { &mut *ctx };
    let ending = c.failed;
    let Some(aside) = Unwinding::suspend_failure(c) else {
        held.release(ctx);
        unsafe { cleanup.release_function_if_last(ctx) };
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
            let (_, back) = unsafe {
                call_recording_slots(ctx, [Held::Closure(cleanup), Held::Carried(a)], call)
            };
            let [Held::Closure(cleanup), Held::Carried(a)] = back else {
                unreachable!("a record gives back what it held");
            };
            (cleanup, Some(a))
        }
        None => {
            let (_, back) = unsafe { call_recording_slots(ctx, [Held::Closure(cleanup)], call) };
            let [Held::Closure(cleanup)] = back else {
                unreachable!("a record gives back what it held");
            };
            (cleanup, None)
        }
    };
    unsafe { cleanup.release_function_if_last(ctx) };
    if !aside.resume_failure(unsafe { &mut *ctx }) {
        if let Some(a) = returned {
            a.release(ctx);
        }
        return Handed::of(0);
    }
    returned.map_or(Handed::of(0), Carried::handed)
}

#[repr(C)]
pub(crate) struct Unwinding {
    ending: i64,
    target: Word,
    pending: Option<usize>,
    entered: Option<crate::tasks::OpenBracket>,
}

pub const UNWINDING_WORDS: usize = 16;
const _: () = assert!(std::mem::size_of::<Unwinding>() <= 8 * UNWINDING_WORDS);

impl Unwinding {
    fn suspend_failure(c: &mut Ctx) -> Option<Unwinding> {
        let ending = c.failed;
        if ![
            0,
            FAILED_CANCELLED,
            FAILED_UNWOUND_TO_HANDLER,
            FAILED_PENDING_RAISE,
        ]
        .contains(&ending)
        {
            return None;
        }
        let pending = c.pending.take().map(|raise| {
            let failure = c.diagnostic.take();
            c.keep(Kept::Pending(raise, failure))
        });
        c.failed = 0;
        let entered = crate::tasks::shielded(c, crate::tasks::BracketPhase::Release);
        Some(Unwinding {
            ending,
            target: c.target,
            pending,
            entered,
        })
    }

    fn resume_failure(self, c: &mut Ctx) -> bool {
        let pending = self.pending.map(|here| match c.pop_saved(here) {
            Kept::Pending(raise, failure) => (raise, failure),
            _ => unreachable!("a release keeps the pending raise it runs during"),
        });
        let cancelled = crate::tasks::leave_bracket(c, self.entered);
        if c.failed != 0 || cancelled {
            if let Some((raise, _)) = pending {
                raise.release();
            }
            if c.failed == 0 {
                c.failed = FAILED_CANCELLED;
            } else if cancelled {
                cancel_pending_failure(c);
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

pub unsafe extern "C" fn rt_unwind_hold(ctx: *mut Ctx, held: Word) -> Word {
    let c = unsafe { &mut *ctx };
    if c.failed == 0 {
        return 0;
    }
    let Some(aside) = Unwinding::suspend_failure(c) else {
        return 0;
    };
    unsafe { (held as usize as *mut Unwinding).write(aside) };
    1
}

pub unsafe extern "C" fn rt_unwind_resume(ctx: *mut Ctx, held: Word) {
    let aside = unsafe { (held as usize as *mut Unwinding).read() };
    aside.resume_failure(unsafe { &mut *ctx });
}

fn cancel_pending_failure(c: &mut Ctx) {
    if ![FAILED_UNWOUND_TO_HANDLER, FAILED_PENDING_RAISE].contains(&c.failed) {
        return;
    }
    if let Some(raise) = c.pending.take() {
        raise.release();
        c.diagnostic = None;
    }
    c.failed = FAILED_CANCELLED;
}

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

pub unsafe extern "C" fn rt_note_update(_ctx: *mut Ctx, _in_place: i64, _copied: i64) {}

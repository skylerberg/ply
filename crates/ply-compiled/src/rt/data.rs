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

/// A builtin emitted code writes in place whose rejection of its operands it hands the runtime:
/// through [`rt_refused`] where the rejection names numbers, and through
/// [`rt_refused_text`](crate::machine::rt_refused_text) where it names text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rejected {
    ArrayNew,
    ColumnsNew,
    ArrayOfList,
    ColumnsOfList,
    ArrayGet,
    ArraySet,
    ColumnsGet,
    ColumnsSet,
    BytesAt,
    BytesU32Le,
    BytesSlice,
    StringSlice,
    ByteOfInt,
    BytesIndexOfByte,
    BytesIndexOfFrom,
    BytesScan,
    BytesScanUntil,
    Range,
    BytesSplit,
    StringSplit,
    StringFind,
    Assert,
    BytesPosition,
    SecretDecode,
    SecretEncode,
    SecretRsaJwk,
}

/// Every [`Rejected`], each numbered by its place here as emitted code numbers it.
pub const REJECTED: [Rejected; 26] = [
    Rejected::ArrayNew,
    Rejected::ColumnsNew,
    Rejected::ArrayOfList,
    Rejected::ColumnsOfList,
    Rejected::ArrayGet,
    Rejected::ArraySet,
    Rejected::ColumnsGet,
    Rejected::ColumnsSet,
    Rejected::BytesAt,
    Rejected::BytesU32Le,
    Rejected::BytesSlice,
    Rejected::StringSlice,
    Rejected::ByteOfInt,
    Rejected::BytesIndexOfByte,
    Rejected::BytesIndexOfFrom,
    Rejected::BytesScan,
    Rejected::BytesScanUntil,
    Rejected::Range,
    Rejected::BytesSplit,
    Rejected::StringSplit,
    Rejected::StringFind,
    Rejected::Assert,
    Rejected::BytesPosition,
    Rejected::SecretDecode,
    Rejected::SecretEncode,
    Rejected::SecretRsaJwk,
];

impl Rejected {
    /// Its name in the prelude.
    pub fn name(self) -> &'static str {
        match self {
            Rejected::ArrayNew => "Array::new",
            Rejected::ColumnsNew => "Columns::new",
            Rejected::ArrayOfList => "Array::of_list",
            Rejected::ColumnsOfList => "Columns::of_list",
            Rejected::ArrayGet => "Array::get",
            Rejected::ArraySet => "Array::set",
            Rejected::ColumnsGet => "Columns::get",
            Rejected::ColumnsSet => "Columns::set",
            Rejected::BytesAt => "Bytes::at",
            Rejected::BytesU32Le => "Bytes::u32_le",
            Rejected::BytesSlice => "Bytes::slice",
            Rejected::StringSlice => "String::slice",
            Rejected::ByteOfInt => "byte_of_int",
            Rejected::BytesIndexOfByte => "Bytes::index_of_byte",
            Rejected::BytesIndexOfFrom => "Bytes::index_of_from",
            Rejected::BytesScan => "Bytes::scan",
            Rejected::BytesScanUntil => "Bytes::scan_until",
            Rejected::Range => "range",
            Rejected::BytesSplit => "Bytes::split",
            Rejected::StringSplit => "String::split",
            Rejected::StringFind => "String::find",
            Rejected::Assert => "assert",
            Rejected::BytesPosition => "Bytes::position",
            Rejected::SecretDecode => "Secret::decode",
            Rejected::SecretEncode => "Secret::encode",
            Rejected::SecretRsaJwk => "Secret::rsa_jwk",
        }
    }

    /// Whether its rejection raises for `abort.raise` to handle, as the prelude declares it, rather
    /// than ending the run.
    pub fn raises(self) -> bool {
        !matches!(
            self,
            Rejected::ArrayOfList | Rejected::ColumnsOfList | Rejected::Range
        )
    }
}

/// The rejection [`REJECTED`] numbers `what`.
pub(crate) fn rejected_numbered(what: I64) -> Option<Rejected> {
    usize::try_from(what.0)
        .ok()
        .and_then(|at| REJECTED.get(at).copied())
}

/// The rejection `d` of `r`: raised for `abort.raise` to handle where `r` raises, else failing the
/// run.
pub(crate) fn refuse(c: &mut Ctx, r: Rejected, d: Diagnostic) {
    if r.raises() {
        raise_error(c, d);
    } else {
        c.fail(d);
    }
}

/// The entry failed as Ply's fault: emitted code handed a rejection `what` that names no builtin
/// the native rejects.
pub(crate) fn rejected_unknown(c: &mut Ctx, what: I64) {
    c.fail(
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("a refused operation numbered {}, which none is", what.0),
        )
        .note("this is Ply's fault, not the program's"),
    );
}

/// An operation emitted code writes in place, rejected numbers among its operands: left in the
/// context as [`refuse`] leaves the builtin's own rejection of them. `what` numbers the builtin as
/// [`REJECTED`] does; `a`, `b` and `c` are what its rejection names: `Array::new`, `Columns::new`,
/// `Array::of_list` and `Columns::of_list` a length `a`; `Array::get`, `Array::set`,
/// `Columns::get`, `Columns::set`, `Bytes::at` and `Bytes::u32_le` an index `a` into `b` elements or
/// bytes; `Bytes::slice` and `String::slice` a range `a..b` of `c` bytes or characters;
/// `byte_of_int` and `Bytes::index_of_byte` a byte `a`; `Bytes::index_of_from` a start `a` in `b`
/// bytes, and `Bytes::scan` and `Bytes::scan_until` that and a budget `c`; `range` its ends `a` and
/// `b`; `Bytes::split` and `String::split` nothing; `Secret::rsa_jwk` a list of `a` numbers, which
/// is not six. Any other `what` fails the entry as Ply's fault.
#[allow(improper_ctypes_definitions)]
pub unsafe extern "C" fn rt_refused(ctx: *mut Ctx, what: I64, a: I64, b: I64, c: I64) -> Raises {
    use ply_core::builtins as said;
    let cx = unsafe { &mut *ctx };
    let site = cx.site();
    let len = |n: I64| usize::try_from(n.0).unwrap_or(0);
    let Some(r) = rejected_numbered(what) else {
        rejected_unknown(cx, what);
        return Raises(());
    };
    let name = r.name();
    let rejected = match r {
        Rejected::ArrayNew | Rejected::ColumnsNew | Rejected::ArrayOfList | Rejected::ColumnsOfList => {
            said::array_length(site, name, a.0)
        }
        Rejected::ArrayGet | Rejected::ArraySet | Rejected::ColumnsGet | Rejected::ColumnsSet => {
            said::out_of_range(site, name, a.0, len(b), "elements")
        }
        Rejected::BytesAt => said::out_of_range(site, name, a.0, len(b), "bytes"),
        // Told at the last index it would read, as the builtin tells it.
        Rejected::BytesU32Le => said::out_of_range(site, name, a.0.wrapping_add(3), len(b), "bytes"),
        Rejected::BytesSlice => said::range_outside(site, name, a.0, b.0, len(c), "bytes"),
        Rejected::StringSlice => said::range_outside(site, name, a.0, b.0, len(c), "characters"),
        Rejected::ByteOfInt => said::byte_of_int_rejected(site, a.0),
        Rejected::BytesIndexOfByte => said::not_a_byte(site, name, a.0),
        Rejected::BytesIndexOfFrom | Rejected::BytesPosition => {
            said::start_outside(a.0, len(b), site, name)
        }
        Rejected::BytesScan | Rejected::BytesScanUntil => {
            if usize::try_from(a.0).is_ok_and(|from| from <= len(b)) {
                said::negative_budget(site, name, c.0)
            } else {
                said::start_outside(a.0, len(b), site, name)
            }
        }
        Rejected::Range => said::range_too_long(site, a.0, b.0),
        Rejected::BytesSplit | Rejected::StringSplit => said::empty_separator(site, name),
        Rejected::SecretRsaJwk => ply_core::crypto::misuse(
            site,
            "`Secret::rsa_jwk`",
            format!(
                "an RSA key's private numbers are six, and this list holds {}",
                a.0
            ),
        ),
        _ => {
            rejected_unknown(cx, what);
            return Raises(());
        }
    };
    refuse(cx, r, rejected);
    Raises(())
}

/// `assert_eq` found its two values unequal: raised for `abort.raise` to handle, showing the actual
/// value, the `n` PLV1 bytes at `actual`, and the expected, the `m` at `expected`, which the CLI
/// renders, noting where the two first part.
#[allow(improper_ctypes_definitions)]
pub unsafe extern "C" fn rt_assert_failed(
    ctx: *mut Ctx,
    actual: Addr,
    n: I64,
    expected: Addr,
    m: I64,
) -> Raises {
    let c = unsafe { &mut *ctx };
    let (e, a) = (ply_core::slot(0), ply_core::slot(1));
    let d = Diagnostic::error(
        codes::ASSERTION_FAILED,
        format!("assertion failed: expected {e}, found {a}"),
    )
    .primary(c.site(), "these values are not equal")
    .note(format!("expected: {e}"))
    .note(format!("actual:   {a}"))
    .showing(vec![
        unsafe { borrowed(expected, m) }.to_vec(),
        unsafe { borrowed(actual, n) }.to_vec(),
    ]);
    raise_error(c, d);
    Raises(())
}

/// A fused `iterate`'s failure: `what` 0 a budget under one, 1 the budget spent, 2 a step result
/// that is no `Iter`, which its type rules out.
pub unsafe extern "C" fn rt_iterate_bad(ctx: *mut Ctx, what: i64, n: i64) {
    let ctx = unsafe { &mut *ctx };
    match what {
        0 => raise_error(
            ctx,
            error(format!(
                "`iterate` needs a budget of at least 1, and this is {n}"
            )),
        ),
        1 => raise_error(
            ctx,
            error(format!("`iterate` did not stop within its budget of {n}")),
        ),
        _ => ctx.fail(
            error("the step given to `iterate` answered neither `Continue` nor `Stop`")
                .note("this is Ply's fault, not the program's"),
        ),
    };
}

/// A shift count outside the word; `which` indexes [`ply_core::INT_TYPES`], or is `-1` for `Int`.
pub unsafe extern "C" fn rt_shift_count(ctx: *mut Ctx, n: i64, which: i64) {
    let ctx = unsafe { &mut *ctx };
    let (ty, width) = match usize::try_from(which)
        .ok()
        .and_then(|i| ply_core::INT_TYPES.get(i))
    {
        Some(t) => (t.name(), i64::from(t.bits())),
        None => ("Int", 64),
    };
    // The machine says this on a label at the shift; compiled code has no span for one.
    let d = error("shift count out of range")
        .note(format!("{n} is not in 0..={}", width - 1))
        .note(format!(
            "a `{ty}` is {width} bits, so no other count names a shift of it"
        ));
    ctx.fail(d);
}

/// A write of one element the emitted code made: in place, or into a copy of `copied` elements.
pub unsafe extern "C" fn rt_note_update(_ctx: *mut Ctx, in_place: i64, copied: i64) {
    ply_core::rc::note_update_of(in_place != 0, copied as usize, Span::DUMMY);
}

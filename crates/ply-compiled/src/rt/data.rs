//! The runtime's part of `bracket`, and the refusals and notes compiled code over data raises or makes.

use super::*;

/// `bracket(acquire, release, body)`: `body` given what `acquire` answered and `cleanup` given it
/// after, however `body` ends: by returning, by a raise or a clause that did not resume it unwinding
/// through, or by a cancel. It runs where the bracket stands, with the handlers around it, and a
/// failure in it replaces whatever was unwinding. Each closure is let go once called. `held` is the
/// type of what `acquire` answers, which both take a count of, and `answer` the type of what `body`
/// answers, let go of where `cleanup` fails. A value a clause unwinds with is abandoned with the
/// unwind, uncounted: its type is not known here. What this holds across each call is in a record
/// on the stack's chain, so a stop under the call that is resumed more than once holds it once a
/// resume.
pub unsafe extern "C" fn rt_with_cleanup(
    ctx: *mut Ctx,
    acquire: Block,
    body: Block,
    cleanup: Block,
    held: *const PlyType,
    answer: *const PlyType,
) -> Handed {
    use crate::frames::{Held, held_across};
    let c = unsafe { &mut *ctx };
    // The three share the bracket's row: each is handed the words its caller set for it.
    let words = (c.entering_desc, c.entering_words);
    let entering = move || {
        let c = unsafe { &mut *ctx };
        (c.entering_desc, c.entering_words) = words;
    };
    c.entering_desc = 0;
    c.entering_words = 0;
    let entered = crate::simulate::shield(c, Shield::Acquire);
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
    let cancelled = crate::simulate::unshield(c, entered);
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
    let standing = c.fuel;
    let (answered, body, cleanup, held) = if cancelled {
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
        let r = unsafe { Carried::new(Handed::of(r), answer) };
        (Some(r), body, cleanup, held)
    };
    unsafe { body.let_go_function(ctx) };
    let c = unsafe { &mut *ctx };
    // `cleanup` runs as deep as the bracket stands, however deep the body was when it ended.
    c.fuel = standing;
    let ending = c.failed;
    if ending != 0
        && ending != FAILED_UNWIND
        && ending != FAILED_CANCELLED
        && ending != FAILED_ABORT
        && ending != FAILED_TARGET
        && ending != FAILED_PENDING
    {
        held.release(ctx);
        unsafe { cleanup.let_go_function(ctx) };
        return Handed::of(0);
    }
    let answered = answered.filter(|_| ending == 0);
    let unwinding = c.unwind.take();
    let raising = c.aborting.take();
    let target = c.target;
    let pending = c
        .pending
        .take()
        .map(|message| (message, c.diagnostic.take()));
    c.failed = 0;
    let entered = crate::simulate::shield(c, Shield::Release);
    let raise = raising.map(|a| c.keep(Kept::Raising(a)));
    let (f, given) = (cleanup.word(), held.handed().word());
    let call = || {
        entering();
        unsafe { Block::of(f).call(ctx, &[given]) }
    };
    let (cleanup, answered) = match answered {
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
    let c = unsafe { &mut *ctx };
    let raising = raise.map(|here| match c.kept_back(here) {
        Kept::Raising(a) => a,
        _ => unreachable!("a cleanup keeps the raise it runs during"),
    });
    let cancelled = crate::simulate::unshield(c, entered);
    if c.failed != 0 || cancelled {
        if let Some(raise) = raising {
            raise.into_failure();
        }
        if let Some(a) = answered {
            a.release(ctx);
        }
        let c = unsafe { &mut *ctx };
        if c.failed == 0 {
            c.failed = FAILED_CANCELLED;
        } else if cancelled {
            cancel_carried(c);
        }
        return Handed::of(0);
    }
    c.failed = ending;
    c.unwind = unwinding;
    c.aborting = raising;
    c.target = target;
    if let Some((message, diagnostic)) = pending {
        c.pending = Some(message);
        c.diagnostic = diagnostic;
    }
    answered.map_or(Handed::of(0), Carried::handed)
}

/// [`cancel_here`] where what a clause unwinds with is emitted code's, which is abandoned uncounted.
fn cancel_carried(c: &mut Ctx) {
    if ![FAILED_UNWIND, FAILED_ABORT, FAILED_TARGET, FAILED_PENDING].contains(&c.failed) {
        return;
    }
    c.unwind = None;
    if let Some(raise) = c.aborting.take() {
        raise.into_failure();
    }
    if c.pending.take().is_some() {
        c.diagnostic = None;
    }
    c.failed = FAILED_CANCELLED;
}

/// A builtin emitted code writes in place whose refusal of its operands it hands the runtime:
/// through [`rt_refused`] where the refusal names numbers, and through
/// [`rt_refused_text`](crate::machine::rt_refused_text) where it names text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refused {
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
}

/// Every [`Refused`], each numbered by its place here as emitted code numbers it.
pub const REFUSED: [Refused; 23] = [
    Refused::ArrayNew,
    Refused::ColumnsNew,
    Refused::ArrayOfList,
    Refused::ColumnsOfList,
    Refused::ArrayGet,
    Refused::ArraySet,
    Refused::ColumnsGet,
    Refused::ColumnsSet,
    Refused::BytesAt,
    Refused::BytesU32Le,
    Refused::BytesSlice,
    Refused::StringSlice,
    Refused::ByteOfInt,
    Refused::BytesIndexOfByte,
    Refused::BytesIndexOfFrom,
    Refused::BytesScan,
    Refused::BytesScanUntil,
    Refused::Range,
    Refused::BytesSplit,
    Refused::StringSplit,
    Refused::StringFind,
    Refused::Assert,
    Refused::BytesPosition,
];

impl Refused {
    /// Its name in the prelude.
    pub fn name(self) -> &'static str {
        match self {
            Refused::ArrayNew => "Array::new",
            Refused::ColumnsNew => "Columns::new",
            Refused::ArrayOfList => "Array::of_list",
            Refused::ColumnsOfList => "Columns::of_list",
            Refused::ArrayGet => "Array::get",
            Refused::ArraySet => "Array::set",
            Refused::ColumnsGet => "Columns::get",
            Refused::ColumnsSet => "Columns::set",
            Refused::BytesAt => "Bytes::at",
            Refused::BytesU32Le => "Bytes::u32_le",
            Refused::BytesSlice => "Bytes::slice",
            Refused::StringSlice => "String::slice",
            Refused::ByteOfInt => "byte_of_int",
            Refused::BytesIndexOfByte => "Bytes::index_of_byte",
            Refused::BytesIndexOfFrom => "Bytes::index_of_from",
            Refused::BytesScan => "Bytes::scan",
            Refused::BytesScanUntil => "Bytes::scan_until",
            Refused::Range => "range",
            Refused::BytesSplit => "Bytes::split",
            Refused::StringSplit => "String::split",
            Refused::StringFind => "String::find",
            Refused::Assert => "assert",
            Refused::BytesPosition => "Bytes::position",
        }
    }

    /// Whether its refusal raises for `abort.raise` to answer, as the prelude declares it, rather
    /// than ending the run.
    pub fn raises(self) -> bool {
        !matches!(
            self,
            Refused::ArrayOfList | Refused::ColumnsOfList | Refused::Range
        )
    }
}

/// The refusal [`REFUSED`] numbers `what`.
pub(crate) fn refused_numbered(what: I64) -> Option<Refused> {
    usize::try_from(what.0)
        .ok()
        .and_then(|at| REFUSED.get(at).copied())
}

/// The refusal `d` of `r`: raised for `abort.raise` to answer where `r` raises, else failing the
/// run.
pub(crate) fn refuse(c: &mut Ctx, r: Refused, d: Diagnostic) {
    if r.raises() {
        raise_error(c, d);
    } else {
        c.fail(d);
    }
}

/// The entry failed as Ply's fault: emitted code handed a refusal `what` that names no builtin the
/// native refuses.
pub(crate) fn refused_unknown(c: &mut Ctx, what: I64) {
    c.fail(
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("a refused operation numbered {}, which none is", what.0),
        )
        .note("this is Ply's fault, not the program's"),
    );
}

/// An operation emitted code writes in place, refused numbers among its operands: left in the
/// context as [`refuse`] leaves the builtin's own refusal of them. `what` numbers the builtin as
/// [`REFUSED`] does, and `a`, `b` and `c` are what its refusal names: `Array::new`, `Columns::new`,
/// `Array::of_list` and `Columns::of_list` a length `a`; `Array::get`, `Array::set`,
/// `Columns::get`, `Columns::set`, `Bytes::at` and `Bytes::u32_le` an index `a` into `b` elements or
/// bytes; `Bytes::slice` and `String::slice` a range `a..b` of `c` bytes or characters;
/// `byte_of_int` and `Bytes::index_of_byte` a byte `a`; `Bytes::index_of_from` a start `a` in `b`
/// bytes, and `Bytes::scan` and `Bytes::scan_until` that and a budget `c`; `range` its ends `a` and
/// `b`; `Bytes::split` and `String::split` nothing. Any other `what` fails the entry as Ply's fault.
#[allow(improper_ctypes_definitions)]
pub unsafe extern "C" fn rt_refused(ctx: *mut Ctx, what: I64, a: I64, b: I64, c: I64) -> Raises {
    use ply_core::builtins as said;
    let cx = unsafe { &mut *ctx };
    let site = cx.site();
    let len = |n: I64| usize::try_from(n.0).unwrap_or(0);
    let Some(r) = refused_numbered(what) else {
        refused_unknown(cx, what);
        return Raises(());
    };
    let name = r.name();
    let refused = match r {
        Refused::ArrayNew | Refused::ColumnsNew | Refused::ArrayOfList | Refused::ColumnsOfList => {
            said::array_length(site, name, a.0)
        }
        Refused::ArrayGet | Refused::ArraySet | Refused::ColumnsGet | Refused::ColumnsSet => {
            said::out_of_range(site, name, a.0, len(b), "elements")
        }
        Refused::BytesAt => said::out_of_range(site, name, a.0, len(b), "bytes"),
        // Told at the last index it would read, as the builtin tells it.
        Refused::BytesU32Le => said::out_of_range(site, name, a.0.wrapping_add(3), len(b), "bytes"),
        Refused::BytesSlice => said::range_outside(site, name, a.0, b.0, len(c), "bytes"),
        Refused::StringSlice => said::range_outside(site, name, a.0, b.0, len(c), "characters"),
        Refused::ByteOfInt => said::byte_of_int_refused(site, a.0),
        Refused::BytesIndexOfByte => said::not_a_byte(site, name, a.0),
        Refused::BytesIndexOfFrom | Refused::BytesPosition => {
            said::start_outside(a.0, len(b), site, name)
        }
        Refused::BytesScan | Refused::BytesScanUntil => {
            if usize::try_from(a.0).is_ok_and(|from| from <= len(b)) {
                said::negative_budget(site, name, c.0)
            } else {
                said::start_outside(a.0, len(b), site, name)
            }
        }
        Refused::Range => said::range_too_long(site, a.0, b.0),
        Refused::BytesSplit | Refused::StringSplit => said::empty_separator(site, name),
        _ => {
            refused_unknown(cx, what);
            return Raises(());
        }
    };
    refuse(cx, r, refused);
    Raises(())
}

/// A fused `iterate`'s failure: `what` 0 a budget under one, 1 the budget spent, 2 a step answer
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

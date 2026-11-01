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

    pub fn raises(self) -> bool {
        !matches!(
            self,
            Rejected::ArrayOfList | Rejected::ColumnsOfList | Rejected::Range
        )
    }
}

pub(crate) fn rejected_operation_from_id(what: I64) -> Option<Rejected> {
    usize::try_from(what.0)
        .ok()
        .and_then(|at| REJECTED.get(at).copied())
}

pub(crate) fn refuse(c: &mut Ctx, r: Rejected, d: Diagnostic) {
    if r.raises() {
        raise_error(c, d);
    } else {
        c.fail(d);
    }
}

pub(crate) fn fail_unknown_rejected_operation(c: &mut Ctx, what: I64) {
    c.fail(
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("a rejected operation numbered {}, which none is", what.0),
        )
        .note("this is Ply's fault, not the program's"),
    );
}

#[allow(improper_ctypes_definitions)]
pub unsafe extern "C" fn rt_refused(ctx: *mut Ctx, what: I64, a: I64, b: I64, c: I64) -> Raises {
    use ply_core::builtins as said;
    let cx = unsafe { &mut *ctx };
    let site = cx.site();
    let len = |n: I64| usize::try_from(n.0).unwrap_or(0);
    let Some(r) = rejected_operation_from_id(what) else {
        fail_unknown_rejected_operation(cx, what);
        return Raises(());
    };
    let name = r.name();
    let rejected = match r {
        Rejected::ArrayNew
        | Rejected::ColumnsNew
        | Rejected::ArrayOfList
        | Rejected::ColumnsOfList => said::array_length(site, name, a.0),
        Rejected::ArrayGet | Rejected::ArraySet | Rejected::ColumnsGet | Rejected::ColumnsSet => {
            said::out_of_range(site, name, a.0, len(b), "elements")
        }
        Rejected::BytesAt => said::out_of_range(site, name, a.0, len(b), "bytes"),
        Rejected::BytesU32Le => {
            said::out_of_range(site, name, a.0.wrapping_add(3), len(b), "bytes")
        }
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
            fail_unknown_rejected_operation(cx, what);
            return Raises(());
        }
    };
    refuse(cx, r, rejected);
    Raises(())
}

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
            error("the step given to `iterate` returned neither `Continue` nor `Stop`")
                .note("this is Ply's fault, not the program's"),
        ),
    };
}

pub unsafe extern "C" fn rt_shift_count(ctx: *mut Ctx, n: i64, which: i64) {
    let ctx = unsafe { &mut *ctx };
    let (ty, width) = match usize::try_from(which)
        .ok()
        .and_then(|i| ply_core::INT_TYPES.get(i))
    {
        Some(t) => (t.name(), i64::from(t.bits())),
        None => ("Int", 64),
    };
    let d = error("shift count out of range")
        .note(format!("{n} is not in 0..={}", width - 1))
        .note(format!(
            "a `{ty}` is {width} bits, so no other count names a shift of it"
        ));
    ctx.fail(d);
}

pub unsafe extern "C" fn rt_note_update(_ctx: *mut Ctx, in_place: i64, copied: i64) {
    ply_core::rc::record_update(in_place != 0, copied as usize, Span::DUMMY);
}

//! What an operation emitted code writes in place leaves in the context when it hands the runtime
//! its refusal of its operands: the builtin's own refusal of them, word for word, raised where the
//! builtin raises, which `abort.raise` answers, and failing the run where it fails.

use crate::bare::{context_of, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::{Handed, PlyFnDesc, PlyType};
use ply_compiled::heap::Word;
use ply_compiled::rt::{Ctx, FAILED_ABORT};
use ply_core::builtins::call;
use ply_core::{Builtin, Diagnostic, Span, Value, codes};

type Refused = unsafe extern "C" fn(*mut Ctx, Word, Word, Word, Word);
type RefusedText = unsafe extern "C" fn(*mut Ctx, Word, *const u8, Word, *const u8, Word);

/// The diagnostic `refuse` leaves in a fresh context.
fn left(refuse: impl FnOnce(&mut Ctx)) -> Diagnostic {
    let mut ctx = context_of(&[], Vec::new());
    ctx.begin(1_000);
    refuse(&mut ctx);
    let d = ctx.take_failure().expect("a diagnostic");
    ctx.failed = 0;
    ctx.end();
    d
}

/// What `?refused` makes of `what` and the numbers `a`, `b` and `c`.
fn refused(what: Word, a: Word, b: Word, c: Word) -> Diagnostic {
    left(|ctx| unsafe { helper::<Refused>(&builtin_helper_name("?refused"))(ctx, what, a, b, c) })
}

/// What `?refused_text` makes of `what` and the texts `p` and `q`, `p` none where it is `None`.
fn refused_text(what: Word, p: Option<&[u8]>, q: &[u8]) -> Diagnostic {
    let (at, n) = p.map_or((std::ptr::null(), -1), |p| (p.as_ptr(), p.len() as Word));
    left(|ctx| unsafe {
        helper::<RefusedText>(&builtin_helper_name("?refused_text"))(
            ctx,
            what,
            at,
            n,
            q.as_ptr(),
            q.len() as Word,
        )
    })
}

/// What builtin `b` refuses `args` with, over values.
fn builtin_refusal(b: Builtin, args: Vec<Value>) -> Diagnostic {
    match call(b, args, Span::DUMMY) {
        Err(d) => d,
        Ok(v) => panic!("`{}` answered {v:?}", b.name()),
    }
}

/// What a diagnostic says, apart from where.
fn said(d: &Diagnostic) -> String {
    format!("{} {} {:?} {:?}", d.code, d.message, d.notes, d.values)
}

#[test]
fn a_length_refused_raises_as_the_operation_of_that_length_does() {
    for (what, name, n) in [
        (0, "Array::new", -1),
        (1, "Columns::new", 1 << 27),
        (2, "Array::of_list", (1 << 26) + 1),
        (3, "Columns::of_list", (1 << 26) + 1),
    ] {
        assert_eq!(
            refused(what, n, 0, 0).message,
            format!("`{name}` of {n} elements is outside `0` to `67108864`")
        );
    }
}

#[test]
fn an_index_refused_raises_as_the_operation_of_that_index_does() {
    for (what, name, n) in [
        (4, "Array::get", 3),
        (5, "Array::set", -1),
        (6, "Columns::get", 7),
        (7, "Columns::set", 3),
    ] {
        assert_eq!(
            refused(what, n, 3, 0).message,
            format!("`{name}` index {n} is outside a value of 3 elements")
        );
    }
}

#[test]
fn a_string_operation_refused_numbers_says_what_its_builtin_says_over_values() {
    let abc = || Value::bytes(*b"abc");
    for (what, numbers, b, args) in [
        (8, [5, 3, 0], Builtin::BytesAt, vec![abc(), Value::Int(5)]),
        (8, [-1, 3, 0], Builtin::BytesAt, vec![abc(), Value::Int(-1)]),
        (
            9,
            [0, 3, 0],
            Builtin::BytesU32Le,
            vec![abc(), Value::Int(0)],
        ),
        (
            10,
            [2, 1, 3],
            Builtin::BytesSlice,
            vec![abc(), Value::Int(2), Value::Int(1)],
        ),
        (
            11,
            [1, 9, 5],
            Builtin::StringSlice,
            vec![Value::str("héllo"), Value::Int(1), Value::Int(9)],
        ),
        (12, [256, 0, 0], Builtin::ByteOfInt, vec![Value::Int(256)]),
        (
            13,
            [-1, 0, 0],
            Builtin::BytesIndexOfByte,
            vec![abc(), Value::Int(-1)],
        ),
        (
            14,
            [4, 3, 0],
            Builtin::BytesIndexOfFrom,
            vec![abc(), Value::bytes(*b""), Value::Int(4)],
        ),
        (
            15,
            [9, 3, 1],
            Builtin::BytesScan,
            vec![abc(), Value::Int(9), Value::bytes(*b"a"), Value::Int(1)],
        ),
        (
            16,
            [0, 3, -1],
            Builtin::BytesScanUntil,
            vec![abc(), Value::Int(0), Value::bytes(*b"a"), Value::Int(-1)],
        ),
        (
            17,
            [0, 20_000_000, 0],
            Builtin::Range,
            vec![Value::Int(0), Value::Int(20_000_000)],
        ),
        (
            17,
            [i64::MIN, i64::MAX, 0],
            Builtin::Range,
            vec![Value::Int(i64::MIN), Value::Int(i64::MAX)],
        ),
        (
            18,
            [0, 0, 0],
            Builtin::BytesSplit,
            vec![abc(), Value::bytes(*b"")],
        ),
        (
            19,
            [0, 0, 0],
            Builtin::StringSplit,
            vec![Value::str("abc"), Value::str("")],
        ),
        (
            23,
            [9, 3, 0],
            Builtin::BytesPosition,
            vec![abc(), Value::Int(9), Value::Int(0)],
        ),
    ] {
        let [a, b2, c] = numbers;
        assert_eq!(
            said(&refused(what, a, b2, c)),
            said(&builtin_refusal(b, args)),
            "`{}`",
            b.name()
        );
    }
}

#[test]
fn a_string_operation_refused_text_says_what_its_builtin_says_over_values() {
    let none = || Value::ctor("None", Vec::new());
    let some = |s: &str| Value::ctor("Some", vec![Value::str(s)]);
    for (what, p, q, b, args) in [
        (
            20,
            Some(&b"abc"[..]),
            &b"z"[..],
            Builtin::StringFind,
            vec![Value::str("abc"), Value::str("z")],
        ),
        (
            21,
            Some(&b"a\xffb"[..]),
            &b""[..],
            Builtin::StringOfBytes,
            vec![Value::bytes(*b"a\xffb")],
        ),
        (
            21,
            Some(&b"a\xe2\x82"[..]),
            &b""[..],
            Builtin::StringOfBytes,
            vec![Value::bytes(*b"a\xe2\x82")],
        ),
        (
            22,
            None,
            &b""[..],
            Builtin::Assert,
            vec![Value::Bool(false), none()],
        ),
        (
            22,
            Some(&b"why"[..]),
            &b""[..],
            Builtin::Assert,
            vec![Value::Bool(false), some("why")],
        ),
    ] {
        assert_eq!(
            said(&refused_text(what, p, q)),
            said(&builtin_refusal(b, args)),
            "`{}`",
            b.name()
        );
    }
}

#[test]
fn an_operation_no_number_names_fails_the_entry_as_plys_fault() {
    for d in [
        refused(24, 0, 0, 0),
        refused(-1, 0, 0, 0),
        refused(20, 0, 0, 0),
        refused_text(8, None, b""),
    ] {
        assert_eq!(d.code, codes::INTERNAL_ERROR);
        assert!(d.notes.iter().any(|n| n.contains("Ply's fault")));
    }
}

/// A `String` read and dropped: what the runtime lifts its message through.
unsafe extern "C" fn read_text(ctx: *mut Ctx) -> Word {
    let text: unsafe extern "C" fn(*mut Ctx) -> Word = helper(&builtin_helper_name("?in_str"));
    let taken: unsafe extern "C" fn(*mut Ctx, *mut u8, Word) =
        helper(&builtin_helper_name("?taken"));
    let n = unsafe { text(ctx) };
    let mut b = vec![0u8; n as usize];
    unsafe { taken(ctx, b.as_mut_ptr(), n) };
    0
}

unsafe extern "C" fn unread(_ctx: *mut Ctx, _w: Word) -> Word {
    0
}

static STRING_T: PlyType = PlyType {
    identity: 1,
    spelled: b"String".as_ptr(),
    spelled_len: 6,
    inc: unread,
    dec: unread,
    lower: unread,
    lift: read_text,
};

unsafe extern "C" fn never_called(_ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    0
}

unsafe extern "C" fn stays(_ctx: *mut Ctx, _w: Word) -> Word {
    0
}

unsafe extern "C" fn released(_ctx: *mut Ctx, _f: Handed) -> Word {
    0
}

static CLAUSE: PlyFnDesc = PlyFnDesc {
    uniform: never_called,
    typed: None,
    convention: 0,
    release: released,
    share: stays,
    keep: stays,
};

/// A static closure over nothing: an immortal count, then the descriptor's address.
#[repr(C)]
struct Static {
    rc: u32,
    header: [u32; 3],
    desc: &'static PlyFnDesc,
}

static CLOSURE: Static = Static {
    rc: u32::MAX,
    header: [0; 3],
    desc: &CLAUSE,
};

/// How the context failed once `refuse` ran under a `handle` with an `abort.raise` clause.
fn failed_under_a_clause(refuse: impl FnOnce(*mut Ctx)) -> i64 {
    let mut ctx = context_of(&["abort", "raise"], Vec::new());
    ctx.begin(1_000);
    let c: *mut Ctx = &mut ctx;
    let clause = [
        0,
        -1,
        1,
        &raw const CLOSURE as Word,
        3,
        &raw const STRING_T as Word,
    ];
    let install: unsafe extern "C" fn(*mut Ctx, *const Word, Word, Word) -> Word =
        helper("rt_handle_install");
    let land: unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word = helper("rt_handle_land");
    let failed = unsafe {
        let depth = install(c, clause.as_ptr(), 1, 0);
        refuse(c);
        let failed = (*c).failed;
        land(c, depth, 0);
        if failed == FAILED_ABORT {
            assert_eq!((*c).failed, 0, "the clause answered the raise");
        }
        failed
    };
    let _ = ctx.take_failure();
    ctx.failed = 0;
    ctx.end();
    failed
}

#[test]
fn a_refusal_is_a_raise_where_its_builtin_raises_and_fails_the_run_where_it_fails() {
    let numbers = |what: Word, a: Word, b: Word| {
        failed_under_a_clause(|c| unsafe {
            helper::<Refused>(&builtin_helper_name("?refused"))(c, what, a, b, 0)
        })
    };
    assert_eq!(numbers(4, 9, 2), FAILED_ABORT, "`Array::get` raises");
    assert_eq!(numbers(8, 9, 2), FAILED_ABORT, "`Bytes::at` raises");
    assert_eq!(numbers(17, 0, 20_000_000), 1, "`range` fails the run");
    let text = failed_under_a_clause(|c| unsafe {
        helper::<RefusedText>(&builtin_helper_name("?refused_text"))(
            c,
            21,
            b"\xff".as_ptr(),
            1,
            std::ptr::null(),
            0,
        )
    });
    assert_eq!(text, FAILED_ABORT, "`String::of_bytes` raises");
}

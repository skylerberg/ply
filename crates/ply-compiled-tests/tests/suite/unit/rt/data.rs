//! What an operation emitted code writes in place leaves in the context when it hands the runtime
//! its refusal of its operands: raised where the operation raises, which `abort.raise` answers, and
//! failing the run where it fails.

use crate::bare::{context_of, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::PlyType;
use ply_compiled::heap::Word;
use ply_compiled::rt::{Ctx, FAILED_PENDING};
use ply_core::{Diagnostic, codes};

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
fn an_operation_no_number_names_fails_the_entry_as_plys_fault() {
    for d in [
        refused(23, 0, 0, 0),
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

type Pending = unsafe extern "C" fn(*mut Ctx, *const PlyType, Word) -> Word;

/// How the context failed once `refuse` ran, a raise it left pending taken as code holding a clause
/// for `abort.raise` takes it.
fn failed_under_a_clause(refuse: impl FnOnce(*mut Ctx)) -> i64 {
    let mut ctx = context_of(&["abort", "raise"], Vec::new());
    ctx.begin(1_000);
    let c: *mut Ctx = &mut ctx;
    let failed = unsafe {
        refuse(c);
        let failed = (*c).failed;
        if failed == FAILED_PENDING {
            helper::<Pending>("rt_pending_raised")(c, &STRING_T, 0);
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
    // A raise is left pending for the code around the builtin, which a clause for it takes.
    assert_eq!(numbers(4, 9, 2), FAILED_PENDING, "`Array::get` raises");
    assert_eq!(numbers(8, 9, 2), FAILED_PENDING, "`Bytes::at` raises");
    assert_eq!(numbers(17, 0, 20_000_000), 1, "`range` fails the run");
    let text = failed_under_a_clause(|c| unsafe {
        helper::<RefusedText>(&builtin_helper_name("?refused_text"))(
            c,
            20,
            b"abc".as_ptr(),
            3,
            b"z".as_ptr(),
            1,
        )
    });
    assert_eq!(text, FAILED_PENDING, "`String::find` raises");
}

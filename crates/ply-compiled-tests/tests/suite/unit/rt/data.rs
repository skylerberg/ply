use crate::bare::{context_of, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::PlyType;
use ply_compiled::heap::Word;
use ply_compiled::rt::{Ctx, FAILED_PENDING_RAISE};
use ply_core::{Diagnostic, codes};

type Rejected = unsafe extern "C" fn(*mut Ctx, Word, Word, Word, Word);
type RejectedText = unsafe extern "C" fn(*mut Ctx, Word, *const u8, Word, *const u8, Word);

fn left(refuse: impl FnOnce(&mut Ctx)) -> Diagnostic {
    let mut ctx = context_of(&[], Vec::new());
    ctx.begin();
    refuse(&mut ctx);
    let d = ctx.take_failure().expect("a diagnostic");
    ctx.failed = 0;
    ctx.end();
    d
}

fn rejected(what: Word, a: Word, b: Word, c: Word) -> Diagnostic {
    left(|ctx| unsafe { helper::<Rejected>(&builtin_helper_name("?refused"))(ctx, what, a, b, c) })
}

fn rejected_text(what: Word, p: Option<&[u8]>, q: &[u8]) -> Diagnostic {
    let (at, n) = p.map_or((std::ptr::null(), -1), |p| (p.as_ptr(), p.len() as Word));
    left(|ctx| unsafe {
        helper::<RejectedText>(&builtin_helper_name("?refused_text"))(
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
fn a_rejected_length_fails_with_a_message_naming_the_operation_and_the_allowed_range() {
    for (what, name, n) in [
        (0, "Array::new", -1),
        (1, "Columns::new", 1 << 27),
        (2, "Array::of_list", (1 << 26) + 1),
        (3, "Columns::of_list", (1 << 26) + 1),
    ] {
        assert_eq!(
            rejected(what, n, 0, 0).message,
            format!("`{name}` of {n} elements is outside `0` to `67108864`")
        );
    }
}

#[test]
fn a_rejected_index_fails_with_a_message_naming_the_operation_and_the_size_of_the_value() {
    for (what, name, n) in [
        (4, "Array::get", 3),
        (5, "Array::set", -1),
        (6, "Columns::get", 7),
        (7, "Columns::set", 3),
    ] {
        assert_eq!(
            rejected(what, n, 3, 0).message,
            format!("`{name}` index {n} is outside a value of 3 elements")
        );
    }
}

#[test]
fn an_unknown_operation_number_fails_the_entry_as_an_internal_error() {
    for d in [
        rejected(23, 0, 0, 0),
        rejected(-1, 0, 0, 0),
        rejected(20, 0, 0, 0),
        rejected_text(8, None, b""),
    ] {
        assert_eq!(d.code, codes::INTERNAL_ERROR);
        assert!(d.notes.iter().any(|n| n.contains("Ply's fault")));
    }
}

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

fn failed_under_a_clause(refuse: impl FnOnce(*mut Ctx)) -> i64 {
    let mut ctx = context_of(&["abort", "raise"], Vec::new());
    ctx.begin();
    let c: *mut Ctx = &mut ctx;
    let failed = unsafe {
        refuse(c);
        let failed = (*c).failed;
        if failed == FAILED_PENDING_RAISE {
            helper::<Pending>("rt_pending_raised")(c, &STRING_T, 0);
            assert_eq!((*c).failed, 0, "the clause handled the raise");
        }
        failed
    };
    let _ = ctx.take_failure();
    ctx.failed = 0;
    ctx.end();
    failed
}

#[test]
fn a_rejected_argument_raises_a_catchable_effect_for_array_get_bytes_at_and_string_find_but_range_fails_the_run()
 {
    let numbers = |what: Word, a: Word, b: Word| {
        failed_under_a_clause(|c| unsafe {
            helper::<Rejected>(&builtin_helper_name("?refused"))(c, what, a, b, 0)
        })
    };
    assert_eq!(
        numbers(4, 9, 2),
        FAILED_PENDING_RAISE,
        "`Array::get` raises"
    );
    assert_eq!(numbers(8, 9, 2), FAILED_PENDING_RAISE, "`Bytes::at` raises");
    assert_eq!(numbers(17, 0, 20_000_000), 1, "`range` fails the run");
    let text = failed_under_a_clause(|c| unsafe {
        helper::<RejectedText>(&builtin_helper_name("?refused_text"))(
            c,
            20,
            b"abc".as_ptr(),
            3,
            b"z".as_ptr(),
            1,
        )
    });
    assert_eq!(text, FAILED_PENDING_RAISE, "`String::find` raises");
}

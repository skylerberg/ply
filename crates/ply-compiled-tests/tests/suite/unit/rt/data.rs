//! What an array's or a columns value's operation emitted code writes raises when it is refused a
//! length or an index: the runtime's own operation's raise, word for word, which `abort.raise`
//! answers.

use crate::bare::{context_of, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::{Handed, PlyFnDesc, PlyType};
use ply_compiled::heap::Word;
use ply_compiled::rt::{Ctx, FAILED_ABORT};
use ply_core::codes;

type Bad = unsafe extern "C" fn(*mut Ctx, Word, Word, Word);

/// What `?array_bad` makes of `what`, `n` and `len`: how the context failed, and its message.
fn refused(what: Word, n: Word, len: Word) -> (i64, String) {
    let mut ctx = context_of(&[], Vec::new());
    ctx.begin(1_000);
    unsafe { helper::<Bad>(&builtin_helper_name("?array_bad"))(&mut ctx, what, n, len) };
    let failed = ctx.failed;
    let d = ctx.take_failure().expect("a diagnostic");
    ctx.failed = 0;
    ctx.end();
    (failed, d.message)
}

#[test]
fn a_length_refused_raises_as_the_operation_of_that_length_does() {
    for (what, name, n) in [
        (0, "Array::new", -1),
        (1, "Columns::new", 1 << 27),
        (2, "Array::of_list", (1 << 26) + 1),
        (3, "Columns::of_list", (1 << 26) + 1),
    ] {
        let (_, said) = refused(what, n, 0);
        assert_eq!(
            said,
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
        let (_, said) = refused(what, n, 3);
        assert_eq!(
            said,
            format!("`{name}` index {n} is outside a value of 3 elements")
        );
    }
}

#[test]
fn an_operation_no_number_names_fails_the_entry_as_plys_fault() {
    let mut ctx = context_of(&[], Vec::new());
    ctx.begin(1_000);
    unsafe { helper::<Bad>(&builtin_helper_name("?array_bad"))(&mut ctx, 8, 0, 0) };
    let d = ctx.take_failure().expect("a diagnostic");
    assert_eq!(d.code, codes::INTERNAL_ERROR);
    assert!(d.message.contains("numbered 8"), "{}", d.message);
    assert!(d.notes.iter().any(|n| n.contains("Ply's fault")));
    ctx.failed = 0;
    ctx.end();
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

#[test]
fn a_refusal_is_a_raise_an_abort_raise_clause_answers() {
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
    unsafe {
        let depth = install(c, clause.as_ptr(), 1, 0);
        helper::<Bad>(&builtin_helper_name("?array_bad"))(c, 4, 9, 2);
        assert_eq!(
            ctx.failed, FAILED_ABORT,
            "bound for the `abort.raise` clause"
        );
        land(c, depth, 0);
    }
    assert_eq!(ctx.failed, 0, "the clause answered the raise");
    ctx.end();
}

use crate::bare::{context_of, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::PlyType;
use ply_compiled::heap::Word;
use ply_compiled::rt::{Ctx, FAILED_PENDING_RAISE};
use ply_core::{Diagnostic, codes};

fn left(refuse: impl FnOnce(&mut Ctx)) -> Diagnostic {
    let mut ctx = context_of(&[], Vec::new());
    ctx.begin();
    refuse(&mut ctx);
    let d = ctx.take_failure().expect("a diagnostic");
    ctx.failed = 0;
    ctx.end();
    d
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

type Failed = unsafe extern "C" fn(*mut Ctx, Word, *const Word, Word, Word);

fn failed(ctx: *mut Ctx, raises: Word, texts: &[&[u8]], notes: Word, shown: Word) {
    let words: Vec<Word> = texts
        .iter()
        .flat_map(|t| [t.as_ptr() as Word, t.len() as Word])
        .collect();
    unsafe { helper::<Failed>("rt_failed")(ctx, raises, words.as_ptr(), notes, shown) }
}

#[test]
fn a_failure_compiled_code_wrote_keeps_its_code_text_notes_and_values() {
    let shown = b"PLV1\x00";
    let d = left(|ctx| {
        let texts: [&[u8]; 6] = [b"E0501", b"it failed", b"here", b"one", b"two", shown];
        failed(ctx, 0, &texts, 2, 1);
    });
    assert_eq!(d.code, codes::ASSERTION_FAILED);
    assert_eq!(d.message, "it failed");
    assert_eq!(d.labels[0].message, "here");
    assert_eq!(d.notes, ["one", "two"]);
    assert_eq!(d.values.iter().collect::<Vec<_>>(), [&shown.to_vec()]);
}

#[test]
fn a_failure_compiled_code_wrote_raises_only_where_it_says_to() {
    let texts: [&[u8]; 3] = [b"E0502", b"it failed", b"here"];
    assert_eq!(
        failed_under_a_clause(|c| failed(c, 1, &texts, 0, 0)),
        FAILED_PENDING_RAISE
    );
    assert_eq!(failed_under_a_clause(|c| failed(c, 0, &texts, 0, 0)), 1);
}

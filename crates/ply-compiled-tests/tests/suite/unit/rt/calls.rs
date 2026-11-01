use crate::bare::{context_of, helper};
use ply_compiled::heap::{HEADER, IMMORTAL, Word};
use ply_compiled::rt::{Ctx, FAILED};
use std::sync::atomic::Ordering;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Keep = unsafe extern "C" fn(*mut Ctx, Word, Word);
type ConstFailed = unsafe extern "C" fn(*mut Ctx);
type Definition = unsafe extern "C" fn(*mut Ctx, *const Word) -> Word;

const BOX: Word = HEADER as Word + 8;

fn boxed(ctx: *mut Ctx, n: Word) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, BOX) };
    unsafe { *(w as *mut Word).byte_add(HEADER) = n };
    w
}

fn count(w: Word) -> u32 {
    unsafe { *(w as *const u32) }
}

fn let_go(ctx: *mut Ctx, w: Word) {
    if count(w) != IMMORTAL {
        unsafe { helper::<Free>("rt_free")(ctx, w, BOX) };
    }
}

fn defining(f: Definition) -> Ctx {
    let mut ctx = context_of(&[], vec![f as usize]);
    ctx.begin();
    ctx
}

unsafe extern "C" fn nothing(_ctx: *mut Ctx, _args: *const Word) -> Word {
    0
}

fn kept_at(ctx: &Ctx, index: usize) -> i64 {
    unsafe { (*ctx.constants.add(index)).load(Ordering::Acquire) }
}

#[test]
fn the_first_copy_kept_stays_for_every_later_read() {
    let mut ctx = defining(nothing);
    let c: *mut Ctx = &mut ctx;
    let keep = helper::<Keep>("rt_keep_constant");
    unsafe { keep(c, 0, 11) };
    unsafe { keep(c, 0, 22) };
    assert_eq!(kept_at(&ctx, 0), 11);
    ctx.end();
}

#[test]
fn a_copy_of_a_constant_outside_the_table_is_kept_nowhere() {
    let mut ctx = defining(nothing);
    let c: *mut Ctx = &mut ctx;
    let keep = helper::<Keep>("rt_keep_constant");
    unsafe { keep(c, 3, 11) };
    unsafe { keep(c, -1, 11) };
    assert_eq!(kept_at(&ctx, 0), 0);
    ctx.end();
}

type Entered = unsafe extern "C" fn(*mut Ctx) -> Word;
type Left = unsafe extern "C" fn(*mut Ctx, Word);

#[test]
fn the_scheduler_blocks_stay_separate_from_the_program_blocks_at_any_nesting_depth() {
    let mut ctx = context_of(&[], Vec::new());
    ctx.begin();
    let c: *mut Ctx = &mut ctx;
    let (entered, left) = (
        helper::<Entered>("rt_scheduler_entered"),
        helper::<Left>("rt_scheduler_left"),
    );
    let mine = boxed(c, 1);
    let before = ctx.live();
    let outer = unsafe { entered(c) };
    let inner = unsafe { entered(c) };
    let theirs = boxed(c, 2);
    unsafe { left(c, inner) };
    let nested = boxed(c, 3);
    assert_eq!(
        ctx.live(),
        before,
        "a nested entry leaves the scheduler running"
    );
    unsafe { left(c, outer) };
    assert_eq!((outer, inner), (0, 1));
    let_go(c, nested);
    let_go(c, theirs);
    let_go(c, mine);
    ctx.end();
}

unsafe extern "C" fn failing(ctx: *mut Ctx, _args: *const Word) -> Word {
    let raised: unsafe extern "C" fn(*mut Ctx, Word, *const Word, Word, Word) = helper("rt_failed");
    let texts = [b"E0502".as_slice(), b"it failed", b"here"];
    let words: Vec<Word> = texts
        .iter()
        .flat_map(|t| [t.as_ptr() as Word, t.len() as Word])
        .collect();
    unsafe { raised(ctx, 1, words.as_ptr(), 0, 0) };
    0
}

#[test]
fn an_effect_raised_inside_a_constant_fails_the_run_with_an_error_that_no_handler_can_catch() {
    let mut ctx = defining(nothing);
    let c: *mut Ctx = &mut ctx;
    assert_eq!(unsafe { failing(c, std::ptr::null()) }, 0);
    unsafe { helper::<ConstFailed>("rt_const_failed")(c) };
    assert_eq!(ctx.failed, FAILED, "a plain failure, no raise pending");
    let said = ctx.take_failure().expect("a diagnostic");
    assert!(
        said.notes
            .iter()
            .any(|n| n.contains("a constant must return")),
        "{said:?}"
    );
    ctx.end();
}

#[test]
fn a_failure_that_is_no_raise_stands_as_it_is() {
    let mut ctx = defining(nothing);
    let c: *mut Ctx = &mut ctx;
    ctx.failed = FAILED;
    unsafe { helper::<ConstFailed>("rt_const_failed")(c) };
    assert_eq!(ctx.failed, FAILED);
    ctx.failed = 0;
    ctx.end();
}

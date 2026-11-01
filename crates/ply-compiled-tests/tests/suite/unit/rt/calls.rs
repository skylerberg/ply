use crate::bare::{context_of, helper};
use ply_compiled::emitted::KeepRoot;
use ply_compiled::heap::{HEADER, IMMORTAL, Word};
use ply_compiled::rt::{Ctx, FAILED, Tables};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Memo = unsafe extern "C" fn(*mut Ctx, Word, KeepRoot) -> Word;
type Definition = unsafe extern "C" fn(*mut Ctx, *const Word) -> Word;

const BOX: Word = HEADER as Word + 8;

fn boxed(ctx: *mut Ctx, n: Word) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, BOX) };
    unsafe { *(w as *mut Word).byte_add(HEADER) = n };
    w
}

fn held(w: Word) -> Word {
    unsafe { *(w as *const Word).byte_add(HEADER) }
}

fn count(w: Word) -> u32 {
    unsafe { *(w as *const u32) }
}

fn let_go(ctx: *mut Ctx, w: Word) {
    if count(w) != IMMORTAL {
        unsafe { helper::<Free>("rt_free")(ctx, w, BOX) };
    }
}

unsafe extern "C" fn keep_box(ctx: *mut Ctx, w: Word) -> Word {
    let kept = unsafe { helper::<Alloc>("rt_alloc_kept")(ctx, BOX) };
    unsafe { *(kept as *mut Word).byte_add(HEADER) = held(w) };
    kept
}

unsafe extern "C" fn keeps_nothing(_ctx: *mut Ctx, _w: Word) -> Word {
    0
}

fn read(ctx: *mut Ctx, keep: KeepRoot) -> Word {
    unsafe { helper::<Memo>("rt_memo")(ctx, 0, keep) }
}

fn defining(f: Definition) -> Ctx {
    let mut ctx = context_of(&[], vec![f as usize]);
    ctx.begin();
    ctx
}

static FORTY_TWOS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn forty_two(ctx: *mut Ctx, _args: *const Word) -> Word {
    FORTY_TWOS.fetch_add(1, Ordering::SeqCst);
    boxed(ctx, 42)
}

#[test]
fn the_first_read_evaluates_and_caches_and_each_later_one_returns_the_copy() {
    let mut ctx = defining(forty_two);
    let c: *mut Ctx = &mut ctx;
    let first = read(c, keep_box);
    assert_eq!(
        (held(first), count(first)),
        (42, 1),
        "the evaluation's own value"
    );
    let second = read(c, keep_box);
    let third = read(c, keep_box);
    assert_eq!(FORTY_TWOS.load(Ordering::SeqCst), 1, "evaluated once");
    assert_ne!(second, first);
    assert_eq!((held(second), count(second)), (42, IMMORTAL));
    assert_eq!(third, second, "every later read returns the one copy kept");
    let_go(c, first);
    assert_eq!(
        ctx.heap.live(),
        0,
        "the copy is the unit's, not the entry's"
    );
    ctx.end();
}

static SEVENS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn seven(ctx: *mut Ctx, _args: *const Word) -> Word {
    SEVENS.fetch_add(1, Ordering::SeqCst);
    boxed(ctx, 7)
}

#[test]
fn a_constant_whose_cache_check_rejects_its_value_is_not_cached_and_is_recomputed_on_each_read() {
    let mut ctx = defining(seven);
    let c: *mut Ctx = &mut ctx;
    let first = read(c, keeps_nothing);
    let second = read(c, keeps_nothing);
    assert_eq!(SEVENS.load(Ordering::SeqCst), 2);
    assert_eq!((count(first), count(second)), (1, 1));
    assert_eq!(ctx.tables.memoized(0), None);
    let_go(c, first);
    let_go(c, second);
    assert_eq!(ctx.heap.live(), 0);
    ctx.end();
}

static NINES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn nine_after_a_failure(ctx: *mut Ctx, _args: *const Word) -> Word {
    if NINES.fetch_add(1, Ordering::SeqCst) == 0 {
        unsafe { (*ctx).failed = 1 };
        return 0;
    }
    boxed(ctx, 9)
}

#[test]
fn an_evaluation_that_fails_returns_0_caches_nothing_and_the_next_read_evaluates_again() {
    let mut ctx = defining(nine_after_a_failure);
    let c: *mut Ctx = &mut ctx;
    assert_eq!(read(c, keep_box), 0);
    assert_eq!(ctx.failed, 1, "the context as the evaluation left it");
    assert_eq!(ctx.tables.memoized(0), None);
    ctx.failed = 0;
    let evaluated = read(c, keep_box);
    let kept = read(c, keep_box);
    assert_eq!(NINES.load(Ordering::SeqCst), 2);
    assert_eq!((held(evaluated), count(evaluated)), (9, 1));
    assert_eq!((held(kept), count(kept)), (9, IMMORTAL));
    let_go(c, evaluated);
    ctx.end();
}

#[test]
fn reading_a_definition_index_outside_the_code_table_fails_with_a_diagnostic() {
    let mut ctx = defining(seven);
    let c: *mut Ctx = &mut ctx;
    assert_eq!(unsafe { helper::<Memo>("rt_memo")(c, 3, keep_box) }, 0);
    let said = ctx.take_failure().expect("a diagnostic").message;
    assert!(said.contains("definition 3 of a code table of 1"), "{said}");
    ctx.end();
}

struct Shared(Arc<Tables>);
unsafe impl Send for Shared {}

impl Shared {
    fn context(self) -> Ctx {
        Ctx::new(self.0)
    }
}

unsafe extern "C" fn eleven(ctx: *mut Ctx, _args: *const Word) -> Word {
    boxed(ctx, 11)
}

#[test]
fn contexts_reading_one_definition_at_once_each_return_it_and_one_copy_is_cached() {
    let mut ctx = defining(eleven);
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let tables = Shared(Arc::clone(&ctx.tables));
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut mine = tables.context();
                mine.begin();
                let c: *mut Ctx = &mut mine;
                barrier.wait();
                let w = read(c, keep_box);
                let returned = held(w);
                let_go(c, w);
                assert_eq!(mine.heap.live(), 0);
                mine.end();
                returned
            })
        })
        .collect();
    for reader in readers {
        assert_eq!(reader.join().expect("a reader returns"), 11);
    }
    let kept = ctx.tables.memoized(0).expect("a copy was kept");
    let c: *mut Ctx = &mut ctx;
    assert_eq!(
        read(c, keep_box),
        kept,
        "a later read returns the copy kept first"
    );
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

fn read_const(ctx: *mut Ctx) -> Word {
    unsafe { helper::<Memo>("rt_const")(ctx, 0, keep_box) }
}

static TENS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn ten(ctx: *mut Ctx, _args: *const Word) -> Word {
    TENS.fetch_add(1, Ordering::SeqCst);
    boxed(ctx, 10)
}

#[test]
fn a_constant_is_evaluated_once_and_every_later_read_returns_the_copy() {
    let mut ctx = defining(ten);
    let c: *mut Ctx = &mut ctx;
    let first = read_const(c);
    let second = read_const(c);
    let third = read_const(c);
    assert_eq!(TENS.load(Ordering::SeqCst), 1, "evaluated once");
    assert_eq!(held(first), 10);
    assert_eq!((held(second), count(second)), (10, IMMORTAL));
    assert_eq!(third, second);
    let_go(c, first);
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
    let mut ctx = defining(failing);
    let c: *mut Ctx = &mut ctx;
    assert_eq!(read_const(c), 0);
    assert_eq!(ctx.failed, FAILED, "a plain failure, no raise pending");
    let said = ctx.take_failure().expect("a diagnostic");
    assert!(
        said.notes
            .iter()
            .any(|n| n.contains("a constant must return")),
        "{said:?}"
    );
    assert_eq!(ctx.tables.memoized(0), None);
    ctx.end();
}

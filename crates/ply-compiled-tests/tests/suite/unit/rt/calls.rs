use crate::bare::{context_of, helper};
use ply_compiled::emitted::KeepRoot;
use ply_compiled::heap::{HEADER, IMMORTAL, Word};
use ply_compiled::rt::{Context, FAILED, Tables};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

type Alloc = unsafe extern "C" fn(*mut Context, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Context, Word, Word);
type Memo = unsafe extern "C" fn(*mut Context, Word, KeepRoot) -> Word;
type Definition = unsafe extern "C" fn(*mut Context, *const Word) -> Word;

const BOX: Word = HEADER as Word + 8;

fn boxed(context: *mut Context, n: Word) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(context, BOX) };
    unsafe { *(w as *mut Word).byte_add(HEADER) = n };
    w
}

fn first_word(w: Word) -> Word {
    unsafe { *(w as *const Word).byte_add(HEADER) }
}

fn count(w: Word) -> u32 {
    unsafe { *(w as *const u32) }
}

fn let_go(context: *mut Context, w: Word) {
    if count(w) != IMMORTAL {
        unsafe { helper::<Free>("rt_free")(context, w, BOX) };
    }
}

unsafe extern "C" fn copy_to_immortal_box(context: *mut Context, w: Word) -> Word {
    let kept = unsafe { helper::<Alloc>("rt_alloc_kept")(context, BOX) };
    unsafe { *(kept as *mut Word).byte_add(HEADER) = first_word(w) };
    kept
}

unsafe extern "C" fn returns_zero(_context: *mut Context, _w: Word) -> Word {
    0
}

fn read(context: *mut Context, keep: KeepRoot) -> Word {
    unsafe { helper::<Memo>("rt_memo")(context, 0, keep) }
}

fn defining(f: Definition) -> Context {
    let mut context = context_of(&[], vec![f as usize]);
    context.begin();
    context
}

static FORTY_TWOS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn forty_two(context: *mut Context, _args: *const Word) -> Word {
    FORTY_TWOS.fetch_add(1, Ordering::SeqCst);
    boxed(context, 42)
}

#[test]
fn the_first_read_evaluates_and_caches_and_each_later_one_returns_the_copy() {
    let mut context = defining(forty_two);
    let c: *mut Context = &mut context;
    let first = read(c, copy_to_immortal_box);
    assert_eq!(
        (first_word(first), count(first)),
        (42, 1),
        "the evaluation's own value"
    );
    let second = read(c, copy_to_immortal_box);
    let third = read(c, copy_to_immortal_box);
    assert_eq!(FORTY_TWOS.load(Ordering::SeqCst), 1, "evaluated once");
    assert_ne!(second, first);
    assert_eq!((first_word(second), count(second)), (42, IMMORTAL));
    assert_eq!(third, second, "every later read returns the one copy kept");
    let_go(c, first);
    assert_eq!(
        context.heap.live(),
        0,
        "the copy is the unit's, not the entry's"
    );
    context.end();
}

static SEVENS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn seven(context: *mut Context, _args: *const Word) -> Word {
    SEVENS.fetch_add(1, Ordering::SeqCst);
    boxed(context, 7)
}

#[test]
fn a_constant_whose_cache_check_rejects_its_value_is_not_cached_and_is_recomputed_on_each_read() {
    let mut context = defining(seven);
    let c: *mut Context = &mut context;
    let first = read(c, returns_zero);
    let second = read(c, returns_zero);
    assert_eq!(SEVENS.load(Ordering::SeqCst), 2);
    assert_eq!((count(first), count(second)), (1, 1));
    assert_eq!(context.tables.memoized(0), None);
    let_go(c, first);
    let_go(c, second);
    assert_eq!(context.heap.live(), 0);
    context.end();
}

static NINES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn nine_after_a_failure(context: *mut Context, _args: *const Word) -> Word {
    if NINES.fetch_add(1, Ordering::SeqCst) == 0 {
        unsafe { (*context).failed = 1 };
        return 0;
    }
    boxed(context, 9)
}

#[test]
fn an_evaluation_that_fails_returns_0_caches_nothing_and_the_next_read_evaluates_again() {
    let mut context = defining(nine_after_a_failure);
    let c: *mut Context = &mut context;
    assert_eq!(read(c, copy_to_immortal_box), 0);
    assert_eq!(context.failed, 1, "the context as the evaluation left it");
    assert_eq!(context.tables.memoized(0), None);
    context.failed = 0;
    let evaluated = read(c, copy_to_immortal_box);
    let kept = read(c, copy_to_immortal_box);
    assert_eq!(NINES.load(Ordering::SeqCst), 2);
    assert_eq!((first_word(evaluated), count(evaluated)), (9, 1));
    assert_eq!((first_word(kept), count(kept)), (9, IMMORTAL));
    let_go(c, evaluated);
    context.end();
}

#[test]
fn reading_a_definition_index_outside_the_code_table_fails_with_a_diagnostic() {
    let mut context = defining(seven);
    let c: *mut Context = &mut context;
    assert_eq!(
        unsafe { helper::<Memo>("rt_memo")(c, 3, copy_to_immortal_box) },
        0
    );
    let said = context.take_failure().expect("a diagnostic").message;
    assert!(said.contains("definition 3 of a code table of 1"), "{said}");
    context.end();
}

struct Shared(Arc<Tables>);
unsafe impl Send for Shared {}

impl Shared {
    fn context(self) -> Context {
        Context::new(self.0)
    }
}

unsafe extern "C" fn eleven(context: *mut Context, _args: *const Word) -> Word {
    boxed(context, 11)
}

#[test]
fn contexts_reading_one_definition_at_once_each_return_it_and_one_copy_is_cached() {
    let mut context = defining(eleven);
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let tables = Shared(Arc::clone(&context.tables));
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut mine = tables.context();
                mine.begin();
                let c: *mut Context = &mut mine;
                barrier.wait();
                let w = read(c, copy_to_immortal_box);
                let returned = first_word(w);
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
    let kept = context.tables.memoized(0).expect("a copy was kept");
    let c: *mut Context = &mut context;
    assert_eq!(
        read(c, copy_to_immortal_box),
        kept,
        "a later read returns the copy kept first"
    );
    context.end();
}

type Entered = unsafe extern "C" fn(*mut Context) -> Word;
type Left = unsafe extern "C" fn(*mut Context, Word);

#[test]
fn the_scheduler_blocks_stay_separate_from_the_program_blocks_at_any_nesting_depth() {
    let mut context = context_of(&[], Vec::new());
    context.begin();
    let c: *mut Context = &mut context;
    let (entered, left) = (
        helper::<Entered>("rt_scheduler_entered"),
        helper::<Left>("rt_scheduler_left"),
    );
    let mine = boxed(c, 1);
    let before = context.live();
    let outer = unsafe { entered(c) };
    let inner = unsafe { entered(c) };
    let theirs = boxed(c, 2);
    unsafe { left(c, inner) };
    let nested = boxed(c, 3);
    assert_eq!(
        context.live(),
        before,
        "a nested entry leaves the scheduler running"
    );
    unsafe { left(c, outer) };
    assert_eq!((outer, inner), (0, 1));
    let_go(c, nested);
    let_go(c, theirs);
    let_go(c, mine);
    context.end();
}

fn read_const(context: *mut Context) -> Word {
    unsafe { helper::<Memo>("rt_const")(context, 0, copy_to_immortal_box) }
}

static TENS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn ten(context: *mut Context, _args: *const Word) -> Word {
    TENS.fetch_add(1, Ordering::SeqCst);
    boxed(context, 10)
}

#[test]
fn a_constant_is_evaluated_once_and_every_later_read_returns_the_copy() {
    let mut context = defining(ten);
    let c: *mut Context = &mut context;
    let first = read_const(c);
    let second = read_const(c);
    let third = read_const(c);
    assert_eq!(TENS.load(Ordering::SeqCst), 1, "evaluated once");
    assert_eq!(first_word(first), 10);
    assert_eq!((first_word(second), count(second)), (10, IMMORTAL));
    assert_eq!(third, second);
    let_go(c, first);
    context.end();
}

unsafe extern "C" fn failing(context: *mut Context, _args: *const Word) -> Word {
    let raised: unsafe extern "C" fn(*mut Context, Word, *const Word, Word, Word) =
        helper("rt_failed");
    let texts = [b"E0502".as_slice(), b"it failed", b"here"];
    let words: Vec<Word> = texts
        .iter()
        .flat_map(|t| [t.as_ptr() as Word, t.len() as Word])
        .collect();
    unsafe { raised(context, 1, words.as_ptr(), 0, 0) };
    0
}

#[test]
fn an_effect_raised_inside_a_constant_fails_the_run_with_an_error_that_no_handler_can_catch() {
    let mut context = defining(failing);
    let c: *mut Context = &mut context;
    assert_eq!(read_const(c), 0);
    assert_eq!(context.failed, FAILED, "a plain failure, no raise pending");
    let said = context.take_failure().expect("a diagnostic");
    assert!(
        said.notes
            .iter()
            .any(|n| n.contains("a constant must return")),
        "{said:?}"
    );
    assert_eq!(context.tables.memoized(0), None);
    context.end();
}

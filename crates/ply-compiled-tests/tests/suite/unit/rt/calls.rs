//! Reads of a pure nullary definition, as emitted code writes one for a constant: the first
//! evaluates the definition and keeps its value for good through the type's keep root, and each
//! later read answers that copy, charged what evaluating it cost. The definitions and keep roots
//! here are written as emitted code writes them, over blocks laid out as it lays one.

use crate::bare::{context_of, helper};
use ply_compiled::emitted::KeepRoot;
use ply_compiled::heap::{HEADER, IMMORTAL, Word};
use ply_compiled::rt::{Cost, Ctx, Tables};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Memo = unsafe extern "C" fn(*mut Ctx, Word, KeepRoot) -> Word;
type Definition = unsafe extern "C" fn(*mut Ctx, *const Word) -> Word;

/// A value of `m.Box`: a block of one word, the number it holds.
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

/// Lets go of a box the caller holds the last count of; a kept one is immortal, and nothing frees it.
fn let_go(ctx: *mut Ctx, w: Word) {
    if count(w) != IMMORTAL {
        unsafe { helper::<Free>("rt_free")(ctx, w, BOX) };
    }
}

/// `keep#m.Box`: the box copied onto the unit's persistent heap.
unsafe extern "C" fn keep_box(ctx: *mut Ctx, w: Word) -> Word {
    let kept = unsafe { helper::<Alloc>("rt_alloc_kept")(ctx, BOX) };
    unsafe { *(kept as *mut Word).byte_add(HEADER) = held(w) };
    kept
}

/// The keep root of a type whose values may not outlive their entry.
unsafe extern "C" fn keeps_nothing(_ctx: *mut Ctx, _w: Word) -> Word {
    0
}

fn read(ctx: *mut Ctx, keep: KeepRoot) -> Word {
    unsafe { helper::<Memo>("rt_memo")(ctx, 0, keep) }
}

/// A context over a unit of the one definition `f`.
fn defining(f: Definition) -> Ctx {
    let mut ctx = context_of(&[], vec![f as usize]);
    ctx.begin(1_000);
    ctx
}

static FORTY_TWOS: AtomicUsize = AtomicUsize::new(0);

/// Five calls and a box of 42.
unsafe extern "C" fn forty_two(ctx: *mut Ctx, _args: *const Word) -> Word {
    FORTY_TWOS.fetch_add(1, Ordering::SeqCst);
    unsafe { (*ctx).ticks += 5 };
    boxed(ctx, 42)
}

#[test]
fn the_first_read_evaluates_and_keeps_and_each_later_one_answers_the_copy_charged_its_cost() {
    let mut ctx = defining(forty_two);
    let c: *mut Ctx = &mut ctx;
    let first = read(c, keep_box);
    assert_eq!(
        (held(first), count(first)),
        (42, 1),
        "the evaluation's own value"
    );
    assert_eq!(ctx.charged, Cost::default());
    let second = read(c, keep_box);
    let third = read(c, keep_box);
    assert_eq!(FORTY_TWOS.load(Ordering::SeqCst), 1, "evaluated once");
    assert_ne!(second, first);
    assert_eq!((held(second), count(second)), (42, IMMORTAL));
    assert_eq!(third, second, "every later read answers the one copy kept");
    let twice = Cost {
        steps: 2 * 5,
        allocations: 2,
        bytes: 2 * BOX,
    };
    assert_eq!(
        ctx.charged, twice,
        "each later read charged what evaluating cost"
    );
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
fn a_value_its_keep_root_refuses_is_kept_nowhere_and_each_read_evaluates_it() {
    let mut ctx = defining(seven);
    let c: *mut Ctx = &mut ctx;
    let first = read(c, keeps_nothing);
    let second = read(c, keeps_nothing);
    assert_eq!(SEVENS.load(Ordering::SeqCst), 2);
    assert_eq!((count(first), count(second)), (1, 1));
    assert_eq!(ctx.tables.memoized(0), None);
    assert_eq!(
        ctx.charged,
        Cost::default(),
        "nothing kept, nothing charged"
    );
    let_go(c, first);
    let_go(c, second);
    assert_eq!(ctx.heap.live(), 0);
    ctx.end();
}

static NINES: AtomicUsize = AtomicUsize::new(0);

/// Fails the first time it is evaluated, and answers a box of 9 after.
unsafe extern "C" fn nine_after_a_failure(ctx: *mut Ctx, _args: *const Word) -> Word {
    if NINES.fetch_add(1, Ordering::SeqCst) == 0 {
        unsafe { (*ctx).failed = 1 };
        return 0;
    }
    boxed(ctx, 9)
}

#[test]
fn an_evaluation_that_fails_answers_0_keeps_nothing_and_the_next_read_evaluates_again() {
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
fn a_definition_outside_the_code_table_fails_the_context() {
    let mut ctx = defining(seven);
    let c: *mut Ctx = &mut ctx;
    assert_eq!(unsafe { helper::<Memo>("rt_memo")(c, 3, keep_box) }, 0);
    let said = ctx.take_failure().expect("a diagnostic").message;
    assert!(said.contains("definition 3 of a code table of 1"), "{said}");
    ctx.end();
}

/// A unit's tables, shared with threads as a `parallel` block's branches share them.
struct Shared(Arc<Tables>);
unsafe impl Send for Shared {}

impl Shared {
    /// A context over the tables, on the thread that takes them.
    fn context(self) -> Ctx {
        Ctx::new(self.0)
    }
}

unsafe extern "C" fn eleven(ctx: *mut Ctx, _args: *const Word) -> Word {
    boxed(ctx, 11)
}

#[test]
fn contexts_reading_one_definition_at_once_each_answer_it_and_one_copy_is_kept() {
    let mut ctx = defining(eleven);
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let tables = Shared(Arc::clone(&ctx.tables));
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut mine = tables.context();
                mine.begin(1_000);
                let c: *mut Ctx = &mut mine;
                barrier.wait();
                let w = read(c, keep_box);
                let answered = held(w);
                let_go(c, w);
                assert_eq!(mine.heap.live(), 0);
                mine.end();
                answered
            })
        })
        .collect();
    for reader in readers {
        assert_eq!(reader.join().expect("a reader answers"), 11);
    }
    let kept = ctx.tables.memoized(0).expect("a copy was kept");
    let c: *mut Ctx = &mut ctx;
    assert_eq!(
        read(c, keep_box),
        kept,
        "a later read answers the copy kept first"
    );
    ctx.end();
}

type Entered = unsafe extern "C" fn(*mut Ctx) -> Word;
type Left = unsafe extern "C" fn(*mut Ctx, Word);

/// The program's calls and allocations, as `metered` and a budget count them.
fn spent(ctx: &Ctx) -> (i64, usize, usize) {
    (ctx.ticks, ctx.heap.allocated(), ctx.heap.allocated_bytes())
}

#[test]
fn the_schedulers_own_work_costs_the_program_nothing_however_it_nests() {
    let mut ctx = context_of(&[], Vec::new());
    ctx.begin_bounded(1_000, 0, 0);
    let c: *mut Ctx = &mut ctx;
    let (entered, left) = (
        helper::<Entered>("rt_scheduler_entered"),
        helper::<Left>("rt_scheduler_left"),
    );
    ctx.ticks = 5;
    let mine = boxed(c, 1);
    let before = spent(&ctx);
    let outer = unsafe { entered(c) };
    let inner = unsafe { entered(c) };
    ctx.ticks += 100;
    let theirs = boxed(c, 2);
    unsafe { left(c, inner) };
    assert_ne!(
        spent(&ctx),
        before,
        "a nested entry leaves the scheduler running"
    );
    unsafe { left(c, outer) };
    assert_eq!((outer, inner), (0, 1));
    assert_eq!(spent(&ctx), before);
    let_go(c, theirs);
    let_go(c, mine);
    ctx.end();
}

#[test]
fn no_budget_of_calls_bounds_the_schedulers_own_work() {
    let mut ctx = context_of(&[], Vec::new());
    ctx.begin_bounded(1_000, 10, 0);
    let c: *mut Ctx = &mut ctx;
    let was = unsafe { helper::<Entered>("rt_scheduler_entered")(c) };
    assert_eq!(
        ctx.next_tick,
        i64::MAX,
        "no tick is asked for while only a budget bounds the run"
    );
    ctx.ticks += 50;
    unsafe { helper::<Left>("rt_scheduler_left")(c, was) };
    assert_eq!((ctx.ticks, ctx.next_tick), (0, 11));
    assert!(ctx.take_failure().is_none());
    ctx.end();
}

type Tick = unsafe extern "C" fn(*mut Ctx);

/// `n` calls as compiled code counts each, calling back where the counter reaches `next_tick`,
/// until the context fails.
fn calls_made(ctx: *mut Ctx, n: i64) {
    for _ in 0..n {
        let c = unsafe { &mut *ctx };
        c.ticks += 1;
        if c.ticks >= c.next_tick {
            unsafe { helper::<Tick>("rt_tick")(ctx) };
        }
        if unsafe { (*ctx).failed } != 0 {
            return;
        }
    }
}

// Main read the clock every 4096 calls, so a run past its deadline was abandoned within them.
#[test]
fn a_time_budget_ends_a_program_that_gives_way_to_the_scheduler_between_short_stretches() {
    let mut ctx = context_of(&[], Vec::new());
    ctx.begin_bounded(1_000, 0, 1);
    let c: *mut Ctx = &mut ctx;
    std::thread::sleep(std::time::Duration::from_millis(5));
    let (entered, left) = (
        helper::<Entered>("rt_scheduler_entered"),
        helper::<Left>("rt_scheduler_left"),
    );
    for _ in 0..1_000 {
        calls_made(c, 10);
        let was = unsafe { entered(c) };
        calls_made(c, 100);
        unsafe { left(c, was) };
        if unsafe { (*c).failed } != 0 {
            break;
        }
    }
    assert!(
        ctx.take_failure().is_some(),
        "110000 calls past the deadline, and the clock never read"
    );
    ctx.end();
}

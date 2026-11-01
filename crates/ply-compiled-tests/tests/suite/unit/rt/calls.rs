//! What the runtime does for a read of a constant, which emitted code otherwise does itself: it
//! keeps the first copy it is handed, and ends the run on a raise that left a `const fn`. The
//! blocks here are laid out as emitted code lays one.

use crate::bare::{context_of, helper};
use ply_compiled::heap::{HEADER, IMMORTAL, Word};
use ply_compiled::rt::{Ctx, FAILED, Tables};
use std::sync::Arc;
use std::sync::atomic::Ordering;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type KeepConstant = unsafe extern "C" fn(*mut Ctx, Word, Word);
type ConstFailed = unsafe extern "C" fn(*mut Ctx);

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

/// A box of `n` on the unit's persistent heap, as a keep root copies one.
fn kept_box(ctx: *mut Ctx, n: Word) -> Word {
    let kept = unsafe { helper::<Alloc>("rt_alloc_kept")(ctx, BOX) };
    unsafe { *(kept as *mut Word).byte_add(HEADER) = n };
    kept
}

/// The word emitted code reads for the unit's one constant.
fn constant(ctx: &Ctx) -> Word {
    unsafe { &*ctx.constants }.load(Ordering::Acquire)
}

/// A context over a unit of one definition.
fn with_one_constant() -> Ctx {
    let mut ctx = context_of(&[], vec![0]);
    ctx.begin();
    ctx
}

#[test]
fn only_the_first_copy_kept_of_a_constant_stays() {
    let mut ctx = with_one_constant();
    let c: *mut Ctx = &mut ctx;
    let keep = helper::<KeepConstant>("rt_keep_constant");
    let first = kept_box(c, 1);
    unsafe { keep(c, 0, first) };
    unsafe { keep(c, 0, kept_box(c, 2)) };
    assert_eq!(constant(&ctx), first);
    assert_eq!((held(first), count(first)), (1, IMMORTAL));
    assert_eq!(
        ctx.heap.live(),
        0,
        "a kept copy is the unit's, not the entry's"
    );
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

#[test]
fn contexts_keeping_one_constant_at_once_all_read_the_one_copy_that_stayed() {
    let mut ctx = with_one_constant();
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let keepers: Vec<_> = (0..4)
        .map(|n| {
            let tables = Shared(Arc::clone(&ctx.tables));
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut mine = tables.context();
                mine.begin();
                let c: *mut Ctx = &mut mine;
                barrier.wait();
                unsafe { helper::<KeepConstant>("rt_keep_constant")(c, 0, kept_box(c, n)) };
                let read = constant(&mine);
                mine.end();
                read
            })
        })
        .collect();
    for keeper in keepers {
        let read = keeper.join().expect("a keeper returns");
        assert_eq!(read, constant(&ctx));
    }
    ctx.end();
}

type Entered = unsafe extern "C" fn(*mut Ctx) -> Word;
type Left = unsafe extern "C" fn(*mut Ctx, Word);

#[test]
fn the_schedulers_own_blocks_are_not_the_programs_however_it_nests() {
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

/// A failure compiled code wrote, left pending as a raise of `abort.raise`.
fn raised(ctx: *mut Ctx) {
    let raise: unsafe extern "C" fn(*mut Ctx, Word, *const Word, Word, Word) = helper("rt_failed");
    let texts = [b"E0502".as_slice(), b"it failed", b"here"];
    let words: Vec<Word> = texts
        .iter()
        .flat_map(|t| [t.as_ptr() as Word, t.len() as Word])
        .collect();
    unsafe { raise(ctx, 1, words.as_ptr(), 0, 0) };
}

#[test]
fn a_raise_that_leaves_a_constant_fails_the_entry_with_nothing_left_for_a_handler() {
    let mut ctx = with_one_constant();
    let c: *mut Ctx = &mut ctx;
    raised(c);
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

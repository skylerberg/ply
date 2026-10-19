//! Blocks emitted code lays out, as its C calls the runtime for them: allocated by bytes, freed by
//! size, counted at offset 0, and released past a bounded depth through the context's buffer. The
//! drop roots here are written as emitted code writes one.

use crate::bare::{context, helper};
use ply_compiled::emitted::Block;
use ply_compiled::heap::{
    CLOSURE_CAPTURES, IMMORTAL, KIND_DEAD, KIND_LIST, KIND_REFUSED, SHARED, Word, kind, obj,
    word_at,
};
use ply_compiled::native::Answered;
use ply_compiled::rt::{Ctx, RELEASE_DEPTH};
use ply_core::{Closure, ClosureKind, Value};
use std::cell::Cell;
use std::sync::{Arc, OnceLock};

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Counted = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Given = unsafe extern "C" fn(*mut Ctx, Word);
type Later = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Drain = unsafe extern "C" fn(*mut Ctx);
type Root = unsafe extern "C" fn(*mut Ctx, Word) -> Word;

/// The helpers a drop root calls, bound once.
struct Bound {
    alloc: Alloc,
    free: Free,
    later: Later,
    drain: Drain,
}

fn bound() -> &'static Bound {
    static BOUND: OnceLock<Bound> = OnceLock::new();
    BOUND.get_or_init(|| Bound {
        alloc: helper("rt_alloc_bytes"),
        free: helper("rt_free"),
        later: helper("rt_release_later"),
        drain: helper("rt_release_drain"),
    })
}

/// The count at a block's offset 0.
fn count(w: Word) -> u32 {
    unsafe { *(w as *const u32) }
}

fn alloc(ctx: &mut Ctx, bytes: Word) -> Word {
    unsafe { (bound().alloc)(ctx, bytes) }
}

fn free(ctx: &mut Ctx, w: Word, bytes: Word) {
    unsafe { (bound().free)(ctx, w, bytes) }
}

#[test]
fn the_fields_a_drop_root_keeps_follow_those_every_unit_reads() {
    use std::mem::offset_of;
    assert_eq!(offset_of!(Ctx, release_depth), 72);
    assert_eq!(offset_of!(Ctx, release_waiting), 80);
}

#[test]
fn a_block_is_allocated_by_its_bytes_and_given_back_at_them() {
    let mut ctx = context();
    let w = alloc(&mut ctx, 16 + 8 * 4);
    assert_eq!((count(w), kind(w)), (1, KIND_REFUSED));
    assert_eq!((ctx.heap.allocated(), ctx.heap.allocated_bytes()), (1, 48));
    free(&mut ctx, w, 16 + 8 * 4);
    assert_eq!(kind(w), KIND_DEAD);
    assert_eq!(ctx.heap.live(), 0);
    assert_eq!(
        alloc(&mut ctx, 48),
        w,
        "the next block of its size took the freed one's"
    );
    assert_eq!(ctx.heap.recycled(), 1);
}

#[test]
fn a_block_freed_under_another_context_goes_back_to_the_heap_it_came_from() {
    let (mut mine, mut other) = (context(), context());
    let w = alloc(&mut mine, 32);
    free(&mut other, w, 32);
    assert_eq!(kind(w), KIND_DEAD);
    assert_eq!(mine.heap.live(), 0);
    assert_ne!(alloc(&mut other, 32), w, "a heap took a block of another's");
}

#[test]
fn a_box_the_runtime_made_of_a_scalars_bits_is_freed_at_its_size() {
    let mut ctx = context();
    let b = ctx.heap.boxed_float(1.5);
    free(&mut ctx, b, 16 + 8);
    assert_eq!(kind(b), KIND_DEAD);
    assert_eq!(ctx.heap.live(), 0);
}

/// A debug build knows each block's size class, which is all a free relies on.
#[test]
fn a_block_freed_at_another_size_than_it_was_allocated_at_is_refused() {
    if !cfg!(debug_assertions) {
        return;
    }
    let mut ctx = context();
    let w = alloc(&mut ctx, 40);
    let heap = &raw mut ctx.heap;
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        Block::of(w).free(heap, 16 + 8 * 20)
    }))
    .expect_err("a block of 40 bytes was freed as 176");
    let said = refused.downcast_ref::<String>().expect("a message");
    assert!(
        said.contains("was freed as 176 bytes, and its block was taken as 40"),
        "{said}"
    );
}

#[test]
fn a_kept_block_is_immortal_and_outlives_the_entry_that_made_it() {
    let mut ctx = context();
    let kept: Alloc = helper("rt_alloc_kept");
    ctx.begin(1_000);
    let w = unsafe { kept(&mut ctx, 32) };
    unsafe { *(w as *mut Word).add(2) = 42 };
    assert_eq!((count(w), kind(w)), (IMMORTAL, KIND_REFUSED));
    assert_eq!(ctx.heap.allocated(), 0, "the entry's own heap made it");
    ctx.end();
    let held = unsafe { *(w as *const Word).add(2) };
    assert_eq!((count(w), kind(w), held), (IMMORTAL, KIND_REFUSED, 42));
}

/// `rt_dec_shared_last` reads no context, so the holders here hand it none.
#[test]
fn of_holders_on_many_threads_only_the_last_to_let_a_shared_block_go_releases_it() {
    const HOLDERS: u32 = 8;
    let mut ctx = context();
    let w = alloc(&mut ctx, 32);
    unsafe { *(w as *mut u32) = SHARED | HOLDERS };
    let last: Counted = helper("rt_dec_shared_last");
    let lasts: Word = std::thread::scope(|s| {
        let holders: Vec<_> = (0..HOLDERS)
            .map(|_| s.spawn(move || unsafe { last(std::ptr::null_mut(), w) }))
            .collect();
        holders
            .into_iter()
            .map(|h| h.join().expect("a holder lets go"))
            .sum()
    });
    assert_eq!(lasts, 1);
    assert_eq!(count(w), SHARED);
    free(&mut ctx, w, 32);
    assert_eq!(ctx.heap.live(), 0);
}

#[test]
fn a_bridge_is_given_back_with_the_value_it_bridged() {
    let mut ctx = context();
    let held: Arc<str> = Arc::from("held");
    let b = ctx.heap.bridge(Value::Str(held.clone()));
    let free_handle: Given = helper("rt_handle_free");
    unsafe { free_handle(&mut ctx, b) };
    assert_eq!(kind(b), KIND_DEAD);
    assert_eq!(Arc::strong_count(&held), 1);
    assert_eq!(ctx.heap.bridges(), 0);
}

#[test]
fn a_hold_is_given_back_without_a_read_of_what_it_held() {
    let mut ctx = context();
    let tables = Arc::clone(&ctx.tables);
    let hold = Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Hold {
            held: Some(Value::list(vec![Value::Int(1)])),
            id: 1,
        },
    }));
    let h = ctx.heap.to_word(&tables.layouts, &hold);
    let held = unsafe { word_at(obj(h), CLOSURE_CAPTURES) };
    let free_handle: Given = helper("rt_handle_free");
    unsafe { free_handle(&mut ctx, h) };
    assert_eq!(kind(h), KIND_DEAD);
    assert_eq!(
        (kind(held), count(held)),
        (KIND_LIST, 1),
        "the hold's release let go of what it held"
    );
}

/// Derived, because a filter naming no test runs nothing and exits 0, which reads as a pass.
fn own_test_name(leaf: &str) -> String {
    match module_path!().split_once("::") {
        Some((_binary, module)) => format!("{module}::{leaf}"),
        None => leaf.to_string(),
    }
}

/// A trap ends its process, so the test that springs it runs in a process of its own.
#[test]
fn a_block_released_twice_ends_the_run_naming_where() {
    let name = own_test_name("releases_a_block_twice");
    let out = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args([name.as_str(), "--exact", "--ignored", "--nocapture"])
        .output()
        .expect("the test binary runs");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "the run went on past the trap:\n{}{said}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        said.contains("was released after its last holder let it go, at root 7 bytes 3..9"),
        "{said}"
    );
}

#[test]
#[ignore = "ends its process: run by `a_block_released_twice_ends_the_run_naming_where`"]
fn releases_a_block_twice() {
    let mut ctx = context();
    let w = alloc(&mut ctx, 32);
    (ctx.site_root, ctx.site_start, ctx.site_end) = (7, 3, 9);
    let twice: Given = helper("rt_released_twice");
    unsafe { twice(&mut ctx, w) };
}

/// A node: two words, each a node or 0, released by [`drop_node`].
const NODE: Word = 16 + 8 * 2;

thread_local! {
    static NESTED: Cell<i64> = const { Cell::new(0) };
    static DEEPEST: Cell<i64> = const { Cell::new(0) };
    static PUT_OFF: Cell<u64> = const { Cell::new(0) };
}

/// `ply_last`: whether this holder was a block's last, its count taken otherwise.
fn last(w: Word) -> bool {
    let rc = w as *mut u32;
    unsafe {
        if *rc > 1 {
            *rc -= 1;
            return false;
        }
    }
    true
}

/// A node's drop root, recursing into both its words, with no continuing child: every node past the
/// bound is put off.
unsafe extern "C" fn drop_node(ctx: *mut Ctx, o: Word) -> Word {
    let b = bound();
    unsafe {
        if (*ctx).release_depth >= RELEASE_DEPTH {
            (b.later)(ctx, o, drop_node as Root as usize as Word);
            PUT_OFF.with(|n| n.set(n.get() + 1));
            return 0;
        }
        (*ctx).release_depth += 1;
        let nested = NESTED.with(|n| {
            n.set(n.get() + 1);
            n.get()
        });
        DEEPEST.with(|d| d.set(d.get().max(nested)));
        for at in 0..2 {
            let child = *(o as *const Word).add(2 + at);
            if child != 0 && last(child) {
                drop_node(ctx, child);
            }
        }
        (b.free)(ctx, o, NODE);
        (*ctx).release_depth -= 1;
        if (*ctx).release_depth == 0 && (*ctx).release_waiting != 0 {
            (b.drain)(ctx);
        }
        NESTED.with(|n| n.set(n.get() - 1));
    }
    0
}

/// A node holding `first` and `second`.
fn node(ctx: &mut Ctx, first: Word, second: Word) -> Word {
    let n = alloc(ctx, NODE);
    unsafe {
        *(n as *mut Word).add(2) = first;
        *(n as *mut Word).add(3) = second;
    }
    n
}

/// What releasing the tree `build` makes did: the objects left live, how deep drop roots nested on
/// the stack, how many releases were put off, and the depth and waiting count left in the context.
/// Run on a thread whose stack holds a few hundred frames, which a release nesting once per node
/// would overflow.
fn released(build: fn(&mut Ctx) -> Word) -> (usize, i64, u64, i64, i64) {
    std::thread::Builder::new()
        .stack_size(256 << 10)
        .spawn(move || {
            let mut ctx = context();
            let root = build(&mut ctx);
            if last(root) {
                unsafe { drop_node(&mut ctx, root) };
            }
            (
                ctx.heap.live(),
                DEEPEST.with(Cell::get),
                PUT_OFF.with(Cell::get),
                ctx.release_depth,
                ctx.release_waiting,
            )
        })
        .expect("a thread starts")
        .join()
        .expect("the release ran to its end")
}

/// The deepest tree the tests release: a hundred thousand nodes, one under the other.
const DEEP: usize = 100_000;

fn chain(ctx: &mut Ctx) -> Word {
    (0..DEEP).fold(0, |under, _| node(ctx, under, 0))
}

/// Each node of the chain holds a leaf as well, the leaves past the bound put off beside the chain.
fn comb(ctx: &mut Ctx) -> Word {
    (0..DEEP).fold(0, |under, _| {
        let leaf = node(ctx, 0, 0);
        node(ctx, under, leaf)
    })
}

#[test]
fn a_release_a_hundred_thousand_deep_frees_everything_and_nests_no_deeper_than_the_bound() {
    for (build, nodes) in [(chain as fn(&mut Ctx) -> Word, DEEP), (comb, 2 * DEEP)] {
        let (live, deepest, put_off, depth, waiting) = released(build);
        assert_eq!(live, 0, "of {nodes} nodes");
        assert_eq!(deepest, RELEASE_DEPTH, "of {nodes} nodes");
        assert!(put_off > 0, "no release of {nodes} nodes was put off");
        assert_eq!((depth, waiting), (0, 0), "of {nodes} nodes");
    }
}

#[test]
fn a_release_within_the_bound_puts_nothing_off() {
    let shallow = |ctx: &mut Ctx| (0..RELEASE_DEPTH).fold(0, |under, _| node(ctx, under, 0));
    let (live, deepest, put_off, depth, waiting) = released(shallow);
    assert_eq!(live, 0);
    assert_eq!(deepest, RELEASE_DEPTH);
    assert_eq!(put_off, 0);
    assert_eq!((depth, waiting), (0, 0));
}

/// A block another holder keeps survives its parent's release, past the bound as within it.
#[test]
fn a_node_held_twice_outlives_the_release_of_the_tree_past_the_bound() {
    std::thread::Builder::new()
        .stack_size(256 << 10)
        .spawn(|| {
            let mut ctx = context();
            let deep = (0..DEEP / 2).fold(0, |under, _| node(&mut ctx, under, 0));
            unsafe { *(deep as *mut u32) += 1 };
            let root = (0..DEEP / 2).fold(deep, |under, _| node(&mut ctx, under, 0));
            if last(root) {
                unsafe { drop_node(&mut ctx, root) };
            }
            assert_eq!(ctx.heap.live(), DEEP / 2, "the kept half was released");
            assert_eq!((count(deep), kind(deep)), (1, KIND_REFUSED));
            if last(deep) {
                unsafe { drop_node(&mut ctx, deep) };
            }
            assert_eq!(ctx.heap.live(), 0);
        })
        .expect("a thread starts")
        .join()
        .expect("the releases ran to their ends");
}

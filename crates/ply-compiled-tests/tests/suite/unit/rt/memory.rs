use crate::bare::{context, helper};
use crate::unit::heap::Holding;
use crate::unit::ignored_test_command;
use ply_compiled::emitted::Block;
use ply_compiled::heap::{HEADER, IMMORTAL, SHARED, Word, obj, word_at};
use ply_compiled::rt::{Ctx, RELEASE_DEPTH};
use std::cell::Cell;
use std::sync::{Arc, OnceLock};

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Counted = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type WordConsumer = unsafe extern "C" fn(*mut Ctx, Word);
type Later = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Drain = unsafe extern "C" fn(*mut Ctx);
type Root = unsafe extern "C" fn(*mut Ctx, Word) -> Word;

struct Bound {
    alloc: Alloc,
    free: Free,
    later: Later,
    drain: Drain,
}

fn bound() -> &'static Bound {
    static MEMORY_FUNCTIONS: OnceLock<Bound> = OnceLock::new();
    MEMORY_FUNCTIONS.get_or_init(|| Bound {
        alloc: helper("rt_alloc_bytes"),
        free: helper("rt_free"),
        later: helper("rt_release_later"),
        drain: helper("rt_release_drain"),
    })
}

fn count(w: Word) -> u32 {
    unsafe { *(w as *const u32) }
}

fn sized(words: usize) -> Word {
    (HEADER + 8 * words) as Word
}

fn alloc(ctx: &mut Ctx, bytes: Word) -> Word {
    unsafe { (bound().alloc)(ctx, bytes) }
}

fn free(ctx: &mut Ctx, w: Word, bytes: Word) {
    unsafe { (bound().free)(ctx, w, bytes) }
}

#[test]
fn the_context_fields_release_depth_and_release_waiting_are_at_the_offsets_compiled_code_expects() {
    use std::mem::offset_of;
    assert_eq!(offset_of!(Ctx, release_depth), 72);
    assert_eq!(offset_of!(Ctx, release_waiting), 80);
}

#[test]
fn a_block_is_allocated_and_freed_with_the_same_byte_size_and_then_reused() {
    let mut ctx = context();
    let w = alloc(&mut ctx, sized(4));
    assert_eq!(count(w), 1);
    assert_eq!(ctx.heap.live(), 1);
    free(&mut ctx, w, sized(4));
    assert_eq!(count(w), 0);
    assert_eq!(ctx.heap.live(), 0);
    assert_eq!(
        alloc(&mut ctx, sized(4)),
        w,
        "the next block of its size took the freed one's"
    );
}

#[test]
fn a_block_freed_through_another_context_goes_back_to_the_heap_it_came_from() {
    let (mut mine, mut other) = (context(), context());
    let w = alloc(&mut mine, 32);
    free(&mut other, w, 32);
    assert_eq!(count(w), 0);
    assert_eq!(mine.heap.live(), 0);
    assert_ne!(alloc(&mut other, 32), w, "a heap took a block of another's");
}

#[test]
fn a_block_freed_at_another_size_than_it_was_allocated_at_is_rejected() {
    if !cfg!(debug_assertions) {
        return;
    }
    let mut ctx = context();
    let w = alloc(&mut ctx, 40);
    let heap = &raw mut ctx.heap;
    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        std::mem::transmute::<Word, Block>(w).free(heap, sized(20))
    }))
    .expect_err("a block of 40 bytes was freed as larger");
    let said = rejected.downcast_ref::<String>().expect("a message");
    assert!(
        said.contains(&format!(
            "was freed as {} bytes, and its block was taken as 40",
            sized(20)
        )),
        "{said}"
    );
}

#[test]
fn a_block_from_rt_alloc_kept_is_never_freed_and_stays_valid_after_the_entry_that_allocated_it_ends()
 {
    let mut ctx = context();
    let kept: Alloc = helper("rt_alloc_kept");
    ctx.begin();
    let w = unsafe { kept(&mut ctx, 32) };
    unsafe { *(w as *mut Word).byte_add(HEADER) = 42 };
    assert_eq!(count(w), IMMORTAL);
    assert_eq!(ctx.heap.live(), 0, "the entry's own heap made it");
    ctx.end();
    let held = unsafe { *(w as *const Word).byte_add(HEADER) };
    assert_eq!((count(w), held), (IMMORTAL, 42));
}

#[test]
fn a_shared_block_used_by_many_threads_is_freed_only_by_the_last_thread_to_release_it() {
    const THREAD_COUNT: u32 = 8;
    let mut ctx = context();
    let w = alloc(&mut ctx, 32);
    unsafe { *(w as *mut u32) = SHARED | THREAD_COUNT };
    let last: Counted = helper("rt_dec_shared_last");
    let lasts: Word = std::thread::scope(|s| {
        let holders: Vec<_> = (0..THREAD_COUNT)
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
fn freeing_a_native_object_also_drops_the_value_it_contains() {
    let mut ctx = context();
    let held: Arc<str> = Arc::from("held");
    let b = ctx.heap.native(Box::new(Holding(Arc::clone(&held))));
    let free_handle: WordConsumer = helper("rt_handle_free");
    unsafe { free_handle(&mut ctx, b) };
    assert_eq!(count(b), 0);
    assert_eq!(Arc::strong_count(&held), 1);
    assert_eq!(ctx.heap.natives(), 0);
}

#[test]
fn a_scoped_value_stores_the_word_it_wraps_at_the_offset_the_runtime_reports() {
    use ply_compiled::c::builtin_helper_name;
    let mut ctx = context();
    ctx.begin();
    let held = alloc(&mut ctx, sized(1));
    let made: Counted = helper(&builtin_helper_name("?hold_made"));
    let h = unsafe { made(&mut ctx, held) };
    let at = ply_compiled::rt::SCOPED_VALUE;
    assert_eq!(unsafe { word_at(obj(h), at) }, held);
    unsafe { *(h as *mut Word).byte_add(HEADER).add(at) = 0 };
    free(&mut ctx, held, sized(1));
    let free_handle: WordConsumer = helper("rt_handle_free");
    unsafe { free_handle(&mut ctx, h) };
    assert_eq!(ctx.heap.live(), 0);
    ctx.end();
}

#[test]
fn a_freed_block_has_a_reference_count_of_zero_however_it_was_freed() {
    let (mut mine, mut other) = (context(), context());
    let sized = alloc(&mut mine, 32);
    let elsewhere = alloc(&mut mine, 32);
    let given_back = mine.heap.native(Box::new(Holding(Arc::from("given back"))));
    let released = mine.heap.native(Box::new(Holding(Arc::from("released"))));
    free(&mut mine, sized, 32);
    free(&mut other, elsewhere, 32);
    let free_handle: WordConsumer = helper("rt_handle_free");
    unsafe { free_handle(&mut mine, given_back) };
    ply_compiled::heap::release_object(&raw mut mine, released);
    for (w, given) in [
        (sized, "freed by its size"),
        (elsewhere, "freed under another context"),
        (given_back, "a handle given back"),
        (released, "a handle whose last holder let it go"),
    ] {
        assert_eq!(count(w), 0, "a block {given}");
    }
}

#[test]
fn releasing_a_block_twice_stops_the_program_and_reports_the_source_location() {
    let out = ignored_test_command(releases_a_block_twice)
        .output()
        .expect("the test binary runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "the run went on past the trap:\n{}{stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        stderr.contains("was released after its last holder let it go, at root 7 bytes 3..9"),
        "{stderr}"
    );
}

#[test]
#[ignore = "ends its process: run by the test above"]
fn releases_a_block_twice() {
    let mut ctx = context();
    let w = alloc(&mut ctx, 32);
    (ctx.site_root, ctx.site_start, ctx.site_end) = (7, 3, 9);
    let twice: WordConsumer = helper("rt_released_twice");
    unsafe { twice(&mut ctx, w) };
}

const NODE: Word = (HEADER + 8 * 2) as Word;

thread_local! {
    static NESTED: Cell<i64> = const { Cell::new(0) };
    static DEEPEST: Cell<i64> = const { Cell::new(0) };
    static DEFERRED_RELEASES: Cell<u64> = const { Cell::new(0) };
}

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

unsafe extern "C" fn drop_node(ctx: *mut Ctx, o: Word) -> Word {
    let b = bound();
    unsafe {
        if (*ctx).release_depth >= RELEASE_DEPTH {
            (b.later)(ctx, o, drop_node as Root as usize as Word);
            DEFERRED_RELEASES.with(|n| n.set(n.get() + 1));
            return 0;
        }
        (*ctx).release_depth += 1;
        let nested = NESTED.with(|n| {
            n.set(n.get() + 1);
            n.get()
        });
        DEEPEST.with(|d| d.set(d.get().max(nested)));
        for at in 0..2 {
            let child = *(o as *const Word).byte_add(HEADER).add(at);
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

fn node(ctx: &mut Ctx, first: Word, second: Word) -> Word {
    let n = alloc(ctx, NODE);
    unsafe {
        *(n as *mut Word).byte_add(HEADER) = first;
        *(n as *mut Word).byte_add(HEADER).add(1) = second;
    }
    n
}

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
                DEFERRED_RELEASES.with(Cell::get),
                ctx.release_depth,
                ctx.release_waiting,
            )
        })
        .expect("a thread starts")
        .join()
        .expect("the release ran to its end")
}

const DEEP: usize = 100_000;

fn chain(ctx: &mut Ctx) -> Word {
    (0..DEEP).fold(0, |under, _| node(ctx, under, 0))
}

fn comb(ctx: &mut Ctx) -> Word {
    (0..DEEP).fold(0, |under, _| {
        let leaf = node(ctx, 0, 0);
        node(ctx, under, leaf)
    })
}

#[test]
fn releasing_a_tree_a_hundred_thousand_nodes_deep_frees_every_node_without_exceeding_the_recursion_limit()
 {
    for (build, nodes) in [(chain as fn(&mut Ctx) -> Word, DEEP), (comb, 2 * DEEP)] {
        let (live, deepest, put_off, depth, waiting) = released(build);
        assert_eq!(live, 0, "of {nodes} nodes");
        assert_eq!(deepest, RELEASE_DEPTH, "of {nodes} nodes");
        assert!(put_off > 0, "no release of {nodes} nodes was put off");
        assert_eq!((depth, waiting), (0, 0), "of {nodes} nodes");
    }
}

#[test]
fn releasing_a_tree_no_deeper_than_the_recursion_limit_never_postpones_any_release() {
    let shallow = |ctx: &mut Ctx| (0..RELEASE_DEPTH).fold(0, |under, _| node(ctx, under, 0));
    let (live, deepest, put_off, depth, waiting) = released(shallow);
    assert_eq!(live, 0);
    assert_eq!(deepest, RELEASE_DEPTH);
    assert_eq!(put_off, 0);
    assert_eq!((depth, waiting), (0, 0));
}

#[test]
fn a_node_with_two_references_survives_the_release_of_a_tree_deeper_than_the_recursion_limit() {
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
            assert_eq!(count(deep), 1);
            if last(deep) {
                unsafe { drop_node(&mut ctx, deep) };
            }
            assert_eq!(ctx.heap.live(), 0);
        })
        .expect("a thread starts")
        .join()
        .expect("the releases ran to their ends");
}

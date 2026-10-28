use crate::bare::context;
use ply_compiled::emitted::Block;
use ply_compiled::heap::{
    HEADER, Heap, Native, Obj, Word, enter, leave, native_of, poison, poisoning, release_object,
    word_at,
};
use ply_compiled::rt::Ctx;
use std::sync::Arc;

/// A block of `payload` bytes past its header, from `h`: its word, which a `Block` is.
fn block(h: &mut Heap, payload: usize) -> Word {
    unsafe { std::mem::transmute::<Block, Word>(Block::alloc(h, (HEADER + payload) as Word)) }
}

/// Gives back the block `w` of `payload` bytes past its header to `h`, the heap the entry runs on.
fn freed(h: &mut Heap, w: Word, payload: usize) {
    unsafe { std::mem::transmute::<Word, Block>(w).free(h, (HEADER + payload) as Word) }
}

/// What a native object holds in these tests: text, whose count of holders says how many native
/// objects holding it are not yet dropped.
pub(super) struct Holding(pub(super) Arc<str>);

impl Native for Holding {
    fn release(&mut self, _ctx: *mut Ctx) {}
}

fn holding(text: &str) -> Box<dyn Native> {
    Box::new(Holding(Arc::from(text)))
}

/// The text the native object `w` holds.
fn held_by(w: Word) -> Arc<str> {
    let held = unsafe { native_of::<Holding>(w) }.expect("a native object holding text");
    Arc::clone(&held.0)
}

#[test]
fn an_ended_entry_gives_back_what_it_held() {
    let mut h = Heap::new();
    block(&mut h, 8);
    block(&mut h, 200 << 20);
    assert!(h.held() > 200 << 20);
    h.end();
    assert_eq!((h.held(), h.live()), (0, 0));
    block(&mut h, 8);
    assert_eq!(h.live(), 1, "an ended heap allocates afresh");
}

/// A block is its size rounded up by an eighth at most, and a page ends short of one more.
#[test]
fn a_heap_holds_little_more_than_its_blocks_take() {
    let mut h = Heap::new();
    for _ in 0..4096 {
        block(&mut h, 4096);
    }
    let held = h.held();
    assert!(held < 4096 * (4096 + HEADER) * 5 / 4, "{held} bytes held");
    let mut big = Heap::new();
    block(&mut big, 1 << 20);
    assert_eq!(big.held(), (1 << 20) + HEADER);
}

/// What blocks of one size held serves blocks of another once every block beside them is given back.
#[test]
fn memory_one_size_gave_up_serves_another() {
    let mut h = Heap::new();
    enter(&mut h);
    let small: Vec<Word> = (0..100_000).map(|_| block(&mut h, 8)).collect();
    let peak = h.held();
    for w in small {
        freed(&mut h, w, 8);
    }
    for _ in 0..30_000 {
        block(&mut h, 40);
    }
    assert!(h.held() <= peak, "{} bytes held past {peak}", h.held());
    leave();
    h.end();
}

#[test]
fn a_block_given_back_is_reused_by_the_next_of_its_size_within_an_entry() {
    let mut h = Heap::new();
    enter(&mut h);
    let first = block(&mut h, 16);
    let kept = block(&mut h, 16);
    freed(&mut h, first, 16);
    assert_eq!(
        block(&mut h, 16),
        first,
        "a block of its size took its place"
    );
    freed(&mut h, kept, 16);
    assert_ne!(block(&mut h, 64), kept, "a larger block is not its size's");
    leave();
    h.end();
}

#[test]
fn a_dead_native_objects_block_is_the_next_native_objects() {
    let mut ctx = context();
    let first = ctx.heap.native(holding("first"));
    release_object(&raw mut ctx, first);
    let again = ctx.heap.native(holding("second"));
    assert_eq!(
        again, first,
        "a dead native object's block is the next native object's"
    );
    assert_eq!(&*held_by(again), "second");
    assert_eq!(ctx.heap.recycled(), 1);
    assert_eq!(ctx.heap.natives(), 1);
}

/// The end drops what the table lists, so a block listed twice would drop what it holds twice.
#[test]
fn the_table_lists_each_live_native_object_once_and_the_end_drops_each_once() {
    let mut ctx = context();
    let held: Arc<str> = Arc::from("held");
    let words: Vec<Word> = (0..8)
        .map(|_| ctx.heap.native(Box::new(Holding(Arc::clone(&held)))))
        .collect();
    for i in [3, 0, 7, 4] {
        release_object(&raw mut ctx, words[i]);
    }
    assert_eq!(
        ctx.heap.natives(),
        4,
        "a dead native object stayed in the table"
    );
    assert_eq!(Arc::strong_count(&held), 5);

    let reused: Vec<Word> = (0..4)
        .map(|i| ctx.heap.native(holding(&i.to_string())))
        .collect();
    assert!(
        reused.iter().all(|w| words.contains(w)),
        "a native object took fresh memory with dead ones' blocks on hand"
    );
    assert_eq!(ctx.heap.natives(), 8);
    for (i, w) in reused.iter().enumerate() {
        assert_eq!(&*held_by(*w), i.to_string());
    }
    for i in [1, 2, 5, 6] {
        assert!(Arc::ptr_eq(&held_by(words[i]), &held));
    }

    // Out of listing order, so removals move entries into the holes they leave.
    for w in [reused[1], words[6], reused[3], words[1], reused[0]] {
        release_object(&raw mut ctx, w);
    }
    assert_eq!(ctx.heap.natives(), 3);
    assert_eq!(Arc::strong_count(&held), 3);

    // Let go under another context, the block cannot go back to its heap, so it stays listed, dead.
    let mut other = context();
    release_object(&raw mut other, words[5]);
    assert_eq!(ctx.heap.natives(), 3);
    assert_eq!(Arc::strong_count(&held), 2);
    ctx.heap.end();
    assert_eq!(ctx.heap.natives(), 0);
    assert_eq!(
        Arc::strong_count(&held),
        1,
        "the end dropped what a native object holds other than once"
    );
}

/// Derived, because a filter naming no test runs nothing and exits 0, which reads as a pass.
fn own_test_name(leaf: &str) -> String {
    match module_path!().split_once("::") {
        Some((_binary, module)) => format!("{module}::{leaf}"),
        None => leaf.to_string(),
    }
}

/// `PLY_HEAP_POISON` is read once per process, so the poisoned heap runs in a process of its own.
#[test]
fn under_poisoning_a_stale_word_that_reaches_a_reused_native_object_block_is_caught() {
    let name = own_test_name("a_stale_word_meets_a_reused_native_object_block_poisoned");
    let out = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args([name.as_str(), "--exact", "--ignored", "--nocapture"])
        .env("PLY_HEAP_POISON", "1")
        .env_remove("PLY_HEAP_DELAY")
        .output()
        .expect("the test binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the stale read was not caught:\n{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the child ran no test matching `{name}`, and a filter that matches nothing exits 0:\n{stdout}"
    );
}

#[test]
#[ignore = "run under `PLY_HEAP_POISON` by the test above"]
#[should_panic(expected = "a stale read: a block that has died")]
fn a_stale_word_meets_a_reused_native_object_block_poisoned() {
    assert!(poisoning(), "`PLY_HEAP_POISON` is not set");
    let mut ctx = context();
    let stale = ctx.heap.native(holding("first"));
    release_object(&raw mut ctx, stale);
    let tenant = ctx.heap.native(holding("second"));
    assert_eq!(
        tenant, stale,
        "the dead native object's block was not reused"
    );
    assert_eq!(&*held_by(tenant), "second");
    release_object(&raw mut ctx, tenant);
    let payload = unsafe { word_at(stale as *mut Obj, 1) };
    assert_eq!(
        payload,
        poison::word(),
        "the dead native object's payload was not poisoned"
    );
    held_by(stale);
}

/// A block another heap gave out goes back to it, not to the heap of the entry running.
#[test]
fn a_block_dying_under_another_heap_goes_back_to_its_own() {
    let mut outer = Heap::new();
    let mut inner = Heap::new();
    let made = block(&mut outer, 16);
    enter(&mut inner);
    freed(&mut inner, made, 16);
    let other = block(&mut inner, 16);
    assert_ne!(other, made, "a heap took a block of another's");
    leave();
    assert_eq!(
        (inner.live(), outer.live()),
        (1, 0),
        "the block made under the inner heap is its own"
    );
    inner.end();
    outer.end();
}

/// A branch's blocks are the parent's once it adopts the branch's heap, and the parent keeps
/// bumping where it was; the branch's native objects join the parent's table after its own.
#[test]
fn an_adopted_heap_keeps_its_blocks_and_its_native_objects() {
    let mut ctx = context();
    block(&mut ctx.heap, 8);
    let own = ctx.heap.native(holding("the parent's"));
    let mut branch = Heap::new();
    block(&mut branch, 24);
    let adopted = branch.native(holding("the branch's"));
    let (mine, theirs) = (ctx.heap.live(), branch.live());
    ctx.heap.adopt_heap(branch);
    block(&mut ctx.heap, 8);
    assert_eq!(
        ctx.heap.live(),
        mine + theirs + 1,
        "the branch's blocks are the parent's"
    );
    assert_eq!(ctx.heap.natives(), 2);
    assert_eq!(&*held_by(adopted), "the branch's");
    release_object(&raw mut ctx, adopted);
    assert_eq!(ctx.heap.natives(), 1);
    assert_eq!(&*held_by(own), "the parent's");
    ctx.heap.end();
}

#[test]
fn a_census_tells_blocks_by_the_bytes_past_their_header_most_bytes_first() {
    let mut h = Heap::new();
    enter(&mut h);
    for _ in 0..3 {
        block(&mut h, 16);
    }
    block(&mut h, 40);
    let gone = block(&mut h, 40);
    freed(&mut h, gone, 40);
    assert_eq!(h.census(), vec![(16, 3, 48), (40, 1, 40)]);
    leave();
    h.end();
}

#[test]
fn live_bytes_are_the_blocks_live_values_take_and_a_block_given_back_leaves_them() {
    let mut h = Heap::new();
    enter(&mut h);
    assert_eq!(h.live_bytes(), 0);
    let small = block(&mut h, 8);
    block(&mut h, 1 << 20);
    let big = (1 << 20) + HEADER;
    assert_eq!(h.live_bytes(), Heap::object_size(8) + big);
    freed(&mut h, small, 8);
    assert_eq!(h.live_bytes(), big, "the given-back block still counted");
    assert!(h.live_bytes() <= h.held());
    leave();
    h.end();
}

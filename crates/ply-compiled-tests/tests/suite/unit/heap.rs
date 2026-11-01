use crate::bare::new_context;
use crate::unit::ignored_test_command;
use ply_compiled::emitted::Block;
use ply_compiled::heap::{
    HEADER, Heap, Native, Object, Word, enter, leave, native_of, poison_freed, poisoning_enabled,
    release_object, word_at,
};
use ply_compiled::rt::Context;
use std::sync::Arc;

fn block(h: &mut Heap, payload: usize) -> Word {
    unsafe { std::mem::transmute::<Block, Word>(Block::alloc(h, (HEADER + payload) as Word)) }
}

fn freed(h: &mut Heap, w: Word, payload: usize) {
    unsafe { std::mem::transmute::<Word, Block>(w).free(h, (HEADER + payload) as Word) }
}

pub(super) struct Holding(pub(super) Arc<str>);

impl Native for Holding {
    fn release(&mut self, _context: *mut Context) {}
}

fn holding(text: &str) -> Box<dyn Native> {
    Box::new(Holding(Arc::from(text)))
}

fn held_by(w: Word) -> Arc<str> {
    let held = unsafe { native_of::<Holding>(w) }.expect("a native object holding text");
    Arc::clone(&held.0)
}

#[test]
fn ending_a_heap_frees_all_its_memory_and_lets_it_allocate_again() {
    let mut h = Heap::new();
    block(&mut h, 8);
    block(&mut h, 200 << 20);
    assert!(h.retained_bytes() > 200 << 20);
    h.end();
    assert_eq!((h.retained_bytes(), h.live()), (0, 0));
    block(&mut h, 8);
    assert_eq!(h.live(), 1, "an ended heap allocates afresh");
}

#[test]
fn a_heap_reserves_little_more_memory_than_its_blocks_need() {
    let mut h = Heap::new();
    for _ in 0..4096 {
        block(&mut h, 4096);
    }
    let held = h.retained_bytes();
    assert!(held < 4096 * (4096 + HEADER) * 5 / 4, "{held} bytes held");
    let mut big = Heap::new();
    block(&mut big, 1 << 20);
    assert_eq!(big.retained_bytes(), (1 << 20) + HEADER);
}

#[test]
fn memory_freed_from_blocks_of_one_size_is_reused_for_blocks_of_another_size() {
    let mut h = Heap::new();
    enter(&mut h);
    let small: Vec<Word> = (0..100_000).map(|_| block(&mut h, 8)).collect();
    let peak = h.retained_bytes();
    for w in small {
        freed(&mut h, w, 8);
    }
    for _ in 0..30_000 {
        block(&mut h, 40);
    }
    assert!(
        h.retained_bytes() <= peak,
        "{} bytes held past {peak}",
        h.retained_bytes()
    );
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
fn the_block_of_a_freed_native_object_is_reused_for_the_next_native_object() {
    let mut context = new_context();
    let first = context.heap.native(holding("first"));
    release_object(&raw mut context, first);
    let again = context.heap.native(holding("second"));
    assert_eq!(
        again, first,
        "a dead native object's block is the next native object's"
    );
    assert_eq!(&*held_by(again), "second");
    assert_eq!(context.heap.natives(), 1);
}

#[test]
fn the_heap_lists_each_live_native_object_once_and_ending_the_heap_frees_each_native_object_once() {
    let mut context = new_context();
    let held: Arc<str> = Arc::from("held");
    let words: Vec<Word> = (0..8)
        .map(|_| context.heap.native(Box::new(Holding(Arc::clone(&held)))))
        .collect();
    for i in [3, 0, 7, 4] {
        release_object(&raw mut context, words[i]);
    }
    assert_eq!(
        context.heap.natives(),
        4,
        "a dead native object stayed in the table"
    );
    assert_eq!(Arc::strong_count(&held), 5);

    let reused: Vec<Word> = (0..4)
        .map(|i| context.heap.native(holding(&i.to_string())))
        .collect();
    assert!(
        reused.iter().all(|w| words.contains(w)),
        "a native object took fresh memory with dead ones' blocks on hand"
    );
    assert_eq!(context.heap.natives(), 8);
    for (i, w) in reused.iter().enumerate() {
        assert_eq!(&*held_by(*w), i.to_string());
    }
    for i in [1, 2, 5, 6] {
        assert!(Arc::ptr_eq(&held_by(words[i]), &held));
    }

    for w in [reused[1], words[6], reused[3], words[1], reused[0]] {
        release_object(&raw mut context, w);
    }
    assert_eq!(context.heap.natives(), 3);
    assert_eq!(Arc::strong_count(&held), 3);

    let mut other = new_context();
    release_object(&raw mut other, words[5]);
    assert_eq!(context.heap.natives(), 3);
    assert_eq!(Arc::strong_count(&held), 2);
    context.heap.end();
    assert_eq!(context.heap.natives(), 0);
    assert_eq!(
        Arc::strong_count(&held),
        1,
        "the end dropped what a native object holds other than once"
    );
}

#[test]
fn with_heap_poisoning_on_using_a_freed_native_object_pointer_after_its_block_is_reused_is_detected()
 {
    let out = ignored_test_command(
        using_a_pointer_to_a_freed_native_object_finds_its_block_overwritten_with_poison_values,
    )
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
        "the child ran no test, and a filter that matches nothing exits 0:\n{stdout}"
    );
}

#[test]
#[ignore = "run under `PLY_HEAP_POISON` by the test above"]
#[should_panic(expected = "a stale read: a block that has died")]
fn using_a_pointer_to_a_freed_native_object_finds_its_block_overwritten_with_poison_values() {
    assert!(poisoning_enabled(), "`PLY_HEAP_POISON` is not set");
    let mut context = new_context();
    let stale = context.heap.native(holding("first"));
    release_object(&raw mut context, stale);
    let tenant = context.heap.native(holding("second"));
    assert_eq!(
        tenant, stale,
        "the dead native object's block was not reused"
    );
    assert_eq!(&*held_by(tenant), "second");
    release_object(&raw mut context, tenant);
    let payload = unsafe { word_at(stale as *mut Object, 1) };
    assert_eq!(
        payload,
        poison_freed::word(),
        "the dead native object's payload was not poisoned"
    );
    held_by(stale);
}

#[test]
fn a_block_freed_while_another_heap_is_active_returns_to_the_heap_that_allocated_it() {
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

#[test]
fn merging_a_branch_heap_into_a_parent_heap_moves_the_branch_blocks_and_native_objects_into_the_parent()
 {
    let mut context = new_context();
    block(&mut context.heap, 8);
    let own = context.heap.native(holding("the parent's"));
    let mut branch = Heap::new();
    block(&mut branch, 24);
    let adopted = branch.native(holding("the branch's"));
    let (mine, theirs) = (context.heap.live(), branch.live());
    context.heap.adopt_heap(branch);
    block(&mut context.heap, 8);
    assert_eq!(
        context.heap.live(),
        mine + theirs + 1,
        "the branch's blocks are the parent's"
    );
    assert_eq!(context.heap.natives(), 2);
    assert_eq!(&*held_by(adopted), "the branch's");
    release_object(&raw mut context, adopted);
    assert_eq!(context.heap.natives(), 1);
    assert_eq!(&*held_by(own), "the parent's");
    context.heap.end();
}

#[test]
fn the_heap_census_groups_live_blocks_by_their_size_and_lists_the_group_with_most_bytes_first() {
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
fn live_bytes_count_the_blocks_of_live_values_and_drop_a_block_once_it_is_freed() {
    let mut h = Heap::new();
    enter(&mut h);
    assert_eq!(h.live_bytes(), 0);
    let small = block(&mut h, 8);
    block(&mut h, 1 << 20);
    let big = (1 << 20) + HEADER;
    assert_eq!(h.live_bytes(), Heap::object_size(8) + big);
    freed(&mut h, small, 8);
    assert_eq!(h.live_bytes(), big, "the given-back block still counted");
    assert!(h.live_bytes() <= h.retained_bytes());
    leave();
    h.end();
}

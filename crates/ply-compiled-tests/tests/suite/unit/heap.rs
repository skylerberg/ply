use ply_compiled::emitted::Block;
use ply_compiled::heap::{
    HEADER, Heap, Obj, Word, bridged, enter, leave, obj, poison, poisoning, release_handle, word_at,
};
use ply_core::Value;
use std::sync::Arc;

/// A block of `payload` bytes past its header, from `h`: its word, which a `Block` is.
fn block(h: &mut Heap, payload: usize) -> Word {
    unsafe { std::mem::transmute::<Block, Word>(Block::alloc(h, (HEADER + payload) as Word)) }
}

/// Gives back the block `w` of `payload` bytes past its header to `h`, the heap the entry runs on.
fn freed(h: &mut Heap, w: Word, payload: usize) {
    unsafe { std::mem::transmute::<Word, Block>(w).free(h, (HEADER + payload) as Word) }
}

/// The value bridge `w` holds.
fn bridged_value(w: Word) -> Value {
    unsafe { bridged(obj(w)) }.clone()
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
    let bridge = h.bridge(Value::Float(1.5));
    release_handle(bridge);
    let again = h.bridge(Value::Float(2.5));
    assert_eq!(again, bridge, "a dead bridge's block is the next bridge's");
    assert_eq!(bridged_value(again), Value::Float(2.5));
    leave();
    h.end();
}

/// The end drops what the table lists, so a block listed twice would drop its tenant twice.
#[test]
fn the_bridge_table_lists_each_live_bridge_once_and_the_end_drops_each_once() {
    let mut h = Heap::new();
    let held: Arc<str> = Arc::from("held");
    enter(&mut h);
    let words: Vec<Word> = (0..8).map(|_| h.bridge(Value::Str(held.clone()))).collect();
    for i in [3, 0, 7, 4] {
        release_handle(words[i]);
    }
    assert_eq!(h.bridges(), 4, "a dead bridge stayed in the table");
    assert_eq!(Arc::strong_count(&held), 5);

    let reused: Vec<Word> = (0..4).map(|i| h.bridge(Value::Int(i))).collect();
    assert!(
        reused.iter().all(|w| words.contains(w)),
        "a bridge took fresh memory with dead bridges' blocks on hand"
    );
    assert_eq!(h.bridges(), 8);
    for (i, w) in reused.iter().enumerate() {
        assert_eq!(bridged_value(*w), Value::Int(i as i64));
    }
    for i in [1, 2, 5, 6] {
        assert_eq!(bridged_value(words[i]), Value::Str(held.clone()));
    }

    // Out of listing order, so removals move entries into the holes they leave.
    for w in [reused[1], words[6], reused[3], words[1], reused[0]] {
        release_handle(w);
    }
    assert_eq!(h.bridges(), 3);
    assert_eq!(Arc::strong_count(&held), 3);

    // With no heap entered the block cannot go back, so it stays listed, dead.
    leave();
    release_handle(words[5]);
    assert_eq!(h.bridges(), 3);
    assert_eq!(Arc::strong_count(&held), 2);
    h.end();
    assert_eq!(h.bridges(), 0);
    assert_eq!(
        Arc::strong_count(&held),
        1,
        "the end dropped a bridged value other than once"
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
fn under_poisoning_a_stale_word_that_reaches_a_reused_bridge_block_is_caught() {
    let name = own_test_name("a_stale_word_meets_a_reused_bridge_block_poisoned");
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
fn a_stale_word_meets_a_reused_bridge_block_poisoned() {
    assert!(poisoning(), "`PLY_HEAP_POISON` is not set");
    let mut h = Heap::new();
    enter(&mut h);
    let stale = h.bridge(Value::Float(1.5));
    release_handle(stale);
    let tenant = h.bridge(Value::Float(2.5));
    assert_eq!(tenant, stale, "the dead bridge's block was not reused");
    assert_eq!(bridged_value(tenant), Value::Float(2.5));
    release_handle(tenant);
    let payload = unsafe { word_at(stale as *mut Obj, 1) };
    assert_eq!(
        payload,
        poison::word(),
        "the dead bridge's payload was not poisoned"
    );
    bridged_value(stale);
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
/// bumping where it was.
#[test]
fn an_adopted_heap_keeps_its_blocks_and_its_bridges() {
    let mut parent = Heap::new();
    block(&mut parent, 8);
    let mut branch = Heap::new();
    block(&mut branch, 24);
    let float = branch.bridge(Value::Float(2.5));
    let (mine, theirs) = (parent.live(), branch.live());
    parent.adopt_heap(branch);
    block(&mut parent, 8);
    assert_eq!(
        parent.live(),
        mine + theirs + 1,
        "the branch's blocks are the parent's"
    );
    assert_eq!(bridged_value(float), Value::Float(2.5));
    enter(&mut parent);
    release_handle(float);
    leave();
    assert_eq!(parent.bridges(), 0);
    parent.end();
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

use ply_compiled::heap::{self, Heap, KIND_DEAD, Word, dec, inc, kind, obj};
use ply_compiled::list::{get, kids, leaf_of, len, root, to_vec, unique_throughout};

fn ints(n: usize) -> Vec<Word> {
    (0..n).map(|i| heap::imm(i as i64)).collect()
}

fn values(w: Word) -> Vec<i64> {
    to_vec(obj(w))
        .into_iter()
        .map(|w| heap::as_int(w).unwrap())
        .collect()
}

#[test]
fn a_list_built_from_items_reads_back_across_the_tail_and_the_trie() {
    let mut h = Heap::new();
    for n in [0, 1, 4, 31, 32, 33, 64, 65, 1024, 1025, 3000] {
        let w = h.list_from(&ints(n));
        assert_eq!(len(obj(w)), n);
        assert_eq!(values(w), (0..n as i64).collect::<Vec<_>>(), "n = {n}");
        for i in 0..n {
            assert_eq!(
                heap::as_int(get(obj(w), i)),
                Some(i as i64),
                "n = {n}, i = {i}"
            );
        }
    }
    h.end();
}

#[test]
fn a_list_s_leaves_are_read_in_order_from_the_one_its_first_item_lies_in() {
    let mut h = Heap::new();
    for n in [0, 31, 32, 33, 64, 1024, 1025, 1100, 33000] {
        let w = h.list_from(&ints(n));
        let o = obj(w);
        let leaves = (0..).take_while(|j| leaf_of(o, *j) != 0).count();
        // A list of more than 32 items made at once holds every whole 32 of them in its trie.
        assert_eq!(leaves, if n > 32 { n / 32 } else { 0 }, "n = {n}");
        for j in 0..leaves as i64 {
            let leaf = kids(obj(leaf_of(o, j)));
            assert_eq!(leaf.len(), 32);
            assert_eq!(heap::as_int(leaf[0]), Some(j * 32), "n = {n}, j = {j}");
        }
        assert_eq!(leaf_of(o, -1), 0);
        dec(w);
    }
    // A `rest` cut's first leaf is the one its first item lies in.
    let xs = h.list_from(&ints(2000));
    let ys = h.list_skip(xs, 1500);
    let o = obj(ys);
    assert_eq!(
        heap::as_int(kids(obj(leaf_of(o, 0)))[1500 % 32]),
        Some(1500)
    );
    assert_eq!(heap::as_int(kids(obj(leaf_of(o, 1)))[0]), Some(1504));
    assert_eq!(leaf_of(o, (1984 - 1472) / 32), 0);
    dec(ys);
    dec(xs);
    h.end();
}

#[test]
fn pushing_onto_a_list_held_once_writes_in_place_and_onto_a_shared_one_copies_a_tail() {
    let mut h = Heap::new();
    let mut xs = h.list_from(&[]);
    for i in 0..2000 {
        let before = xs;
        xs = h.list_push(xs, heap::imm(i));
        // A tail that ran out of room moved the list once; every other push stayed.
        if i >= 32 {
            assert_eq!(xs, before, "push {i} moved a list nobody else held");
        }
    }
    assert_eq!(values(xs), (0..2000).collect::<Vec<_>>());
    inc(xs);
    let ys = h.list_push(xs, heap::imm(2000));
    assert_ne!(ys, xs);
    assert_eq!(len(obj(xs)), 2000);
    assert_eq!(len(obj(ys)), 2001);
    assert_eq!(heap::as_int(get(obj(ys), 2000)), Some(2000));
    assert_eq!(heap::as_int(get(obj(ys), 1999)), Some(1999));
    assert!(!unique_throughout(ys), "the trie is shared with `xs`");
    dec(xs);
    assert!(unique_throughout(ys), "`xs` gone, the trie is `ys`'s alone");
    h.end();
}

#[test]
fn a_rest_shares_the_trie_and_a_chain_of_them_ends_holding_only_a_tail() {
    let mut h = Heap::new();
    let xs = h.list_from(&ints(100));
    let r = root(obj(xs));
    let ys = h.list_skip(xs, 3);
    assert_eq!(root(obj(ys)), r, "the trie is shared");
    assert_eq!(values(ys), (3..100).collect::<Vec<_>>());
    let mut zs = ys;
    for _ in 0..90 {
        let next = h.list_skip(zs, 1);
        dec(zs);
        zs = next;
    }
    assert_eq!(values(zs), (93..100).collect::<Vec<_>>());
    assert_eq!(
        root(obj(zs)),
        r,
        "three leaves hold the first 96, so the trie is still read"
    );
    for _ in 0..4 {
        let next = h.list_skip(zs, 1);
        dec(zs);
        zs = next;
    }
    assert_eq!(values(zs), (97..100).collect::<Vec<_>>());
    assert_eq!(
        root(obj(zs)),
        0,
        "the prefix covers the trie, so the tail is the list"
    );
    dec(xs);
    assert_eq!(kind(r), KIND_DEAD, "nothing holds the trie any more");
    assert_eq!(values(zs), (97..100).collect::<Vec<_>>());
    h.end();
}

#[test]
fn a_list_dying_releases_its_leaves_and_a_shared_leaf_survives() {
    let mut h = Heap::new();
    let xs = h.list_from(&ints(40));
    let ys = h.list_skip(xs, 1);
    let leaf = root(obj(xs));
    dec(xs);
    assert_ne!(kind(leaf), KIND_DEAD);
    dec(ys);
    assert_eq!(kind(leaf), KIND_DEAD);
    h.end();
}

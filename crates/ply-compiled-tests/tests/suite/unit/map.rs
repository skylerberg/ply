use ply_compiled::heap::{self, Heap, Layouts, Word, dec, inc, kind, obj};
use ply_compiled::map::{at, get, height, len, root, to_vec};
use ply_core::Symbol;

fn layouts() -> Layouts {
    Layouts::new(vec![(Symbol::new("Some"), 1), (Symbol::new("None"), 0)])
}

fn entries(m: Word) -> Vec<(i64, i64)> {
    to_vec(obj(m))
        .into_iter()
        .map(|(k, v)| (heap::as_int(k).unwrap(), heap::as_int(v).unwrap()))
        .collect()
}

fn pair(entry: Option<(Word, Word)>) -> Option<(i64, i64)> {
    entry.map(|(k, v)| (heap::as_int(k).unwrap(), heap::as_int(v).unwrap()))
}

/// Every place is the one the entries in order have: what a branch's sizes say, checked against a
/// walk that never reads them. The height the map keeps is the depth of every leaf.
fn placed(m: Word) {
    let all = entries(m);
    assert_eq!(len(obj(m)), all.len());
    for (i, (k, v)) in all.iter().enumerate() {
        assert_eq!(pair(at(obj(m), i)), Some((*k, *v)), "the entry at {i}");
    }
    assert!(at(obj(m), all.len()).is_none());
    let r = root(obj(m));
    if r != 0 {
        let d = depths(r, 0);
        assert_eq!(d, vec![height(obj(m)); d.len()], "the height the map keeps");
    } else {
        assert_eq!(height(obj(m)), 0, "the empty map's height");
    }
}

/// The depth of each leaf under `node`, which stands `depth` below the root, in key order.
fn depths(node: Word, depth: usize) -> Vec<usize> {
    if kind(node) == heap::KIND_MLEAF {
        return vec![depth];
    }
    (0..unsafe { (*obj(node)).len } as usize)
        .flat_map(|i| depths(unsafe { heap::word_at(obj(node), i) }, depth + 1))
        .collect()
}

#[test]
fn a_map_built_from_sorted_entries_keeps_its_height() {
    let mut h = Heap::new();
    for n in [0usize, 1, 32, 33, 1024, 1025, 40_000] {
        let pairs: Vec<(Word, Word)> = (0..n as i64)
            .map(|i| (heap::imm(i), heap::imm(-i)))
            .collect();
        let m = h.map_from_sorted(&pairs);
        placed(m);
        dec(m);
    }
    h.end();
}

#[test]
fn inserts_in_any_order_read_back_sorted_across_leaves_and_branches() {
    let mut h = Heap::new();
    let l = layouts();
    let mut m = h.map_new();
    let n = 5000i64;
    for i in 0..n {
        let k = i * 7919 % 100003;
        m = h.map_insert(&l, m, heap::imm(k), heap::imm(i));
    }
    assert_eq!(len(obj(m)), n as usize);
    let got = entries(m);
    let mut want: Vec<(i64, i64)> = (0..n).map(|i| (i * 7919 % 100003, i)).collect();
    want.sort();
    assert_eq!(got, want);
    for (k, v) in &want {
        assert_eq!(
            heap::as_int(get(&l, obj(m), heap::imm(*k)).unwrap()),
            Some(*v)
        );
    }
    assert!(get(&l, obj(m), heap::imm(-1)).is_none());
    // A replaced key keeps the count and takes the value.
    m = h.map_insert(&l, m, heap::imm(want[10].0), heap::imm(-7));
    assert_eq!(len(obj(m)), n as usize);
    assert_eq!(
        heap::as_int(get(&l, obj(m), heap::imm(want[10].0)).unwrap()),
        Some(-7)
    );
    h.end();
}

#[test]
fn a_shared_insert_copies_one_path_and_leaves_the_original_whole() {
    let mut h = Heap::new();
    let l = layouts();
    let mut m = h.map_new();
    for i in 0..3000i64 {
        m = h.map_insert(&l, m, heap::imm(i), heap::imm(i));
    }
    inc(m);
    let before = h.allocated();
    let other = h.map_insert(&l, m, heap::imm(100_000), heap::imm(1));
    let copied = h.allocated() - before;
    assert!(
        copied <= 6,
        "a shared insert copied {copied} nodes for a map of three levels"
    );
    assert_ne!(other, m);
    assert_eq!(len(obj(m)), 3000);
    assert_eq!(len(obj(other)), 3001);
    assert!(get(&l, obj(m), heap::imm(100_000)).is_none());
    assert!(get(&l, obj(other), heap::imm(100_000)).is_some());
    dec(other);
    assert_eq!(len(obj(m)), 3000);
    assert_eq!(entries(m).len(), 3000);
    h.end();
}

#[test]
fn an_equal_key_keeps_the_key_the_map_holds_and_takes_the_value() {
    let mut h = Heap::new();
    let l = layouts();
    let first = h.to_word(&l, &ply_core::Value::str("k"));
    let again = h.to_word(&l, &ply_core::Value::str("k"));
    assert_ne!(first, again);
    let m = h.map_new();
    let m = h.map_insert(&l, m, first, heap::imm(1));
    let m = h.map_insert(&l, m, again, heap::imm(2));
    assert_eq!(len(obj(m)), 1);
    let (k, v) = at(obj(m), 0).unwrap();
    assert_eq!((k, heap::as_int(v)), (first, Some(2)));
    dec(m);
    h.end();
}

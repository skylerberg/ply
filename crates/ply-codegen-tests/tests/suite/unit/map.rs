use ply_codegen::heap::{self, Heap, KIND_DEAD, Layouts, Word, dec, inc, kind, obj};
use ply_codegen::map::{get, len, root, to_vec};
use ply_span::Symbol;

fn layouts() -> Layouts {
    Layouts::new(vec![(Symbol::new("Some"), 1), (Symbol::new("None"), 0)])
}

fn entries(m: Word) -> Vec<(i64, i64)> {
    to_vec(obj(m))
        .into_iter()
        .map(|(k, v)| (heap::as_int(k).unwrap(), heap::as_int(v).unwrap()))
        .collect()
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
fn removals_empty_leaves_and_collapse_the_root_and_a_dying_map_lets_its_nodes_go() {
    let mut h = Heap::new();
    let l = layouts();
    let mut m = h.map_new();
    for i in 0..1000i64 {
        m = h.map_insert(&l, m, heap::imm(i), heap::imm(i));
    }
    for i in (0..1000i64).filter(|i| i % 3 != 0) {
        m = h.map_remove(&l, m, heap::imm(i));
    }
    let got = entries(m);
    let want: Vec<(i64, i64)> = (0..1000).filter(|i| i % 3 == 0).map(|i| (i, i)).collect();
    assert_eq!(got, want);
    for i in (0..1000i64).filter(|i| i % 3 == 0) {
        m = h.map_remove(&l, m, heap::imm(i));
    }
    assert_eq!(len(obj(m)), 0);
    assert_eq!(root(obj(m)), 0);
    m = h.map_insert(&l, m, heap::imm(5), heap::imm(5));
    let leaf = root(obj(m));
    dec(m);
    assert_eq!(kind(leaf), KIND_DEAD);
    h.end();
}

use ply_codegen::heap::{self, Heap, KIND_DEAD, Layouts, Word, dec, inc, kind, obj, str_of};
use ply_codegen::map::{at, for_each_from, get, len, locate, root, to_vec};
use ply_eval::Symbol;

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

/// The map `2 * i -> i` for `i` below `n`, so every odd key is absent.
fn evens(h: &mut Heap, l: &Layouts, n: i64) -> Word {
    let mut m = h.map_new();
    for i in 0..n {
        m = h.map_insert(l, m, heap::imm(2 * i), heap::imm(i));
    }
    m
}

/// Every place, and every key's place, is the one the entries in order have: what a branch's
/// sizes say, checked against a walk that never reads them.
fn placed(l: &Layouts, m: Word) {
    let all = entries(m);
    assert_eq!(len(obj(m)), all.len());
    for (i, (k, v)) in all.iter().enumerate() {
        assert_eq!(pair(at(obj(m), i)), Some((*k, *v)), "the entry at {i}");
        let (below, found) = locate(l, obj(m), heap::imm(*k));
        assert_eq!((below, found.and_then(heap::as_int)), (i, Some(*v)));
    }
    assert!(at(obj(m), all.len()).is_none());
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

#[test]
fn every_place_in_key_order_is_found_through_inserts_and_removals() {
    let mut h = Heap::new();
    let l = layouts();
    let mut m = h.map_new();
    for i in 0..5000i64 {
        m = h.map_insert(&l, m, heap::imm(i * 7919 % 100003), heap::imm(i));
    }
    placed(&l, m);
    for i in (0..5000i64).filter(|i| i % 3 != 0) {
        m = h.map_remove(&l, m, heap::imm(i * 7919 % 100003));
    }
    placed(&l, m);
    let all = entries(m);
    for probe in [-1, all[0].0 + 1, all[700].0 - 1, all[700].0 + 1, 200_000] {
        let below = all.iter().filter(|(k, _)| *k < probe).count();
        let held = all.iter().find(|(k, _)| *k == probe).map(|(_, v)| *v);
        let (placed_below, found) = locate(&l, obj(m), heap::imm(probe));
        assert_eq!(
            (placed_below, found.and_then(heap::as_int)),
            (below, held),
            "{probe}"
        );
    }
    for (start, most) in [
        (0, 0),
        (0, 5),
        (31, 2),
        (32, 70),
        (1600, 100),
        (all.len(), 3),
        (9000, 1),
    ] {
        let mut walked = Vec::new();
        for_each_from(obj(m), start, most, |k, v| {
            walked.push(pair(Some((k, v))).unwrap())
        });
        let want: Vec<(i64, i64)> = all.iter().skip(start).take(most).copied().collect();
        assert_eq!(walked, want, "{most} from {start}");
    }
    let empty = h.map_new();
    assert_eq!(
        locate(&l, obj(empty), heap::imm(1)).0,
        0,
        "nothing lies below a key in the empty map"
    );
    assert!(at(obj(empty), 0).is_none());
    h.end();
}

#[test]
fn a_pop_of_a_map_held_once_writes_in_place_and_of_a_shared_one_copies_a_path() {
    let mut h = Heap::new();
    let l = layouts();
    let n = 3000i64;
    let mut m = evens(&mut h, &l, n);
    let before = h.allocated();
    let (mut lo, mut hi) = (0, n - 1);
    while hi - lo > 1000 {
        let (rest, least) = h.map_pop(&l, m, false);
        assert_eq!(pair(least), Some((2 * lo, lo)));
        let (rest, greatest) = h.map_pop(&l, rest, true);
        assert_eq!(pair(greatest), Some((2 * hi, hi)));
        m = rest;
        lo += 1;
        hi -= 1;
    }
    assert_eq!(h.allocated(), before, "a pop of a map held once allocated");
    assert_eq!(
        entries(m),
        (lo..=hi).map(|i| (2 * i, i)).collect::<Vec<_>>()
    );
    placed(&l, m);
    inc(m);
    let before = h.allocated();
    let (rest, least) = h.map_pop(&l, m, false);
    let copied = h.allocated() - before;
    assert!(
        copied <= 4,
        "a shared pop copied {copied} objects for a map of three levels"
    );
    assert_eq!(pair(least), Some((2 * lo, lo)));
    assert_ne!(rest, m);
    assert_eq!(len(obj(m)), (hi - lo + 1) as usize);
    assert_eq!(len(obj(rest)), (hi - lo) as usize);
    placed(&l, m);
    placed(&l, rest);
    dec(rest);
    dec(m);
    let empty = h.map_new();
    let (same, nothing) = h.map_pop(&l, empty, false);
    assert_eq!(same, empty);
    assert!(nothing.is_none());
    let one = h.map_insert(&l, same, heap::imm(5), heap::imm(6));
    let (none_left, only) = h.map_pop(&l, one, true);
    assert_eq!(pair(only), Some((5, 6)));
    assert_eq!((len(obj(none_left)), root(obj(none_left))), (0, 0));
    dec(none_left);
    assert!(h.live_by_kind().is_empty(), "{:?}", h.live_by_kind());
    h.end();
}

#[test]
fn a_split_cuts_at_a_key_present_or_absent_and_both_halves_know_their_places() {
    let l = layouts();
    for n in [0i64, 1, 2, 31, 32, 33, 64, 1000, 5000] {
        for probe in [-1, 0, 1, n, n + 1, 2 * n - 2, 2 * n - 1, 2 * n + 7] {
            let mut h = Heap::new();
            let m = evens(&mut h, &l, n);
            let before = h.allocated();
            let (below, value, above) = h.map_split(&l, m, heap::imm(probe));
            let made = h.allocated() - before;
            assert!(
                made <= 5,
                "a split of a map of {n} held once made {made} objects"
            );
            let all = (0..n).map(|i| (2 * i, i));
            assert_eq!(
                entries(below),
                all.clone().filter(|(k, _)| *k < probe).collect::<Vec<_>>(),
                "below {probe} of {n}"
            );
            assert_eq!(
                entries(above),
                all.clone().filter(|(k, _)| *k > probe).collect::<Vec<_>>(),
                "above {probe} of {n}"
            );
            let held = (probe >= 0 && probe % 2 == 0 && probe / 2 < n).then_some(probe / 2);
            assert_eq!(value.and_then(heap::as_int), held, "at {probe} of {n}");
            placed(&l, below);
            placed(&l, above);
            // Each half is a map like any other: it takes the key back and gives one up.
            let below = h.map_insert(&l, below, heap::imm(probe), heap::imm(-1));
            let above = h.map_remove(&l, above, heap::imm(probe + 1));
            let above = h.map_remove(&l, above, heap::imm(probe + 2));
            placed(&l, below);
            placed(&l, above);
            dec(below);
            dec(above);
            assert!(
                h.live_by_kind().is_empty(),
                "{probe} of {n}: {:?}",
                h.live_by_kind()
            );
            h.end();
        }
    }
}

#[test]
fn a_split_of_a_shared_map_leaves_the_original_whole() {
    let mut h = Heap::new();
    let l = layouts();
    let n = 3000i64;
    let m = evens(&mut h, &l, n);
    for probe in [-1, 2999, 3000, 6001] {
        inc(m);
        let before = h.allocated();
        let (below, value, above) = h.map_split(&l, m, heap::imm(probe));
        let copied = h.allocated() - before;
        assert!(
            copied <= 8,
            "a shared split at {probe} copied {copied} objects for a map of three levels"
        );
        assert_eq!(
            value.and_then(heap::as_int),
            (probe == 3000).then_some(1500)
        );
        assert_eq!(entries(m), (0..n).map(|i| (2 * i, i)).collect::<Vec<_>>());
        assert_eq!(
            len(obj(below)) + len(obj(above)) + usize::from(value.is_some()),
            n as usize
        );
        placed(&l, below);
        placed(&l, above);
        dec(below);
        dec(above);
    }
    placed(&l, m);
    dec(m);
    assert!(h.live_by_kind().is_empty(), "{:?}", h.live_by_kind());
    h.end();
}

#[test]
fn a_pop_and_a_split_hand_over_each_key_and_value_exactly_once() {
    let mut h = Heap::new();
    let l = layouts();
    let mut m = h.map_new();
    for i in 0..200 {
        let (k, v) = (h.str(&format!("key {i:03}")), h.str(&format!("value {i}")));
        m = h.map_insert(&l, m, k, v);
    }
    let text = |w: Word| unsafe { str_of(obj(w)) }.to_string();
    // Held twice, so every node on the way is a copy that holds its words once more.
    inc(m);
    let probe = h.str("key 100");
    let (below, value, above) = h.map_split(&l, m, probe);
    dec(probe);
    let value = value.expect("the key is held");
    assert_eq!(text(value), "value 100");
    dec(value);
    let (below, greatest) = h.map_pop(&l, below, true);
    let (k, v) = greatest.expect("a hundred entries lie below");
    assert_eq!((text(k), text(v)), ("key 099".into(), "value 99".into()));
    dec(k);
    dec(v);
    let (above, least) = h.map_pop(&l, above, false);
    let (k, v) = least.expect("ninety-nine entries lie above");
    assert_eq!((text(k), text(v)), ("key 101".into(), "value 101".into()));
    dec(k);
    dec(v);
    assert_eq!((len(obj(below)), len(obj(above))), (99, 98));
    assert_eq!(len(obj(m)), 200);
    dec(below);
    dec(above);
    dec(m);
    assert!(h.live_by_kind().is_empty(), "{:?}", h.live_by_kind());
    h.end();
}

use ply_codegen::heap::*;
use ply_codegen::{list, map};
use ply_eval::{Fields, Symbol, Value};
use std::sync::Arc;

fn layouts() -> Layouts {
    Layouts::new(vec![(Symbol::new("Some"), 1), (Symbol::new("None"), 0)])
}

#[test]
fn two_byte_strings_compare_in_byte_order_at_every_length() {
    let mut h = Heap::new();
    let l = layouts();
    let long: Vec<u8> = (0..40u8).collect();
    let mut longer = long.clone();
    longer.push(0);
    let cases: [(&[u8], &[u8]); 6] = [
        (b"kA", b"kB"),
        (b"kB", b"kA"),
        (b"k", b"kA"),
        (b"kA", b"kA"),
        (&long, &longer),
        (&longer, &long),
    ];
    for (x, y) in cases {
        let (a, b) = (h.bytes(x), h.bytes(y));
        assert_eq!(cmp_words(&l, a, b), x.cmp(y), "{x:?} against {y:?}");
        assert_eq!(
            cmp_words(&l, a, b),
            Heap::to_value(&l, a).cmp(&Heap::to_value(&l, b))
        );
    }
    let (s, b) = (h.str("kA"), h.bytes(b"kA"));
    assert_eq!(
        cmp_words(&l, s, b),
        Heap::to_value(&l, s).cmp(&Heap::to_value(&l, b)),
        "a string against bytes takes the general path"
    );
}

#[test]
fn a_flat_record_is_released_without_a_walk_and_loses_the_flag_when_it_gains_a_count() {
    let mut h = Heap::new();
    let o = h.alloc(KIND_RECORD, FLAT, 2, 0);
    unsafe {
        set_word(o, 0, imm(1));
        set_word(o, 1, imm(2));
    }
    ply_codegen::heap::enter(&mut h);
    dec(o as Word);
    ply_codegen::heap::leave();
    unsafe { assert_eq!((*o).kind, KIND_DEAD) };
    let again = h.alloc(KIND_RECORD, 0, 2, 0);
    assert_eq!(again, o, "a flat record's memory was not recycled");
    let child = h.alloc(KIND_RECORD, 0, 1, 0);
    unsafe {
        set_word(child, 0, imm(1));
        set_word(again, 0, child as Word);
        set_word(again, 1, imm(2));
        (*again).flags = 0;
    }
    ply_codegen::heap::enter(&mut h);
    dec(again as Word);
    ply_codegen::heap::leave();
    unsafe { assert_eq!((*child).kind, KIND_DEAD, "a counted field was not let go") };
}

#[test]
fn a_reset_record_keeps_its_memory_and_lets_its_fields_go() {
    let mut h = Heap::new();
    let child = h.alloc(KIND_RECORD, 0, 1, 0);
    unsafe { set_word(child, 0, imm(1)) };
    let o = h.alloc(KIND_RECORD, 0, 2, 0);
    unsafe {
        set_word(o, 0, child as Word);
        set_word(o, 1, imm(7));
    }
    inc(child as Word);
    assert_eq!(reset(o as Word), o as Word);
    unsafe {
        assert_eq!((*o).len, 0);
        assert_eq!((*o).rc, 1);
        assert_eq!((*o).kind, KIND_RECORD);
        assert_eq!((*child).rc, 1, "the field was let go once");
    }
    dec(o as Word);
    unsafe { assert_eq!((*o).kind, KIND_DEAD) };

    let shared = h.alloc(KIND_RECORD, 0, 1, 0);
    unsafe { set_word(shared, 0, imm(1)) };
    inc(shared as Word);
    assert_eq!(
        reset(shared as Word),
        0,
        "a record held twice is released, not kept"
    );
    unsafe { assert_eq!((*shared).rc, 1) };
    assert_eq!(reset(imm(3)), 0);
    dec(child as Word);
    dec(shared as Word);
}

#[test]
fn an_int_that_fits_is_an_immediate_and_one_that_does_not_is_boxed() {
    let mut h = Heap::new();
    let l = layouts();
    for n in [0i64, 1, -1, i64::MAX >> 1, i64::MIN >> 1] {
        let w = h.boxed_int(n);
        assert!(is_imm(w));
        assert_eq!(as_int(w), Some(n));
    }
    for n in [i64::MAX, i64::MIN, (i64::MAX >> 1) + 1] {
        let w = h.boxed_int(n);
        assert!(!is_imm(w));
        assert_eq!(as_int(w), Some(n));
        assert_eq!(Heap::to_value(&l, w), Value::Int(n));
    }
}

#[test]
fn every_value_kind_round_trips() {
    let mut h = Heap::new();
    let l = layouts();
    let record = Value::Record(Arc::new(Fields::from_unsorted(vec![
        (Symbol::new("b"), Value::Int(2)),
        (
            Symbol::new("a"),
            Value::list(vec![Value::Bool(true), Value::Unit]),
        ),
    ])));
    let values = vec![
        Value::Int(7),
        Value::Bool(false),
        Value::Unit,
        Value::str("hi"),
        Value::bytes(b"raw"),
        record.clone(),
        Value::ctor("Some", vec![record.clone()]),
        Value::ctor("None", vec![]),
        Value::list(vec![Value::Int(1), Value::str("x"), record]),
        Value::map(vec![(Value::Int(1), Value::Int(2))]),
    ];
    for v in values {
        let w = h.to_word(&l, &v);
        assert_eq!(Heap::to_value(&l, w), v, "{v:?}");
    }
    h.end();
}

#[test]
fn a_record_is_laid_out_in_sorted_field_order_whatever_order_it_was_written_in() {
    let mut h = Heap::new();
    let l = layouts();
    let v = Value::Record(Arc::new(Fields::from_unsorted(vec![
        (Symbol::new("z"), Value::Int(26)),
        (Symbol::new("a"), Value::Int(1)),
    ])));
    let w = h.to_word(&l, &v);
    let o = obj(w);
    let shape = unsafe { (*o).layout };
    assert_eq!(l.offset(shape, &Symbol::new("a")), Some(0));
    assert_eq!(l.offset(shape, &Symbol::new("z")), Some(1));
    assert_eq!(as_int(unsafe { word_at(o, 0) }), Some(1));
    assert_eq!(as_int(unsafe { word_at(o, 1) }), Some(26));
}

#[test]
fn the_last_holder_dismantles_and_the_memory_waits_for_the_end() {
    let mut h = Heap::new();
    let l = layouts();
    let inner = Value::list(vec![Value::Int(1)]);
    let w = h.to_word(&l, &Value::ctor("Some", vec![inner]));
    let child = unsafe { word_at(obj(w), 0) };
    inc(w);
    dec(w);
    assert_eq!(kind(w), KIND_CTOR);
    dec(w);
    assert_eq!(kind(w), KIND_DEAD);
    assert_eq!(kind(child), KIND_DEAD);
    h.end();
}

#[test]
fn a_dead_object_is_reused_by_the_next_of_its_class_within_an_entry() {
    let mut h = Heap::new();
    enter(&mut h);
    let l = layouts();
    let first = h.to_word(&l, &Value::list(vec![Value::Int(1), Value::Int(2)]));
    let record = h.to_word(
        &l,
        &Value::Record(Arc::new(Fields::from_unsorted(vec![
            (Symbol::new("a"), Value::Int(1)),
            (Symbol::new("b"), Value::Int(2)),
        ]))),
    );
    dec(first);
    let again = h.to_word(&l, &Value::list(vec![Value::Int(3), Value::Int(4)]));
    assert_eq!(
        again, first,
        "a list of the same class took the dead list's slot"
    );
    assert_eq!(
        Heap::to_value(&l, again),
        Value::list(vec![Value::Int(3), Value::Int(4)])
    );
    dec(record);
    let other = h.to_word(&l, &Value::str("ab"));
    assert_ne!(other, record, "a string is not a record's class");
    let bridged = h.bridge(Value::Float(1.5));
    dec(bridged);
    let bridged_again = h.bridge(Value::Float(2.5));
    assert_eq!(
        bridged_again, bridged,
        "a dead bridge's block is the next bridge's"
    );
    assert_eq!(Heap::to_value(&l, bridged_again), Value::Float(2.5));
    leave();
    h.end();
}

/// The end drops what the table lists, so a block listed twice would drop its tenant twice.
#[test]
fn the_bridge_table_lists_each_live_bridge_once_and_the_end_drops_each_once() {
    let mut h = Heap::new();
    let l = layouts();
    let held: Arc<str> = Arc::from("held");
    enter(&mut h);
    let words: Vec<Word> = (0..8).map(|_| h.bridge(Value::Str(held.clone()))).collect();
    for i in [3, 0, 7, 4] {
        dec(words[i]);
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
        assert_eq!(Heap::to_value(&l, *w), Value::Int(i as i64));
    }
    for i in [1, 2, 5, 6] {
        assert_eq!(Heap::to_value(&l, words[i]), Value::Str(held.clone()));
    }

    // Out of listing order, so removals move entries into the holes they leave.
    for w in [reused[1], words[6], reused[3], words[1], reused[0]] {
        dec(w);
    }
    assert_eq!(h.bridges(), 3);
    assert_eq!(Arc::strong_count(&held), 3);

    // With no heap entered the block cannot go back, so it stays listed, dead.
    leave();
    dec(words[5]);
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
#[should_panic(expected = "a stale read: an object that has died")]
fn a_stale_word_meets_a_reused_bridge_block_poisoned() {
    assert!(poisoning(), "`PLY_HEAP_POISON` is not set");
    let mut h = Heap::new();
    let l = layouts();
    enter(&mut h);
    let stale = h.bridge(Value::Float(1.5));
    dec(stale);
    let tenant = h.bridge(Value::Float(2.5));
    assert_eq!(tenant, stale, "the dead bridge's block was not reused");
    assert_eq!(Heap::to_value(&l, tenant), Value::Float(2.5));
    dec(tenant);
    let payload = unsafe { word_at(stale as *mut Obj, 0) };
    assert_eq!(
        payload,
        poison::word(),
        "the dead bridge's payload was not poisoned"
    );
    Heap::to_value(&l, stale);
}

#[test]
fn a_shared_child_survives_its_parent() {
    let mut h = Heap::new();
    let l = layouts();
    let child = h.to_word(&l, &Value::str("kept"));
    inc(child);
    let parent = h.list_from(&[child]);
    dec(parent);
    assert_eq!(kind(child), KIND_STR);
    assert_eq!(Heap::to_value(&l, child), Value::str("kept"));
    dec(child);
    assert_eq!(kind(child), KIND_DEAD);
    h.end();
}

#[test]
fn appending_to_a_string_nobody_else_holds_writes_in_place_and_to_a_shared_one_copies() {
    let mut h = Heap::new();
    let l = layouts();
    let mut s = h.str("ab");
    let first = s;
    s = h.append(s, b"cd");
    assert_ne!(
        s, first,
        "an exact-sized value has no room, so the first append copies"
    );
    assert_eq!(kind(first), KIND_DEAD);
    let grown = s;
    s = h.append(s, b"e");
    assert_eq!(
        s, grown,
        "the copy left room, so the next append is in place"
    );
    assert_eq!(Heap::to_value(&l, s), Value::str("abcde"));
    inc(s);
    let other = h.append(s, b"f");
    assert_ne!(other, s, "a value held twice is not written into");
    assert_eq!(Heap::to_value(&l, s), Value::str("abcde"));
    assert_eq!(Heap::to_value(&l, other), Value::str("abcdef"));
    let raw = h.bytes(b"\xff\x00");
    assert_eq!(Heap::to_value(&l, raw), Value::bytes(b"\xff\x00"));
    assert_eq!(kind(raw), KIND_BYTES);
    h.end();
}

#[test]
fn words_order_exactly_as_the_values_they_denote() {
    let mut h = Heap::new();
    let l = layouts();
    let record = |a: i64, b: &str| {
        Value::Record(Arc::new(Fields::from_unsorted(vec![
            (Symbol::new("n"), Value::Int(a)),
            (Symbol::new("s"), Value::str(b)),
        ])))
    };
    let values = vec![
        Value::Unit,
        Value::Bool(false),
        Value::Bool(true),
        Value::Int(-3),
        Value::Int(7),
        Value::Int(i64::MAX),
        Value::str("a"),
        Value::str("b"),
        Value::bytes(b"a"),
        Value::list(vec![]),
        Value::list(vec![Value::Int(1)]),
        Value::list(vec![Value::Int(1), Value::Int(0)]),
        Value::map(vec![(Value::Int(1), Value::str("x"))]),
        record(1, "z"),
        record(2, "a"),
        Value::ctor("None", vec![]),
        Value::ctor("Some", vec![Value::Int(1)]),
        Value::ctor("Some", vec![Value::Int(2)]),
    ];
    let words: Vec<Word> = values.iter().map(|v| h.to_word(&l, v)).collect();
    for (i, a) in values.iter().enumerate() {
        for (j, b) in values.iter().enumerate() {
            assert_eq!(
                cmp_words(&l, words[i], words[j]),
                a.cmp(b),
                "{a:?} vs {b:?}"
            );
        }
    }
    h.end();
}

#[test]
fn a_native_map_holds_its_entries_in_the_interpreters_order_and_round_trips() {
    let mut h = Heap::new();
    let l = layouts();
    let mut m = h.map_new();
    for (k, v) in [
        (5, "five"),
        (1, "one"),
        (3, "three"),
        (1, "uno"),
        (4, "four"),
    ] {
        let kw = h.to_word(&l, &Value::Int(k));
        let vw = h.to_word(&l, &Value::str(v));
        m = h.map_insert(&l, m, kw, vw);
    }
    assert_eq!(
        Heap::to_value(&l, m),
        Value::map(vec![
            (Value::Int(1), Value::str("uno")),
            (Value::Int(3), Value::str("three")),
            (Value::Int(4), Value::str("four")),
            (Value::Int(5), Value::str("five")),
        ])
    );
    assert!(map::get(&l, obj(m), imm(3)).is_some());
    assert!(map::get(&l, obj(m), imm(2)).is_none());
    let m = h.map_remove(&l, m, imm(3));
    let m = h.map_remove(&l, m, imm(99));
    assert_eq!(unsafe { (*obj(m)).len }, 3);
    // A shared map is copied by an insert and the original keeps its entries.
    inc(m);
    let kw = h.to_word(&l, &Value::Int(2));
    let m2 = h.map_insert(&l, m, kw, imm(0));
    assert_ne!(m, m2);
    assert_eq!(unsafe { (*obj(m)).len }, 3);
    assert_eq!(unsafe { (*obj(m2)).len }, 4);
    h.end();
}

#[test]
fn an_adopted_word_is_a_copy_that_outlives_the_entry_and_shares_what_was_already_immortal() {
    let l = layouts();
    let mut entry = Heap::new();
    let mut kept = Heap::persistent();
    let shared = kept.immortal(&l, &Value::str("shared"));
    let record = Value::Record(Arc::new(Fields::from_unsorted(vec![
        (Symbol::new("n"), Value::Int(1)),
        (Symbol::new("s"), Value::str("own")),
    ])));
    let w = entry.to_word(&l, &record);
    let list = entry.list_from(&[w, shared]);
    let copy = kept.adopt(list);
    assert_ne!(copy, list);
    assert_eq!(list::get(obj(copy), 1), shared);
    entry.end();
    assert_eq!(
        Heap::to_value(&l, copy),
        Value::list(vec![record, Value::str("shared")])
    );
}

#[test]
fn an_immortal_word_survives_the_end_of_every_entry_and_counts_nothing() {
    let mut h = Heap::persistent();
    let l = layouts();
    let w = h.immortal(&l, &Value::list(vec![Value::str("a"), Value::Int(1)]));
    h.end();
    inc(w);
    dec(w);
    dec(w);
    assert_eq!(
        Heap::to_value(&l, w),
        Value::list(vec![Value::str("a"), Value::Int(1)])
    );
}

#[test]
fn a_word_is_an_object_only_at_a_live_start() {
    let mut heap = Heap::new();
    enter(&mut heap);
    let o = heap.alloc(KIND_RECORD, 0, 2, 0);
    // Filled before the `dec`, as every caller of `alloc` must: until written, the words hold the last tenant's leftovers.
    unsafe {
        set_word(o, 0, imm(1));
        set_word(o, 1, imm(2));
    }
    let w = o as Word;
    assert!(heap.is_object(w));
    assert!(
        !heap.is_object(w + 8),
        "an interior address is not an object"
    );
    assert!(!heap.is_object(imm(7)), "an immediate is not an object");
    assert!(!heap.is_object(0));
    dec(w);
    assert!(!heap.is_object(w), "a released object is not one");
    leave();
    heap.end();
    assert!(!heap.is_object(w), "nothing survives the entry");
}

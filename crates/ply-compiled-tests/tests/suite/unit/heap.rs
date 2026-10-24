use ply_compiled::heap::*;
use ply_compiled::map;
use ply_core::{Fields, Fixed, IntTy, Symbol, Value};
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
fn a_float_is_its_own_object_ordered_and_read_back_as_the_evaluator_orders_and_reads_it() {
    let mut h = Heap::new();
    let l = layouts();
    let nan = f64::from_bits(0x7FF8_0000_0000_0001);
    let floats = [
        f64::NEG_INFINITY,
        -1.5,
        -0.0,
        0.0,
        f64::from_bits(1),
        2.5,
        f64::INFINITY,
        nan,
    ];
    let words: Vec<Word> = floats
        .iter()
        .map(|x| h.to_word(&l, &Value::Float(*x)))
        .collect();
    for (w, x) in words.iter().zip(&floats) {
        assert_eq!(kind(*w), KIND_FLOAT);
        assert_eq!(Heap::to_value(&l, *w), Value::Float(*x));
    }
    for (a, x) in words.iter().zip(&floats) {
        for (b, y) in words.iter().zip(&floats) {
            assert_eq!(
                cmp_words(&l, *a, *b),
                Value::Float(*x).cmp(&Value::Float(*y)),
                "{x} against {y}"
            );
        }
    }
    let canonical = h.to_word(&l, &Value::Float(f64::NAN));
    assert_eq!(
        cmp_words(&l, canonical, words[7]),
        std::cmp::Ordering::Equal,
        "every NaN is one key"
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
    ply_compiled::heap::enter(&mut h);
    dec(o as Word);
    ply_compiled::heap::leave();
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
    ply_compiled::heap::enter(&mut h);
    dec(again as Word);
    ply_compiled::heap::leave();
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
        assert_eq!((*o).len, 2);
        assert_eq!((*o).flags & FLAT, FLAT);
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
fn a_reset_record_released_unused_goes_back_to_its_own_size() {
    let mut h = Heap::new();
    let child = h.alloc(KIND_RECORD, 0, 1, 0);
    unsafe { set_word(child, 0, imm(1)) };
    let o = h.alloc(KIND_RECORD, 0, 3, 0);
    unsafe {
        set_word(o, 0, child as Word);
        set_word(o, 1, imm(2));
        set_word(o, 2, imm(3));
    }
    ply_compiled::heap::enter(&mut h);
    assert_eq!(reset(o as Word), o as Word);
    dec(o as Word);
    ply_compiled::heap::leave();
    unsafe { assert_eq!((*child).kind, KIND_DEAD, "the field was let go") };
    let one = h.alloc(KIND_RECORD, 0, 1, 0);
    assert_ne!(one, o, "a three-word block was filed as a one-word one");
    assert_eq!(h.alloc(KIND_RECORD, 0, 3, 0), o);
}

#[test]
fn an_ended_entry_gives_back_what_it_held() {
    let mut h = Heap::new();
    h.alloc(KIND_RECORD, 0, 1, 0);
    h.alloc_bytes(KIND_BYTES, 200 << 20);
    assert!(h.held() > 200 << 20);
    h.end();
    assert_eq!((h.held(), h.live()), (0, 0));
    h.alloc(KIND_RECORD, 0, 1, 0);
    assert_eq!(h.live(), 1, "an ended heap allocates afresh");
}

/// A block is its object's size rounded up by an eighth at most, and a page ends short of one more.
#[test]
fn a_heap_holds_little_more_than_its_objects_take() {
    let mut h = Heap::new();
    for _ in 0..4096 {
        h.alloc_bytes(KIND_BYTES, 4096);
    }
    let held = h.held();
    assert!(held < 4096 * (4096 + HEADER) * 5 / 4, "{held} bytes held");
    let mut big = Heap::new();
    big.alloc_bytes(KIND_BYTES, 1 << 20);
    assert_eq!(big.held(), (1 << 20) + HEADER);
}

/// What an object held serves an object of any other size once every block beside it is dead.
#[test]
fn memory_one_size_gave_up_serves_another() {
    let mut h = Heap::new();
    enter(&mut h);
    let small: Vec<*mut Obj> = (0..100_000)
        .map(|_| {
            let o = h.alloc(KIND_RECORD, 0, 1, 0);
            unsafe { set_word(o, 0, imm(1)) };
            o
        })
        .collect();
    let peak = h.held();
    for o in small {
        dec(o as Word);
    }
    for _ in 0..40_000 {
        let o = h.alloc(KIND_RECORD, 0, 5, 0);
        for i in 0..5 {
            unsafe { set_word(o, i, imm(1)) };
        }
    }
    assert!(h.held() <= peak, "{} bytes held past {peak}", h.held());
    leave();
    h.end();
}

#[test]
fn a_reset_constructor_keeps_its_memory_and_lets_its_argument_go() {
    let mut h = Heap::new();
    let child = h.alloc(KIND_RECORD, 0, 1, 0);
    unsafe { set_word(child, 0, imm(1)) };
    let o = h.alloc(KIND_CTOR, 0, 1, 0);
    unsafe { set_word(o, 0, child as Word) };
    inc(child as Word);
    assert_eq!(reset(o as Word), o as Word);
    unsafe {
        assert_eq!((*o).len, 1);
        assert_eq!((*o).flags & FLAT, FLAT);
        assert_eq!((*o).rc, 1);
        assert_eq!((*child).rc, 1, "the argument was let go once");
    }
    dec(o as Word);
    let boxed = h.boxed_int(i64::MAX);
    assert_eq!(reset(boxed), 0, "only a record or a constructor is a token");
    dec(child as Word);
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

/// Compiled code holds `()` as the word 0, which no object lies at.
#[test]
fn the_zero_word_is_unit_as_the_runtime_s_own_is() {
    let l = layouts();
    assert_eq!(Heap::to_value(&l, 0), Value::Unit);
    assert_eq!(cmp_words(&l, 0, unit()), std::cmp::Ordering::Equal);
    assert_eq!(
        cmp_words(&l, 0, bool(false)),
        Value::Unit.cmp(&Value::Bool(false))
    );
    assert!(native_key(&l, 0));
}

/// Below 64 bits a width is the immediate compiled code computes with; at 64 bits it is the
/// runtime's own object and reads back as itself.
#[test]
fn a_width_crosses_as_the_word_compiled_code_holds() {
    let mut h = Heap::new();
    let l = layouts();
    let fixed = |ty: IntTy, n: i128| Value::Fixed(Fixed::of(ty, n).expect("a value of the width"));
    for (ty, n) in [
        (IntTy::U8, 255),
        (IntTy::I8, -128),
        (IntTy::U16, 65_535),
        (IntTy::I16, -300),
        (IntTy::U32, 4_294_967_295),
        (IntTy::I32, -2_147_483_648),
    ] {
        assert_eq!(h.to_word(&l, &fixed(ty, n)), imm(n as i64), "{ty} {n}");
    }
    for n in [i128::from(u64::MAX), 1 << 63, 0] {
        let v = fixed(IntTy::U64, n);
        let w = h.to_word(&l, &v);
        assert!(!is_imm(w), "a `U64` is no immediate");
        assert_eq!(Heap::to_value(&l, w), v);
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
    assert_eq!(unsafe { word_at(o, 0) } as i64, 1);
    assert_eq!(unsafe { word_at(o, 1) } as i64, 26);
}

#[test]
fn an_indexed_field_reads_where_a_search_of_its_shape_finds_it() {
    let mut l = layouts();
    let names: Vec<Symbol> = ["a", "b", "z", "a", "q"]
        .into_iter()
        .map(Symbol::new)
        .collect();
    let shapes = [vec!["z", "a"], vec!["b"], vec!["q", "z", "b", "a"], vec![]]
        .map(|fields| l.shape(fields.into_iter().map(Symbol::new).collect()));
    l.index_fields(&names);
    for shape in shapes {
        for (index, name) in names.iter().enumerate() {
            assert_eq!(
                l.offset_by_index(shape, index, &names),
                l.offset(shape, name),
                "`{name}` in shape {shape}"
            );
        }
    }
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

/// A credential handed to compiled code is held by its word and by nothing the heap keeps: once
/// the last holder lets the word go, and once the entry ends, the payload is its owner's alone,
/// however deep in a value it crossed.
#[test]
fn a_credential_crossing_into_a_word_is_let_go_with_the_word() {
    let mut h = Heap::new();
    let l = layouts();
    let secret = Value::secret_text("hunter2");
    let Value::Secret(payload) = &secret else {
        unreachable!("a secret is a `Secret`")
    };
    let payload = Arc::clone(payload);
    for crossing in [
        secret.clone(),
        Value::list(vec![Value::Int(1), secret.clone()]),
        Value::ctor("Some", vec![secret.clone()]),
    ] {
        let w = h.to_word(&l, &crossing);
        assert_eq!(Heap::to_value(&l, w), crossing);
        dec(w);
        drop(crossing);
    }
    h.end();
    drop(secret);
    assert_eq!(
        Arc::strong_count(&payload),
        1,
        "something the heap kept still holds a credential every word of was let go"
    );
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
    // A shared map is copied by an insert and the original keeps its entries.
    inc(m);
    let kw = h.to_word(&l, &Value::Int(2));
    let m2 = h.map_insert(&l, m, kw, imm(0));
    assert_ne!(m, m2);
    assert_eq!(unsafe { (*obj(m)).len }, 4);
    assert_eq!(unsafe { (*obj(m2)).len }, 5);
    h.end();
}

#[test]
fn an_immortal_word_survives_the_end_of_every_entry_and_counts_nothing() {
    let mut h = Heap::persistent();
    let l = layouts();
    let w = h.to_word(&l, &Value::list(vec![Value::str("a"), Value::Int(1)]));
    mark_immortal(w);
    h.end();
    inc(w);
    dec(w);
    dec(w);
    assert_eq!(
        Heap::to_value(&l, w),
        Value::list(vec![Value::str("a"), Value::Int(1)])
    );
}

/// What another thread may hold is marked once, all of it, and is never unique after.
#[test]
fn sharing_marks_everything_under_the_words_and_counts_it_apart() {
    let mut h = Heap::new();
    enter(&mut h);
    let l = layouts();
    let outer = h.to_word(
        &l,
        &Value::ctor(
            "Some",
            vec![Value::list(vec![Value::str("a"), Value::Int(2)])],
        ),
    );
    let inner = unsafe { word_at(obj(outer), 0) };
    assert!(share(&[outer]));
    for w in [outer, inner] {
        assert!(unsafe { (*obj(w)).rc } >= SHARED);
        assert!(!is_unique(w));
    }
    inc(outer);
    dec(outer);
    assert_eq!(kind(outer), KIND_CTOR);
    dec(outer);
    assert_eq!(kind(outer), KIND_DEAD);
    assert_eq!(kind(inner), KIND_DEAD);
    let next = h.to_word(&l, &Value::ctor("Some", vec![Value::Int(1)]));
    assert_eq!(next, outer, "its heap took the dead object's block back");
    leave();
    h.end();
}

/// A thread that is not the heap's own hands a dead object's block back to it.
#[test]
fn an_object_another_thread_lets_go_of_last_is_its_heaps_again() {
    let mut h = Heap::new();
    enter(&mut h);
    let l = layouts();
    let made = h.to_word(&l, &Value::ctor("Some", vec![Value::Int(1)]));
    let filler: Vec<Word> = (0..10_000)
        .map(|i| h.to_word(&l, &Value::ctor("Some", vec![Value::Int(i)])))
        .collect();
    assert!(share(&[made]));
    std::thread::scope(|s| {
        s.spawn(|| dec(made));
    });
    assert_eq!(kind(made), KIND_DEAD);
    let again: Vec<Word> = (0..100_000)
        .map(|i| h.to_word(&l, &Value::ctor("Some", vec![Value::Int(i)])))
        .collect();
    assert!(
        again.contains(&made),
        "the block was never handed out again"
    );
    drop(filler);
    leave();
    h.end();
}

/// An entry running inside another's hands the outer one's dead objects back to the outer heap.
#[test]
fn an_object_dying_under_another_heap_goes_back_to_its_own() {
    let mut outer = Heap::new();
    let mut inner = Heap::new();
    let l = layouts();
    let made = outer.to_word(&l, &Value::ctor("Some", vec![Value::Int(1)]));
    enter(&mut inner);
    dec(made);
    let other = inner.to_word(&l, &Value::ctor("Some", vec![Value::Int(2)]));
    assert_ne!(other, made, "a heap took a block of another's");
    leave();
    assert_eq!(
        (inner.live(), outer.live()),
        (1, 0),
        "the object made under the inner heap is its own"
    );
    inner.end();
    outer.end();
}

/// A task is a handle into this thread's scheduler: nothing under words that reach one is marked.
#[test]
fn sharing_what_reaches_a_task_marks_nothing() {
    let mut h = Heap::new();
    let l = layouts();
    let plain = h.to_word(&l, &Value::list(vec![Value::Int(1)]));
    let task = Value::Task(ply_core::TaskHandle::unowned(
        ply_core::SimId(0),
        ply_core::sim::TaskId(0),
    ));
    let holder = h.to_word(&l, &Value::ctor("Some", vec![task]));
    let handle = unsafe { word_at(obj(holder), 0) };
    assert!(!share(&[plain, holder]));
    for w in [plain, holder, handle] {
        assert!(
            unsafe { (*obj(w)).rc } < SHARED,
            "a refused share left a mark"
        );
    }
    h.end();
}

/// A branch's objects are its block's once the block adopts its heap, and the block keeps bumping
/// where it was.
#[test]
fn an_adopted_heap_keeps_its_objects_and_its_bridges() {
    let l = layouts();
    let mut parent = Heap::new();
    let before = parent.to_word(&l, &Value::str("before"));
    let mut branch = Heap::new();
    let made = branch.to_word(&l, &Value::list(vec![Value::Int(7), Value::str("x")]));
    let float = branch.bridge(Value::Float(2.5));
    let (mine, theirs) = (parent.live(), branch.live());
    parent.adopt_heap(branch);
    let after = parent.to_word(&l, &Value::str("after"));
    assert_eq!(
        parent.live(),
        mine + theirs + 1,
        "the branch's objects are the parent's"
    );
    assert_eq!(
        Heap::to_value(&l, made),
        Value::list(vec![Value::Int(7), Value::str("x")])
    );
    assert_eq!(Heap::to_value(&l, float), Value::Float(2.5));
    assert_eq!(Heap::to_value(&l, before), Value::str("before"));
    assert_eq!(Heap::to_value(&l, after), Value::str("after"));
    enter(&mut parent);
    dec(float);
    leave();
    parent.end();
}

/// A block of two words, as emitted code's C allocates one.
fn emitted(ctx: &mut ply_compiled::rt::Ctx) -> Word {
    type Alloc = unsafe extern "C" fn(*mut ply_compiled::rt::Ctx, Word) -> Word;
    unsafe { crate::bare::helper::<Alloc>("rt_alloc_bytes")(ctx, 16 + 8 * 2) }
}

#[test]
fn a_block_emitted_code_laid_out_is_counted_under_a_kind_of_its_own() {
    let mut ctx = crate::bare::context();
    let w = emitted(&mut ctx);
    assert_eq!(kind(w), KIND_REFUSED);
    assert_eq!(ctx.heap.allocated_by_kind()[KIND_REFUSED as usize], 1);
    assert_eq!(ctx.heap.live_by_kind(), vec![(KIND_REFUSED, 1, 16)]);
}

#[test]
fn a_census_tells_a_record_by_its_fields_and_an_emitted_block_by_its_size_most_bytes_first() {
    let mut ctx = crate::bare::context();
    for _ in 0..3 {
        emitted(&mut ctx);
    }
    let shape = ctx
        .tables
        .layouts
        .shape(vec![Symbol::new("a"), Symbol::new("b")]);
    let r = ctx.heap.alloc(KIND_RECORD, FLAT, 2, shape);
    ctx.heap.alloc_bytes(KIND_STR, 40);
    unsafe {
        set_word(r, 0, imm(1));
        set_word(r, 1, imm(2));
    }
    assert_eq!(
        ctx.census(),
        vec![
            (
                "emitted blocks of 16 bytes past the header".to_string(),
                3,
                48
            ),
            ("strings".to_string(), 1, 40),
            ("{a, b}".to_string(), 1, 16),
        ]
    );
}

#[test]
fn live_bytes_are_the_blocks_live_objects_take_and_a_dead_objects_block_leaves_them() {
    let mut h = Heap::new();
    enter(&mut h);
    assert_eq!(h.live_bytes(), 0);
    let small = h.alloc(KIND_RECORD, FLAT, 1, 0);
    unsafe { set_word(small, 0, imm(1)) };
    h.alloc_bytes(KIND_BYTES, 1 << 20);
    let big = (1 << 20) + HEADER;
    assert_eq!(h.live_bytes(), Heap::object_size(8) + big);
    dec(small as Word);
    assert_eq!(h.live_bytes(), big, "the dead object's block still counted");
    assert!(h.live_bytes() <= h.held());
    leave();
    h.end();
}

/// A walker named, over a word.
type Walk<'a> = (&'static str, Box<dyn Fn(Word) + 'a>);

#[test]
fn every_walker_refuses_a_block_emitted_code_laid_out_by_its_own_name() {
    let mut ctx = crate::bare::context();
    let l = layouts();
    let walks: Vec<Walk> = vec![
        ("dismantle", Box::new(dec)),
        ("share", Box::new(|w| _ = share(&[w]))),
        ("Heap::read", Box::new(|w| _ = Heap::to_value(&l, w))),
        ("mark_immortal", Box::new(mark_immortal)),
        ("world_independent", Box::new(|w| _ = world_independent(w))),
        ("native_key", Box::new(|w| _ = native_key(&l, w))),
        ("cmp_words", Box::new(|w| _ = cmp_words(&l, w, imm(1)))),
        ("Layouts::parts", Box::new(|w| _ = l.parts(obj(w)))),
    ];
    for (walker, walk) in walks {
        let w = emitted(&mut ctx);
        let refused =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| walk(w))).expect_err(walker);
        let said = refused.downcast_ref::<String>().map_or("", String::as_str);
        assert!(
            said.contains(&format!(
                "`{walker}` was handed a block emitted code laid out"
            )),
            "{walker}: {said}"
        );
    }
}

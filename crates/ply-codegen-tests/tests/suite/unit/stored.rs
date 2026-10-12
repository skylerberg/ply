use ply_codegen::heap::*;
use ply_codegen::stored::{read, text};
use ply_eval::{Fields, Fixed, IntTy, Symbol, Value};
use std::cmp::Ordering;
use std::sync::Arc;

fn layouts() -> Layouts {
    Layouts::new(vec![
        (Symbol::new("Some"), 1),
        (Symbol::new("None"), 0),
        (Symbol::new("m.Pair"), 2),
    ])
}

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(Arc::new(Fields::from_unsorted(
        fields
            .into_iter()
            .map(|(name, v)| (Symbol::new(name), v))
            .collect(),
    )))
}

/// A row of a table: a record whose fields are of every kind a row holds.
fn row(i: i64) -> Value {
    record(vec![
        ("code", Value::Int(i * 7 - 3)),
        ("name", Value::str(format!("row {i}"))),
        ("wide", Value::Int(i64::MAX - i)),
        (
            "small",
            Value::Fixed(Fixed::new(IntTy::U8, (i % 256) as u128)),
        ),
        (
            "big",
            Value::Fixed(Fixed::new(IntTy::U64, u128::from(u64::MAX))),
        ),
        ("ratio", Value::Float(1.5 * i as f64)),
        ("letter", Value::Char('é')),
        ("raw", Value::bytes([0u8, 255, b'@', b'@'])),
        ("flag", Value::Bool(i % 2 == 0)),
        ("none", Value::Unit),
        (
            "maybe",
            if i % 3 == 0 {
                Value::ctor("None", vec![])
            } else {
                Value::ctor("Some", vec![Value::Int(i)])
            },
        ),
        (
            "pair",
            Value::ctor("m.Pair", vec![Value::Int(i), Value::str("x")]),
        ),
    ])
}

fn table() -> Value {
    record(vec![
        ("rows", Value::list((0..100).map(row).collect())),
        ("ints", Value::array((0..5000).map(Value::Int).collect())),
        (
            "by_code",
            Value::map((0..70).map(|i| (Value::Int(i), Value::str(format!("{i}"))))),
        ),
        ("empty", Value::list(vec![])),
        ("no_map", Value::map(Vec::<(Value, Value)>::new())),
        ("no_array", Value::array(vec![])),
        ("no_bytes", Value::bytes([])),
    ])
}

fn kept(layouts: &Layouts, w: Word) -> (Heap, Word) {
    let written = text(layouts, w).expect("a plain value has a text");
    assert!(
        written.iter().all(|b| b.is_ascii_hexdigit()),
        "a text is hex, which a C string literal holds as it is"
    );
    let mut into = Heap::persistent();
    let read = read(layouts, &[], &mut into, &written).expect("a text reads back");
    (into, read)
}

#[test]
fn a_value_reads_back_as_the_value_it_was_at_every_kind() {
    let l = layouts();
    let mut h = Heap::new();
    let value = table();
    let w = h.to_word(&l, &value);
    let (_into, read) = kept(&l, w);
    assert_eq!(cmp_words(&l, w, read), Ordering::Equal);
    assert_eq!(Heap::to_value(&l, read), Heap::to_value(&l, w));
}

#[test]
fn everything_read_is_immortal_and_none_of_it_is_the_entrys() {
    let l = layouts();
    let mut h = Heap::new();
    let w = h.to_word(&l, &table());
    let before = h.allocated();
    let (_into, read) = kept(&l, w);
    assert_eq!(
        h.allocated(),
        before,
        "reading allocated in the entry's heap"
    );
    let mut pending = vec![read];
    let mut objects = 0;
    while let Some(w) = pending.pop() {
        if is_imm(w) {
            continue;
        }
        objects += 1;
        let o = obj(w);
        unsafe {
            assert_eq!((*o).rc, IMMORTAL, "an object of kind {}", (*o).kind);
            match (*o).kind {
                KIND_RECORD | KIND_CTOR | KIND_ARRAY => {
                    pending.extend(
                        (0..(*o).len as usize)
                            .filter(|i| counted(o, *i))
                            .map(|i| word_at(o, i)),
                    );
                }
                KIND_LIST => pending.extend(ply_codegen::list::to_vec(o)),
                KIND_MAP => pending.extend(
                    ply_codegen::map::to_vec(o)
                        .into_iter()
                        .flat_map(|(k, v)| [k, v]),
                ),
                _ => {}
            }
        }
    }
    assert!(objects > 1000, "the walk reached {objects} objects");
}

#[test]
fn an_array_of_immediates_is_one_object_and_what_a_value_shares_is_read_once() {
    let l = layouts();
    let mut h = Heap::new();
    let ints: Vec<Word> = (0..30_000).map(imm).collect();
    let array = h.array_from(&ints);
    let shared = h.str("the same text");
    let holder = h.list_from(&[shared, array, shared, array]);
    let mut into = Heap::persistent();
    let written = text(&l, holder).unwrap();
    let before = into.allocated();
    let read = read(&l, &[], &mut into, &written).unwrap();
    // The list, the string and the array: thirty thousand elements made no object of their own.
    assert_eq!(into.allocated() - before, 3);
    let items = ply_codegen::list::to_vec(obj(read));
    assert_eq!(items[0], items[2]);
    assert_eq!(items[1], items[3]);
    assert_eq!(kind(items[1]), KIND_ARRAY);
    assert_eq!(ply_codegen::array::items(obj(items[1])), &ints[..]);
}

#[test]
fn a_nullary_constructor_reads_as_the_units_one_object_of_it() {
    let l = layouts();
    let mut h = Heap::new();
    let none = h.to_word(&l, &Value::ctor("None", vec![]));
    let written = text(&l, none).unwrap();
    let mut into = Heap::persistent();
    let singleton = into.alloc(KIND_CTOR, 0, 0, 1) as Word;
    mark_immortal(singleton);
    assert_eq!(
        read(&l, &[0, singleton, 0], &mut into, &written).unwrap(),
        singleton
    );
}

#[test]
fn a_function_has_no_text_and_a_text_cut_short_reads_as_none() {
    let l = layouts();
    let mut h = Heap::new();
    let closure = h.alloc(KIND_CLOSURE, 0, 1, 0);
    unsafe { set_word(closure, CLOSURE_CODE, 0) };
    let holder = h.list_from(&[imm(1), closure as Word]);
    assert_eq!(text(&l, holder), Err("a function".to_string()));
    let secret = h.bridge(Value::secret_text("key"));
    assert_eq!(text(&l, secret), Err("a Secret".to_string()));

    let whole = text(&l, h.to_word(&l, &table())).unwrap();
    let mut into = Heap::persistent();
    for cut in [0, 1, 7, whole.len() / 2, whole.len() - 2] {
        assert!(
            read(&l, &[], &mut into, &whole[..cut]).is_err(),
            "{cut} bytes of {} read as a value",
            whole.len()
        );
    }
    let mut wrong = whole.clone();
    wrong[10] = b'g';
    assert!(read(&l, &[], &mut into, &wrong).is_err());
    // A value of another unit's types names a constructor this one does not hold.
    let other = Layouts::new(vec![(Symbol::new("Some"), 1)]);
    let err = read(&other, &[], &mut into, &whole).unwrap_err();
    assert!(
        err.contains("a constructor this unit does not have"),
        "{err}"
    );
}

/// A record as compiled code builds `{ name: String, weight: Float }`: `weight` held as its bits.
fn weighed(h: &mut Heap, l: &Layouts, name: &str, weight: f64) -> Word {
    let shape = l.shape(vec![
        Symbol::new("name"),
        spelled(&Symbol::new("weight"), Slot::Float),
    ]);
    let o = h.alloc(KIND_RECORD, MIXED, 2, shape);
    let s = h.str(name);
    unsafe {
        set_word(o, 0, s);
        set_word(o, 1, ply_eval::float::bits(weight) as i64);
        set_word(o, 2, 0b01);
    }
    o as Word
}

#[test]
fn a_record_holds_its_floats_as_bits_whoever_makes_it_and_reads_as_the_value_it_is() {
    let l = layouts();
    let mut h = Heap::new();
    let r = weighed(&mut h, &l, "a", 2.5);
    let value = record(vec![
        ("name", Value::str("a")),
        ("weight", Value::Float(2.5)),
    ]);
    assert_eq!(Heap::to_value(&l, r), value);
    // Whoever makes the record, it holds its `Float` as its bits.
    let made = h.to_word(&l, &value);
    assert_eq!(unsafe { (*obj(made)).layout }, unsafe { (*obj(r)).layout });
    assert!(unsafe { !counted(obj(made), 1) });
    assert_eq!(cmp_words(&l, r, made), Ordering::Equal);
    let (_into, read) = kept(&l, r);
    assert!(unsafe { !counted(obj(read), 1) });
    assert_eq!(Heap::to_value(&l, read), value);
    // The bits are no child: the record lets go of its string alone.
    dec(r);
    dec(made);
}

#[test]
fn a_record_of_any_width_holds_every_float_as_its_bits() {
    let l = layouts();
    let mut h = Heap::new();
    let value = record(
        (0..70)
            .map(|i| {
                let name: &'static str = Box::leak(format!("f{i:02}").into_boxed_str());
                let v = if i % 2 == 0 {
                    Value::Float(i as f64 + 0.5)
                } else {
                    Value::str(format!("s{i}"))
                };
                (name, v)
            })
            .collect(),
    );
    let w = h.to_word(&l, &value);
    let o = obj(w);
    // Past 64 fields the bitmap after the words takes a second word.
    assert_eq!(unsafe { (*o).flags }, MIXED);
    assert!((0..70).all(|i| unsafe { counted(o, i) } == (i % 2 == 1)));
    assert_eq!(Heap::to_value(&l, w), value);
    let (_into, read) = kept(&l, w);
    assert_eq!(Heap::to_value(&l, read), value);
    dec(w);
}

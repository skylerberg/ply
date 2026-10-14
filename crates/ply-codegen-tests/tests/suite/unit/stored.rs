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
        if is_imm(w) || w == 0 {
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
    assert!(objects > 600, "the walk reached {objects} objects");
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
    let o = h.alloc(KIND_RECORD, MIXED | RAW, 2, shape);
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
    assert_eq!(unsafe { (*o).flags }, MIXED | RAW);
    assert!((0..70).all(|i| unsafe { counted(o, i) } == (i % 2 == 1)));
    assert_eq!(Heap::to_value(&l, w), value);
    let (_into, read) = kept(&l, w);
    assert_eq!(Heap::to_value(&l, read), value);
    dec(w);
}

/// `{from: {x, y}, label, to: {x, y}}`, each point two `Float`s.
fn segment(label: &str, x: f64) -> Value {
    let point = |x: f64| record(vec![("x", Value::Float(x)), ("y", Value::Float(x + 1.0))]);
    record(vec![
        ("from", point(x)),
        ("label", Value::str(label)),
        ("to", point(x + 10.0)),
    ])
}

#[test]
fn a_record_holds_a_record_in_place_and_reads_as_the_value_it_is() {
    let l = layouts();
    let mut h = Heap::new();
    let value = segment("s", 0.5);
    let w = h.to_word(&l, &value);
    let o = obj(w);
    let shape = unsafe { (*o).layout };
    // `from` takes a header's two words and its own two, `label` the fifth, `to` the four after,
    // and a word of bitmap after them says the fifth alone is counted.
    assert_eq!(l.shape_words(shape), (9, 1));
    assert_eq!(&l.shape_offsets(shape)[..], &[0, 4, 5]);
    assert_eq!(unsafe { (*o).flags }, MIXED | RAW);
    assert!((0..9).all(|i| unsafe { counted(o, i) } == (i == 4)));
    assert_eq!(Heap::to_value(&l, w), value);
    let same = h.to_word(&l, &segment("s", 0.5));
    assert_eq!(cmp_words(&l, w, same), Ordering::Equal);
    let later = h.to_word(&l, &segment("s", 0.75));
    assert_eq!(cmp_words(&l, w, later), Ordering::Less);
    let (_into, read) = kept(&l, w);
    assert_eq!(Heap::to_value(&l, read), value);
    assert_eq!(unsafe { (*obj(read)).layout }, shape);
    dec(w);
    dec(same);
    dec(later);
}

#[test]
fn a_record_held_in_place_is_counted_by_the_record_holding_it() {
    let l = layouts();
    let mut h = Heap::new();
    let label = h.str("held");
    inc(label);
    let inner = h.record(&l, &[(Symbol::new("label"), label)]);
    let outer = h.record(
        &l,
        &[
            (Symbol::new("count"), imm(1)),
            (Symbol::new("inner"), inner),
        ],
    );
    // The inner record was copied in and let go: the string is held by this test and by the copy.
    assert_eq!(unsafe { (*obj(label)).rc }, 2);
    let shape = unsafe { (*obj(outer)).layout };
    let at = l.offset(shape, &Symbol::new("inner")).expect("the field");
    let held = field_word(&mut h, &l, obj(outer), at, l.slot_at(shape, at));
    // A read of it is the holder's own words, which the holder counts for, and an object a stack
    // scan finds.
    assert_eq!(unsafe { (*obj(outer)).rc }, 2);
    assert!(h.is_object(held));
    assert_eq!(
        Heap::to_value(&l, held),
        record(vec![("label", Value::str("held"))])
    );
    assert!(share(&[held]));
    assert_ne!(unsafe { (*obj(outer)).rc } & SHARED, 0);
    dec(outer);
    assert_eq!(unsafe { (*obj(label)).rc } & !SHARED, 2);
    // The last holder lets the string go once, through the copy.
    dec(held);
    assert_eq!(unsafe { (*obj(label)).rc } & !SHARED, 1);
}

#[test]
fn a_record_held_in_place_inside_a_large_record_is_an_object_a_stack_scan_finds() {
    let l = layouts();
    let mut h = Heap::new();
    let mut fields: Vec<(&str, Value)> = (0..1100)
        .map(|i| {
            let name: &'static str = Box::leak(format!("f{i:04}").into_boxed_str());
            (name, Value::Int(i))
        })
        .collect();
    fields.push(("inner", record(vec![("label", Value::str("deep"))])));
    let value = record(fields);
    let w = h.to_word(&l, &value);
    let shape = unsafe { (*obj(w)).layout };
    let at = l.offset(shape, &Symbol::new("inner")).expect("the field");
    let inner = field_word(&mut h, &l, obj(w), at, l.slot_at(shape, at));
    assert!(h.is_object(inner));
    assert_eq!(Heap::to_value(&l, w), value);
    dec(inner);
    dec(w);
}

#[test]
fn a_wide_width_is_an_object_of_its_own_ordered_by_its_value_and_kept_as_it_is() {
    let l = layouts();
    let mut h = Heap::new();
    let wides = [
        Value::Fixed(Fixed::new(IntTy::U64, 7)),
        Value::Fixed(Fixed::new(IntTy::U64, 1 << 63)),
        Value::Fixed(Fixed::new(IntTy::U64, u128::from(u64::MAX))),
    ];
    let signed = [
        Value::Fixed(Fixed::of(IntTy::I64, i128::from(i64::MIN)).expect("an i64")),
        Value::Fixed(Fixed::of(IntTy::I64, -1).expect("an i64")),
        Value::Fixed(Fixed::of(IntTy::I64, 5).expect("an i64")),
    ];
    for values in [&wides[..], &signed[..]] {
        let words: Vec<Word> = values.iter().map(|v| h.to_word(&l, v)).collect();
        for (w, v) in words.iter().zip(values) {
            assert_eq!(kind(*w), KIND_FIXED);
            assert!(native_key(&l, *w));
            assert_eq!(Heap::to_value(&l, *w), *v);
            let (_into, read) = kept(&l, *w);
            assert_eq!(Heap::to_value(&l, read), *v);
        }
        // Listed in ascending order, which for `U64` past 2^63 and for a negative `I64` is not
        // the order of the bits read as an `Int`.
        for pair in words.windows(2) {
            assert_eq!(cmp_words(&l, pair[0], pair[1]), Ordering::Less);
        }
    }
    let widest = Value::Fixed(Fixed::new(IntTy::U128, u128::MAX - 3));
    let w = h.to_word(&l, &widest);
    assert_eq!(kind(w), KIND_FIXED);
    assert_eq!(Heap::to_value(&l, kept(&l, w).1), widest);
}

#[test]
fn a_wide_width_a_store_bridged_reads_back_as_the_width_s_own_object() {
    // A bridge holding the width is what a `ply` that bridged 64-bit widths stored.
    let l = layouts();
    let mut h = Heap::new();
    let value = Value::Fixed(Fixed::new(IntTy::U64, 1 << 63));
    let bridged = h.bridge(value.clone());
    let (_into, read) = kept(&l, bridged);
    assert_eq!(kind(read), KIND_FIXED);
    assert_eq!(Heap::to_value(&l, read), value);
}

#[test]
fn a_record_holds_a_wide_width_as_its_bits() {
    let l = layouts();
    let mut h = Heap::new();
    let value = record(vec![
        (
            "count",
            Value::Fixed(Fixed::new(IntTy::U64, u128::from(u64::MAX))),
        ),
        ("label", Value::str("w")),
        (
            "offset",
            Value::Fixed(Fixed::of(IntTy::I64, -9).expect("an i64")),
        ),
    ]);
    let w = h.to_word(&l, &value);
    let shape = unsafe { (*obj(w)).layout };
    assert_eq!(
        l.shape_spelled(shape)
            .iter()
            .map(|s| s.as_str().to_string())
            .collect::<Vec<_>>(),
        ["count!u64", "label", "offset!i64"]
    );
    assert!((0..3).all(|i| unsafe { counted(obj(w), i) } == (i == 1)));
    assert_eq!(Heap::to_value(&l, w), value);
    let at = l.offset(shape, &Symbol::new("offset")).expect("the field");
    let offset = field_word(&mut h, &l, obj(w), at, l.slot_at(shape, at));
    assert_eq!(kind(offset), KIND_FIXED);
    let (_into, read) = kept(&l, w);
    assert_eq!(Heap::to_value(&l, read), value);
    dec(w);
    dec(offset);
}

#[test]
fn an_array_holds_its_records_in_place_and_its_floats_as_their_bits() {
    let l = layouts();
    let mut h = Heap::new();
    let points = Value::array((0..3).map(|i| segment("p", f64::from(i))).collect());
    let w = h.to_word(&l, &points);
    let o = obj(w);
    assert!(ply_codegen::array::in_place(o));
    assert_eq!(ply_codegen::array::len(o), 3);
    // Each element takes a header's two words beside a segment's nine and its bitmap's one.
    assert_eq!(unsafe { (*o).aux }, 12);
    assert_eq!(Heap::to_value(&l, w), points);
    // Each element is a part whose word is its header's address in the array, twelve words apart.
    let parts = l.parts(o);
    assert_eq!(parts.len(), 3);
    assert_eq!(
        unsafe { part_word(o, parts[1].0, parts[1].1) },
        Some(unsafe { words(o).add(12) } as Word)
    );
    let second = ply_codegen::array::element(&mut h, &l, o, 1);
    assert_eq!(Heap::to_value(&l, second), segment("p", 1.0));
    assert_eq!(unsafe { (*o).rc }, 2);
    let (_into, read) = kept(&l, w);
    assert_eq!(Heap::to_value(&l, read), points);
    // Set in place where it holds the array alone, and on a copy where another holds it too.
    let replaced = h.to_word(&l, &segment("q", 9.0));
    let shared = h.array_set(&l, w, 0, replaced);
    assert_ne!(
        shared, w,
        "the element read out holds the array, so the set copies it"
    );
    assert_eq!(
        Heap::to_value(&l, shared),
        Value::array(vec![
            segment("q", 9.0),
            segment("p", 1.0),
            segment("p", 2.0)
        ])
    );
    assert_eq!(Heap::to_value(&l, second), segment("p", 1.0));
    dec(second);
    // An array that counts some of its words takes a word of bitmap after them.
    let items: Vec<Word> = (0..3)
        .map(|i| h.to_word(&l, &segment("p", f64::from(i))))
        .collect();
    let before = h.allocated_bytes();
    let built = h.array_of(&l, &items);
    assert_eq!(
        h.allocated_bytes() - before,
        Heap::object_size(8 * (36 + 1))
    );
    dec(built);
    let floats = Value::array(vec![Value::Float(0.5), Value::Float(-2.0)]);
    let f = h.to_word(&l, &floats);
    assert!(ply_codegen::array::in_place(obj(f)));
    assert_eq!(unsafe { (*obj(f)).aux }, 1);
    assert_eq!(Heap::to_value(&l, f), floats);
    let boxed = ply_codegen::array::element(&mut h, &l, obj(f), 1);
    assert_eq!(kind(boxed), KIND_FLOAT);
    assert_eq!(cmp_words(&l, f, h.to_word(&l, &floats)), Ordering::Equal);
    dec(shared);
    dec(boxed);
}

#[test]
fn a_char_is_its_own_word_ordered_and_kept_as_the_char_it_is() {
    let l = layouts();
    let mut h = Heap::new();
    let chars = ['\0', 'a', 'z', 'é', '\u{10ffff}'];
    let words: Vec<Word> = chars
        .iter()
        .map(|c| h.to_word(&l, &Value::Char(*c)))
        .collect();
    for (w, c) in words.iter().zip(&chars) {
        assert!(is_imm(*w) && !is_int(*w), "{c:?} is held in its word");
        assert_eq!(kind(*w), KIND_CHAR);
        assert_eq!(Heap::to_value(&l, *w), Value::Char(*c));
        assert_eq!(as_int(*w), None);
    }
    for (a, x) in words.iter().zip(&chars) {
        for (b, y) in words.iter().zip(&chars) {
            assert_eq!(cmp_words(&l, *a, *b), x.cmp(y), "{x:?} against {y:?}");
        }
    }
    let text = Value::list(chars.iter().map(|c| Value::Char(*c)).collect());
    let w = h.to_word(&l, &text);
    let (_into, read) = kept(&l, w);
    assert_eq!(Heap::to_value(&l, read), text);
    let pair = record(vec![("c", Value::Char('q')), ("n", Value::Int(7))]);
    let p = h.to_word(&l, &pair);
    let (_into, read) = kept(&l, p);
    assert_eq!(Heap::to_value(&l, read), pair);
}

/// `m.Pair`'s arguments as its declaration types them: a segment in place, then a `Float`.
fn laid_pair() -> Layouts {
    let mut l = Layouts::of(layouts().ctors.clone(), &[], RecordLayout::CURRENT);
    l.set_ctor_args(&[
        vec![],
        vec![],
        vec![
            Symbol::new("a000!r(from!r(x!f,y!f),label,to!r(x!f,y!f))"),
            Symbol::new("a001!f"),
        ],
    ]);
    l
}

#[test]
fn a_constructor_holds_its_arguments_as_its_declaration_types_them() {
    let l = laid_pair();
    let mut h = Heap::new();
    let pair = |x: f64| Value::ctor("m.Pair", vec![segment("s", 0.5), Value::Float(x)]);
    let w = h.to_word(&l, &pair(2.5));
    let o = obj(w);
    // The segment takes a header's two words, its own nine and its bitmap's one; the `Float` its
    // bits after them; and a word of bitmap after those says which the constructor counts.
    assert_eq!(kind(w), KIND_CTOR);
    assert_eq!(unsafe { (*o).len }, 13);
    assert_eq!(l.ctor_arity(o), 2);
    assert!(l.ctor_arg(o, 1) == (12, Slot::Float));
    // The segment is a part whose word is its header's address in the constructor.
    let parts = l.parts(o);
    assert_eq!(parts.len(), 2);
    assert_eq!(
        unsafe { part_word(o, parts[0].0, parts[0].1) },
        Some(unsafe { words(o) } as Word)
    );
    assert_eq!(unsafe { part_word(o, parts[1].0, parts[1].1) }, None);
    assert_eq!(unsafe { (*o).flags }, MIXED | RAW);
    assert!((0..13).all(|i| unsafe { counted(o, i) } == (i == 6)));
    assert_eq!(Heap::to_value(&l, w), pair(2.5));
    let later = h.to_word(&l, &pair(3.0));
    assert_eq!(cmp_words(&l, w, later), Ordering::Less);
    let (_into, read) = kept(&l, w);
    assert_eq!(Heap::to_value(&l, read), pair(2.5));
    dec(w);
    dec(later);
}

fn decimal(text: &str) -> Value {
    Value::Decimal(text.parse().expect("a decimal"))
}

#[test]
fn a_decimal_is_an_object_of_its_own_and_a_record_or_an_array_holds_its_two_words() {
    let l = layouts();
    let mut h = Heap::new();
    let price = decimal("19.990");
    let w = h.to_word(&l, &price);
    assert_eq!(kind(w), KIND_DECIMAL);
    assert_eq!(Heap::to_value(&l, w), price);
    // Ordered as the numbers they are, whatever their scales.
    let same = h.to_word(&l, &decimal("19.99"));
    assert_eq!(cmp_words(&l, w, same), Ordering::Equal);
    // A map keeps one spelling of a number: only the least scale is a key as it stands.
    assert!(!native_key(&l, w));
    assert!(native_key(&l, same));
    let line = record(vec![("price", price.clone()), ("qty", Value::Int(3))]);
    let r = h.to_word(&l, &line);
    // `price` takes two words and `qty` the third, its own 64 bits: none counts, so no bitmap.
    let shape = unsafe { (*obj(r)).layout };
    assert_eq!(l.shape_words(shape), (3, 0));
    assert_eq!(&l.shape_offsets(shape)[..], &[0, 2]);
    assert_eq!(Heap::to_value(&l, r), line);
    let prices = Value::array(vec![decimal("1.5"), decimal("-2.25")]);
    let a = h.to_word(&l, &prices);
    assert!(ply_codegen::array::in_place(obj(a)));
    assert_eq!(unsafe { (*obj(a)).aux }, 2);
    assert_eq!(Heap::to_value(&l, a), prices);
    let second = ply_codegen::array::element(&mut h, &l, obj(a), 1);
    assert_eq!(kind(second), KIND_DECIMAL);
    assert_eq!(Heap::to_value(&l, second), decimal("-2.25"));
    for x in [w, r, a] {
        let (_into, read) = kept(&l, x);
        assert_eq!(Heap::to_value(&l, read), Heap::to_value(&l, x));
    }
    // A unit that reads a `Decimal`'s word has the runtime hold it so.
    let words = Layouts::of(
        vec![],
        &[],
        RecordLayout {
            raw_ints: false,
            raw_decimals: false,
            ..RecordLayout::CURRENT
        },
    );
    let by_word = h.to_word(&words, &line);
    let spelled = words.shape_spelled(unsafe { (*obj(by_word)).layout });
    assert!(spelled.iter().all(|s| !s.as_str().contains('!')));
    assert_eq!(Heap::to_value(&words, by_word), line);
    for x in [w, same, r, a, second, by_word] {
        dec(x);
    }
}

#[test]
fn a_record_or_an_array_holds_an_option_as_its_payloads_word() {
    let l = layouts();
    let mut h = Heap::new();
    let some = |v: Value| Value::ctor("Some", vec![v]);
    let none = || Value::ctor("None", vec![]);
    let line = record(vec![
        ("best", some(Value::Float(1.5))),
        ("note", none()),
        ("qty", Value::Int(3)),
    ]);
    let r = h.to_word(&l, &line);
    let o = obj(r);
    // Each option is a word the record counts, its payload's or zero, so the record is three words.
    let shape = unsafe { (*o).layout };
    assert_eq!(l.shape_words(shape).0, 3);
    let spelled = l.shape_spelled(shape);
    assert!(spelled.iter().any(|s| s.as_str() == "best!m"));
    assert!(spelled.iter().any(|s| s.as_str() == "note!m"));
    let at = |i: usize| unsafe { word_at(o, l.shape_offsets(shape)[i]) };
    assert_eq!(kind(at(0)), KIND_FLOAT);
    assert_eq!(at(1), 0);
    assert_eq!(unsafe { (*o).flags } & RAW, RAW);
    // Read, compared, keyed and stored as the record of objects it stands for.
    assert_eq!(Heap::to_value(&l, r), line);
    let absent = h.to_word(
        &l,
        &record(vec![
            ("best", none()),
            ("note", none()),
            ("qty", Value::Int(3)),
        ]),
    );
    assert_eq!(cmp_words(&l, absent, r), Ordering::Less);
    assert!(native_key(&l, absent));
    assert!(
        !native_key(&l, r),
        "a `Float` is no map's key, held in an option or not"
    );
    let (_into, read) = kept(&l, r);
    assert_eq!(Heap::to_value(&l, read), line);
    let best = field_word(&mut h, &l, o, l.shape_offsets(shape)[0], Slot::Maybe);
    assert_eq!(Heap::to_value(&l, best), some(Value::Float(1.5)));
    let opts = Value::array(vec![some(Value::Int(1)), none(), some(Value::Int(2))]);
    let a = h.to_word(&l, &opts);
    assert!(ply_codegen::array::in_place(obj(a)));
    assert_eq!(Heap::to_value(&l, a), opts);
    let second = ply_codegen::array::element(&mut h, &l, obj(a), 1);
    assert_eq!(Heap::to_value(&l, second), none());
    let (_into, read) = kept(&l, a);
    assert_eq!(Heap::to_value(&l, read), opts);
    // A unit that reads an `Option`'s word has the runtime hold it so.
    let words = Layouts::of(
        layouts().ctors.clone(),
        &[],
        RecordLayout {
            raw_ints: false,
            option_words: false,
            ..RecordLayout::CURRENT
        },
    );
    let by_word = h.to_word(&words, &line);
    let spelled = words.shape_spelled(unsafe { (*obj(by_word)).layout });
    assert!(spelled.iter().all(|s| !s.as_str().contains('!')));
    assert_eq!(Heap::to_value(&words, by_word), line);
    for x in [r, absent, best, a, second, by_word] {
        dec(x);
    }
}

#[test]
fn a_long_slice_of_bytes_is_a_view_that_holds_its_value_and_never_grows_in_place() {
    let l = layouts();
    let mut h = Heap::new();
    let all: Vec<u8> = (0..200u8).collect();
    let whole = h.bytes(&all);
    inc(whole);
    let view = h.slice(&l, whole, 10, 150);
    let o = obj(view);
    assert_eq!(unsafe { (*o).flags } & VIEW, VIEW);
    assert_eq!(unsafe { bytes_of(o) }, &all[10..150]);
    assert_eq!(
        unsafe { (*obj(whole)).rc },
        2,
        "the view holds the value it views"
    );
    // A view of a view views the value under both, and a short slice is a copy.
    inc(view);
    let inner = h.slice(&l, view, 20, 100);
    assert_eq!(unsafe { word_at(obj(inner), 0) }, whole);
    assert_eq!(unsafe { bytes_of(obj(inner)) }, &all[30..110]);
    inc(view);
    let short = h.slice(&l, view, 0, 8);
    assert_eq!(unsafe { (*obj(short)).flags } & VIEW, 0);
    assert_eq!(unsafe { bytes_of(obj(short)) }, &all[10..18]);
    // All of a value is the value itself.
    inc(view);
    assert_eq!(h.slice(&l, view, 0, 140), view);
    dec(view);
    // Appending to a view copies, leaving the value it views as it was.
    inc(view);
    let grown = h.append(view, b"!");
    assert_ne!(grown, view);
    assert_eq!(unsafe { bytes_of(obj(grown)) }.len(), 141);
    assert_eq!(unsafe { bytes_of(obj(whole)) }, &all[..]);
    assert_eq!(Heap::to_value(&l, view), Value::bytes(&all[10..150]));
    let (_into, read) = kept(&l, inner);
    assert_eq!(Heap::to_value(&l, read), Value::bytes(&all[30..110]));
    for w in [view, inner, short, grown] {
        dec(w);
    }
    assert_eq!(
        unsafe { (*obj(whole)).rc },
        1,
        "letting the views go lets the value go"
    );
    // A unit whose code reads bytes in place has the runtime copy them.
    let copies = Layouts::of(
        vec![],
        &[],
        RecordLayout {
            bytes_views: false,
            ..RecordLayout::CURRENT
        },
    );
    inc(whole);
    let copied = h.slice(&copies, whole, 10, 150);
    assert_eq!(unsafe { (*obj(copied)).flags } & VIEW, 0);
    dec(copied);
    dec(whole);
}

#[test]
fn marking_a_view_immortal_marks_the_value_it_views() {
    let l = layouts();
    let mut h = Heap::new();
    let all: Vec<u8> = (0..200u8).collect();
    let whole = h.bytes(&all);
    inc(whole);
    let view = h.slice(&l, whole, 0, 100);
    mark_immortal(view);
    assert_eq!(unsafe { (*obj(whole)).rc }, IMMORTAL);
}

/// `{label, x}`, `x` a `Float`.
fn tagged(x: f64, label: &str) -> Value {
    record(vec![("label", Value::str(label)), ("x", Value::Float(x))])
}

#[test]
fn columns_hold_each_field_of_their_records_apart() {
    let l = layouts();
    let mut h = Heap::new();
    let items: Vec<Word> = (0..3)
        .map(|i| h.to_word(&l, &tagged(f64::from(i), "p")))
        .collect();
    let cs = h.columns_of(&l, &items).expect("records of one shape");
    let o = obj(cs);
    assert_eq!(kind(cs), KIND_COLUMNS);
    assert_eq!(ply_codegen::columns::len(o), 3);
    // `x` is the second field, its column the `Float`s' bits in place.
    assert!(ply_codegen::array::in_place(ply_codegen::columns::column(
        o, 1
    )));
    let all = Value::array((0..3).map(|i| tagged(f64::from(i), "p")).collect());
    assert_eq!(Heap::to_value(&l, cs), all);
    let second = ply_codegen::columns::record(&mut h, &l, o, 1);
    assert_eq!(Heap::to_value(&l, second), tagged(1.0, "p"));
    // Set on a copy where another holds them, and in place where none does.
    inc(cs);
    let replaced = h.to_word(&l, &tagged(9.0, "q"));
    let set = h.columns_set(&l, cs, 0, replaced);
    assert_ne!(set, cs);
    assert_eq!(Heap::to_value(&l, cs), all);
    let again = h.to_word(&l, &tagged(7.0, "r"));
    let in_place = h.columns_set(&l, set, 2, again);
    assert_eq!(in_place, set);
    assert_eq!(
        Heap::to_value(&l, in_place),
        Value::array(vec![tagged(9.0, "q"), tagged(1.0, "p"), tagged(7.0, "r")])
    );
    assert_eq!(cmp_words(&l, cs, h.to_word(&l, &all)), Ordering::Equal);
    let (_into, read) = kept(&l, in_place);
    assert_eq!(kind(read), KIND_COLUMNS);
    assert_eq!(Heap::to_value(&l, read), Heap::to_value(&l, in_place));
    // What is not records of one shape is not held in columns.
    let ints = [h.to_word(&l, &Value::Int(1))];
    assert!(h.columns_of(&l, &ints).is_none());
    for w in [cs, second, in_place] {
        dec(w);
    }
}

#[test]
fn a_buffer_a_view_holds_still_grows_in_place_where_the_view_reads_none_of_the_growth() {
    let l = layouts();
    let mut h = Heap::new();
    let all: Vec<u8> = (0..200u8).collect();
    let start = h.bytes(&all[..100]);
    let buf = h.append(start, &all[100..]);
    inc(buf);
    let view = h.slice(&l, buf, 0, 100);
    assert_eq!(unsafe { (*obj(buf)).aux }, 1, "the buffer counts its view");
    assert!(grows_in_place(buf));
    let grown = h.append(buf, b"!");
    assert_eq!(grown, buf, "appended in place past the view");
    assert_eq!(unsafe { bytes_of(obj(view)) }, &all[..100]);
    assert_eq!(unsafe { bytes_of(obj(grown)) }.len(), 201);
    dec(view);
    assert_eq!(
        unsafe { (*obj(grown)).aux },
        0,
        "letting the view go uncounts it"
    );
    // A second full holder is no view: the buffer is copied.
    inc(grown);
    let copied = h.append(grown, b"?");
    assert_ne!(copied, grown);
    dec(copied);
    dec(grown);
}

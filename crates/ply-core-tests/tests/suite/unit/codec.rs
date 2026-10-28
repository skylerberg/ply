use ply_core::codec::{Writer, decode, decode_with, encode, encode_with, fun, tag};
use ply_core::{Closure, ClosureKind, Decimal, Fields, Fixed, IntTy, Symbol, Synth, Value};
use std::sync::Arc;

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(Arc::new(Fields::from_unsorted(
        fields
            .into_iter()
            .map(|(k, v)| (Symbol::new(k), v))
            .collect(),
    )))
}

fn round_trip(v: &Value) -> Value {
    decode(&encode(v).expect("plain data encodes")).expect("its own bytes decode")
}

#[test]
fn every_plain_value_comes_back_equal() {
    let row = |name: &str, n: i64| {
        record(vec![
            ("name", Value::bytes(name.as_bytes())),
            ("index", Value::Int(n)),
            ("public", Value::Bool(n % 2 == 0)),
        ])
    };
    let v = record(vec![
        ("unit", Value::Unit),
        (
            "ints",
            Value::list(vec![
                Value::Int(0),
                Value::Int(-1),
                Value::Int(i64::MIN),
                Value::Int(i64::MAX),
            ]),
        ),
        ("float", Value::Float(-0.5)),
        ("decimal", Value::Decimal(Decimal::new(-12345, 3))),
        (
            "fixed",
            Value::Fixed(Fixed::new(IntTy::U64, u128::from(u64::MAX))),
        ),
        (
            "wide",
            Value::Fixed(Fixed::new(IntTy::I128, i128::MIN as u128)),
        ),
        (
            "unsigned wide",
            Value::Fixed(Fixed::new(IntTy::U128, u128::MAX)),
        ),
        ("text", Value::str("né")),
        ("rows", Value::list((0..40).map(|i| row("r", i)).collect())),
        (
            "some",
            Value::ctor("Some", vec![Value::ctor("None", vec![])]),
        ),
        (
            "map",
            Value::map([
                (Value::str("b"), Value::Int(2)),
                (Value::str("a"), Value::Int(1)),
            ]),
        ),
    ]);
    assert_eq!(round_trip(&v), v);
}

#[test]
fn a_name_repeated_in_every_row_is_written_once() {
    let rows = |n: i64| {
        Value::list(
            (0..n)
                .map(|i| record(vec![("a_rather_long_field_name", Value::Int(i))]))
                .collect(),
        )
    };
    let small = encode(&rows(1)).unwrap().len();
    let large = encode(&rows(101)).unwrap().len();
    assert!(
        large - small < 100 * 6,
        "each further row costs its tag, the name's number and the int: {small} then {large}"
    );
}

#[test]
fn what_is_not_plain_data_is_refused() {
    let secret = Value::secret_text("hunter2");
    let err = encode(&record(vec![("key", secret)])).unwrap_err();
    assert!(err.contains("not plain data"), "{err}");
}

#[test]
fn bytes_that_are_not_an_encoded_value_are_refused() {
    assert!(decode(b"").is_err());
    assert!(decode(b"PLV1").is_err());
    let mut whole = encode(&Value::list(vec![Value::Int(1), Value::Int(2)])).unwrap();
    whole.push(0);
    assert!(decode(&whole).unwrap_err().contains("follow the value"));
    let cut = encode(&Value::str("a string that is cut short")).unwrap();
    assert!(decode(&cut[..cut.len() - 3]).is_err());
    let mut huge = b"PLV1".to_vec();
    huge.extend_from_slice(&[9, 0xff, 0xff, 0xff, 0xff, 0x0f]);
    assert!(decode(&huge).unwrap_err().contains("fewer bytes left"));
    let mut unknown = b"PLV1".to_vec();
    unknown.push(200);
    assert!(decode(&unknown).unwrap_err().contains("tagged 200"));
}

fn generated(arity: u8, rule: Synth) -> Value {
    Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Synth { arity, rule },
    }))
}

#[test]
fn a_generated_function_comes_back_answering_as_it_did_nested_or_alone() {
    let constant = generated(2, Synth::Const(Value::str("three")));
    let project = generated(3, Synth::Project(1));
    let table = generated(
        1,
        Synth::Table {
            entries: vec![
                (Value::Int(0), Value::list(vec![Value::Int(7)])),
                (Value::Int(1), record(vec![("x", Value::Int(8))])),
            ],
            default: Value::Unit,
        },
    );
    let nested = record(vec![
        ("fs", Value::list(vec![constant.clone(), project.clone()])),
        ("t", Value::ctor("Some", vec![table.clone()])),
    ]);
    let back = round_trip(&nested);
    let Value::Record(fields) = &back else {
        panic!("{back:?}")
    };
    let field = |name: &str| fields.iter().find(|(n, _)| n.as_str() == name).unwrap().1;
    let Value::List(fs) = field("fs") else {
        panic!()
    };
    let fs: Vec<&Value> = fs.iter().collect();
    let Value::Ctor { args, .. } = field("t") else {
        panic!()
    };
    // `Value` does not compare closures; their structures say what each answers.
    let structure = |v: &Value| format!("{v:?}");
    assert_eq!(structure(fs[0]), structure(&constant));
    assert_eq!(structure(fs[1]), structure(&project));
    assert_eq!(structure(&args[0]), structure(&table));
    for f in [constant, project, table] {
        assert_eq!(encode(&round_trip(&f)).unwrap(), encode(&f).unwrap());
    }
}

#[test]
fn a_function_the_program_wrote_does_not_decode() {
    for case in [fun::NAMED, fun::ANON] {
        let mut w = Writer::new();
        w.fun(case);
        if case == fun::NAMED {
            w.name(b"m.double");
        }
        let refused = decode(&w.finish()).unwrap_err();
        assert!(refused.contains("only its own run can hold"), "{refused}");
    }
    let mut w = Writer::new();
    w.fun(fun::PROJECT);
    w.varint(2);
    w.varint(2);
    assert!(
        decode(&w.finish()).is_err(),
        "argument 2 of a function of 2"
    );
}

/// Each value the bytes cannot hold crosses as a position among the handles kept beside them, in
/// the order the encoding meets them, and comes back as whatever the reader keeps at that position.
#[test]
fn values_the_bytes_cannot_hold_cross_as_positions_among_the_handles() {
    let secret = || Value::secret_text("hunter2");
    let v = record(vec![
        ("a", secret()),
        ("b", Value::list(vec![Value::Int(1), secret()])),
    ]);
    let mut kept = Vec::new();
    let bytes = encode_with(&v, &mut |x| {
        kept.push(x.type_name());
        Ok(kept.len() as u64 - 1)
    })
    .expect("each handle is kept");
    assert_eq!(kept.len(), 2);
    let back = decode_with(&bytes, &mut |at| Ok(Value::Int(100 + at as i64))).unwrap();
    assert_eq!(
        back,
        record(vec![
            ("a", Value::Int(100)),
            ("b", Value::list(vec![Value::Int(1), Value::Int(101)])),
        ])
    );
    let refused = decode(&bytes).unwrap_err();
    assert!(refused.contains("no handles were kept"), "{refused}");
}

/// `AGAIN n` is the `n`th string, bytes, list, array or map the bytes hold, numbered as each opens,
/// nested ones after the one holding them.
#[test]
fn again_is_a_copy_of_the_value_it_numbers() {
    let mut w = Writer::new();
    w.count(tag::LIST, 4);
    // 0: the list itself, still open, so nothing in it may name it.
    w.count(tag::LIST, 2);
    // 1: [ "ab", b"c" ], holding 2 and 3.
    w.str(b"ab");
    w.bytes(b"c");
    w.again(2);
    w.count(tag::RECORD, 1);
    w.name(b"field");
    w.again(1);
    w.again(3);
    let inner = Value::list(vec![Value::str("ab"), Value::bytes(b"c")]);
    assert_eq!(
        decode(&w.finish()).unwrap(),
        Value::list(vec![
            inner.clone(),
            Value::str("ab"),
            record(vec![("field", inner)]),
            Value::bytes(b"c"),
        ])
    );
}

/// A value read again reads its names as the first reading numbered them, so names after it keep
/// their numbers.
#[test]
fn names_inside_a_value_read_again_keep_their_numbers() {
    let mut w = Writer::new();
    w.count(tag::LIST, 4);
    w.count(tag::LIST, 1);
    w.count(tag::RECORD, 1);
    w.name(b"first");
    w.int(1);
    w.again(1);
    w.count(tag::RECORD, 2);
    w.name(b"first");
    w.int(2);
    w.name(b"second");
    w.int(3);
    // By its number alone, which a second reading of `first` would have taken.
    w.count(tag::RECORD, 1);
    w.name(b"second");
    w.int(4);
    let one = Value::list(vec![record(vec![("first", Value::Int(1))])]);
    assert_eq!(
        decode(&w.finish()).unwrap(),
        Value::list(vec![
            one.clone(),
            one,
            record(vec![("first", Value::Int(2)), ("second", Value::Int(3))]),
            record(vec![("second", Value::Int(4))]),
        ])
    );
}

#[test]
fn again_of_a_value_not_yet_written_or_still_open_is_refused() {
    let mut w = Writer::new();
    w.again(0);
    assert!(
        decode(&w.finish())
            .unwrap_err()
            .contains("before it was written")
    );
    let mut w = Writer::new();
    w.count(tag::LIST, 1);
    w.again(0);
    assert!(decode(&w.finish()).unwrap_err().contains("inside itself"));
}

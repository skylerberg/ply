use ply_eval::codec::{decode, encode};
use ply_eval::{Decimal, Fields, Fixed, IntTy, Symbol, Value};
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
    let secret = Value::Secret(Arc::new(Value::str("hunter2")));
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
}

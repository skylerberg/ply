//! `machine.Value`: what a `machine.call` argument or answer is, on the way across.

// A `Value` pins `Arc` for shared payloads and `Rc` for shared code, so none of these `Arc`s can be `Send`.
#![allow(clippy::arc_with_non_send_sync)]

use ply_eval::{Span, Symbol, Value};
use ply_machine::payload::{
    adt_to_wire, machine_value, value_from_wire, value_of_adt, value_to_wire, wire_to_adt,
};

/// A record with the fields a program declared, as a value.
fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(std::sync::Arc::new(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    ))
}

#[test]
fn every_crossable_value_survives_the_wire() {
    let values = vec![
        Value::Unit,
        Value::Bool(true),
        Value::Int(-7),
        Value::str("hello"),
        Value::bytes([0u8, 1, 254, 255]),
        Value::list(vec![Value::Int(1), Value::Int(2)]),
        record(vec![("a", Value::Int(1)), ("b", Value::str("two"))]),
        Value::ctor(Symbol::new("std.json.Number"), vec![Value::Int(3)]),
        Value::map(vec![(Value::str("k"), Value::Int(9))]),
        Value::Fixed(ply_eval::Fixed::of(ply_eval::IntTy::U32, 4000000000).unwrap()),
        Value::Fixed(ply_eval::Fixed::of(ply_eval::IntTy::I8, -5).unwrap()),
    ];
    for value in values {
        let wire = value_to_wire(&value);
        let back = value_from_wire(&wire, Span::DUMMY).expect("the wire reads back");
        assert_eq!(
            back.to_string(),
            value.to_string(),
            "{} did not survive",
            value.type_name()
        );
    }
}

#[test]
fn a_fixed_width_integer_crosses_with_its_type() {
    let value = Value::Fixed(ply_eval::Fixed::of(ply_eval::IntTy::U32, 4_000_000_000).unwrap());
    let adt = machine_value(&value, "m").unwrap();
    assert_eq!(adt.to_string(), "m.VFixed(\"U32\", 4000000000)");
    assert_eq!(
        value_of_adt(&adt, Span::DUMMY, "m").unwrap().to_string(),
        "4000000000"
    );
    // A U64 above `Int`'s range has no `Int` to cross as.
    let too_big = Value::Fixed(ply_eval::Fixed::new(ply_eval::IntTy::U64, u64::MAX));
    machine_value(&too_big, "m").expect_err("a U64 above i64::MAX does not cross");
}

#[test]
fn a_value_that_cannot_cross_is_refused_by_name() {
    let closure = Value::Closure(std::sync::Arc::new(ply_eval::Closure {
        name: None,
        kind: ply_eval::ClosureKind::Ctor {
            name: Symbol::new("m.Wrapped"),
            arity: 1,
        },
    }));
    let err = machine_value(&closure, "m").expect_err("a closure does not cross");
    assert!(
        err.message.contains("cannot cross"),
        "the refusal names the reason: {}",
        err.message
    );
}

#[test]
fn a_value_names_its_constructors_after_the_callers_module() {
    let value = Value::list(vec![Value::Int(1)]);
    let adt = machine_value(&value, "m").expect("an int list crosses");
    // The constructors are the caller's: `m.VList` over `m.VInt`.
    assert_eq!(adt.to_string(), "m.VList([m.VInt(1)])");

    let back = value_of_adt(&adt, Span::DUMMY, "m").expect("the ADT reads back");
    assert_eq!(back.to_string(), value.to_string());
}

#[test]
fn an_adt_of_a_foreign_module_is_refused() {
    // `VInt` of another module is not this program's `Value`.
    let foreign = Value::ctor(Symbol::new("other.VInt"), vec![Value::Int(1)]);
    value_of_adt(&foreign, Span::DUMMY, "m").expect_err("a foreign ctor is not a `machine.Value`");
}

#[test]
fn the_adt_wire_carries_records_and_ctors_both_ways() {
    let value = record(vec![
        ("n", Value::Int(4)),
        (
            "c",
            Value::ctor(Symbol::new("m.Some"), vec![Value::str("x")]),
        ),
    ]);
    let wire = adt_to_wire(&machine_value(&value, "m").unwrap(), Span::DUMMY, "m").unwrap();
    let back = wire_to_adt(&wire, Span::DUMMY, "m").unwrap();
    assert_eq!(
        back.to_string(),
        machine_value(&value, "m").unwrap().to_string()
    );
}

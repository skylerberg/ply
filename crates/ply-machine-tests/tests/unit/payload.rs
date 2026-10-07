//! `std.value.Value`: what a `machine.call` argument or answer is, and what a raise carries.

// A `Value` pins `Arc` for shared payloads and `Rc` for shared code, so none of these `Arc`s can be `Send`.
#![allow(clippy::arc_with_non_send_sync)]

use ply_eval::reflect::{plain_of, value_of};
use ply_eval::{Diagnostic, Fixed, IntTy, Plain, Span, Symbol, Value, codes, slot};
use ply_machine::payload::{diag_value, raised_value};

/// A record with the fields a program declared, as a value.
fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(std::sync::Arc::new(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    ))
}

fn field(value: &Value, name: &str) -> Value {
    let Value::Record(fields) = value else {
        panic!("not a record: {value:?}");
    };
    fields.named(name).cloned().expect("the field is there")
}

/// The whole trip a `machine.call` argument takes, and its answer takes back.
fn crossed(value: &Value) -> Value {
    let data = value_of(&Plain::of(value));
    plain_of(&data, Span::DUMMY)
        .expect("`std.value` data reads back")
        .into_value()
        .expect("a crossable value becomes one again")
}

#[test]
fn every_crossable_value_survives_the_crossing() {
    let values = vec![
        Value::Unit,
        Value::Bool(true),
        Value::Int(-7),
        Value::Float(1.5),
        Value::str("hello"),
        Value::bytes([0u8, 1, 254, 255]),
        Value::list(vec![Value::Int(1), Value::Int(2)]),
        record(vec![("a", Value::Int(1)), ("b", Value::str("two"))]),
        Value::ctor(Symbol::new("std.json.Number"), vec![Value::Int(3)]),
        Value::map(vec![(Value::str("k"), Value::Int(9))]),
        Value::Fixed(Fixed::of(IntTy::U32, 4000000000).unwrap()),
        Value::Fixed(Fixed::of(IntTy::I8, -5).unwrap()),
        Value::Fixed(Fixed::new(IntTy::U64, u128::from(u64::MAX))),
        Value::Fixed(Fixed::new(IntTy::U128, u128::MAX)),
        Value::Fixed(Fixed::of(IntTy::I128, i128::MIN).unwrap()),
    ];
    for value in values {
        assert_eq!(crossed(&value), value, "{value:?} did not survive");
    }
}

#[test]
fn a_fixed_width_integer_crosses_as_the_pattern_its_width_reads() {
    let data = value_of(&Plain::of(&Value::Fixed(Fixed::of(IntTy::I8, -5).unwrap())));
    assert_eq!(
        data,
        Value::ctor(
            "std.value.VFixed",
            vec![
                Value::str("I8"),
                Value::Fixed(Fixed::new(IntTy::U128, 0xFB))
            ]
        )
    );
}

#[test]
fn a_generated_function_crosses_as_its_rule_and_is_callable_again() {
    let table = Plain::Fn(ply_eval::Fun::Table {
        arity: 1,
        entries: vec![(Plain::Int(1), Plain::Bool(true))],
        default: Box::new(Plain::Bool(false)),
    });
    let back = plain_of(&value_of(&table), Span::DUMMY).expect("reads back");
    assert_eq!(back, table);
    let called = back
        .into_value()
        .expect("a generated function is one again");
    let Value::Closure(c) = &called else {
        panic!("not a function");
    };
    let ply_eval::ClosureKind::Synth { rule, .. } = &c.kind else {
        panic!("not a generated function");
    };
    assert_eq!(
        rule.apply(&[Value::Int(1)]).expect("total"),
        Value::Bool(true)
    );
    assert_eq!(
        rule.apply(&[Value::Int(2)]).expect("total"),
        Value::Bool(false)
    );
}

#[test]
fn a_value_only_its_own_run_can_hold_is_refused_by_what_it_is() {
    let closure = Value::Closure(std::sync::Arc::new(ply_eval::Closure {
        name: Some(Symbol::new("m.Wrapped")),
        kind: ply_eval::ClosureKind::Ctor {
            name: Symbol::new("m.Wrapped"),
            arity: 1,
        },
    }));
    let copied = Plain::of(&closure);
    assert_eq!(
        copied,
        Plain::Fn(ply_eval::Fun::Named("m.Wrapped".to_string()))
    );
    let why = copied
        .into_value()
        .expect_err("a program's own function does not cross");
    assert!(why.contains("only its own run"), "{why}");
    let secret = Plain::of(&Value::secret_text("hunter2"));
    assert_eq!(secret, Plain::Secret);
    assert!(secret.into_value().is_err());
}

#[test]
fn an_answer_that_is_no_std_value_is_refused() {
    let foreign = Value::ctor(Symbol::new("other.Thing"), vec![Value::Int(1)]);
    plain_of(&foreign, Span::DUMMY).expect_err("a foreign constructor is not a `std.value.Value`");
}

#[test]
fn a_raise_hands_over_its_values_and_a_plain_crossing_says_what_they_are() {
    let d = Diagnostic::error(
        codes::ASSERTION_FAILED,
        format!("expected {}, found {}", slot(0), slot(1)),
    )
    .showing(vec![Plain::Int(1), Plain::Str("a".to_string())]);

    let raised = raised_value(&d);
    let message = field(&field(&raised, "diag"), "message");
    assert_eq!(
        message,
        Value::bytes(format!("expected {}, found {}", slot(0), slot(1)))
    );
    assert_eq!(
        field(&raised, "values"),
        Value::list(vec![
            value_of(&Plain::Int(1)),
            value_of(&Plain::Str("a".to_string()))
        ])
    );

    assert_eq!(
        field(&diag_value(&d), "message"),
        Value::bytes("expected an `Int`, found a `String`")
    );
}

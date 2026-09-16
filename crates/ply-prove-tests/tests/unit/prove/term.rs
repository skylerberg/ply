use ply_prove::prove::term::{Node, Terms};
use ply_span::Symbol;
use ply_ty::Type;

#[test]
fn a_linear_combination_is_canonical() {
    let mut terms = Terms::new();
    let x = terms.sym(Some(Type::int()));
    let zero = terms.int_lit(0);
    let plus_zero = terms.add(x, zero).unwrap();
    assert_eq!(plus_zero, x, "`x + 0` and `x` are one term");

    let one = terms.int_lit(1);
    let a = terms.add(x, one).unwrap();
    let b = terms.add(one, x).unwrap();
    assert_eq!(a, b, "addition commutes into one canonical form");

    let back = terms.sub(a, one).unwrap();
    assert_eq!(back, x);
}

/// The prover's own arithmetic must never wrap.
#[test]
fn a_coefficient_that_overflows_produces_no_term() {
    let mut terms = Terms::new();
    let x = terms.sym(Some(Type::int()));
    let big = terms.int_lit(i64::MAX);
    let scaled = terms.mul(x, big).unwrap();
    let again = terms.mul(scaled, big).unwrap();
    // i64::MAX cubed leaves i128.
    assert!(terms.mul(again, big).is_none());
}

/// A sum of two `i64::MAX`s is not an `Int`, so there is no literal to fold it to and the
/// prover declines rather than wrapping.
#[test]
fn a_constant_outside_int_produces_no_term() {
    let mut terms = Terms::new();
    let big = terms.int_lit(i64::MAX);
    assert!(terms.add(big, big).is_none());
    let small = terms.int_lit(i64::MIN);
    assert!(terms.add(small, small).is_none());
}

#[test]
fn multiplication_of_two_symbolics_is_not_arithmetic() {
    let mut terms = Terms::new();
    let x = terms.sym(Some(Type::int()));
    let y = terms.sym(Some(Type::int()));
    assert!(terms.mul(x, y).is_none());
}

#[test]
fn projection_reduces_over_a_record_literal() {
    let mut terms = Terms::new();
    let v = terms.int_lit(7);
    let record = terms.mk(Node::Record(vec![(Symbol::new("balance"), v)]), None);
    assert_eq!(terms.field(record, Symbol::new("balance")), v);
}

#[test]
fn every_fresh_symbol_is_a_distinct_term() {
    let mut terms = Terms::new();
    let a = terms.sym(None);
    let b = terms.sym(None);
    assert_ne!(a, b);
}

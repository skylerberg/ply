//! What the bit operators may and may not do to the `proved` tier.

use super::tests::{attempt_for_test, fixture, not_proved, proof};
use super::{Blocker, Decision};

const BITS: &str = r#"
law "a mask is a function" forall (x: Int, y: Int) { x & y == x & y }
law "a complement is a function" forall (x: Int) { ~x == ~x }
law "a masked value is a value" forall (x: Int, y: Int) where x & y > 0 { x & y > 0 }

law "a mask is its left operand" forall (x: Int, y: Int) { x & y == x }
law "a mask is no larger than what it masks" forall (x: Int, y: Int) { x & y <= x }
law "a left shift is a doubling" forall (x: Int) where x > 0 { x << 1 > x }
law "a complement is a bounded negation" forall (x: Int) where x > 0 && x < 100
  { ~x == 0 - x }

law "and is or" forall (x: Int, y: Int) { x & y == x | y }
law "xor is and" forall (x: Int, y: Int) { x ^ y == x & y }
law "the right shifts agree" forall (x: Int, n: Int) where n >= 0 && n <= 63
  { x >> n == x >>> n }

law "a shift is a function" forall (x: Int, n: Int) { x << n == x << n }
law "a bounded shift is a function" forall (x: Int, n: Int) where n >= 0 && n <= 63
  { x << n == x << n }
law "a shift by the last bit is a function" forall (x: Int) { x >>> 63 == x >>> 63 }
law "a shift past the last bit is a function" forall (x: Int) { x >>> 64 == x >>> 64 }
law "a shift by a negative is a function" forall (x: Int) { x >> -1 == x >> -1 }

law "masking commutes" forall (x: Int, y: Int) { x & y == y & x }
law "a complement is an involution" forall (x: Int) { ~(~x) == x }
"#;

/// The wrong `proved`s: the laws that would pass if an operator folded into the arithmetic.
#[test]
fn no_bit_operator_folds_into_the_arithmetic() {
    let f = fixture(BITS);
    for label in [
        "a mask is its left operand",
        "a mask is no larger than what it masks",
        "a left shift is a doubling",
        "a complement is a bounded negation",
    ] {
        not_proved(&f, label);
    }
}

/// Seven operators, seven symbols.
#[test]
fn the_bit_operators_are_seven_functions_and_not_one() {
    let f = fixture(BITS);
    not_proved(&f, "and is or");
    not_proved(&f, "xor is and");
    not_proved(&f, "the right shifts agree");
}

/// Uninterpreted is not unknown: congruence still decides that a function equals itself.
#[test]
fn a_bit_operator_is_a_function_of_its_operands() {
    let f = fixture(BITS);
    proof(&f, "a mask is a function");
    proof(&f, "a complement is a function");
    proof(&f, "a masked value is a value");
}

/// A shift count outside `0..=63` raises (ADR 0033 §2.2), so a shift is a value only where its
/// count is a bit position — and the fragment decides that condition, so the guarded restatement is
/// back in reach.
#[test]
fn a_shift_is_a_value_only_where_its_count_is_a_bit_position() {
    let f = fixture(BITS);
    not_proved(&f, "a shift is a function");
    proof(&f, "a bounded shift is a function");

    proof(&f, "a shift by the last bit is a function");
    not_proved(&f, "a shift past the last bit is a function");
    not_proved(&f, "a shift by a negative is a function");
}

/// True of every `Int` and still `property`, which is the cost side of the decision and belongs in
/// the record beside the benefit.
#[test]
fn a_true_law_about_the_bit_pattern_is_property_and_not_proved() {
    let f = fixture(BITS);
    for label in ["masking commutes", "a complement is an involution"] {
        let (decision, blockers) = attempt_for_test(&f, label);
        assert!(
            !matches!(decision, Decision::Proved(_)),
            "`{label}` is true of every Int and outside the fragment: {decision:?}"
        );
        assert!(
            blockers.contains(&Blocker::BitOperator),
            "`{label}` left the fragment at a bit operator, and the reach table \
             is what says so: {blockers:?}"
        );
    }
}

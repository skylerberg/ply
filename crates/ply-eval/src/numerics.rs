//! Evaluating `Float` and `Decimal`.

use crate::build::*;
use crate::value::{Decimal, values_equal};
use crate::{Machine, Value};
use ply_span::{Diagnostic, Span, codes};
use ply_syntax::ast::{BinOp, Expr, ExprKind, Ident, Lit, QName, UnOp};

fn eval(e: Expr) -> Result<Value, Diagnostic> {
    let (program, resolved) = standalone(Vec::new());
    Machine::for_program(&program, &resolved).eval_expr_for_test(&e)
}

#[track_caller]
fn ok(e: Expr) -> Value {
    eval(e).unwrap_or_else(|d| panic!("expected a value, got {}: {}", d.code, d.message))
}

#[track_caller]
fn ok_float(e: Expr) -> f64 {
    match ok(e) {
        Value::Float(f) => f,
        other => panic!("expected a Float, got {other}"),
    }
}

#[track_caller]
fn ok_decimal(e: Expr) -> Decimal {
    match ok(e) {
        Value::Decimal(d) => d,
        other => panic!("expected a Decimal, got {other}"),
    }
}

#[track_caller]
fn err(e: Expr) -> Diagnostic {
    match eval(e) {
        Err(d) => d,
        Ok(v) => panic!("expected a diagnostic, got {v}"),
    }
}

fn lit(l: Lit) -> Expr {
    Expr {
        kind: ExprKind::Lit(l),
        span: Span::DUMMY,
    }
}

fn float(v: f64) -> Expr {
    lit(Lit::Float(v))
}

fn dec(mantissa: i128, scale: u32) -> Expr {
    lit(Lit::Decimal { mantissa, scale })
}

fn d(mantissa: i128, scale: u32) -> Decimal {
    Decimal::try_from_i128_with_scale(mantissa, scale).expect("in range")
}

/// A `Rounding` constructor, named bare.
fn rounding(mode: &str) -> Expr {
    Expr {
        kind: ExprKind::Var(QName::bare(Ident::new(mode, Span::DUMMY))),
        span: Span::DUMMY,
    }
}

// -- Float ------------------------------------------------------------------

/// IEEE, unmodified.
#[test]
fn float_arithmetic_is_ieee_at_the_edges() {
    assert_eq!(
        ok_float(bin(BinOp::Div, float(1.0), float(0.0))),
        f64::INFINITY
    );
    assert_eq!(
        ok_float(bin(BinOp::Div, float(-1.0), float(0.0))),
        f64::NEG_INFINITY
    );
    assert!(ok_float(bin(BinOp::Div, float(0.0), float(0.0))).is_nan());
    assert!(ok_float(bin(BinOp::Add, float(f64::NAN), float(1.0))).is_nan());
    // `1.0 / -0.0` is what makes `0.0` and `-0.0` two definitions rather than one, so it had better
    // tell them apart.
    assert_eq!(
        ok_float(bin(BinOp::Div, float(1.0), un(UnOp::Neg, float(0.0)))),
        f64::NEG_INFINITY
    );
}

/// The headline arithmetic difference between the two numeric types, in one test, because it is the
/// reason both exist.
#[test]
fn binary_floating_point_loses_a_hundredth_and_decimal_does_not() {
    assert_ne!(ok_float(bin(BinOp::Add, float(0.1), float(0.2))), 0.3);
    assert_eq!(
        ok_decimal(bin(BinOp::Add, dec(1, 1), dec(2, 1))),
        d(3, 1),
        "0.1m + 0.2m is exactly 0.3m"
    );
}

/// `==` on `Float` is IEEE's, which is the source of every restriction on the type: not an ordered
/// key type, not derivable for `ord`, never `proved`.
#[test]
fn float_equality_is_not_reflexive_and_zero_has_no_sign() {
    assert_eq!(
        ok(bin(BinOp::Eq, float(f64::NAN), float(f64::NAN))),
        Value::Bool(false)
    );
    assert_eq!(
        ok(bin(BinOp::Ne, float(f64::NAN), float(f64::NAN))),
        Value::Bool(true)
    );
    assert_eq!(
        ok(bin(BinOp::Eq, float(0.0), un(UnOp::Neg, float(0.0)))),
        Value::Bool(true)
    );
}

/// A `NaN` comparison is false in both directions, so `<` is not the negation of `>=`.
#[test]
fn a_nan_comparison_is_false_in_both_directions() {
    for op in [BinOp::Lt, BinOp::Le, BinOp::Gt, BinOp::Ge] {
        assert_eq!(
            ok(bin(op, float(f64::NAN), float(1.0))),
            Value::Bool(false),
            "{op:?} against a NaN"
        );
    }
    assert_eq!(
        ok(bin(BinOp::Lt, float(1.0), float(2.0))),
        Value::Bool(true)
    );
}

/// `values_equal` is the language's `==` and `Value::cmp` is the map's order.
#[test]
fn the_language_equality_and_the_map_order_part_only_at_nan_and_signed_zero() {
    let nan = Value::Float(f64::NAN);
    let zero = Value::Float(0.0);
    let negative_zero = Value::Float(-0.0);

    assert!(!values_equal(&nan, &nan, Span::DUMMY).unwrap());
    assert_eq!(nan.cmp(&nan), std::cmp::Ordering::Equal);

    assert!(values_equal(&zero, &negative_zero, Span::DUMMY).unwrap());
    assert_ne!(zero.cmp(&negative_zero), std::cmp::Ordering::Equal);

    // Everywhere else the two agree, which is what makes `Float` the documented exception rather
    // than an unexplored corner.
    for a in [1.0f64, -1.0, f64::INFINITY, f64::MAX, 0.5] {
        for b in [1.0f64, -1.0, f64::INFINITY, f64::MAX, 0.5] {
            let (x, y) = (Value::Float(a), Value::Float(b));
            assert_eq!(
                values_equal(&x, &y, Span::DUMMY).unwrap(),
                x.cmp(&y) == std::cmp::Ordering::Equal,
                "{a} vs {b}"
            );
        }
    }
}

#[test]
fn a_float_renders_so_it_cannot_be_read_as_an_int() {
    assert_eq!(Value::Float(1.0).render(), "1.0");
    assert_eq!(Value::Float(-0.0).render(), "-0.0");
    assert_eq!(Value::Float(f64::NAN).render(), "NaN");
    assert_eq!(Value::Float(f64::INFINITY).render(), "Infinity");
    assert_eq!(Value::Float(f64::NEG_INFINITY).render(), "-Infinity");
    assert_eq!(Value::Float(1.0).type_name(), "Float");
}

// -- Decimal ----------------------------------------------------------------

/// Exact, or a diagnostic.
#[test]
fn a_decimal_addition_that_overflows_the_mantissa_is_a_runtime_error() {
    let max = Decimal::MAX;
    let e = err(bin(BinOp::Add, dec(max.mantissa(), max.scale()), dec(1, 0)));
    assert_eq!(e.code, codes::RUNTIME_ERROR);
    assert!(e.message.contains("overflow"), "{}", e.message);
    assert!(
        e.notes.iter().any(|n| n.contains("will not round")),
        "the note has to say it did not round: {:?}",
        e.notes
    );
}

/// `%` is exact, and is therefore allowed where `/` is not: the *remainder* of a decimal division
/// is a decimal even when the quotient is not.
#[test]
fn decimal_remainder_is_exact_and_a_zero_divisor_is_an_error() {
    assert_eq!(ok_decimal(bin(BinOp::Rem, dec(10, 0), dec(3, 0))), d(1, 0));
    assert_eq!(
        err(bin(BinOp::Rem, dec(1, 0), dec(0, 0))).code,
        codes::RUNTIME_ERROR
    );
}

/// The evaluator's own refusal, behind inference's.
#[test]
fn decimal_division_is_refused_by_the_evaluator_too() {
    let e = err(bin(BinOp::Div, dec(1, 0), dec(3, 0)));
    assert_eq!(e.code, codes::DECIMAL_DIVISION);
    assert!(
        e.notes.iter().any(|n| n.contains("decimal_div")),
        "the diagnostic has to name the replacement: {:?}",
        e.notes
    );
}

/// Half-to-even, at the two points that tell it from half-up: `0.125` has an even digit below it
/// and rounds down, `0.135` has an odd one and rounds up.
#[test]
fn decimal_div_and_round_are_half_to_even() {
    let div = callv(
        "decimal_div",
        vec![dec(1, 0), dec(3, 0), int(2), rounding("HalfEven")],
    );
    assert_eq!(ok_decimal(div), d(33, 2));

    let down = callv(
        "decimal_round",
        vec![dec(125, 3), int(2), rounding("HalfEven")],
    );
    assert_eq!(ok_decimal(down), d(12, 2));

    let up = callv(
        "decimal_round",
        vec![dec(135, 3), int(2), rounding("HalfEven")],
    );
    assert_eq!(ok_decimal(up), d(14, 2));

    // Half-up is a different answer at the same point, which is why the mode is an argument rather
    // than a default.
    let half_up = callv(
        "decimal_round",
        vec![dec(125, 3), int(2), rounding("HalfUp")],
    );
    assert_eq!(ok_decimal(half_up), d(13, 2));
}

#[test]
fn decimal_div_refuses_a_zero_divisor_and_a_scale_outside_the_range() {
    let zero = callv(
        "decimal_div",
        vec![dec(1, 0), dec(0, 0), int(2), rounding("HalfEven")],
    );
    assert_eq!(err(zero).code, codes::RUNTIME_ERROR);

    let scale = callv(
        "decimal_div",
        vec![dec(1, 0), dec(3, 0), int(29), rounding("HalfEven")],
    );
    let e = err(scale);
    assert_eq!(e.code, codes::RUNTIME_ERROR);
    assert!(e.message.contains("0..=28"), "{}", e.message);

    let negative = callv(
        "decimal_round",
        vec![dec(1, 0), int(-1), rounding("HalfEven")],
    );
    assert_eq!(err(negative).code, codes::RUNTIME_ERROR);
}

/// Identity, scale included.
#[test]
fn decimal_to_string_after_decimal_of_string_is_identity() {
    for text in [
        "0",
        "1",
        "1.5",
        "1.50",
        "-1.50",
        "0.00",
        "19.99",
        "79228162514264337593543950335",
        "0.0000000000000000000000000001",
    ] {
        let round_trip = callv(
            "decimal_to_string",
            vec![unwrap_some(callv("decimal_of_string", vec![string(text)]))],
        );
        assert_eq!(ok(round_trip), Value::str(text), "round-tripping `{text}`");
    }
}

#[test]
fn decimal_of_string_answers_none_rather_than_guessing() {
    for text in [
        "",
        "abc",
        "1.2.3",
        "1e400",
        "1e-40",
        "999999999999999999999999999999999",
    ] {
        assert_eq!(
            ok(callv("decimal_of_string", vec![string(text)])),
            Value::ctor("None", Vec::new()),
            "`{text}` is not a Decimal"
        );
    }
}

/// JSON's number grammar admits an exponent and `std.json` hands the whole token here, so a number
/// that is well inside `Decimal`'s range must not be reported as outside it.
#[test]
fn decimal_of_string_reads_the_exponent_form() {
    for (text, expect) in [
        ("1e3", "1000"),
        ("1E3", "1000"),
        ("1e+3", "1000"),
        ("-1e3", "-1000"),
        ("1e-3", "0.001"),
        ("1.05e2", "105"),
        ("1e28", "10000000000000000000000000000"),
        ("1e-28", "0.0000000000000000000000000001"),
    ] {
        let parsed = unwrap_some(callv("decimal_of_string", vec![string(text)]));
        assert_eq!(
            ok(callv("decimal_to_string", vec![parsed])),
            Value::str(expect),
            "`{text}`"
        );
    }
}

/// The **shortest** decimal that round-trips the float, which is the only defensible choice: `0.1`
/// as a binary64 is not `0.1`, and any other answer is an arbitrary number of digits of a binary
/// approximation.
#[test]
fn decimal_of_float_is_the_shortest_round_tripping_decimal() {
    let shortest = callv(
        "decimal_to_string",
        vec![unwrap_some(callv("decimal_of_float", vec![float(0.1)]))],
    );
    assert_eq!(ok(shortest), Value::str("0.1"));

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300] {
        assert_eq!(
            ok(callv("decimal_of_float", vec![float(bad)])),
            Value::ctor("None", Vec::new()),
            "{bad} has no Decimal"
        );
    }
}

#[test]
fn the_float_bit_pattern_round_trips_every_value_including_nan() {
    assert_eq!(
        ok(callv("bits_of_float", vec![float(1.5)])),
        Value::Int(0x3FF8_0000_0000_0000)
    );
    assert_eq!(
        ok(callv("bits_of_float", vec![float(-0.0)])),
        Value::Int(i64::MIN),
        "the sign of a negative zero is a bit like any other"
    );
    assert_eq!(
        ok_float(callv("float_of_bits", vec![int(0x3FF8_0000_0000_0000)])),
        1.5
    );
    for f in [0.0, -1.0, 1e300, f64::INFINITY, f64::MIN_POSITIVE] {
        let back = callv(
            "float_of_bits",
            vec![callv("bits_of_float", vec![float(f)])],
        );
        assert_eq!(ok_float(back), f, "{f}");
    }
    let nan = callv("bits_of_float", vec![float(f64::NAN)]);
    let through = callv(
        "bits_of_float",
        vec![callv("float_of_bits", vec![nan.clone()])],
    );
    assert_eq!(
        ok(through),
        ok(nan),
        "a NaN keeps its payload through both directions"
    );
}

#[test]
fn the_int_and_float_conversions_are_total_where_they_claim_to_be() {
    assert_eq!(ok_decimal(callv("decimal_of_int", vec![int(-7)])), d(-7, 0));
    assert_eq!(ok_float(callv("float_of_decimal", vec![dec(15, 1)])), 1.5);
    assert_eq!(
        ok(callv(
            "int_of_decimal",
            vec![dec(15, 1), rounding("HalfEven")]
        )),
        Value::ctor("Some", vec![Value::Int(2)]),
        "1.5 rounds half-to-even to 2"
    );
    assert_eq!(
        ok(callv(
            "int_of_decimal",
            vec![dec(25, 1), rounding("HalfEven")]
        )),
        Value::ctor("Some", vec![Value::Int(2)]),
        "2.5 rounds half-to-even to 2 as well"
    );
    let max = Decimal::MAX;
    assert_eq!(
        ok(callv(
            "int_of_decimal",
            vec![dec(max.mantissa(), max.scale()), rounding("Down")]
        )),
        Value::ctor("None", Vec::new()),
        "outside `i64` is `None`, not a wrap"
    );
}

/// By numeric value, so `1.50m == 1.5m` — which is the same fact that makes the two one map key
/// while leaving them two definitions.
#[test]
fn decimal_equality_is_by_value_and_ignores_the_scale() {
    assert_eq!(
        ok(bin(BinOp::Eq, dec(150, 2), dec(15, 1))),
        Value::Bool(true)
    );
    assert!(
        values_equal(
            &Value::Decimal(d(150, 2)),
            &Value::Decimal(d(15, 1)),
            Span::DUMMY
        )
        .unwrap()
    );
    assert_eq!(
        Value::Decimal(d(150, 2)).cmp(&Value::Decimal(d(15, 1))),
        std::cmp::Ordering::Equal
    );
    // And the rendering still shows what the value carries.
    assert_eq!(Value::Decimal(d(150, 2)).render(), "1.50");
    assert_eq!(Value::Decimal(d(15, 1)).render(), "1.5");
}

#[test]
fn decimal_comparison_is_by_value() {
    assert_eq!(
        ok(bin(BinOp::Lt, dec(150, 2), dec(2, 0))),
        Value::Bool(true)
    );
    assert_eq!(
        ok(bin(BinOp::Le, dec(150, 2), dec(15, 1))),
        Value::Bool(true)
    );
    assert_eq!(
        ok(bin(BinOp::Gt, dec(-1, 0), dec(1, 0))),
        Value::Bool(false)
    );
}

/// Mixing the numeric types is a runtime error rather than a coercion.
#[test]
fn the_numeric_types_do_not_mix_at_runtime() {
    assert_eq!(
        err(bin(BinOp::Add, float(1.0), int(1))).code,
        codes::RUNTIME_ERROR
    );
    assert_eq!(
        err(bin(BinOp::Add, dec(1, 0), int(1))).code,
        codes::RUNTIME_ERROR
    );
    assert_eq!(
        ok(bin(BinOp::Eq, float(1.0), dec(1, 0))),
        Value::Bool(false),
        "`==` across two types is false rather than an error"
    );
}

/// Negation is how a program reaches `-0.0`, and it is exact on a `Decimal` at every value the type
/// holds.
#[test]
fn negation_works_at_all_three_numeric_types() {
    assert!(ok_float(un(UnOp::Neg, float(0.0))).is_sign_negative());
    assert_eq!(ok_decimal(un(UnOp::Neg, dec(150, 2))), d(-150, 2));
    assert_eq!(ok(un(UnOp::Neg, int(5))), Value::Int(-5));
}

/// `Some(x)` from a builtin, unwrapped — the prelude's `Option`, reachable without a `type`
/// declaration anywhere.
fn unwrap_some(e: Expr) -> Expr {
    match_(
        e,
        vec![
            arm(pctor("Some", vec![pvar("v")]), var("v")),
            arm(pwild(), callv("panic", vec![string("None")])),
        ],
    )
}

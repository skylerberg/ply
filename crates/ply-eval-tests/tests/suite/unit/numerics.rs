use crate::unit::build::*;
use ply_eval::{Decimal, values_equal};
use ply_eval::{Machine, Value};
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

fn rounding(mode: &str) -> Expr {
    Expr {
        kind: ExprKind::Var(QName::bare(Ident::new(mode, Span::DUMMY))),
        span: Span::DUMMY,
    }
}

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
    // `1.0 / -0.0` is what tells `0.0` and `-0.0` apart.
    assert_eq!(
        ok_float(bin(BinOp::Div, float(1.0), un(UnOp::Neg, float(0.0)))),
        f64::NEG_INFINITY
    );
}

#[test]
fn binary_floating_point_loses_a_hundredth_and_decimal_does_not() {
    assert_ne!(ok_float(bin(BinOp::Add, float(0.1), float(0.2))), 0.3);
    assert_eq!(
        ok_decimal(bin(BinOp::Add, dec(1, 1), dec(2, 1))),
        d(3, 1),
        "0.1m + 0.2m is exactly 0.3m"
    );
}

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

    // Everywhere else the two agree.
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

/// The remainder of a decimal division is a decimal even when the quotient is not.
#[test]
fn decimal_remainder_is_exact_and_a_zero_divisor_is_an_error() {
    assert_eq!(ok_decimal(bin(BinOp::Rem, dec(10, 0), dec(3, 0))), d(1, 0));
    assert_eq!(
        err(bin(BinOp::Rem, dec(1, 0), dec(0, 0))).code,
        codes::RUNTIME_ERROR
    );
}

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

/// `0.125` has an even digit below it and rounds down; `0.135` has an odd one and rounds up.
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

    // Half-up is a different answer at the same point.
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

/// `std.json` hands the whole number token, exponent included, to this builtin.
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

/// From the lexer itself, so the agreement is with the parse a program's literals go through.
#[track_caller]
fn lexed_float(text: &str) -> f64 {
    let (tokens, diags) = ply_syntax::lexer::lex(ply_span::SourceId(0), text);
    assert!(diags.is_empty(), "`{text}` does not lex: {diags:#?}");
    match tokens.first().map(|t| &t.kind) {
        Some(ply_syntax::lexer::TokenKind::Float(v)) => *v,
        other => panic!("`{text}` does not lex as a float literal: {other:?}"),
    }
}

/// A front end holding only a literal's text hashes it correctly exactly when this is the lexer's parse.
#[test]
fn float_of_string_is_the_parse_the_lexer_makes_of_the_same_literal() {
    for text in [
        "1.5", "0.1", "1e9", "2.5e-1", "1_000.5", "1.0e-30", "1.0e300", "1e400",
    ] {
        let read = ok(callv(
            "bits_of_float",
            vec![unwrap_some(callv("float_of_string", vec![string(text)]))],
        ));
        let literal = ok(callv("bits_of_float", vec![float(lexed_float(text))]));
        assert_eq!(read, literal, "`{text}`");
    }

    // Spellings that are not one float literal, which the builtin still reads: an integer and a sign.
    assert_eq!(
        ok_float(unwrap_some(callv("float_of_string", vec![string("1")]))),
        1.0
    );
    assert_eq!(
        ok_float(unwrap_some(callv("float_of_string", vec![string("-2.5")]))),
        -2.5
    );

    // No `Decimal` sits between these extremes and their text, so this cannot be two calls.
    for f in [1.0e-30, 1.0e300] {
        assert_eq!(
            ok(callv("decimal_of_float", vec![float(f)])),
            Value::ctor("None", Vec::new()),
            "{f} has no Decimal"
        );
    }
}

/// Rust's `f64::from_str` accepts `inf` and `NaN`, which no Ply literal spells.
#[test]
fn float_of_string_answers_none_rather_than_guessing() {
    for text in [
        "", "abc", "inf", "-inf", "infinity", "NaN", "nan", "1.", ".5", "1e", "1e+", "1.2.3",
        "0x10", " 1.5", "1,5", "1.5f",
    ] {
        assert_eq!(
            ok(callv("float_of_string", vec![string(text)])),
            Value::ctor("None", Vec::new()),
            "`{text}` is not a float literal"
        );
    }
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

#[test]
fn negation_works_at_all_three_numeric_types() {
    assert!(ok_float(un(UnOp::Neg, float(0.0))).is_sign_negative());
    assert_eq!(ok_decimal(un(UnOp::Neg, dec(150, 2))), d(-150, 2));
    assert_eq!(ok(un(UnOp::Neg, int(5))), Value::Int(-5));
}

/// `Some(x)` from a builtin, unwrapped.
fn unwrap_some(e: Expr) -> Expr {
    match_(
        e,
        vec![
            arm(pctor("Some", vec![pvar("v")]), var("v")),
            arm(pwild(), callv("panic", vec![string("None")])),
        ],
    )
}

//! Lexing and parsing the two numeric literals W2 adds.

use crate::ast::{Expr, ExprKind, Lit, Pattern, PatternKind, render_float};
use crate::lexer::{TokenKind, lex, render_decimal};
use crate::parser::{parse, parse_expr};
use ply_span::SourceId;

const SRC: SourceId = SourceId(0);

fn kinds(text: &str) -> Vec<TokenKind> {
    let (tokens, diags) = lex(SRC, text);
    assert!(diags.is_empty(), "`{text}` did not lex: {diags:?}");
    tokens.into_iter().map(|t| t.kind).collect()
}

fn lex_err(text: &str) -> String {
    let (_, diags) = lex(SRC, text);
    assert!(!diags.is_empty(), "`{text}` lexed cleanly");
    format!("{} {}", diags[0].code, diags[0].message)
}

fn lit(text: &str) -> Lit {
    match parse_expr(SRC, text) {
        Ok(Expr {
            kind: ExprKind::Lit(l),
            ..
        }) => l,
        other => panic!("`{text}` did not parse as a literal: {other:?}"),
    }
}

#[test]
fn the_three_numeric_literals_are_three_tokens() {
    assert_eq!(kinds("1")[0], TokenKind::Int(1));
    assert_eq!(kinds("1.0")[0], TokenKind::Float(1.0));
    assert_eq!(
        kinds("1m")[0],
        TokenKind::Decimal {
            mantissa: 1,
            scale: 0
        }
    );
}

/// The scale is what the literal said, not what its value needs.
#[test]
fn a_decimal_literal_keeps_its_trailing_zeros() {
    assert_eq!(
        kinds("1.50m")[0],
        TokenKind::Decimal {
            mantissa: 150,
            scale: 2
        }
    );
    assert_eq!(
        kinds("1.5m")[0],
        TokenKind::Decimal {
            mantissa: 15,
            scale: 1
        }
    );
    assert_eq!(
        kinds("0.000m")[0],
        TokenKind::Decimal {
            mantissa: 0,
            scale: 3
        }
    );
}

#[test]
fn floats_take_a_fraction_an_exponent_or_both() {
    assert_eq!(kinds("0.5")[0], TokenKind::Float(0.5));
    assert_eq!(kinds("1e9")[0], TokenKind::Float(1e9));
    assert_eq!(kinds("1E9")[0], TokenKind::Float(1e9));
    assert_eq!(kinds("1.5e-3")[0], TokenKind::Float(1.5e-3));
    assert_eq!(kinds("1_000.5")[0], TokenKind::Float(1000.5));
}

/// `..` is the range separator and `e` starts an identifier, so both have to be distinguishable
/// from a number's continuation by lookahead alone.
#[test]
fn a_number_stops_before_a_range_and_before_a_bare_letter() {
    assert_eq!(
        kinds("1..5"),
        vec![
            TokenKind::Int(1),
            TokenKind::DotDot,
            TokenKind::Int(5),
            TokenKind::Eof
        ]
    );
    // No digits behind the `e`, so it is not an exponent — and an identifier glued to a number is
    // the suffix error it has always been.
    assert!(lex_err("1else").contains("invalid suffix"));
}

/// `m` is a suffix only when nothing follows it that could continue a name; otherwise `1max` would
/// silently become `1m` followed by `ax`.
#[test]
fn the_decimal_suffix_is_only_a_suffix_when_it_ends_the_literal() {
    assert_eq!(
        kinds("1m")[0],
        TokenKind::Decimal {
            mantissa: 1,
            scale: 0
        }
    );
    assert!(lex_err("1max").contains("invalid suffix"));
}

#[test]
fn a_decimal_outside_the_types_range_names_the_limit() {
    let scale = lex_err("1.000000000000000000000000000000m");
    assert!(scale.contains("E0001") && scale.contains("28"), "{scale}");

    let mantissa = lex_err("792281625142643375935439503350m");
    assert!(
        mantissa.contains("E0001") && mantissa.contains("96 bits"),
        "{mantissa}"
    );

    let exponent = lex_err("1e9m");
    assert!(exponent.contains("no exponent"), "{exponent}");
}

/// A decimal-to-binary conversion that overflows produces an infinity; that is what IEEE says, and
/// refusing it here would give `Float` a range the standard does not.
#[test]
fn a_float_literal_beyond_the_range_is_an_infinity_rather_than_an_error() {
    assert_eq!(kinds("1e400")[0], TokenKind::Float(f64::INFINITY));
}

#[test]
fn literals_parse_into_the_expression_and_the_pattern_grammar() {
    assert_eq!(lit("1.5"), Lit::Float(1.5));
    assert_eq!(
        lit("2.50m"),
        Lit::Decimal {
            mantissa: 250,
            scale: 2
        }
    );

    let module = parse(
        SRC,
        "fn f(x: Float) -> Int = match x { -1.5 -> 1, 0.0 -> 2, _ -> 3 }",
    )
    .expect("parses");
    let arms = match &module.items[0] {
        crate::ast::Item::Fn(def) => match &def.body.kind {
            ExprKind::Match { arms, .. } => arms.clone(),
            other => panic!("expected a match: {other:?}"),
        },
        other => panic!("expected a fn: {other:?}"),
    };
    assert!(matches!(
        arms[0].pat,
        Pattern {
            kind: PatternKind::Lit(Lit::Float(f)),
            ..
        } if f == -1.5
    ));
}

/// A negative decimal pattern is one literal, not an operator applied to one: a pattern is not an
/// expression and there is nothing to apply.
#[test]
fn a_negative_decimal_pattern_is_one_literal() {
    let module = parse(
        SRC,
        "fn f(x: Decimal) -> Int = match x { -1.50m -> 1, _ -> 2 }",
    )
    .expect("parses");
    let arms = match &module.items[0] {
        crate::ast::Item::Fn(def) => match &def.body.kind {
            ExprKind::Match { arms, .. } => arms.clone(),
            other => panic!("expected a match: {other:?}"),
        },
        other => panic!("expected a fn: {other:?}"),
    };
    assert!(matches!(
        &arms[0].pat.kind,
        PatternKind::Lit(Lit::Decimal {
            mantissa: -150,
            scale: 2
        })
    ));
}

/// A `Float` never renders as an `Int`: the two are different types, and a diagnostic that printed
/// `1` for both would make an expected/actual pair unreadable at exactly the moment it matters.
#[test]
fn a_float_always_renders_with_a_point_or_an_exponent() {
    assert_eq!(render_float(1.0), "1.0");
    assert_eq!(render_float(-0.0), "-0.0");
    assert_eq!(render_float(0.5), "0.5");
    assert_eq!(render_float(1e300), "1e300");
    assert_eq!(render_float(f64::NAN), "NaN");
    assert_eq!(render_float(f64::INFINITY), "Infinity");
    assert_eq!(render_float(f64::NEG_INFINITY), "-Infinity");
}

#[test]
fn a_decimal_renders_at_the_scale_it_carries() {
    assert_eq!(render_decimal(150, 2), "1.50");
    assert_eq!(render_decimal(15, 1), "1.5");
    assert_eq!(render_decimal(1, 0), "1");
    assert_eq!(render_decimal(-150, 2), "-1.50");
    assert_eq!(render_decimal(5, 3), "0.005");
    assert_eq!(render_decimal(0, 2), "0.00");
}

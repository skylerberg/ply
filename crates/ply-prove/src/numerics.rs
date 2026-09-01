//! Drawing and shrinking `Float` and `Decimal`.

use crate::property::{EDGE_CASES, GenStream, TypeWorld, generatable, generate};
use crate::shrink::{candidates, minimal, size};
use ply_core::Type;
use ply_eval::{Decimal, Value};
use ply_hash::DefHash;

fn world() -> TypeWorld {
    TypeWorld::new(&[])
}

fn key() -> DefHash {
    DefHash([7u8; 32])
}

fn draw(ty: &Type, cases: u32) -> Vec<Value> {
    let world = world();
    let mut stream = GenStream::new(1, key());
    (0..cases)
        .map(|case| generate(ty, &world, &mut stream, case).expect("generatable"))
        .collect()
}

fn floats(cases: u32) -> Vec<f64> {
    draw(&Type::float(), cases)
        .into_iter()
        .map(|v| match v {
            Value::Float(f) => f,
            other => panic!("expected a Float, got {other}"),
        })
        .collect()
}

fn decimals(cases: u32) -> Vec<Decimal> {
    draw(&Type::decimal(), cases)
        .into_iter()
        .map(|v| match v {
            Value::Decimal(d) => d,
            other => panic!("expected a Decimal, got {other}"),
        })
        .collect()
}

/// The same guarantee `Bytes` earned in W1: a new primitive that could not be quantified over would
/// regress M8 on contact.
#[test]
fn both_numeric_types_are_generatable() {
    let world = world();
    assert!(generatable(&Type::float(), &world).is_ok());
    assert!(generatable(&Type::decimal(), &world).is_ok());
    assert!(generatable(&Type::list(Type::float()), &world).is_ok());
}

/// The edge cases are drawn **first and every time**, which is what turns "a generator that samples
/// NaN" into a guarantee rather than a probability.
#[test]
fn the_first_float_cases_are_the_specials() {
    let drawn = floats(EDGE_CASES);
    assert!(drawn[0].is_nan(), "NaN is drawn first: {drawn:?}");
    assert!(
        drawn.iter().any(|f| *f == 0.0 && f.is_sign_positive()),
        "no 0.0: {drawn:?}"
    );
    assert!(
        drawn.iter().any(|f| *f == 0.0 && f.is_sign_negative()),
        "no -0.0: {drawn:?}"
    );
}

/// The rest of the edge — the infinities and the ends of the range — arrives through the biased
/// sampler rather than the first five slots, because there are more specials than there are edge
/// cases.
#[test]
fn an_ordinary_run_reaches_the_infinities_and_the_ends_of_the_range() {
    let drawn = floats(200);
    assert!(drawn.contains(&f64::INFINITY), "no +Infinity: {drawn:?}");
    assert!(
        drawn.contains(&f64::NEG_INFINITY),
        "no -Infinity: {drawn:?}"
    );
    assert!(drawn.contains(&f64::MAX), "no MAX: {drawn:?}");
}

/// A `Float` law is checked over values a program can actually meet, so the draw has to reach
/// ordinary magnitudes as well as the ends of the range.
#[test]
fn the_float_draw_reaches_ordinary_finite_values() {
    let drawn = floats(200);
    assert!(
        drawn
            .iter()
            .any(|f| f.is_finite() && *f != 0.0 && f.abs() < 1e6),
        "everything drawn was a special or enormous: {drawn:?}"
    );
    assert!(
        drawn.iter().any(|f| f.is_sign_negative() && f.is_finite()),
        "nothing negative was drawn"
    );
}

/// Money is written at two places and a rate at four, so the interesting scales are small — and
/// `MIN`/`MAX` are where an exact addition overflows, which is the failure `Decimal` reports rather
/// than hides.
#[test]
fn the_decimal_draw_covers_small_scales_and_the_ends_of_the_range() {
    let drawn = decimals(200);
    assert!(drawn.iter().any(|d| d.is_zero()), "no zero: {drawn:?}");
    assert!(drawn.contains(&Decimal::MAX), "no MAX");
    assert!(drawn.contains(&Decimal::MIN), "no MIN");
    for scale in 0..=6u32 {
        assert!(
            drawn.iter().any(|d| d.scale() == scale),
            "no draw at scale {scale}"
        );
    }
    assert!(
        drawn.iter().all(|d| d.scale() <= 28),
        "a draw left the type's range"
    );
}

/// Deterministic, so a reported `(root, case)` names a tuple somebody can draw again without
/// re-running anything.
#[test]
fn a_numeric_draw_is_a_function_of_its_root_and_case() {
    let first = floats(40);
    let again = floats(40);
    assert_eq!(
        first.iter().map(|f| f.to_bits()).collect::<Vec<_>>(),
        again.iter().map(|f| f.to_bits()).collect::<Vec<_>>()
    );
    assert_eq!(decimals(40), decimals(40));
}

/// `0.0` and not `-0.0`: the two are different values and the positive one is the floor.
#[test]
fn the_floor_of_each_numeric_type_is_its_smallest_value() {
    let world = world();
    let zero = minimal(&Type::float(), &world).unwrap();
    assert!(matches!(zero, Value::Float(f) if f == 0.0 && f.is_sign_positive()));
    assert_eq!(
        minimal(&Type::decimal(), &world).unwrap(),
        Value::Decimal(Decimal::ZERO)
    );
}

/// Every candidate is a strict descent by [`size`], which is what makes the walk terminate whatever
/// the budget is.
#[test]
fn every_numeric_candidate_is_strictly_smaller() {
    let world = world();
    let subjects = [
        Value::Float(0.30000000000000004),
        Value::Float(-1.5),
        Value::Float(f64::NAN),
        Value::Float(f64::INFINITY),
        Value::Float(1e300),
        Value::Decimal(Decimal::new(1500, 3)),
        Value::Decimal(Decimal::new(-19_99, 2)),
        Value::Decimal(Decimal::MAX),
    ];
    for subject in subjects {
        let ty = match subject {
            Value::Float(_) => Type::float(),
            _ => Type::decimal(),
        };
        let here = size(&subject, &world);
        for candidate in candidates(&subject, &ty, &world) {
            assert!(
                size(&candidate, &world) < here,
                "{} offered {} which is not smaller",
                subject.render(),
                candidate.render()
            );
        }
    }
}

/// Greedy descent has to actually arrive: a witness that never reaches `0.0` costs a reader the
/// shortest counterexample there is.
#[test]
fn a_float_shrinks_all_the_way_to_zero() {
    let world = world();
    let mut current = Value::Float(-1234.5);
    let mut steps = 0;
    while let Some(next) = candidates(&current, &Type::float(), &world)
        .into_iter()
        .next()
    {
        current = next;
        steps += 1;
        assert!(steps < 200, "the walk did not terminate");
    }
    assert!(matches!(current, Value::Float(f) if f == 0.0 && f.is_sign_positive()));
}

/// Toward `0m` *and* toward scale 0, so a witness reads `1.5m` rather than `1.500000m`.
#[test]
fn a_decimal_sheds_its_trailing_zeros_before_its_digits() {
    let world = world();
    let padded = Value::Decimal(Decimal::new(1_500_000, 6));
    let offered = candidates(&padded, &Type::decimal(), &world);
    assert!(
        offered
            .iter()
            .any(|c| matches!(c, Value::Decimal(d) if d.scale() == 1 && *d == Decimal::new(15, 1))),
        "normalizing the scale is a candidate: {offered:?}"
    );

    let mut current = padded;
    let mut steps = 0;
    while let Some(next) = candidates(&current, &Type::decimal(), &world)
        .into_iter()
        .next()
    {
        current = next;
        steps += 1;
        assert!(steps < 200, "the walk did not terminate");
    }
    assert_eq!(current, Value::Decimal(Decimal::ZERO));
}

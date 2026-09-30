//! Drawing and shrinking `Float` and `Decimal`.

use ply_eval::{Decimal, DefHash, Value};
use ply_prove::property::{EDGE_CASES, GenStream, generatable, generate};
use ply_prove::shrink::{candidates, minimal, size};
use ply_prove::{Sort, World};

fn world() -> World {
    World::default()
}

fn key() -> DefHash {
    DefHash([7u8; 32])
}

fn draw(ty: &Sort, cases: u32) -> Vec<Value> {
    let world = world();
    let mut stream = GenStream::new(1, key());
    (0..cases)
        .map(|case| generate(ty, &world, &mut stream, case).expect("generatable"))
        .collect()
}

fn floats(cases: u32) -> Vec<f64> {
    draw(&Sort::float(), cases)
        .into_iter()
        .map(|v| match v {
            Value::Float(f) => f,
            other => panic!("expected a Float, got {other}"),
        })
        .collect()
}

fn decimals(cases: u32) -> Vec<Decimal> {
    draw(&Sort::decimal(), cases)
        .into_iter()
        .map(|v| match v {
            Value::Decimal(d) => d,
            other => panic!("expected a Decimal, got {other}"),
        })
        .collect()
}

#[test]
fn both_numeric_types_are_generatable() {
    let world = world();
    assert!(generatable(&Sort::float(), &world).is_ok());
    assert!(generatable(&Sort::decimal(), &world).is_ok());
    assert!(generatable(&Sort::list(Sort::float()), &world).is_ok());
}

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

/// There are more specials than edge-case slots, so the rest arrive through the biased sampler.
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
    let zero = minimal(&Sort::float(), &world).unwrap();
    assert!(matches!(zero, Value::Float(f) if f == 0.0 && f.is_sign_positive()));
    assert_eq!(
        minimal(&Sort::decimal(), &world).unwrap(),
        Value::Decimal(Decimal::ZERO)
    );
}

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
            Value::Float(_) => Sort::float(),
            _ => Sort::decimal(),
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

/// The walk is `proof.shrink`'s, and it tries candidates in order: a float's first is the floor,
/// and the floor offers none.
#[test]
fn a_floats_first_candidate_is_its_floor() {
    let world = world();
    for f in [-1234.5, 0.1, 1e300, f64::NAN, f64::NEG_INFINITY] {
        let first = candidates(&Value::Float(f), &Sort::float(), &world)
            .into_iter()
            .next();
        assert!(
            matches!(first, Some(Value::Float(z)) if z == 0.0 && z.is_sign_positive()),
            "{f} offered {first:?} first"
        );
    }
    assert!(candidates(&Value::Float(0.0), &Sort::float(), &world).is_empty());
}

#[test]
fn a_decimal_offers_its_floor_and_then_its_scale_shed() {
    let world = world();
    let padded = Value::Decimal(Decimal::new(1_500_000, 6));
    let offered = candidates(&padded, &Sort::decimal(), &world);
    assert_eq!(offered.first(), Some(&Value::Decimal(Decimal::ZERO)));
    assert!(
        offered
            .iter()
            .any(|c| matches!(c, Value::Decimal(d) if d.scale() == 1 && *d == Decimal::new(15, 1))),
        "normalizing the scale is a candidate: {offered:?}"
    );
    assert!(candidates(&Value::Decimal(Decimal::ZERO), &Sort::decimal(), &world).is_empty());
}

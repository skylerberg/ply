// `Value`'s `Arc` payloads are deliberately not `Send`.
#![allow(clippy::arc_with_non_send_sync)]

use ply_core::{Decimal, Span, Value, values_equal};
use std::sync::Arc;

/// A credential, and its payload held apart, to count who else holds it.
fn credential(text: &str) -> (Value, Arc<ply_core::Sealed>) {
    let secret = Value::secret_text(text);
    let Value::Secret(payload) = &secret else {
        unreachable!("a secret is a `Secret`")
    };
    let payload = Arc::clone(payload);
    (secret, payload)
}

/// Every value shape this evaluator can hold, in pairs, for the scan below.
fn probe_values() -> Vec<(&'static str, Value)> {
    let dec = |m: i128, s: u32| {
        Value::Decimal(Decimal::try_from_i128_with_scale(m, s).expect("in range"))
    };
    vec![
        ("unit", Value::Unit),
        ("bool false", Value::Bool(false)),
        ("bool true", Value::Bool(true)),
        ("int 0", Value::Int(0)),
        ("int 1", Value::Int(1)),
        ("float 0.0", Value::Float(0.0)),
        ("float -0.0", Value::Float(-0.0)),
        ("float 1.5", Value::Float(1.5)),
        ("float nan", Value::Float(f64::NAN)),
        ("decimal 1.5", dec(15, 1)),
        ("decimal 1.50", dec(150, 2)),
        ("decimal 1.500", dec(1500, 3)),
        ("decimal 2.5", dec(25, 1)),
        ("str empty", Value::str("")),
        ("str a", Value::str("a")),
        ("bytes empty", Value::bytes([])),
        ("bytes a", Value::bytes(b"a")),
        ("list empty", Value::list(vec![])),
        ("list [1]", Value::list(vec![Value::Int(1)])),
        ("list [1.5m]", Value::list(vec![dec(15, 1)])),
        ("list [1.50m]", Value::list(vec![dec(150, 2)])),
        ("map empty", Value::map([])),
        ("map {1: 1}", Value::map([(Value::Int(1), Value::Int(1))])),
        (
            "record {a: 1.5m}",
            Value::Record(Arc::new(
                [(ply_core::Symbol::new("a"), dec(15, 1))]
                    .into_iter()
                    .collect(),
            )),
        ),
        (
            "record {a: 1.50m}",
            Value::Record(Arc::new(
                [(ply_core::Symbol::new("a"), dec(150, 2))]
                    .into_iter()
                    .collect(),
            )),
        ),
        ("ctor Red", Value::ctor("Red", vec![])),
        ("ctor Box(1.5m)", Value::ctor("Box", vec![dec(15, 1)])),
        ("ctor Box(1.50m)", Value::ctor("Box", vec![dec(150, 2)])),
    ]
}

#[test]
fn the_order_and_the_language_equality_part_at_a_nan_and_also_at_negative_zero() {
    let mut disagreements = Vec::new();
    for (an, a) in probe_values() {
        for (bn, b) in probe_values() {
            let ordered = a.cmp(&b) == std::cmp::Ordering::Equal;
            let Ok(equal) = values_equal(&a, &b, Span::DUMMY) else {
                continue;
            };
            if ordered != equal {
                disagreements.push(format!("{an} vs {bn}: cmp={ordered} ==={equal}"));
            }
        }
    }
    disagreements.sort();
    assert_eq!(
        disagreements,
        vec![
            "float -0.0 vs float 0.0: cmp=false ===true".to_string(),
            "float 0.0 vs float -0.0: cmp=false ===true".to_string(),
            "float nan vs float nan: cmp=true ===false".to_string(),
        ],
        "the `Map`'s order and the language's `==` disagree somewhere new; if the new pair is \
         an ordered key type, every `Map` built from those keys holds two entries where a \
         program wrote one"
    );
}

#[test]
fn two_decimals_that_are_one_map_key_build_one_entry_under_the_spelling_given_first() {
    let short = Value::Decimal(Decimal::try_from_i128_with_scale(15, 1).expect("1.5"));
    let long = Value::Decimal(Decimal::try_from_i128_with_scale(150, 2).expect("1.50"));

    assert!(
        values_equal(&short, &long, Span::DUMMY).expect("two decimals compare"),
        "the language's `==` stopped treating `1.5m` and `1.50m` as one value"
    );
    assert_eq!(short.cmp(&long), std::cmp::Ordering::Equal);
    assert_eq!(format!("{short:?}"), "Decimal(1.5)");
    assert_eq!(
        format!("{long:?}"),
        "Decimal(1.50)",
        "two values that are one `Map` key keep two different scales"
    );

    // One key, held as it was first given; a later spelling of it takes the value.
    let short_then_long = Value::map([
        (short.clone(), Value::Int(1)),
        (long.clone(), Value::Int(2)),
    ]);
    let long_then_short = Value::map([
        (long.clone(), Value::Int(1)),
        (short.clone(), Value::Int(2)),
    ]);
    assert!(
        values_equal(&short_then_long, &long_then_short, Span::DUMMY).expect("two maps compare"),
        "the two maps are not even equal, which is a larger defect than the one this pins"
    );
    assert_eq!(
        format!("{short_then_long:?}"),
        "Map([(Decimal(1.5), Int(2))])"
    );
    assert_eq!(
        format!("{long_then_short:?}"),
        "Map([(Decimal(1.50), Int(2))])"
    );
}

#[test]
fn a_value_is_still_thirty_two_bytes_wide_and_an_optional_one_costs_nothing() {
    assert_eq!(
        size_of::<Value>(),
        32,
        "`Value` changed width; the refusal to narrow it and the pool's arithmetic over \
         885.6 Value-wide slots per request were both taken at 32"
    );
    assert_eq!(
        size_of::<Option<Value>>(),
        size_of::<Value>(),
        "`Option<Value>` stopped being niche-optimized, so every arena slot and every scope \
         binding grew"
    );
}

#[test]
fn a_key_holding_a_credential_is_held_as_given_and_never_copied_out() {
    let (secret, payload) = credential("hunter2");
    let key = Value::Record(Arc::new(
        [
            (ply_core::Symbol::new("d"), {
                Value::Decimal(Decimal::try_from_i128_with_scale(150, 2).expect("1.50"))
            }),
            (ply_core::Symbol::new("p"), secret),
        ]
        .into_iter()
        .collect(),
    ));

    let m = Value::map([(key, Value::Int(1))]);
    let Value::Map(entries) = &m else {
        panic!("not a map")
    };
    let (stored, _) = entries.iter().next().expect("one entry");
    let Value::Record(fields) = stored else {
        panic!("the key is not a record")
    };

    assert_eq!(
        format!(
            "{:?}",
            fields
                .get(&ply_core::Symbol::new("d"))
                .expect("the decimal field")
        ),
        "Decimal(1.50)",
        "the map rewrote the key it was given"
    );
    match fields.get(&ply_core::Symbol::new("p")) {
        Some(Value::Secret(held)) => assert!(
            Arc::ptr_eq(held, &payload),
            "the map rebuilt a credential's payload instead of holding the `Arc` it was given"
        ),
        other => panic!("the credential stopped being a `Secret`: {other:?}"),
    }
    let copied = format!("{m:?}");
    assert!(
        copied.contains("Secret"),
        "a key lost its credential: {copied}"
    );
    assert!(
        !copied.contains("hunter2"),
        "a key copied a credential out: {copied}"
    );
}

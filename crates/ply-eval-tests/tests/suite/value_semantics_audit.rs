// `Value`'s `Arc` payloads are deliberately not `Send`.
#![allow(clippy::arc_with_non_send_sync)]

use crate::fixture::Compiled;
use ply_eval::{
    ARGUMENT_VECTOR_CLASSES, Decimal, Diagnostic, Plain, Span, Value, assertion_failure,
    first_difference, values_equal,
};
use std::sync::Arc;

impl Compiled {
    #[track_caller]
    fn must_pass(&self, name: &str) {
        if let Err(d) = self.machine().eval_test(self.index_of(name)).into_parts().0 {
            panic!("{name:?} was expected to pass:\n{d:#?}");
        }
    }

    #[track_caller]
    fn answer_of(&self, name: &str, args: Vec<Value>) -> Result<Value, Diagnostic> {
        self.machine().call(name, args, Span::DUMMY).into_parts().0
    }
}

const SECRET_ARGUMENTS: &str = r#"
fn keep1(s: Secret<String>) -> Bool = secret_is_empty(s)
fn keep2(a: Int, s: Secret<String>) -> Bool = secret_is_empty(s)
fn keep3(a: Int, b: Int, s: Secret<String>) -> Bool = secret_is_empty(s)
fn keep4(a: Int, b: Int, c: Int, s: Secret<String>) -> Bool = secret_is_empty(s)
fn keep5(a: Int, b: Int, c: Int, d: Int, s: Secret<String>) -> Bool = secret_is_empty(s)

pub fn carry1(s: Secret<String>) -> Bool = keep1(s)
pub fn carry2(s: Secret<String>) -> Bool = keep2(1, s)
pub fn carry3(s: Secret<String>) -> Bool = keep3(1, 2, s)
pub fn carry4(s: Secret<String>) -> Bool = keep4(1, 2, 3, s)
pub fn carry5(s: Secret<String>) -> Bool = keep5(1, 2, 3, 4, s)

// Deeper than `argv::KEEP`, so the list fills, overflows and drains again while
// a credential is in every frame's argument vector.
fn descend(depth: Int, s: Secret<String>) -> Int =
  if depth <= 0 { 0 } else { descend(depth - 1, s) + 1 }

pub fn deep(s: Secret<String>) -> Int = descend(2000, s)

// The observation a dirty buffer would break: a call of the same arity made
// right after one that carried a credential must see exactly its own arguments.
pub fn after4(a: Int, b: Int, c: Int, d: Int) -> Int = a * 1000 + b * 100 + c * 10 + d
"#;

/// A credential, and its payload held apart, to count who else holds it.
fn credential(text: &str) -> (Value, Arc<ply_eval::Sealed>) {
    let secret = Value::secret_text(text);
    let Value::Secret(payload) = &secret else {
        unreachable!("a secret is a `Secret`")
    };
    let payload = Arc::clone(payload);
    (secret, payload)
}

#[test]
fn a_credential_passed_as_an_argument_is_unreachable_once_the_call_returns() {
    let compiled = Compiled::new(SECRET_ARGUMENTS);
    for arity in 1..=ARGUMENT_VECTOR_CLASSES + 1 {
        let (secret, payload) = credential("hunter2");
        let answered = compiled
            .answer_of(&format!("m.carry{arity}"), vec![secret])
            .unwrap_or_else(|d| panic!("`carry{arity}` raised: {d:#?}"));
        assert_eq!(answered, Value::Bool(false), "arity {arity}");
        assert_eq!(
            Arc::strong_count(&payload),
            1,
            "after a call at arity {arity} returned, a credential was still reachable from \
             something the evaluator kept — a recycled argument buffer holds its contents"
        );
    }
}

#[test]
fn a_recursion_deeper_than_the_free_lists_bound_leaves_no_credential_behind() {
    let compiled = Compiled::new(SECRET_ARGUMENTS);
    let (secret, payload) = credential("hunter2");
    let answered = compiled
        .answer_of("m.deep", vec![secret])
        .unwrap_or_else(|d| panic!("`deep` raised: {d:#?}"));
    assert_eq!(answered, Value::Int(2000));
    assert_eq!(
        Arc::strong_count(&payload),
        1,
        "a 2000-frame recursion carrying a credential left one reachable after it unwound"
    );
}

#[test]
fn a_call_made_after_one_that_carried_a_credential_sees_only_its_own_arguments() {
    let compiled = Compiled::new(SECRET_ARGUMENTS);
    for _ in 0..64 {
        let secret = Value::secret_text("hunter2");
        let _ = compiled
            .answer_of("m.carry4", vec![secret])
            .expect("`carry4` runs");
        let answered = compiled
            .answer_of(
                "m.after4",
                vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)],
            )
            .expect("`after4` runs");
        assert_eq!(
            answered,
            Value::Int(1234),
            "the call after one that carried a credential was built from a buffer that was \
             not empty"
        );
    }
}

/// A failing assertion's diagnostic is what a report prints and a failure document carries.
#[test]
fn the_assertion_differ_never_descends_into_a_credential() {
    let hidden = "hunter2";
    let other = "correct-horse-battery-staple";
    let secret = Value::secret_text;
    let record = |s: Value| {
        Value::Record(Arc::new(
            [(ply_eval::Symbol::new("password"), s)]
                .into_iter()
                .collect(),
        ))
    };

    let pairs: Vec<(&str, Value, Value)> = vec![
        ("bare", secret(hidden), secret(other)),
        (
            "in a list",
            Value::list(vec![Value::Int(1), secret(hidden)]),
            Value::list(vec![Value::Int(1), secret(other)]),
        ),
        ("in a record", record(secret(hidden)), record(secret(other))),
        (
            "in a variant",
            Value::ctor("Login", vec![secret(hidden)]),
            Value::ctor("Login", vec![secret(other)]),
        ),
        (
            "as a map value",
            Value::map([(Value::str("ada"), secret(hidden))]),
            Value::map([(Value::str("ada"), secret(other))]),
        ),
        (
            "a list against a shorter one",
            Value::list(vec![secret(hidden)]),
            Value::list(vec![secret(hidden), secret(other)]),
        ),
    ];

    for (label, actual, expected) in pairs {
        let failed = assertion_failure(&actual, &expected, Span::DUMMY);
        for text in [
            format!("{failed:?}"),
            format!("{:?}", first_difference(&actual, &expected)),
            format!("{:?}", first_difference(&expected, &actual)),
        ] {
            assert!(
                !text.contains(hidden) && !text.contains(other),
                "{label}: a payload reached what a failure keeps: {text}"
            );
        }
        assert!(
            failed
                .values
                .iter()
                .any(|v| format!("{v:?}").contains("Secret")),
            "{label}: the credential is missing from the carried values, so something else stood in"
        );
    }
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
        ("map empty", Value::empty_map()),
        ("map {1: 1}", Value::map([(Value::Int(1), Value::Int(1))])),
        (
            "record {a: 1.5m}",
            Value::Record(Arc::new(
                [(ply_eval::Symbol::new("a"), dec(15, 1))]
                    .into_iter()
                    .collect(),
            )),
        ),
        (
            "record {a: 1.50m}",
            Value::Record(Arc::new(
                [(ply_eval::Symbol::new("a"), dec(150, 2))]
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
fn two_decimals_that_are_one_map_key_render_two_strings_and_build_one_map() {
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

    // One key, canonical, whichever spelling was written last.
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
        "Map([(Decimal(1.5), Int(2))])",
        "two `==`-equal maps render as two different strings, so `map_keys`, `map_entries`, \
         `map_fold` and every derived encoding over them are functions of insertion history"
    );
}

#[test]
fn map_insert_over_an_equal_decimal_key_reads_back_one_canonical_spelling() {
    let compiled = Compiled::new(
        r#"
pub fn last_wins(ignored: Int) -> String =
  string_of_keys(map_insert(map_insert(map_new(), 1.50m, 1), 1.5m, 2))

pub fn first_spelling(ignored: Int) -> String =
  string_of_keys(map_insert(map_insert(map_new(), 1.5m, 1), 1.50m, 2))

fn string_of_keys(m: Map<Decimal, Int>) -> String =
  fold(map(map_keys(m), decimal_to_string), "", string_concat)

test "the two maps are equal" {
  assert_eq(
    map_insert(map_insert(map_new(), 1.50m, 1), 1.5m, 2),
    map_insert(map_insert(map_new(), 1.5m, 1), 1.50m, 2))
}
"#,
    );
    compiled.must_pass("the two maps are equal");
    assert_eq!(
        compiled
            .answer_of("m.last_wins", vec![Value::Int(0)])
            .unwrap(),
        Value::str("1.5")
    );
    assert_eq!(
        compiled
            .answer_of("m.first_spelling", vec![Value::Int(0)])
            .unwrap(),
        Value::str("1.5"),
        "two maps that `assert_eq` as one value answer `map_keys` with two different lists"
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
fn a_record_key_holding_a_decimal_is_canonical_in_the_compound_key_too() {
    let compiled = Compiled::new(
        r#"
type Line = {sku: String, price: Decimal}

fn prices(m: Map<Line, Int>) -> String =
  fold(map(map_keys(m), |l: Line| decimal_to_string(l.price)), "", string_concat)

pub fn wrote_rounded(ignored: Int) -> String =
  prices(map_insert(
    map_insert(map_new(), {sku: "bolt", price: 1.50m}, 1),
    {sku: "bolt", price: 1.5m}, 2))

pub fn wrote_exact(ignored: Int) -> String =
  prices(map_insert(
    map_insert(map_new(), {sku: "bolt", price: 1.5m}, 1),
    {sku: "bolt", price: 1.50m}, 2))

test "one line, either way" {
  assert_eq(map_len(map_insert(
    map_insert(map_new(), {sku: "bolt", price: 1.50m}, 1),
    {sku: "bolt", price: 1.5m}, 2)), 1)
}
"#,
    );
    compiled.must_pass("one line, either way");
    assert_eq!(
        compiled
            .answer_of("m.wrote_rounded", vec![Value::Int(0)])
            .unwrap(),
        Value::str("1.5")
    );
    assert_eq!(
        compiled
            .answer_of("m.wrote_exact", vec![Value::Int(0)])
            .unwrap(),
        Value::str("1.5"),
        "a record key holding a `Decimal` reads back a price the program did not write last"
    );
}

#[test]
fn canonicalizing_a_key_clones_a_credential_rather_than_rebuilding_it() {
    let (secret, payload) = credential("hunter2");
    let key = Value::Record(Arc::new(
        [
            (ply_eval::Symbol::new("d"), {
                Value::Decimal(Decimal::try_from_i128_with_scale(150, 2).expect("1.50"))
            }),
            (ply_eval::Symbol::new("p"), secret),
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
                .get(&ply_eval::Symbol::new("d"))
                .expect("the decimal field")
        ),
        "Decimal(1.5)",
        "the key was not canonicalized, so this test is not exercising the rebuild"
    );
    match fields.get(&ply_eval::Symbol::new("p")) {
        Some(Value::Secret(held)) => assert!(
            Arc::ptr_eq(held, &payload),
            "canonicalization rebuilt a credential's payload instead of cloning the `Arc`"
        ),
        other => panic!("the credential stopped being a `Secret`: {other:?}"),
    }
    let copied = format!("{:?}", Plain::of(&m));
    assert!(
        copied.contains("Secret"),
        "a canonicalized key lost its credential: {copied}"
    );
    assert!(
        !copied.contains("hunter2"),
        "a canonicalized key copied a credential out: {copied}"
    );
}

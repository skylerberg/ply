//! An adversarial audit of what a `Value` *means* after the argument-vector pool and the constant-value memo.

// A `Value::Record` holds `Arc<BTreeMap<Symbol, Value>>` and a `Value` is not `Send`; the same
// allow `secrets.rs` carries, for the same reason.
#![allow(clippy::arc_with_non_send_sync)]

use crate::fixture::Compiled;
use ply_eval::{
    ARGUMENT_VECTOR_CLASSES, Decimal, SECRET_REDACTED, Value, first_difference, values_equal,
};
use ply_span::{Diagnostic, Span};
use std::sync::Arc;

impl Compiled {
    #[track_caller]
    fn must_pass(&self, name: &str) {
        if let Err(d) = self.machine().eval_test(self.index_of(name)) {
            panic!("{name:?} was expected to pass:\n{d:#?}");
        }
    }

    #[track_caller]
    fn answer_of(&self, name: &str, args: Vec<Value>) -> Result<Value, Diagnostic> {
        self.machine().call(name, args, Span::DUMMY)
    }
}

// --- 1. the argument vector under multi-shot resumption ---------------------

// --- 2. a credential in an argument vector ----------------------------------

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

/// The secret containment claim as a bound on the free list, measured through the machine.
#[test]
fn a_credential_passed_as_an_argument_is_unreachable_once_the_call_returns() {
    let compiled = Compiled::new(SECRET_ARGUMENTS);
    for arity in 1..=ARGUMENT_VECTOR_CLASSES + 1 {
        let payload = Arc::new(Value::str("hunter2"));
        let secret = Value::Secret(Arc::clone(&payload));
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

/// The same, past the free list's bound and back.
#[test]
fn a_recursion_deeper_than_the_free_lists_bound_leaves_no_credential_behind() {
    let compiled = Compiled::new(SECRET_ARGUMENTS);
    let payload = Arc::new(Value::str("hunter2"));
    let secret = Value::Secret(Arc::clone(&payload));
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

/// A buffer that carried a credential is handed to the next call of that arity.
#[test]
fn a_call_made_after_one_that_carried_a_credential_sees_only_its_own_arguments() {
    let compiled = Compiled::new(SECRET_ARGUMENTS);
    for _ in 0..64 {
        let secret = Value::secret(Value::str("hunter2"));
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

// --- 3. rendering paths other than `Value::write` ---------------------------

/// `first_difference` is a second structural walk over a value and it builds text that is
/// **stored** — `builtins::assert_failure` puts it in the note of a failing assertion and
/// `ply-store` caches that as `Outcome::Fail { message }`.
#[test]
fn the_assertion_differ_never_descends_into_a_credential() {
    let hidden = "hunter2";
    let other = "correct-horse-battery-staple";
    let secret = |s: &str| Value::secret(Value::str(s));
    let record = |s: Value| {
        Value::Record(Arc::new(
            [(ply_span::Symbol::new("password"), s)]
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
            "wrapping a compound",
            Value::secret(Value::list(vec![Value::str(hidden)])),
            Value::secret(Value::list(vec![Value::str(other)])),
        ),
        (
            "a list against a shorter one",
            Value::list(vec![secret(hidden)]),
            Value::list(vec![secret(hidden), secret(other)]),
        ),
    ];

    for (label, actual, expected) in pairs {
        for text in [
            actual.render(),
            expected.render(),
            format!("{:?}", first_difference(&actual, &expected)),
            format!("{:?}", first_difference(&expected, &actual)),
        ] {
            assert!(
                !text.contains(hidden) && !text.contains(other),
                "{label}: a payload reached a rendered string: {text}"
            );
        }
        assert!(
            actual.render().contains(SECRET_REDACTED),
            "{label}: the redaction marker is missing, so something else rendered instead"
        );
    }
}

// --- 5. equal values that do not render alike -------------------------------

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
                [(ply_span::Symbol::new("a"), dec(15, 1))]
                    .into_iter()
                    .collect(),
            )),
        ),
        (
            "record {a: 1.50m}",
            Value::Record(Arc::new(
                [(ply_span::Symbol::new("a"), dec(150, 2))]
                    .into_iter()
                    .collect(),
            )),
        ),
        ("ctor Red", Value::ctor("Red", vec![])),
        ("ctor Box(1.5m)", Value::ctor("Box", vec![dec(15, 1)])),
        ("ctor Box(1.50m)", Value::ctor("Box", vec![dec(150, 2)])),
    ]
}

/// The language's `==` and the `Map`'s order, checked against each other over every pair of the
/// probe corpus.
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

/// **The defect this test pinned is fixed; it now asserts the fix.**
#[test]
fn two_decimals_that_are_one_map_key_render_two_strings_and_build_one_map() {
    let short = Value::Decimal(Decimal::try_from_i128_with_scale(15, 1).expect("1.5"));
    let long = Value::Decimal(Decimal::try_from_i128_with_scale(150, 2).expect("1.50"));

    assert!(
        values_equal(&short, &long, Span::DUMMY).expect("two decimals compare"),
        "the language's `==` stopped treating `1.5m` and `1.50m` as one value"
    );
    assert_eq!(short.cmp(&long), std::cmp::Ordering::Equal);
    assert_eq!(short.render(), "1.5");
    assert_eq!(
        long.render(),
        "1.50",
        "two values that are one `Map` key render as two different strings"
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
    assert_eq!(short_then_long.render(), "{1.5: 2}");
    assert_eq!(
        long_then_short.render(),
        "{1.5: 2}",
        "two `==`-equal maps render as two different strings, so `map_keys`, `map_entries`, \
         `map_fold` and every derived encoding over them are functions of insertion history"
    );
}

/// The same claim where a program meets it: `map_insert` with a key equal to one already present
/// replaces the value, and the key a program reads back is the canonical spelling either way.
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

// --- 6. a shared constant inside a seeded simulation ------------------------

// --- 7. one memo, many programs ---------------------------------------------

// --- 8. the width the refusal to narrow `Value` rejected narrowing at -------------------------

/// The refusal to narrow `Value` names, among the things that would make it wrong, *"if a build
/// agent has to widen `Value` past 32 bytes to land any of this"* as one of five conditions that
/// would sink the document.
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

/// The blast radius the defect had, now the blast radius the fix has to cover: it was never only
/// `Map<Decimal, _>`.
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

/// A credential under a key the canonical form rebuilds is **not** descended into and **not**
/// rebuilt.
#[test]
fn canonicalizing_a_key_clones_a_credential_rather_than_rebuilding_it() {
    let payload = Arc::new(Value::str("hunter2"));
    let secret = Value::Secret(Arc::clone(&payload));
    let key = Value::Record(Arc::new(
        [
            (ply_span::Symbol::new("d"), {
                Value::Decimal(Decimal::try_from_i128_with_scale(150, 2).expect("1.50"))
            }),
            (ply_span::Symbol::new("p"), secret),
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
        fields
            .get(&ply_span::Symbol::new("d"))
            .expect("the decimal field")
            .render(),
        "1.5",
        "the key was not canonicalized, so this test is not exercising the rebuild"
    );
    match fields.get(&ply_span::Symbol::new("p")) {
        Some(Value::Secret(held)) => assert!(
            Arc::ptr_eq(held, &payload),
            "canonicalization rebuilt a credential's payload instead of cloning the `Arc`"
        ),
        other => panic!("the credential stopped being a `Secret`: {other:?}"),
    }
    assert!(
        m.render().contains(SECRET_REDACTED),
        "a canonicalized key holding a credential renders it: {}",
        m.render()
    );
    assert!(
        !m.render().contains("hunter2"),
        "a canonicalized key printed a credential: {}",
        m.render()
    );
}

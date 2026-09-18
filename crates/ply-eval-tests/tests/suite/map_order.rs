// `Value`'s `Arc` payloads are deliberately not `Send`.
#![allow(clippy::arc_with_non_send_sync)]

use ply_eval::{Map, Value, values_equal};
use ply_span::Span;
use rust_decimal::Decimal;
use std::cmp::Ordering;
use std::process::Command;
use std::str::FromStr;

/// A tiny xorshift, so a permutation is reproducible without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}

fn eq(a: &Value, b: &Value) -> bool {
    values_equal(a, b, Span::DUMMY).expect("these values are comparable")
}

fn keys(m: &Value) -> Vec<String> {
    let Value::Map(m) = m else {
        panic!("not a map")
    };
    m.keys().map(|k| k.render()).collect()
}

fn map_of(pairs: Vec<(Value, Value)>) -> Value {
    Value::map(pairs)
}

fn dec(s: &str) -> Value {
    Value::Decimal(Decimal::from_str(s).expect("a decimal literal"))
}

#[test]
fn iteration_is_ascending_under_every_insertion_order() {
    let base: Vec<Value> = (0..24)
        .map(|i| Value::str(format!("key-{:02}", (i * 7) % 24)))
        .collect();
    let ascending = {
        let mut sorted = base.clone();
        sorted.sort();
        sorted.dedup();
        sorted.iter().map(|k| k.render()).collect::<Vec<_>>()
    };

    let mut rng = Rng(0x5eed_1234_9abc_def1);
    for round in 0..10_000 {
        let mut pairs: Vec<(Value, Value)> = base
            .iter()
            .cloned()
            .map(|k| (k, Value::Int(round as i64)))
            .collect();
        rng.shuffle(&mut pairs);
        assert_eq!(
            keys(&map_of(pairs)),
            ascending,
            "insertion order {round} changed the iteration order"
        );
    }
}

#[test]
fn two_insertion_orders_build_one_value() {
    let forward = map_of(vec![
        (Value::Int(1), Value::str("a")),
        (Value::Int(2), Value::str("b")),
        (Value::Int(3), Value::str("c")),
    ]);
    let backward = map_of(vec![
        (Value::Int(3), Value::str("c")),
        (Value::Int(2), Value::str("b")),
        (Value::Int(1), Value::str("a")),
    ]);
    assert!(eq(&forward, &backward));
    assert_eq!(forward.render(), backward.render());
    assert_eq!(forward.cmp(&backward), Ordering::Equal);
}

#[test]
fn the_iteration_order_is_pinned() {
    let mut rng = Rng(0xfeed_face_dead_beef);
    let mut pairs: Vec<(Value, Value)> = Vec::new();
    for i in 0..64i64 {
        pairs.push((Value::Int((i * 37) % 64), Value::str(format!("v{i}"))));
        pairs.push((Value::str(format!("s{:02}", (i * 11) % 64)), Value::Int(i)));
        pairs.push((Value::bytes([(i % 251) as u8, 7]), Value::Bool(i % 2 == 0)));
    }
    rng.shuffle(&mut pairs);
    let rendered = map_of(pairs).render();
    assert_eq!(
        blake3::hash(rendered.as_bytes()).to_hex().as_str(),
        "d95a132e0e9c2537b40decf812619093cb2c4f98fcad839380bf556fa43dcab7",
        "the map iteration order moved:\n{rendered}"
    );
}

/// Derived, because a filter naming no test runs nothing and exits 0, which reads as a pass.
fn own_test_name(leaf: &str) -> String {
    match module_path!().split_once("::") {
        Some((_binary, module)) => format!("{module}::{leaf}"),
        None => leaf.to_string(),
    }
}

#[test]
fn a_second_process_iterates_in_the_same_order() {
    let exe = std::env::current_exe().expect("the test binary");
    let name = own_test_name("the_iteration_order_is_pinned");
    let out = Command::new(exe)
        .args([name.as_str(), "--exact", "--nocapture"])
        .output()
        .expect("the child test process runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the pinned order did not reproduce in a second process:\n{stdout}"
    );
    assert!(
        stdout.contains("1 passed"),
        "the child ran no test matching `{name}`, and a filter that matches nothing exits 0 — so \
         the check above passed having run nothing:\n{stdout}"
    );
}

#[test]
fn the_order_and_the_language_agree_except_on_float() {
    let values = corpus();
    for a in &values {
        for b in &values {
            let ordered = a.cmp(b) == Ordering::Equal;
            let equal = values_equal(a, b, Span::DUMMY);
            let Ok(equal) = equal else {
                // Only a function refuses, and it refuses for both sides.
                assert!(
                    matches!(a, Value::Closure(_)) || matches!(b, Value::Closure(_)),
                    "{} vs {} refused comparison",
                    a.render(),
                    b.render()
                );
                continue;
            };
            if ordered == equal {
                continue;
            }
            assert!(
                float_peculiarity(a, b),
                "`cmp` and `values_equal` disagree on {} vs {}: {ordered} and {equal}",
                a.render(),
                b.render()
            );
        }
    }
}

#[test]
fn signed_zeros_are_two_keys_and_are_one_value() {
    let pos = Value::Float(0.0);
    let neg = Value::Float(-0.0);
    assert_ne!(pos.cmp(&neg), Ordering::Equal);
    assert!(eq(&pos, &neg));
    // A lookup for `-0.0` would miss what `0.0` inserted.
    let m = map_of(vec![(pos, Value::Int(1)), (neg, Value::Int(2))]);
    let Value::Map(inner) = &m else { panic!() };
    assert_eq!(inner.size(), 2);
}

#[test]
fn two_nans_are_one_key_and_are_not_equal() {
    let nan = Value::Float(f64::NAN);
    assert_eq!(nan.cmp(&Value::Float(f64::NAN)), Ordering::Equal);
    assert!(!eq(&nan, &Value::Float(f64::NAN)));
    // Why `Float` is not an ordered key type: this map holds a key the language's `==` cannot find.
    let m = map_of(vec![(nan, Value::Int(1))]);
    let Value::Map(m) = &m else { panic!() };
    assert_eq!(m.size(), 1);
}

/// The two places IEEE `==` and a total order cannot both be right.
fn float_peculiarity(a: &Value, b: &Value) -> bool {
    let (Value::Float(x), Value::Float(y)) = (a, b) else {
        return false;
    };
    (x.is_nan() && y.is_nan()) || (*x == 0.0 && *y == 0.0)
}

fn corpus() -> Vec<Value> {
    let mut out = vec![
        Value::Unit,
        Value::Bool(false),
        Value::Bool(true),
        Value::Int(-1),
        Value::Int(0),
        Value::Int(i64::MAX),
        Value::Float(0.0),
        Value::Float(-0.0),
        Value::Float(1.5),
        Value::Float(f64::INFINITY),
        Value::Float(f64::NEG_INFINITY),
        Value::Float(f64::NAN),
        dec("0"),
        dec("1.5"),
        dec("1.50"),
        dec("-2"),
        Value::str(""),
        Value::str("a"),
        Value::str("ab"),
        Value::bytes([]),
        Value::bytes([0]),
        Value::bytes([0, 255]),
        Value::list(vec![]),
        Value::list(vec![Value::Int(1)]),
        Value::list(vec![Value::Int(1), Value::Int(2)]),
        Value::ctor("None", vec![]),
        Value::ctor("Some", vec![Value::Int(1)]),
        Value::ctor("Some", vec![Value::Int(2)]),
        Value::empty_map(),
        map_of(vec![(Value::Int(1), Value::Int(1))]),
        map_of(vec![(Value::Int(1), Value::Int(2))]),
        map_of(vec![
            (Value::Int(1), Value::Int(1)),
            (Value::Int(2), Value::Int(2)),
        ]),
    ];
    out.push(Value::builtin(ply_eval::Builtin::Len));
    out.push(Value::builtin(ply_eval::Builtin::Push));
    out.push(Value::Record(std::sync::Arc::new(
        [
            (ply_span::Symbol::new("a"), Value::Int(1)),
            (ply_span::Symbol::new("b"), Value::Int(2)),
        ]
        .into_iter()
        .collect(),
    )));
    out
}

#[test]
fn the_order_is_total() {
    let values = corpus();
    for a in &values {
        assert_eq!(
            a.cmp(a),
            Ordering::Equal,
            "{} is not equal to itself",
            a.render()
        );
        for b in &values {
            assert_eq!(
                a.cmp(b),
                b.cmp(a).reverse(),
                "{} and {} are not antisymmetric",
                a.render(),
                b.render()
            );
            for c in &values {
                if a.cmp(b) != Ordering::Greater && b.cmp(c) != Ordering::Greater {
                    assert_ne!(
                        a.cmp(c),
                        Ordering::Greater,
                        "transitivity fails at {} {} {}",
                        a.render(),
                        b.render(),
                        c.render()
                    );
                }
            }
        }
    }
}

#[test]
fn an_equal_key_replaces_the_value_and_the_key_is_canonical_either_way() {
    let m = map_of(vec![
        (dec("1.50"), Value::str("first")),
        (dec("1.5"), Value::str("second")),
    ]);
    assert_eq!(keys(&m), vec!["1.5"]);
    assert_eq!(m.render(), "{1.5: \"second\"}");

    let other = map_of(vec![
        (dec("1.5"), Value::str("first")),
        (dec("1.50"), Value::str("second")),
    ]);
    assert_eq!(
        keys(&other),
        vec!["1.5"],
        "the surviving key is still a function of which spelling was written last"
    );
    assert_eq!(other.render(), "{1.5: \"second\"}");
    assert!(
        eq(&m, &other),
        "two maps that hold one key and one value are not equal"
    );
    assert_eq!(
        m.render(),
        other.render(),
        "two `==`-equal maps render as two different strings, so `map_keys`, `map_entries`, \
         `map_fold` and every derived encoding over them are functions of insertion history"
    );
}

#[test]
fn a_decimal_anywhere_under_a_key_is_canonical() {
    let field = |d: &str| {
        Value::Record(std::sync::Arc::new(ply_eval::Fields::from_iter([(
            ply_span::Symbol::new("price"),
            dec(d),
        )])))
    };
    let cases: Vec<(Value, Value, &str)> = vec![
        (
            Value::list(vec![dec("1.50")]),
            Value::list(vec![dec("1.5")]),
            "[1.5]",
        ),
        (field("1.50"), field("1.5"), "{price: 1.5}"),
        (
            Value::ctor("Box", vec![dec("2.00")]),
            Value::ctor("Box", vec![dec("2")]),
            "Box(2)",
        ),
        (
            map_of(vec![(dec("1.50"), dec("3.10"))]),
            map_of(vec![(dec("1.5"), dec("3.1"))]),
            "{1.5: 3.1}",
        ),
    ];
    for (written, canonical, rendered) in cases {
        let a = map_of(vec![(written.clone(), Value::Int(1))]);
        let b = map_of(vec![(canonical.clone(), Value::Int(1))]);
        assert_eq!(keys(&a), vec![rendered.to_string()]);
        assert_eq!(keys(&b), vec![rendered.to_string()]);
        assert!(
            eq(&a, &b),
            "{} and {} are not one map",
            written.render(),
            canonical.render()
        );
    }
}

#[test]
fn a_large_map_iterates_ascending() {
    let mut rng = Rng(0x1234_5678_9abc_def0);
    let mut pairs: Vec<(Value, Value)> = (0..10_000i64)
        .map(|i| (Value::Int(i), Value::Int(i * 2)))
        .collect();
    rng.shuffle(&mut pairs);
    let m = map_of(pairs);
    let Value::Map(inner) = &m else { panic!() };
    assert_eq!(inner.size(), 10_000);
    let mut expected = 0i64;
    for (k, v) in inner.iter() {
        assert_eq!(*k, Value::Int(expected));
        assert_eq!(*v, Value::Int(expected * 2));
        expected += 1;
    }
    assert_eq!(expected, 10_000);
}

#[test]
fn a_map_is_itself_an_ordered_key() {
    let inner_a = map_of(vec![(Value::Int(1), Value::Int(1))]);
    let inner_b = map_of(vec![(Value::Int(1), Value::Int(2))]);
    let outer = map_of(vec![
        (inner_b.clone(), Value::str("b")),
        (inner_a.clone(), Value::str("a")),
    ]);
    assert_eq!(keys(&outer), vec!["{1: 1}", "{1: 2}"]);
}

#[test]
fn a_deep_chain_of_maps_drops_without_aborting() {
    let mut v = Value::empty_map();
    for i in 0..20_000i64 {
        let mut m = Map::new();
        m.insert_mut(Value::Int(i), v);
        v = Value::Map(m);
    }
    drop(v);
}

#[test]
fn a_failing_comparison_locates_the_entry_that_differs() {
    let a = map_of(vec![
        (Value::str("x"), Value::Int(1)),
        (Value::str("y"), Value::Int(2)),
    ]);
    let b = map_of(vec![
        (Value::str("y"), Value::Int(9)),
        (Value::str("x"), Value::Int(1)),
    ]);
    let (path, expected, actual) =
        ply_eval::first_difference(&a, &b).expect("the two differ at one entry");
    assert_eq!(path, "[\"y\"]");
    assert_eq!(expected, "9");
    assert_eq!(actual, "2");

    // Different key sets have no entry to blame, so the pair is reported whole.
    let c = map_of(vec![(Value::str("x"), Value::Int(1))]);
    assert!(ply_eval::first_difference(&a, &c).is_none());
}

//! The property everything else in `Map` rests on: **iteration order is a
//! function of the values, and of nothing else**.
//!
//! Content addressing, the result cache, seeded replay and `--engine both` all
//! assume a value has one canonical form. A hash-ordered map would break every
//! one of them at once, and every one of those failures is a green result over
//! space nobody explored or a red one over correct code — so this file tests the
//! order directly, across permutations, across runs, and across processes.

// `Value` pins `Arc` for its shared payloads and `Rc` for shared code, so none
// of those `Arc`s can ever be `Send` — the intended design, not an oversight.
#![allow(clippy::arc_with_non_send_sync)]

use ply_eval::{Map, Value, values_equal};
use ply_span::Span;
use rust_decimal::Decimal;
use std::cmp::Ordering;
use std::process::Command;
use std::str::FromStr;

/// A tiny xorshift, so a permutation is reproducible without a dependency and
/// without a seed nobody can write down.
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

/// Required test: `map_keys` is ascending regardless of insertion order, over
/// 10,000 random insertion permutations of one key set.
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

/// The same claim in the other direction: two maps built by different insertion
/// orders are one value, not two that happen to agree entry by entry.
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

/// A pin, and therefore a check across runs *and* across processes: the digest
/// below was computed by this order and by nothing else. A hash-ordered map
/// would move it on the next run, on another machine, or under another build.
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

/// The process half of "across runs and processes", asserted rather than
/// assumed: a second process builds the same map from a shuffled order and must
/// print the same keys. Re-runs this binary rather than compiling a second one.
#[test]
fn a_second_process_iterates_in_the_same_order() {
    let exe = std::env::current_exe().expect("the test binary");
    let out = Command::new(exe)
        .args(["the_iteration_order_is_pinned", "--exact", "--nocapture"])
        .output()
        .expect("the child test process runs");
    assert!(
        out.status.success(),
        "the pinned order did not reproduce in a second process:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Required test: `Value::cmp(a, b) == Equal` iff `values_equal(a, b)`, over the
/// range the generator draws from — with the exceptions asserted explicitly
/// rather than excluded, because an exception nobody wrote down is how the two
/// drift apart.
///
/// There are **two**, and both are `Float`, and both are the same fact from
/// opposite ends: `total_cmp` is a total order and IEEE `==` is not an
/// equivalence relation, so they disagree exactly where IEEE is peculiar —
/// `NaN`, which `==` says differs from itself, and `0.0`/`-0.0`, which `==` says
/// are one value and which have different bit patterns. ADR 0012 §2 names only
/// the first; the second is here because it is real, and because both are the
/// same argument for `Float` not being an ordered key type.
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

/// The exceptions themselves, stated as claims rather than as a filter above.
#[test]
fn signed_zeros_are_two_keys_and_are_one_value() {
    let pos = Value::Float(0.0);
    let neg = Value::Float(-0.0);
    assert_ne!(pos.cmp(&neg), Ordering::Equal);
    assert!(eq(&pos, &neg));
    // Two keys where the language sees one value: a lookup for `-0.0` would
    // miss what `0.0` inserted. This is the second half of why `Float` is
    // refused as a key type, and `E0206` is what a program meets instead.
    let m = map_of(vec![(pos, Value::Int(1)), (neg, Value::Int(2))]);
    let Value::Map(inner) = &m else { panic!() };
    assert_eq!(inner.size(), 2);
}

#[test]
fn two_nans_are_one_key_and_are_not_equal() {
    let nan = Value::Float(f64::NAN);
    assert_eq!(nan.cmp(&Value::Float(f64::NAN)), Ordering::Equal);
    assert!(!eq(&nan, &Value::Float(f64::NAN)));
    // Which is exactly why `Float` is not an ordered key type: this map would
    // hold a key the language's `==` cannot find.
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

/// The order is total, which is what a search tree requires of it: irreflexive
/// nowhere, antisymmetric, and transitive over the whole corpus.
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

/// `1.50m` and `1.5m` are one key, the **value** is the last inserted, and the
/// key is the canonical member of the class whichever spelling arrived last.
///
/// **The second half of this test asserted the opposite until 2026-08-21**, and
/// had since W2: under the name `an_equal_key_is_replaced_key_and_value_both`
/// it required `keys(&other)` to be `["1.50"]` — the spelling written last —
/// because `map_insert` replaced the key as well as the value. That made
/// `map_keys` a function of insertion history, which is the failure this
/// file's module note says the whole design exists to prevent.
/// `ply_eval::value::canonical_key` is the fix, and the two `render` and `eq`
/// assertions below are new: two maps that `assert_eq` as one value now render
/// one string and serve one set of encoded bytes.
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

/// A `Decimal` under a compound key is canonical too, at every position
/// [`Value::cmp`] descends into — a list, a record field, a constructor
/// argument and a nested map's key *and* value.
///
/// `Map<{sku: String, price: Decimal}, _>` typechecks, so the compound case is
/// reachable from a well-typed program and is not a Rust-level curiosity.
#[test]
fn a_decimal_anywhere_under_a_key_is_canonical() {
    let field = |d: &str| {
        Value::Record(std::sync::Arc::new(std::collections::BTreeMap::from([(
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

/// A map big enough that the tree is deep, iterated ascending and counted.
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

/// Nested maps sort by their contents, so a map is a key type like any other.
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

/// Dropping a deeply nested chain of maps must not abort the process. The chain
/// is built by iteration, which is the only way to reach a depth the call bound
/// does not.
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

/// An `assert_eq` over two maps has to point at the entry that differs, not at
/// two whole maps. The path is the rendered key, because a map has no field
/// names and no indices a reader could match up.
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

    // Different key *sets* have no entry to blame, so the pair is reported whole
    // rather than walked entry by entry — which would misalign and name the
    // wrong key. `None` is how this function says "at the top", as it already
    // does for two lists of different lengths.
    let c = map_of(vec![(Value::str("x"), Value::Int(1))]);
    assert!(ply_eval::first_difference(&a, &c).is_none());
}

//! One `Value` per constructor per thread, and what that may not change.

use crate::fixture::Compiled;
use ply_eval::{Arena, Machine, RegionKind, Value, values_equal};
use ply_span::Span;
use std::sync::Arc;

/// `twice` mentions one nullary constructor twice and `boxes` mentions one constructor of arity 1
/// twice, each in a fresh position, so a list of two holds what two separate mentions evaluated to.
const SOURCE: &str = r#"
type Colour = Red | Green
type Boxed = Box(Int)

pub fn twice(ignored: Int) -> List<Colour> = [Red, Red]

pub fn boxes(ignored: Int) -> List<(Int) -> Boxed> = [Box, Box]

pub fn rank(c: Colour) -> Int = match c { Red -> 1, Green -> 2 }

pub fn ranked(ignored: Int) -> Int = rank(Red) + rank(Green)

pub fn built(ignored: Int) -> List<Boxed> = [Box(1), Box(2)]
"#;

fn answered(c: &Compiled, name: &str) -> Value {
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    match machine.call(name, vec![Value::Int(0)], Span::DUMMY) {
        Ok(value) => value,
        Err(d) => panic!("`{name}` raised: {d:#?}"),
    }
}

#[track_caller]
fn pair(value: &Value) -> (&Value, &Value) {
    match value {
        Value::List(items) if items.len() == 2 => (&items[0], &items[1]),
        other => panic!("expected a list of two mentions, found {other}"),
    }
}

#[test]
fn two_mentions_of_a_nullary_constructor_are_one_value() {
    let c = Compiled::new(SOURCE);
    let answered = answered(&c, "m.twice");
    {
        let (first, second) = pair(&answered);
        match (first, second) {
            (Value::Ctor { args: x, .. }, Value::Ctor { args: y, .. }) => assert!(
                Arc::ptr_eq(x, y),
                "two mentions of `Red` answered with two values, so `ctor_value` built the \
                 second rather than sharing the first"
            ),
            _ => panic!("a mention of `Red` did not evaluate to a `Ctor`"),
        }
    }
}

#[test]
fn two_mentions_of_a_constructor_of_arity_one_are_one_closure() {
    let c = Compiled::new(SOURCE);
    let answered = answered(&c, "m.boxes");
    {
        let (first, second) = pair(&answered);
        match (first, second) {
            (Value::Closure(x), Value::Closure(y)) => assert!(
                Arc::ptr_eq(x, y),
                "two mentions of `Box` answered with two closures"
            ),
            _ => panic!("a mention of `Box` did not evaluate to a closure"),
        }
    }
}

/// The shared value is the value a fresh one was: it matches the arm a fresh one matched, and it is
/// equal to one built here.
#[test]
fn a_shared_constructor_still_matches_and_still_compares_equal() {
    let c = Compiled::new(SOURCE);
    assert_eq!(
        answered(&c, "m.ranked"),
        Value::Int(3),
        "a shared constructor selected a different arm than a fresh one"
    );
    {
        let twice = answered(&c, "m.twice");
        let (first, second) = pair(&twice);
        let fresh = Value::ctor("m.Red", Vec::new());
        assert!(
            values_equal(first, second, Span::DUMMY).expect("two `Ctor`s compare")
                && values_equal(first, &fresh, Span::DUMMY).expect("two `Ctor`s compare"),
            "the shared `Red` is not equal to a `Red` built beside it"
        );
    }
}

/// A cached value outlives every region in the program, so the one thing it may not do is point at
/// something a region reclaims.
#[test]
fn a_region_closing_under_a_shared_constructor_reclaims_nothing_it_holds() {
    let c = Compiled::new(SOURCE);
    let shared = answered(&c, "m.twice");
    let (held, _) = pair(&shared);
    let held = held.clone();

    let mut arena = Arena::new();
    let region = arena.open(RegionKind::Unique, Span::DUMMY);
    let slot = arena.alloc(held.clone()).expect("inside a region");
    assert!(arena.get(slot).is_some());
    arena.close(region);
    assert!(arena.get(slot).is_none(), "the region did not close");

    let fresh = Value::ctor("m.Red", Vec::new());
    assert!(
        values_equal(&held, &fresh, Span::DUMMY).expect("two `Ctor`s compare"),
        "a region's close reached inside a value the cache is still handing out"
    );
    let again = answered(&c, "m.twice");
    let (after, _) = pair(&again);
    assert!(
        values_equal(after, &fresh, Span::DUMMY).expect("two `Ctor`s compare"),
        "after a region closed over it, the cache answered with something else"
    );
}

/// Sharing the *function* a constructor of arity >= 1 evaluates to may not share what it builds.
#[test]
fn applying_one_shared_constructor_twice_builds_two_values() {
    let c = Compiled::new(SOURCE);
    let answered = answered(&c, "m.built");
    {
        let (first, second) = pair(&answered);
        match (first, second) {
            (Value::Ctor { args: x, .. }, Value::Ctor { args: y, .. }) => {
                assert!(
                    !Arc::ptr_eq(x, y),
                    "`Box(1)` and `Box(2)` share one argument vector"
                );
                assert_eq!(x.as_ref(), &vec![Value::Int(1)]);
                assert_eq!(y.as_ref(), &vec![Value::Int(2)]);
            }
            _ => panic!("an applied `Box` did not evaluate to a `Ctor`"),
        }
        assert!(
            !values_equal(first, second, Span::DUMMY).expect("two `Ctor`s compare"),
            "`Box(1)` and `Box(2)` are one value"
        );
    }
}

/// A shared constructor orders against a fresh one exactly as two fresh ones do, and a `Map` is
/// where that is load-bearing.
#[test]
fn a_shared_constructor_is_the_same_map_key_as_a_fresh_one() {
    let c = Compiled::new(SOURCE);
    let fresh = Value::ctor("m.Red", Vec::new());
    let answered = answered(&c, "m.twice");
    {
        let (shared, _) = pair(&answered);

        let keyed_by_shared = Value::map([(shared.clone(), Value::Int(1))]);
        let keyed_by_fresh = Value::map([(fresh.clone(), Value::Int(1))]);
        for (label, map, probe) in [
            ("a fresh key against a shared one", &keyed_by_shared, &fresh),
            ("a shared key against a fresh one", &keyed_by_fresh, shared),
        ] {
            let Value::Map(m) = map else {
                unreachable!("built by `Value::map`")
            };
            assert!(
                m.get(probe).is_some(),
                "{label}: the two order differently, so one program's map has two entries \
                 where it wrote one"
            );
        }

        // A map built from both spellings has one entry, which is the same claim from the other
        // side: `map_insert` is a fold and a second key equal to the first replaces it rather than
        // joining it.
        let both_spellings = Value::map([
            (shared.clone(), Value::Int(1)),
            (fresh.clone(), Value::Int(2)),
        ]);
        let Value::Map(m) = &both_spellings else {
            unreachable!("built by `Value::map`")
        };
        assert_eq!(m.size(), 1, "a shared `Red` and a fresh one are two keys");
        assert_eq!(shared.render(), fresh.render());
    }
}

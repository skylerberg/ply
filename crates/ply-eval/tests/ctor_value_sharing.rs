//! One `Value` per constructor per thread, and what that may not change.
//!
//! A mention of a constructor is a compile-time constant: `Red` evaluates to a
//! `Value::Ctor` that is a function of the name and the arity alone, and `Box`
//! to an `Arc<Closure>` that is a function of the same two. Until ADR 0019 §2
//! both were rebuilt on every mention — 21.0 and 24.0 allocations per `/health`
//! request, re-taken by `cargo test -p ply-corpus --release --test
//! r4_value_construction -- --nocapture` — and `ply_eval::interp::ctor_value`
//! now answers from a thread-local cache instead.
//!
//! `crates/ply-corpus/tests/r4_value_construction.rs` owns the allocation
//! count. What this file owns is that the shared value is the same value:
//!
//! - **shared, not equal.** Two mentions answer with one `Arc`, on both
//!   engines — an equal value would mean it was rebuilt and nothing was saved.
//! - **meaning did not move.** The shared value matches the arm a fresh one
//!   matched, is equal to a fresh one, and both engines answer the same thing.
//!   **`--engine both` is not the mechanical form of that claim here** — this
//!   note said it was — because `interp::ctor_value` is what both engines call
//!   (`interp.rs:712`, `machine.rs:2093`), so the differential harness compares
//!   one memo against itself and `Arc::ptr_eq` across the two engines is *true*
//!   (`value_semantics_audit.rs::both_engines_answer_a_constructor_mention_from_one_memo_and_a_literal_from_two`).
//!   This file is the evidence, not the corroboration; `on_both`'s note below
//!   has always said why.
//! - **it holds nothing.** A cached value has the program's lifetime, so a
//!   region that closes underneath it must not be able to reclaim anything it
//!   points at, and it must not be reachable from a `Secret`. A nullary
//!   constructor's `args` are empty, which is what makes both true, and
//!   [`a_region_closing_under_a_shared_constructor_reclaims_nothing_it_holds`]
//!   is the statement of the first.

use ply_core::{CheckOutput, check_program};
use ply_eval::{Arena, Interp, Machine, RegionKind, Value, values_equal};
use ply_span::{SourceId, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::sync::Arc;

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

fn compile(source: &str) -> Compiled {
    let inputs = [(SourceId(0), ModuleName::from_dotted("m"), source)];
    let program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
    let resolved = resolve(&program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
    let check = check_program(&program, &resolved)
        .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
    Compiled {
        program,
        resolved,
        check,
    }
}

/// `twice` mentions one nullary constructor twice and `boxes` mentions one
/// constructor of arity 1 twice, each in a fresh position, so a list of two
/// holds what two separate mentions evaluated to.
const SOURCE: &str = r#"
type Colour = Red | Green
type Boxed = Box(Int)

pub fn twice(ignored: Int) -> List<Colour> = [Red, Red]

pub fn boxes(ignored: Int) -> List<(Int) -> Boxed> = [Box, Box]

pub fn rank(c: Colour) -> Int = match c { Red -> 1, Green -> 2 }

pub fn ranked(ignored: Int) -> Int = rank(Red) + rank(Green)

pub fn built(ignored: Int) -> List<Boxed> = [Box(1), Box(2)]
"#;

/// Both engines run every case here: `ctor_value` is shared by the two, so a
/// cache in it is a change to both, and `--engine both` reporting no divergence
/// is what the change is allowed to cost.
fn on_both(c: &Compiled, name: &str) -> [Value; 2] {
    let call = |v: Result<Value, ply_span::Diagnostic>| match v {
        Ok(value) => value,
        Err(d) => panic!("`{name}` raised: {d:#?}"),
    };
    let mut interp = Interp::new(&c.program, &c.resolved, &c.check);
    let walked = call(interp.call(name, vec![Value::Int(0)], Span::DUMMY));
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    let stepped = call(machine.call(name, vec![Value::Int(0)], Span::DUMMY));
    [walked, stepped]
}

#[track_caller]
fn pair(value: &Value) -> (&Value, &Value) {
    match value {
        Value::List(items) if items.len() == 2 => (&items[0], &items[1]),
        other => panic!("expected a list of two mentions, found {other}"),
    }
}

#[test]
fn two_mentions_of_a_nullary_constructor_are_one_value_on_both_engines() {
    let c = compile(SOURCE);
    for (engine, answered) in ["treewalk", "machine"].iter().zip(on_both(&c, "m.twice")) {
        let (first, second) = pair(&answered);
        match (first, second) {
            (Value::Ctor { args: x, .. }, Value::Ctor { args: y, .. }) => assert!(
                Arc::ptr_eq(x, y),
                "on {engine} two mentions of `Red` answered with two values, so `ctor_value` \
                 built the second rather than sharing the first"
            ),
            _ => panic!("on {engine} a mention of `Red` did not evaluate to a `Ctor`"),
        }
    }
}

#[test]
fn two_mentions_of_a_constructor_of_arity_one_are_one_closure_on_both_engines() {
    let c = compile(SOURCE);
    for (engine, answered) in ["treewalk", "machine"].iter().zip(on_both(&c, "m.boxes")) {
        let (first, second) = pair(&answered);
        match (first, second) {
            (Value::Closure(x), Value::Closure(y)) => assert!(
                Arc::ptr_eq(x, y),
                "on {engine} two mentions of `Box` answered with two closures"
            ),
            _ => panic!("on {engine} a mention of `Box` did not evaluate to a closure"),
        }
    }
}

/// The shared value is the value a fresh one was: it matches the arm a fresh
/// one matched, and it is equal to one built here.
#[test]
fn a_shared_constructor_still_matches_and_still_compares_equal() {
    let c = compile(SOURCE);
    for answered in on_both(&c, "m.ranked") {
        assert_eq!(
            answered,
            Value::Int(3),
            "a shared constructor selected a different arm than a fresh one"
        );
    }
    for answered in on_both(&c, "m.twice") {
        let (first, second) = pair(&answered);
        let fresh = Value::ctor("m.Red", Vec::new());
        assert!(
            values_equal(first, second, Span::DUMMY).expect("two `Ctor`s compare")
                && values_equal(first, &fresh, Span::DUMMY).expect("two `Ctor`s compare"),
            "the shared `Red` is not equal to a `Red` built beside it"
        );
    }
}

/// A cached value outlives every region in the program, so the one thing it may
/// not do is point at something a region reclaims. It does not, and the reason
/// is that a nullary constructor's arguments are empty rather than that the
/// arena is careful: the region here allocates the shared value, closes, and the
/// value the cache still holds is unchanged.
#[test]
fn a_region_closing_under_a_shared_constructor_reclaims_nothing_it_holds() {
    let c = compile(SOURCE);
    let [_, shared] = on_both(&c, "m.twice");
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
    let [_, again] = on_both(&c, "m.twice");
    let (after, _) = pair(&again);
    assert!(
        values_equal(after, &fresh, Span::DUMMY).expect("two `Ctor`s compare"),
        "after a region closed over it, the cache answered with something else"
    );
}

/// Sharing the *function* a constructor of arity >= 1 evaluates to may not
/// share what it builds.
///
/// The cache holds the `Arc<Closure>` a mention of `Box` answers with. If it
/// ever held what applying that closure produced, every `Box` in a program
/// would be one value — and the tests above would all still pass, because they
/// only ever mention `Box` and never apply it.
#[test]
fn applying_one_shared_constructor_twice_builds_two_values() {
    let c = compile(SOURCE);
    for (engine, answered) in ["treewalk", "machine"].iter().zip(on_both(&c, "m.built")) {
        let (first, second) = pair(&answered);
        match (first, second) {
            (Value::Ctor { args: x, .. }, Value::Ctor { args: y, .. }) => {
                assert!(
                    !Arc::ptr_eq(x, y),
                    "on {engine} `Box(1)` and `Box(2)` share one argument vector"
                );
                assert_eq!(x.as_ref(), &vec![Value::Int(1)], "on {engine}");
                assert_eq!(y.as_ref(), &vec![Value::Int(2)], "on {engine}");
            }
            _ => panic!("on {engine} an applied `Box` did not evaluate to a `Ctor`"),
        }
        assert!(
            !values_equal(first, second, Span::DUMMY).expect("two `Ctor`s compare"),
            "on {engine} `Box(1)` and `Box(2)` are one value"
        );
    }
}

/// A shared constructor orders against a fresh one exactly as two fresh ones
/// do, and a `Map` is where that is load-bearing.
///
/// `Map`'s iteration order is [`Value`]'s `cmp` and four guarantees rest on a
/// value having one canonical form (`map_order.rs`). A shared key that ordered
/// even one step differently from a fresh one would not raise anything: it
/// would put a second entry where the program wrote one, and `map_get` with the
/// other spelling would answer nothing. So the assertion is a lookup by the
/// *other* value, both ways round, rather than a comparison.
#[test]
fn a_shared_constructor_is_the_same_map_key_as_a_fresh_one() {
    let c = compile(SOURCE);
    let fresh = Value::ctor("m.Red", Vec::new());
    for (engine, answered) in ["treewalk", "machine"].iter().zip(on_both(&c, "m.twice")) {
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
                "on {engine}, {label}: the two order differently, so one program's map has two                  entries where it wrote one"
            );
        }

        // A map built from both spellings has one entry, which is the same
        // claim from the other side: `map_insert` is a fold and a second key
        // equal to the first replaces it rather than joining it.
        let both_spellings = Value::map([
            (shared.clone(), Value::Int(1)),
            (fresh.clone(), Value::Int(2)),
        ]);
        let Value::Map(m) = &both_spellings else {
            unreachable!("built by `Value::map`")
        };
        assert_eq!(
            m.size(),
            1,
            "on {engine} a shared `Red` and a fresh one are two keys"
        );
        assert_eq!(shared.render(), fresh.render(), "on {engine}");
    }
}

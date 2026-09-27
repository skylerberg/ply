// The walk this file's fixtures were built for is `crates/ply-prove/ply/shrink.ply`'s now: the
// helpers that only its tests used are kept here, and the end-to-end claim — a counterexample that
// visibly shrank — is a `ply prove` test.
#![allow(dead_code)]

use crate::property::{Fixture, key};
use ply_eval::Value;
use ply_prove::property::{Judge, TypeWorld, run_property};
use ply_prove::shrink::{candidates, minimal, size};
use ply_prove::{Counterexample, DEFAULT_SHRINK_BUDGET, Discharge, ProvePlan};
use ply_span::{Diagnostic, Span, Symbol};
use ply_ty::{LawBinder, Type};

const ADTS: &str = r#"
type Opt = Nothing | Just(Int)
type Tree = Leaf | Node(Tree, Int, Tree)
"#;

const BOXES: &str = "type Box<a> = B(a)";

fn con(name: &str) -> Type {
    Type::Con(Symbol::new(name), Vec::new())
}

/// Saturating: a single `i64::MIN` already saturates [`size`].
fn total_size(values: &[Value], world: &TypeWorld) -> u64 {
    values
        .iter()
        .fold(0u64, |acc, v| acc.saturating_add(size(v, world)))
}

fn plan() -> ProvePlan {
    ProvePlan {
        cases: 200,
        roots: vec![0],
        prove_budget: 10,
        shrink_budget: DEFAULT_SHRINK_BUDGET,
        step_budget: ply_eval::DEFAULT_STEP_BUDGET,
        sim: Default::default(),
    }
}

/// A judge that checks every tuple the walk accepts against the guard and the property.
struct Watchful<G, B> {
    guard: G,
    body: B,
    accepted: Vec<Vec<Value>>,
}

impl<G, B> Judge for Watchful<G, B>
where
    G: Fn(&[Value]) -> bool,
    B: Fn(&[Value]) -> bool,
{
    fn guard(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        Ok((self.guard)(values))
    }
    fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        let held = (self.body)(values);
        if !held && (self.guard)(values) {
            self.accepted.push(values.to_vec());
        }
        Ok(held)
    }
}

fn refute<G, B>(
    binders: &[LawBinder],
    world: &TypeWorld,
    guard: G,
    body: B,
) -> (Counterexample, Vec<Vec<Value>>)
where
    G: Fn(&[Value]) -> bool + Copy,
    B: Fn(&[Value]) -> bool + Copy,
{
    let mut judge = Watchful {
        guard,
        body,
        accepted: Vec::new(),
    };
    let discharge = run_property(key(5), binders, world, &plan(), Span::DUMMY, &mut judge);
    let Discharge::Refuted(counterexample) = discharge else {
        panic!("expected a refutation, got {discharge:?}");
    };
    (counterexample, judge.accepted)
}

#[test]
fn a_negative_outweighs_its_own_magnitude() {
    let world = TypeWorld::default();
    assert_eq!(size(&Value::Int(0), &world), 0);
    assert!(size(&Value::Int(-5), &world) > size(&Value::Int(5), &world));
    assert!(size(&Value::Int(5), &world) > size(&Value::Int(2), &world));
    // Saturating, so the boundary does not wrap the measure that terminates the walk.
    assert_eq!(size(&Value::Int(i64::MIN), &world), u64::MAX);
}

#[test]
fn an_empty_collection_is_smaller_than_a_populated_one() {
    let world = TypeWorld::default();
    assert!(
        size(&Value::list(vec![Value::Int(0)]), &world) > size(&Value::list(Vec::new()), &world)
    );
    assert!(size(&Value::str("a"), &world) > size(&Value::str(""), &world));
    assert!(size(&Value::str("b"), &world) > size(&Value::str("a"), &world));
}

#[test]
fn a_lower_constructor_is_smaller_than_a_higher_one() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let none = Value::ctor("None", Vec::new());
    let some = Value::ctor("Some", vec![Value::Int(0)]);
    assert!(size(&some, &world) > size(&none, &world));
}

#[test]
fn the_floor_of_every_type_is_its_smallest_value() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    assert_eq!(minimal(&Type::int(), &world).unwrap().render(), "0");
    assert_eq!(minimal(&Type::bool(), &world).unwrap().render(), "false");
    assert_eq!(minimal(&Type::string(), &world).unwrap().render(), "\"\"");
    assert_eq!(minimal(&Type::bytes(), &world).unwrap().render(), "b\"\"");
    assert_eq!(minimal(&Type::unit(), &world).unwrap().render(), "()");
    assert_eq!(
        minimal(&Type::list(Type::int()), &world).unwrap().render(),
        "[]"
    );
    assert_eq!(minimal(&con("Opt"), &world).unwrap().render(), "Nothing");
    assert_eq!(minimal(&con("Tree"), &world).unwrap().render(), "Leaf");
}

#[test]
fn a_recursive_types_floor_terminates() {
    let fixture = Fixture::compile("type Tree = Node(Tree, Int, Tree) | Leaf");
    let world = fixture.world();
    assert_eq!(minimal(&con("Tree"), &world).unwrap().render(), "Leaf");
}

type Property = fn(&[Value]) -> bool;

/// Holds at every proper sublist of its counterexample, so no monotonicity may be assumed.
fn left_is_a_node(value: &Value) -> bool {
    let Value::Ctor { name, args } = value else {
        return false;
    };
    name.as_str().ends_with("Node")
        && matches!(args.first(), Some(Value::Ctor { name, .. }) if name.as_str().ends_with("Node"))
}

#[test]
fn a_value_the_type_does_not_describe_offers_nothing() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let tree = Value::ctor(
        "Node",
        vec![
            Value::ctor("Leaf", vec![]),
            Value::Int(1),
            Value::ctor("Leaf", vec![]),
        ],
    );
    assert!(candidates(&tree, &Type::int(), &world).is_empty());
    assert!(candidates(&tree, &Type::bool(), &world).is_empty());
    assert!(candidates(&Value::Unit, &con("Tree"), &world).is_empty());
    assert!(
        candidates(&Value::ctor("Nowhere", vec![]), &con("Tree"), &world).is_empty(),
        "a constructor the type does not declare is not a member of it"
    );
}

#[test]
fn a_candidate_order_is_fixed() {
    let world = TypeWorld::default();
    let rendered = |v: &Value, t: &Type| {
        candidates(v, t, &world)
            .iter()
            .map(|c| c.render())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rendered(&Value::Int(9), &Type::int()),
        vec!["0", "4", "2", "1", "8"]
    );
    assert_eq!(
        rendered(&Value::Int(-4), &Type::int()),
        vec!["0", "-2", "-1", "-3", "4"]
    );
    assert_eq!(
        rendered(
            &Value::list(vec![Value::Int(1), Value::Int(2)]),
            &Type::list(Type::int())
        ),
        vec![
            "[]", "[1]", "[2]", "[2]", "[1]", "[0, 2]", "[1, 0]", "[1, 1]"
        ]
    );
    assert_eq!(
        rendered(&Value::str("bc"), &Type::string())[..3],
        ["\"\"".to_string(), "\"b\"".to_string(), "\"c\"".to_string()]
    );
    // Length first, then content: `b""`, the two halves, then each byte lowered toward zero.
    assert_eq!(
        rendered(&Value::bytes([2, 4]), &Type::bytes()),
        [
            "b\"\"",
            "b\"\\x02\"",
            "b\"\\x04\"",
            "b\"\\x00\\x04\"",
            "b\"\\x01\\x04\"",
            "b\"\\x02\\x00\"",
            "b\"\\x02\\x02\""
        ]
    );
}

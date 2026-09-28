// `crates/ply-prove/ply/shrink.ply` is the walk. What is left here are the two functions it is
// built on -- `size`, and the candidates it offers -- and neither is about the walk: the order is
// fixed, and a type's floor is its smallest value.

use crate::property::Fixture;
use ply_eval::Value;
use ply_prove::property::TypeWorld;
use ply_prove::shrink::{candidates, minimal, size};
use ply_span::Symbol;
use ply_ty::Type;

const ADTS: &str = r#"
type Opt = Nothing | Just(Int)
type Tree = Leaf | Node(Tree, Int, Tree)
"#;

fn con(name: &str) -> Type {
    Type::Con(Symbol::new(name), Vec::new())
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

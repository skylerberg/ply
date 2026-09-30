// `crates/ply-prove/ply/shrink.ply` is the walk. What is left here are the two functions it is
// built on -- `size`, and the candidates it offers -- and neither is about the walk: the order is
// fixed, and a type's floor is its smallest value.

use ply_eval::Value;
use ply_prove::Sort;
use ply_prove::shrink::{candidates, minimal, size};
use ply_prove::world::{Decl, World};

fn con(name: &str) -> Sort {
    Sort::con(name)
}

/// `type Opt = Nothing | Just(Int)` and `type Tree = Leaf | Node(Tree, Int, Tree)`.
fn adts() -> World {
    World::new(
        [
            Decl::new(
                "Opt",
                0,
                vec![("Nothing", vec![]), ("Just", vec![Sort::int()])],
            ),
            Decl::new(
                "Tree",
                0,
                vec![
                    ("Leaf", vec![]),
                    ("Node", vec![con("Tree"), Sort::int(), con("Tree")]),
                ],
            ),
        ],
        [],
    )
}

#[test]
fn a_negative_outweighs_its_own_magnitude() {
    let world = World::default();
    assert_eq!(size(&Value::Int(0), &world), 0);
    assert!(size(&Value::Int(-5), &world) > size(&Value::Int(5), &world));
    assert!(size(&Value::Int(5), &world) > size(&Value::Int(2), &world));
    // Saturating, so the boundary does not wrap the measure that terminates the walk.
    assert_eq!(size(&Value::Int(i64::MIN), &world), u64::MAX);
}

#[test]
fn an_empty_collection_is_smaller_than_a_populated_one() {
    let world = World::default();
    assert!(
        size(&Value::list(vec![Value::Int(0)]), &world) > size(&Value::list(Vec::new()), &world)
    );
    assert!(size(&Value::str("a"), &world) > size(&Value::str(""), &world));
    assert!(size(&Value::str("b"), &world) > size(&Value::str("a"), &world));
}

#[test]
fn a_lower_constructor_is_smaller_than_a_higher_one() {
    let world = adts();
    let nothing = Value::ctor("Nothing", Vec::new());
    let just = Value::ctor("Just", vec![Value::Int(0)]);
    assert!(size(&just, &world) > size(&nothing, &world));
}

#[test]
fn the_floor_of_every_type_is_its_smallest_value() {
    let world = adts();
    assert_eq!(minimal(&Sort::int(), &world).unwrap(), Value::Int(0));
    assert_eq!(minimal(&Sort::bool(), &world).unwrap(), Value::Bool(false));
    assert_eq!(minimal(&Sort::string(), &world).unwrap(), Value::str(""));
    assert_eq!(minimal(&Sort::bytes(), &world).unwrap(), Value::bytes(b""));
    assert_eq!(minimal(&Sort::unit(), &world).unwrap(), Value::Unit);
    assert_eq!(
        minimal(&Sort::list(Sort::int()), &world).unwrap(),
        Value::list(Vec::new())
    );
    assert_eq!(
        minimal(&con("Opt"), &world).unwrap(),
        Value::ctor("Nothing", Vec::new())
    );
    assert_eq!(
        minimal(&con("Tree"), &world).unwrap(),
        Value::ctor("Leaf", Vec::new())
    );
}

#[test]
fn a_recursive_types_floor_terminates() {
    // The recursive case first, so the floor is not simply the first case.
    let world = World::new(
        [Decl::new(
            "Tree",
            0,
            vec![
                ("Node", vec![con("Tree"), Sort::int(), con("Tree")]),
                ("Leaf", vec![]),
            ],
        )],
        [],
    );
    assert_eq!(
        minimal(&con("Tree"), &world).unwrap(),
        Value::ctor("Leaf", Vec::new())
    );
}

#[test]
fn a_value_the_type_does_not_describe_offers_nothing() {
    let world = adts();
    let tree = Value::ctor(
        "Node",
        vec![
            Value::ctor("Leaf", vec![]),
            Value::Int(1),
            Value::ctor("Leaf", vec![]),
        ],
    );
    assert!(candidates(&tree, &Sort::int(), &world).is_empty());
    assert!(candidates(&tree, &Sort::bool(), &world).is_empty());
    assert!(candidates(&Value::Unit, &con("Tree"), &world).is_empty());
    assert!(
        candidates(&Value::ctor("Nowhere", vec![]), &con("Tree"), &world).is_empty(),
        "a constructor the type does not declare is not a member of it"
    );
}

#[test]
fn a_candidate_order_is_fixed() {
    let world = World::default();
    let offered = |v: &Value, t: &Sort| candidates(v, t, &world);
    let ints = |xs: &[i64]| xs.iter().map(|&i| Value::Int(i)).collect::<Vec<_>>();
    let list = |xs: &[i64]| Value::list(ints(xs));
    assert_eq!(
        offered(&Value::Int(9), &Sort::int()),
        ints(&[0, 4, 2, 1, 8])
    );
    assert_eq!(
        offered(&Value::Int(-4), &Sort::int()),
        ints(&[0, -2, -1, -3, 4])
    );
    assert_eq!(
        offered(&list(&[1, 2]), &Sort::list(Sort::int())),
        vec![
            list(&[]),
            list(&[1]),
            list(&[2]),
            list(&[2]),
            list(&[1]),
            list(&[0, 2]),
            list(&[1, 0]),
            list(&[1, 1])
        ]
    );
    assert_eq!(
        offered(&Value::str("bc"), &Sort::string())[..3],
        [Value::str(""), Value::str("b"), Value::str("c")]
    );
    // Length first, then content: `b""`, the two halves, then each byte lowered toward zero.
    assert_eq!(
        offered(&Value::bytes([2, 4]), &Sort::bytes()),
        [
            &b""[..],
            b"\x02",
            b"\x04",
            b"\x00\x04",
            b"\x01\x04",
            b"\x02\x00",
            b"\x02\x02"
        ]
        .map(Value::bytes)
    );
}

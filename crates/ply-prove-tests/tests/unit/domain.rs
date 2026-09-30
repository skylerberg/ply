use ply_prove::Sort;
use ply_prove::domain::cardinality;
use ply_prove::world::{Decl, World};
use ply_span::Symbol;

fn con(name: &str) -> Sort {
    Sort::con(name)
}

fn kinds() -> World {
    World::new(
        [Decl::new(
            "Kind",
            0,
            vec![("Asset", vec![]), ("Liability", vec![]), ("Equity", vec![])],
        )],
        [],
    )
}

#[test]
fn a_nullary_enum_and_bool_are_finite_and_everything_unbounded_is_not() {
    let world = kinds();
    assert_eq!(cardinality(&con("Bool"), &world), Some(2));
    assert_eq!(cardinality(&con("Unit"), &world), Some(1));
    assert_eq!(cardinality(&con("Kind"), &world), Some(3));
    assert_eq!(cardinality(&con("Int"), &world), None);
    assert_eq!(cardinality(&con("String"), &world), None);
    assert_eq!(
        cardinality(&Sort::Con(Symbol::new("List"), vec![con("Bool")]), &world),
        None
    );
}

#[test]
fn a_type_variable_is_never_finite() {
    assert_eq!(cardinality(&Sort::Var(0), &kinds()), None);
}

#[test]
fn a_recursive_type_is_infinite_even_with_a_nullary_base_case() {
    let world = World::new(
        [Decl::new(
            "Nat",
            0,
            vec![("Zero", vec![]), ("Succ", vec![con("Nat")])],
        )],
        [],
    );
    assert_eq!(cardinality(&con("Nat"), &world), None);
}

use ply_core::{CtorInfo, LawBinder, Scheme, Type};
use ply_eval::Value;
use ply_prove::domain::{cardinality, finite};
use ply_prove::property::TypeWorld;
use ply_span::{Span, Symbol};

fn con(name: &str) -> Type {
    Type::Con(Symbol::new(name), Vec::new())
}

fn ctor(ty: &str, name: &str, index: usize, fields: Vec<Type>) -> CtorInfo {
    CtorInfo {
        name: Symbol::new(name),
        module: ply_syntax::ast::ModuleName::anonymous(),
        simple_name: Symbol::new(name),
        type_name: Symbol::new(ty),
        index,
        arity: fields.len(),
        fields,
        scheme: Scheme::mono(Type::Con(Symbol::new(ty), Vec::new())),
        span: Span::DUMMY,
    }
}

fn binder(name: &str, ty: Type) -> LawBinder {
    LawBinder {
        name: Symbol::new(name),
        ty,
        span: Span::DUMMY,
    }
}

fn kinds() -> TypeWorld {
    let ctors = vec![
        ctor("Kind", "Asset", 0, Vec::new()),
        ctor("Kind", "Liability", 1, Vec::new()),
        ctor("Kind", "Equity", 2, Vec::new()),
    ];
    TypeWorld::new(&ctors)
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
        cardinality(&Type::Con(Symbol::new("List"), vec![con("Bool")]), &world),
        None
    );
}

/// An uninterpreted sort has no cardinality.
#[test]
fn a_type_variable_is_never_finite() {
    assert_eq!(cardinality(&Type::Var(ply_core::TyVar(0)), &kinds()), None);
}

/// A type in a constructor cycle has values of every nesting depth, so it has no finite domain
/// to cover — which is exactly where induction would be needed and is not available.
#[test]
fn a_recursive_type_is_infinite_even_with_a_nullary_base_case() {
    let ctors = vec![
        ctor("Nat", "Zero", 0, Vec::new()),
        ctor("Nat", "Succ", 1, vec![con("Nat")]),
    ];
    assert_eq!(cardinality(&con("Nat"), &TypeWorld::new(&ctors)), None);
}

#[test]
fn a_product_beyond_the_bound_is_refused_rather_than_walked() {
    let world = kinds();
    let wide: Vec<LawBinder> = (0..16)
        .map(|i| binder(&format!("b{i}"), con("Kind")))
        .collect();
    assert!(finite(&wide, &world).is_none(), "3^16 is past the bound");

    let narrow: Vec<LawBinder> = (0..4)
        .map(|i| binder(&format!("b{i}"), con("Kind")))
        .collect();
    assert_eq!(finite(&narrow, &world).map(|d| d.points), Some(81));
}

/// A ground claim is the degenerate finite domain: one point, the empty tuple, and no way to
/// miss any of it.
#[test]
fn a_ground_claim_has_exactly_one_point() {
    let domain = finite(&[], &kinds()).expect("no binders is a finite domain");
    assert_eq!(domain.points, 1);
    assert_eq!(domain.name().as_str(), "unit");
    assert_eq!(domain.point(&kinds(), 0).map(|p| p.len()), Some(0));
}

/// Every point exactly once, in an order two runs agree on — a refutation found here reports
/// its point as the counterexample with no shrinking.
#[test]
fn enumeration_covers_the_domain_once_each_in_a_fixed_order() {
    let world = kinds();
    let binders = [binder("b", con("Bool")), binder("k", con("Kind"))];
    let domain = finite(&binders, &world).expect("both types are finite");
    assert_eq!(domain.points, 6);

    let points: Vec<Vec<Value>> = (0..domain.points)
        .map(|i| domain.point(&world, i).expect("within the domain"))
        .collect();
    let show = |p: &Vec<Value>| p.iter().map(Value::render).collect::<Vec<_>>().join(", ");
    assert_eq!(show(&points[0]), "false, Asset");
    assert_eq!(show(&points[3]), "true, Asset");
    assert_eq!(show(&points[5]), "true, Equity");

    let mut rendered: Vec<String> = points.iter().map(show).collect();
    rendered.sort();
    rendered.dedup();
    assert_eq!(rendered.len(), 6, "a point was visited twice");
}

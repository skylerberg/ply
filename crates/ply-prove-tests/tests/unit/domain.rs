use ply_eval::Value;
use ply_prove::domain::{cardinality, finite};
use ply_prove::property::TypeWorld;
use ply_span::{Span, Symbol};
use ply_ty::{CtorInfo, LawBinder, Scheme, Type};

fn con(name: &str) -> Type {
    Type::Con(Symbol::new(name), Vec::new())
}

fn ctor(ty: &str, name: &str, index: usize, fields: Vec<Type>) -> CtorInfo {
    CtorInfo {
        name: Symbol::new(name),
        module: ply_ty::ModuleName::anonymous(),
        simple_name: Symbol::new(name),
        type_name: Symbol::new(ty),
        index,
        arity: fields.len(),
        fields,
        scheme: Scheme::mono(Type::Con(Symbol::new(ty), Vec::new())),
        span: Span::DUMMY,
    }
}

/// A case of a type that takes one parameter, so a field can be written in terms of it.
fn generic_ctor(ty: &str, name: &str, fields: Vec<Type>) -> CtorInfo {
    let mut info = ctor(ty, name, 0, fields);
    info.scheme.ty_vars = vec![ply_ty::TyVar(0)];
    info
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

#[test]
fn a_type_variable_is_never_finite() {
    assert_eq!(cardinality(&Type::Var(ply_ty::TyVar(0)), &kinds()), None);
}

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

#[test]
fn a_ground_claim_has_exactly_one_point() {
    let domain = finite(&[], &kinds()).expect("no binders is a finite domain");
    assert_eq!(domain.points, 1);
    assert_eq!(domain.name().as_str(), "unit");
    assert_eq!(domain.point(&kinds(), 0).map(|p| p.len()), Some(0));
}

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

    // The name an artifact carries, which the Ply module computes from the same texts.
    assert_eq!(domain.name().as_str(), "Bool × Kind");

    let mut rendered: Vec<String> = points.iter().map(show).collect();
    rendered.sort();
    rendered.dedup();
    assert_eq!(rendered.len(), 6, "a point was visited twice");
}

/// The same shapes `crates/ply-prove/ply/domain.ply` asserts in Ply: a case with fields adds the
/// product of them, a narrow width is a count like any other, and a binder no value inhabits is a
/// vacuity rather than a proof. Two implementations of one rule agree by these numbers or they
/// drift.
#[test]
fn the_numbers_the_packages_domain_module_pins_in_ply_are_these() {
    let world = TypeWorld::new(&[
        ctor("Kind", "Asset", 0, Vec::new()),
        ctor("Kind", "Liability", 1, Vec::new()),
        ctor("Kind", "Equity", 2, Vec::new()),
        ctor("Wrap", "Nothing", 0, Vec::new()),
        ctor("Wrap", "Held", 1, vec![con("Kind"), con("Bool")]),
    ]);
    assert_eq!(cardinality(&con("Wrap"), &world), Some(7));
    assert_eq!(cardinality(&con("U8"), &world), Some(256));
    assert_eq!(cardinality(&con("I32"), &world), Some(1 << 32));
    assert_eq!(cardinality(&con("U64"), &world), None);

    // A parameter is the argument the type was applied to, and a type already being walked is
    // refused whether it nests or recurses.
    let pair = |inner: Type| Type::Con(Symbol::new("Pair"), vec![inner]);
    let generic = TypeWorld::new(&[
        generic_ctor(
            "Pair",
            "Both",
            vec![Type::Var(ply_ty::TyVar(0)), Type::Var(ply_ty::TyVar(0))],
        ),
        ctor("Kind", "Asset", 0, Vec::new()),
        ctor("Kind", "Liability", 1, Vec::new()),
        ctor("Kind", "Equity", 2, Vec::new()),
    ]);
    assert_eq!(cardinality(&pair(con("Bool")), &generic), Some(4));
    assert_eq!(cardinality(&pair(con("Kind")), &generic), Some(9));
    assert_eq!(cardinality(&pair(con("Int")), &generic), None);
    assert_eq!(cardinality(&pair(pair(con("Bool"))), &generic), None);

    // A declared type with no variants has no values, so it is not a domain.
    let empty = TypeWorld::new(&[]);
    assert_eq!(cardinality(&con("Empty"), &empty), None);
    assert!(finite(&[binder("e", con("Empty"))], &empty).is_none());
}

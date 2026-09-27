use ply_eval::Value;
use ply_prove::domain::cardinality;
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

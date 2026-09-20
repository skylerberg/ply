use ply_span::Symbol;
use ply_ty::print::*;
use ply_ty::{EffectAtom, Mode, Resource, Row, RowVar, Scheme, TyVar, Type};

#[test]
fn variables_are_renamed_per_item_starting_at_a() {
    let t = Type::Fn {
        params: vec![Type::Var(TyVar(7))],
        ret: Box::new(Type::Var(TyVar(7))),
        effects: Row::empty(),
    };
    assert_eq!(print_type(&t), "(a) -> a");
}

#[test]
fn a_function_parameter_that_is_itself_a_function_keeps_its_own_argument_list() {
    let inner = Type::Fn {
        params: vec![Type::int()],
        ret: Box::new(Type::int()),
        effects: Row::empty(),
    };
    let outer = Type::Fn {
        params: vec![inner],
        ret: Box::new(Type::int()),
        effects: Row::empty(),
    };
    assert_eq!(print_type(&outer), "((Int) -> Int) -> Int");
}

#[test]
fn a_bare_row_variable_prints_without_braces() {
    let t = Type::Fn {
        params: vec![],
        ret: Box::new(Type::unit()),
        effects: Row::open(RowVar(3)),
    };
    assert_eq!(print_type(&t), "() -> Unit / e");
}

#[test]
fn atoms_and_a_tail_print_together() {
    let atom = EffectAtom::new("db", Resource::Named(Symbol::new("users")), Mode::Read);
    let row = Row {
        atoms: [atom].into(),
        tail: Some(RowVar(0)),
    };
    assert_eq!(print_row(&row), "{db.read[users] | e}");
}

#[test]
fn an_operation_atom_prints_its_operation_in_place_of_the_mode() {
    let row = Row::closed([
        EffectAtom::operation("net", Resource::Named(Symbol::new("conn")), "send"),
        EffectAtom::new("net", Resource::Named(Symbol::new("conn")), Mode::Write),
    ]);
    assert_eq!(print_row(&row), "{net.write[conn], net.send[conn]}");
}

#[test]
fn a_cell_hides_its_phantom_region_but_names_the_resource() {
    let cell = Type::Con(
        Symbol::new("Cell"),
        vec![Type::con(&region_type_name("users")), Type::int()],
    );
    assert_eq!(print_type(&cell), "Cell[users]<Int>");
    let unknown = Type::Con(Symbol::new("Cell"), vec![Type::Var(TyVar(0)), Type::int()]);
    assert_eq!(print_type(&unknown), "Cell<Int>");
}

#[test]
fn a_scheme_prints_its_quantifiers() {
    let s = Scheme {
        ty_vars: vec![TyVar(0), TyVar(1)],
        row_vars: vec![RowVar(0)],
        ty: Type::Fn {
            params: vec![Type::Var(TyVar(0))],
            ret: Box::new(Type::Var(TyVar(1))),
            effects: Row::open(RowVar(0)),
        },
    };
    assert_eq!(print_scheme(&s), "<a, b | e>(a) -> b / e");
}

use ply_span::Symbol;
use ply_ty::print::*;
use ply_ty::{EffectAtom, Footprint, LabelVar, Mode, Resource, Row, RowVar, Scheme, TyVar, Type};

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
        EffectAtom::operation(
            "net",
            Resource::Named(Symbol::new("conn")),
            Mode::Write,
            "send",
        ),
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
        label_vars: vec![],
        ty: Type::Fn {
            params: vec![Type::Var(TyVar(0))],
            ret: Box::new(Type::Var(TyVar(1))),
            effects: Row::open(RowVar(0)),
        },
    };
    assert_eq!(print_scheme(&s), "<a, b | e>(a) -> b / e");
}

/// The head lists the quantifiers in the scheme's order, whichever the body names first.
#[test]
fn a_scheme_head_keeps_its_order_and_its_unused_quantifiers() {
    let s = Scheme {
        ty_vars: vec![TyVar(7), TyVar(2)],
        row_vars: vec![RowVar(9)],
        label_vars: vec![],
        ty: Type::Fn {
            params: vec![Type::Var(TyVar(2))],
            ret: Box::new(Type::Var(TyVar(7))),
            effects: Row::open(RowVar(9)),
        },
    };
    assert_eq!(print_scheme(&s), "<b, a | e>(a) -> b / e");
    let phantom = Scheme {
        ty_vars: vec![TyVar(0), TyVar(1)],
        row_vars: vec![],
        label_vars: vec![],
        ty: Type::Var(TyVar(1)),
    };
    assert_eq!(print_scheme(&phantom), "<b, a>a");
}

#[test]
fn a_head_binds_its_label_parameters_among_the_type_parameters() {
    let send = EffectAtom::operation("net", Resource::Var(LabelVar(0)), Mode::Write, "send");
    let s = Scheme {
        ty_vars: vec![TyVar(0)],
        row_vars: vec![RowVar(0)],
        label_vars: vec![LabelVar(0)],
        ty: Type::Fn {
            params: vec![Type::Var(TyVar(0))],
            ret: Box::new(Type::unit()),
            effects: Row {
                atoms: [send].into(),
                tail: Some(RowVar(0)),
            },
        },
    };
    assert_eq!(
        print_scheme(&s),
        "<a, [l] | e>(a) -> Unit / {net.send[l] | e}"
    );
}

/// A footprint carries its binders as a scheme's head does, in the order its atoms name them.
#[test]
fn a_footprints_head_binds_the_labels_its_atoms_name() {
    let bound = Footprint::from_atoms([
        EffectAtom::operation("net", Resource::Var(LabelVar(0)), Mode::Write, "recv"),
        EffectAtom::operation("net", Resource::Var(LabelVar(1)), Mode::Write, "send"),
        EffectAtom::new("net", Resource::Named(Symbol::new("conn")), Mode::Write),
    ]);
    assert_eq!(
        print_footprint(&bound),
        "<[l],[m]>net.write[conn],net.recv[l],net.send[m]"
    );
    assert_eq!(print_footprint(&Footprint::empty()), "");
}

/// A label variable may not take the name of a resource in the same text, or the text would say
/// the two are one; it steps to the next letter, and past the last to the round.
#[test]
fn a_label_variable_steps_past_a_resource_of_its_name() {
    let op =
        |resource: Resource, name: &str| EffectAtom::operation("net", resource, Mode::Write, name);
    let named = |name: &str| Resource::Named(Symbol::new(name));
    let send = op(named("l"), "send");
    let recv = op(Resource::Var(LabelVar(0)), "recv");
    assert_eq!(
        print_footprint(&Footprint::from_atoms([send.clone(), recv.clone()])),
        "<[m]>net.send[l],net.recv[m]"
    );
    // One operation under a resource and under the variable: two atoms, and two names.
    assert_eq!(
        print_footprint(&Footprint::from_atoms([
            send.clone(),
            op(Resource::Var(LabelVar(0)), "send"),
        ])),
        "<[m]>net.send[l],net.send[m]"
    );
    let scheme = Scheme {
        ty_vars: vec![],
        row_vars: vec![],
        label_vars: vec![LabelVar(0)],
        ty: Type::Fn {
            params: vec![],
            ret: Box::new(Type::unit()),
            effects: Row::closed([send.clone(), recv.clone()]),
        },
    };
    assert_eq!(
        print_scheme(&scheme),
        "<[m]>() -> Unit / {net.send[l], net.recv[m]}"
    );
    let close = op(named("m"), "close");
    assert_eq!(
        print_footprint(&Footprint::from_atoms([
            send.clone(),
            close.clone(),
            recv.clone()
        ])),
        "<[n]>net.send[l],net.close[m],net.recv[n]"
    );
    let connect = op(named("n"), "connect");
    assert_eq!(
        print_footprint(&Footprint::from_atoms([send, close, connect, recv])),
        "<[l1]>net.send[l],net.close[m],net.connect[n],net.recv[l1]"
    );
}

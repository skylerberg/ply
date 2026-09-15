use ply_core::ty::{EffectAtom, Resource, Row, Type};
use ply_core::unify::*;
use ply_span::Symbol;
use ply_syntax::ast::Mode;

fn atom(effect: &str, resource: &str, mode: Mode) -> EffectAtom {
    EffectAtom::new(effect, Resource::Named(Symbol::new(resource)), mode)
}

fn ctx() -> (Subst, Fresh) {
    (Subst::new(), Fresh::default())
}

#[test]
fn a_type_variable_binds_and_resolves_transitively() {
    let (mut s, mut f) = ctx();
    let a = f.ty();
    let b = f.ty();
    unify(&mut s, &mut f, &a, &b).unwrap();
    unify(&mut s, &mut f, &b, &Type::int()).unwrap();
    assert_eq!(s.resolve_ty(&a), Type::int());
}

#[test]
fn the_occurs_check_rejects_a_cyclic_type() {
    let (mut s, mut f) = ctx();
    let a = f.ty();
    let cyclic = Type::list(a.clone());
    let err = unify(&mut s, &mut f, &a, &cyclic).unwrap_err();
    assert!(matches!(*err, UnifyError::OccursTy { .. }));
}

#[test]
fn distinct_tails_split_over_a_fresh_tail() {
    let (mut s, mut f) = ctx();
    let r = atom("db", "users", Mode::Read);
    let w = atom("db", "orders", Mode::Write);
    let left = Row {
        atoms: [r.clone()].into(),
        tail: Some(f.row_var()),
    };
    let right = Row {
        atoms: [w.clone()].into(),
        tail: Some(f.row_var()),
    };
    unify_row(&mut s, &mut f, &left, &right).unwrap();
    let lr = s.resolve_row(&left);
    let rr = s.resolve_row(&right);
    assert_eq!(lr.atoms, [r, w].into());
    assert_eq!(lr.atoms, rr.atoms);
    assert_eq!(lr.tail, rr.tail);
    assert!(lr.tail.is_some());
}

#[test]
fn a_shared_tail_absorbs_the_symmetric_difference() {
    let (mut s, mut f) = ctx();
    let v = f.row_var();
    let a = atom("db", "users", Mode::Read);
    let b = atom("db", "users", Mode::Write);
    let left = Row {
        atoms: [a.clone()].into(),
        tail: Some(v),
    };
    let right = Row {
        atoms: [b.clone()].into(),
        tail: Some(v),
    };
    unify_row(&mut s, &mut f, &left, &right).unwrap();
    assert_eq!(s.resolve_row(&left), s.resolve_row(&right));
    assert_eq!(s.resolve_row(&left).atoms, [a, b].into());
}

#[test]
fn a_closed_row_forces_the_other_tail_to_the_difference() {
    let (mut s, mut f) = ctx();
    let a = atom("db", "users", Mode::Read);
    let b = atom("db", "users", Mode::Write);
    let v = f.row_var();
    let closed = Row::closed([a.clone(), b.clone()]);
    let open = Row {
        atoms: [a].into(),
        tail: Some(v),
    };
    unify_row(&mut s, &mut f, &closed, &open).unwrap();
    assert_eq!(s.resolve_row(&Row::open(v)), Row::closed([b]));
}

#[test]
fn a_closed_row_cannot_absorb_an_atom_the_other_side_lacks() {
    let (mut s, mut f) = ctx();
    let a = atom("db", "users", Mode::Read);
    let closed = Row::empty();
    let open = Row {
        atoms: [a].into(),
        tail: Some(f.row_var()),
    };
    assert!(unify_row(&mut s, &mut f, &closed, &open).is_err());
}

#[test]
fn a_rigid_row_variable_refuses_to_absorb_an_atom() {
    let (mut s, mut f) = ctx();
    let rigid = f.row_var();
    s.mark_rigid_row(rigid);
    let a = atom("db", "users", Mode::Read);
    let err = unify_row(&mut s, &mut f, &Row::open(rigid), &Row::closed([a])).unwrap_err();
    assert!(matches!(*err, UnifyError::RowMismatch { .. }));
}

#[test]
fn a_rigid_row_variable_still_unifies_with_a_flexible_one() {
    let (mut s, mut f) = ctx();
    let rigid = f.row_var();
    s.mark_rigid_row(rigid);
    let flex = f.row_var();
    unify_row(&mut s, &mut f, &Row::open(rigid), &Row::open(flex)).unwrap();
    assert_eq!(s.resolve_row(&Row::open(flex)), Row::open(rigid));
}

#[test]
fn a_rigid_type_variable_does_not_unify_with_a_concrete_type() {
    let (mut s, mut f) = ctx();
    let v = f.ty_var();
    s.mark_rigid_ty(v);
    assert!(unify(&mut s, &mut f, &Type::Var(v), &Type::int()).is_err());
    assert!(unify(&mut s, &mut f, &Type::int(), &Type::Var(v)).is_err());
}

#[test]
fn function_types_unify_pointwise_including_their_rows() {
    let (mut s, mut f) = ctx();
    let a = f.ty();
    let row = f.row();
    let lhs = Type::Fn {
        params: vec![a.clone()],
        ret: Box::new(a),
        effects: row.clone(),
    };
    let atoms = Row::closed([atom("db", "users", Mode::Read)]);
    let rhs = Type::Fn {
        params: vec![Type::int()],
        ret: Box::new(Type::int()),
        effects: atoms.clone(),
    };
    unify(&mut s, &mut f, &lhs, &rhs).unwrap();
    assert_eq!(s.resolve_row(&row), atoms);
}

#[test]
fn arity_is_reported_separately_from_a_shape_mismatch() {
    let (mut s, mut f) = ctx();
    let lhs = Type::Fn {
        params: vec![Type::int()],
        ret: Box::new(Type::int()),
        effects: Row::empty(),
    };
    let rhs = Type::Fn {
        params: vec![],
        ret: Box::new(Type::int()),
        effects: Row::empty(),
    };
    let err = unify(&mut s, &mut f, &lhs, &rhs).unwrap_err();
    assert!(matches!(
        *err,
        UnifyError::Arity {
            expected: 1,
            found: 0
        }
    ));
}

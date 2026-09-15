use ply_core::env::*;
use ply_core::ty::{Row, RowVar, Scheme, TyVar, Type};
use ply_core::unify::{Fresh, Subst, unify};
use ply_span::Symbol;

#[test]
fn instantiation_refreshes_every_quantified_variable_consistently() {
    let mut fresh = Fresh::default();
    let scheme = Scheme {
        ty_vars: vec![TyVar(0)],
        row_vars: vec![RowVar(0)],
        ty: Type::Fn {
            params: vec![Type::Var(TyVar(0))],
            ret: Box::new(Type::Var(TyVar(0))),
            effects: Row::open(RowVar(0)),
        },
    };
    let a = instantiate(&scheme, &mut fresh);
    let b = instantiate(&scheme, &mut fresh);
    assert_ne!(a, b);
    match a {
        Type::Fn { params, ret, .. } => assert_eq!(params[0], *ret),
        _ => panic!("expected a function type"),
    }
}

#[test]
fn generalization_skips_variables_still_reachable_from_the_environment() {
    let subst = Subst::new();
    let mut fresh = Fresh::default();
    let mut env = TypeEnv::new();
    let captured = fresh.ty();
    env.bind_global(Symbol::new("outer"), Scheme::mono(captured.clone()));
    let free = fresh.ty();
    let ty = Type::Fn {
        params: vec![captured],
        ret: Box::new(free.clone()),
        effects: Row::empty(),
    };
    let scheme = generalize(&subst, &mut env, &ty);
    let Type::Var(free_var) = free else {
        unreachable!()
    };
    assert_eq!(scheme.ty_vars, vec![free_var]);
}

#[test]
fn generalization_sees_through_the_substitution() {
    let mut subst = Subst::new();
    let mut fresh = Fresh::default();
    let mut env = TypeEnv::new();
    let a = fresh.ty();
    unify(&mut subst, &mut fresh, &a, &Type::int()).unwrap();
    let scheme = generalize(&subst, &mut env, &a);
    assert!(scheme.ty_vars.is_empty());
    assert_eq!(scheme.ty, Type::int());
}

#[test]
fn a_row_variable_generalizes_alongside_type_variables() {
    let subst = Subst::new();
    let mut fresh = Fresh::default();
    let mut env = TypeEnv::new();
    let row = fresh.row();
    let ty = Type::Fn {
        params: vec![],
        ret: Box::new(Type::unit()),
        effects: row.clone(),
    };
    let scheme = generalize(&subst, &mut env, &ty);
    assert_eq!(scheme.row_vars, vec![row.tail.unwrap()]);
}

#[test]
fn shadowing_is_reported_by_depth() {
    let mut env = TypeEnv::new();
    let name = Symbol::new("cell_get");
    env.bind_global(name.clone(), Scheme::mono(Type::int()));
    assert_eq!(env.depth_of(&name), Some(0));
    env.push();
    env.bind(name.clone(), Scheme::mono(Type::bool()));
    assert_eq!(env.depth_of(&name), Some(1));
    env.pop();
    assert_eq!(env.depth_of(&name), Some(0));
}

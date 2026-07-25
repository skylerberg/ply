use crate::ty::{Row, RowVar, Scheme, TyVar, Type};
use crate::unify::{Fresh, Subst};
use ply_span::Symbol;
use rustc_hash::FxHashMap;
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct TypeEnv {
    scopes: Vec<FxHashMap<Symbol, Scheme>>,
}

impl Default for TypeEnv {
    fn default() -> Self {
        TypeEnv { scopes: vec![FxHashMap::default()] }
    }
}

impl TypeEnv {
    pub fn new() -> Self {
        TypeEnv::default()
    }

    pub fn push(&mut self) {
        self.scopes.push(FxHashMap::default());
    }

    pub fn pop(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    pub fn bind(&mut self, name: Symbol, scheme: Scheme) {
        self.scopes.last_mut().expect("env always has a scope").insert(name, scheme);
    }

    pub fn bind_global(&mut self, name: Symbol, scheme: Scheme) {
        self.scopes[0].insert(name, scheme);
    }

    pub fn remove_global(&mut self, name: &Symbol) {
        self.scopes[0].remove(name);
    }

    pub fn lookup(&self, name: &Symbol) -> Option<&Scheme> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    /// `Some(0)` means the name resolves to a global; anything deeper means a
    /// user binding shadows it, which is how the builtin call forms know to
    /// stand aside.
    pub fn depth_of(&self, name: &Symbol) -> Option<usize> {
        self.scopes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, s)| s.contains_key(name))
            .map(|(i, _)| i)
    }

    pub fn schemes(&self) -> impl Iterator<Item = &Scheme> {
        self.scopes.iter().flat_map(|s| s.values())
    }
}

pub fn instantiate(scheme: &Scheme, fresh: &mut Fresh) -> Type {
    if scheme.ty_vars.is_empty() && scheme.row_vars.is_empty() {
        return scheme.ty.clone();
    }
    let tys: FxHashMap<TyVar, Type> =
        scheme.ty_vars.iter().map(|v| (*v, fresh.ty())).collect();
    let rows: FxHashMap<RowVar, RowVar> =
        scheme.row_vars.iter().map(|v| (*v, fresh.row_var())).collect();
    rename(&scheme.ty, &tys, &rows)
}

fn rename(t: &Type, tys: &FxHashMap<TyVar, Type>, rows: &FxHashMap<RowVar, RowVar>) -> Type {
    match t {
        Type::Var(v) => tys.get(v).cloned().unwrap_or(Type::Var(*v)),
        Type::Con(name, args) => {
            Type::Con(name.clone(), args.iter().map(|a| rename(a, tys, rows)).collect())
        }
        Type::Fn { params, ret, effects } => Type::Fn {
            params: params.iter().map(|p| rename(p, tys, rows)).collect(),
            ret: Box::new(rename(ret, tys, rows)),
            effects: rename_row(effects, rows),
        },
        Type::Record(fields) => Type::Record(
            fields.iter().map(|(k, v)| (k.clone(), rename(v, tys, rows))).collect(),
        ),
    }
}

fn rename_row(r: &Row, rows: &FxHashMap<RowVar, RowVar>) -> Row {
    Row {
        atoms: r.atoms.clone(),
        tail: r.tail.map(|v| rows.get(&v).copied().unwrap_or(v)),
    }
}

pub fn generalize(subst: &Subst, env: &TypeEnv, ty: &Type) -> Scheme {
    let ty = subst.resolve_ty(ty);
    let (mut env_tys, mut env_rows) = (BTreeSet::new(), BTreeSet::new());
    for scheme in env.schemes() {
        let (mut tys, mut rows) = (BTreeSet::new(), BTreeSet::new());
        subst.free_vars(&scheme.ty, &mut tys, &mut rows);
        for v in &scheme.ty_vars {
            tys.remove(v);
        }
        for v in &scheme.row_vars {
            rows.remove(v);
        }
        env_tys.extend(tys);
        env_rows.extend(rows);
    }
    let (mut tys, mut rows) = (BTreeSet::new(), BTreeSet::new());
    subst.free_vars(&ty, &mut tys, &mut rows);
    Scheme {
        ty_vars: tys.difference(&env_tys).copied().collect(),
        row_vars: rows.difference(&env_rows).copied().collect(),
        ty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unify::unify;

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
        let scheme = generalize(&subst, &env, &ty);
        let Type::Var(free_var) = free else { unreachable!() };
        assert_eq!(scheme.ty_vars, vec![free_var]);
    }

    #[test]
    fn generalization_sees_through_the_substitution() {
        let mut subst = Subst::new();
        let mut fresh = Fresh::default();
        let env = TypeEnv::new();
        let a = fresh.ty();
        unify(&mut subst, &mut fresh, &a, &Type::int()).unwrap();
        let scheme = generalize(&subst, &env, &a);
        assert!(scheme.ty_vars.is_empty());
        assert_eq!(scheme.ty, Type::int());
    }

    #[test]
    fn a_row_variable_generalizes_alongside_type_variables() {
        let subst = Subst::new();
        let mut fresh = Fresh::default();
        let env = TypeEnv::new();
        let row = fresh.row();
        let ty = Type::Fn { params: vec![], ret: Box::new(Type::unit()), effects: row.clone() };
        let scheme = generalize(&subst, &env, &ty);
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
}

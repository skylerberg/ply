use crate::ty::{Row, RowVar, Scheme, TyVar, Type};
use crate::unify::{Fresh, Subst};
use ply_span::Symbol;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default)]
struct Scope {
    schemes: FxHashMap<Symbol, Scheme>,
    /// Names whose scheme has not yet been shown to contribute no free variable
    /// to [`generalize`]. See [`TypeEnv::free_vars`].
    open: FxHashSet<Symbol>,
}

#[derive(Clone, Debug)]
pub struct TypeEnv {
    scopes: Vec<Scope>,
}

impl Default for TypeEnv {
    fn default() -> Self {
        TypeEnv {
            scopes: vec![Scope::default()],
        }
    }
}

impl TypeEnv {
    pub fn new() -> Self {
        TypeEnv::default()
    }

    pub fn push(&mut self) {
        self.scopes.push(Scope::default());
    }

    pub fn pop(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    pub fn bind(&mut self, name: Symbol, scheme: Scheme) {
        let scope = self.scopes.last_mut().expect("env always has a scope");
        scope.open.insert(name.clone());
        scope.schemes.insert(name, scheme);
    }

    pub fn bind_global(&mut self, name: Symbol, scheme: Scheme) {
        self.scopes[0].open.insert(name.clone());
        self.scopes[0].schemes.insert(name, scheme);
    }

    pub fn remove_global(&mut self, name: &Symbol) {
        self.scopes[0].open.remove(name);
        self.scopes[0].schemes.remove(name);
    }

    pub fn lookup(&self, name: &Symbol) -> Option<&Scheme> {
        self.scopes.iter().rev().find_map(|s| s.schemes.get(name))
    }

    /// `Some(0)` means the name resolves to a global; anything deeper means a
    /// user binding shadows it, which is how the builtin call forms know to
    /// stand aside.
    pub fn depth_of(&self, name: &Symbol) -> Option<usize> {
        self.scopes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, s)| s.schemes.contains_key(name))
            .map(|(i, _)| i)
    }

    pub fn schemes(&self) -> impl Iterator<Item = &Scheme> {
        self.scopes.iter().flat_map(|s| s.schemes.values())
    }

    /// The bindings the definition being checked introduced, globals excluded.
    ///
    /// A global is bound from a scheme that was generalized first, so its
    /// variables are quantified and a later unification reaches a fresh copy
    /// rather than the scheme — which is why the region escape check can ask
    /// about locals alone and still see every binding a region could store into.
    pub fn locals(&self) -> impl Iterator<Item = (&Symbol, &Scheme)> {
        self.scopes.iter().skip(1).flat_map(|s| s.schemes.iter())
    }

    /// The type and row variables still unsolved anywhere in scope — the ones
    /// [`generalize`] may not quantify.
    ///
    /// A scheme that contributes nothing is dropped from the scan for good.
    /// That is sound because contributing nothing is *monotone*: a substitution
    /// only ever gains bindings, and a binding is never replaced, so a scheme
    /// whose every variable is either quantified or already solved can never
    /// acquire a free one. Rebinding a name puts it back under scrutiny.
    /// Scanning them all instead is quadratic in the size of the program,
    /// because the global scope holds every definition already checked.
    fn free_vars(&mut self, subst: &Subst) -> (BTreeSet<TyVar>, BTreeSet<RowVar>) {
        let (mut env_tys, mut env_rows) = (BTreeSet::new(), BTreeSet::new());
        for scope in &mut self.scopes {
            let schemes = &scope.schemes;
            scope.open.retain(|name| {
                let Some(scheme) = schemes.get(name) else {
                    return false;
                };
                let (mut tys, mut rows) = (BTreeSet::new(), BTreeSet::new());
                subst.free_vars(&scheme.ty, &mut tys, &mut rows);
                for v in &scheme.ty_vars {
                    tys.remove(v);
                }
                for v in &scheme.row_vars {
                    rows.remove(v);
                }
                if tys.is_empty() && rows.is_empty() {
                    return false;
                }
                env_tys.extend(tys);
                env_rows.extend(rows);
                true
            });
        }
        (env_tys, env_rows)
    }
}

pub fn instantiate(scheme: &Scheme, fresh: &mut Fresh) -> Type {
    instantiate_with(scheme, fresh).0
}

/// [`instantiate`], and what each quantified type variable became, in the
/// scheme's own order.
///
/// A `where derivable(D, a)` names its parameter by that position, because a
/// name is exactly what a published interface may not depend on. A call site
/// checking the constraint needs the argument the instantiation handed it, and
/// this is the only place that knows.
pub fn instantiate_with(scheme: &Scheme, fresh: &mut Fresh) -> (Type, Vec<Type>) {
    if scheme.ty_vars.is_empty() && scheme.row_vars.is_empty() {
        return (scheme.ty.clone(), Vec::new());
    }
    let args: Vec<Type> = scheme.ty_vars.iter().map(|_| fresh.ty()).collect();
    let tys: FxHashMap<TyVar, Type> = scheme
        .ty_vars
        .iter()
        .copied()
        .zip(args.iter().cloned())
        .collect();
    let rows: FxHashMap<RowVar, RowVar> = scheme
        .row_vars
        .iter()
        .map(|v| (*v, fresh.row_var()))
        .collect();
    (rename(&scheme.ty, &tys, &rows), args)
}

/// [`instantiate`] onto variables the caller chose rather than fresh ones, so
/// that the result can be quantified again over exactly those.
pub fn rename_scheme(scheme: &Scheme, ty_vars: &[TyVar], row_vars: &[RowVar]) -> Type {
    let tys: FxHashMap<TyVar, Type> = scheme
        .ty_vars
        .iter()
        .zip(ty_vars)
        .map(|(from, to)| (*from, Type::Var(*to)))
        .collect();
    let rows: FxHashMap<RowVar, RowVar> = scheme
        .row_vars
        .iter()
        .zip(row_vars)
        .map(|(from, to)| (*from, *to))
        .collect();
    rename(&scheme.ty, &tys, &rows)
}

fn rename(t: &Type, tys: &FxHashMap<TyVar, Type>, rows: &FxHashMap<RowVar, RowVar>) -> Type {
    match t {
        Type::Var(v) => tys.get(v).cloned().unwrap_or(Type::Var(*v)),
        Type::Con(name, args) => Type::Con(
            name.clone(),
            args.iter().map(|a| rename(a, tys, rows)).collect(),
        ),
        Type::Fn {
            params,
            ret,
            effects,
        } => Type::Fn {
            params: params.iter().map(|p| rename(p, tys, rows)).collect(),
            ret: Box::new(rename(ret, tys, rows)),
            effects: rename_row(effects, rows),
        },
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), rename(v, tys, rows)))
                .collect(),
        ),
    }
}

fn rename_row(r: &Row, rows: &FxHashMap<RowVar, RowVar>) -> Row {
    Row {
        atoms: r.atoms.clone(),
        tail: r.tail.map(|v| rows.get(&v).copied().unwrap_or(v)),
    }
}

pub fn generalize(subst: &Subst, env: &mut TypeEnv, ty: &Type) -> Scheme {
    let ty = subst.resolve_ty(ty);
    let (env_tys, env_rows) = env.free_vars(subst);
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
}

//! Canonical form for a stored interface.

use ply_ty::{Row, RowVar, Scheme, TyVar, Type};
use std::collections::HashMap;

use crate::frontend::{CachedCtor, CachedOp, DeclBody};

/// Alpha-renames a scheme to its canonical numbering.
pub fn canonicalize_scheme(scheme: &Scheme) -> Scheme {
    Renumber::default().scheme(scheme)
}

/// Canonicalizes a declaration's signatures under **one** numbering, because a type's parameters
/// are shared by every constructor: renumbering each constructor independently would make `P(a)`
/// and `Q(b)` of `type Pair<a, b>` both mention `t0`.
pub fn canonicalize_decl_body(body: &DeclBody) -> DeclBody {
    Renumber::default().decl_body(body)
}

#[derive(Default)]
struct Renumber {
    tys: HashMap<TyVar, TyVar>,
    rows: HashMap<RowVar, RowVar>,
}

impl Renumber {
    fn ty_var(&mut self, v: TyVar) -> TyVar {
        let next = TyVar(self.tys.len() as u32);
        *self.tys.entry(v).or_insert(next)
    }

    fn row_var(&mut self, v: RowVar) -> RowVar {
        let next = RowVar(self.rows.len() as u32);
        *self.rows.entry(v).or_insert(next)
    }

    fn ty(&mut self, ty: &Type) -> Type {
        crate::codec::grow(|| self.ty_inner(ty))
    }

    fn ty_inner(&mut self, ty: &Type) -> Type {
        match ty {
            Type::Var(v) => Type::Var(self.ty_var(*v)),
            Type::Con(name, args) => {
                Type::Con(name.clone(), args.iter().map(|a| self.ty(a)).collect())
            }
            Type::Fn {
                params,
                ret,
                effects,
            } => Type::Fn {
                params: params.iter().map(|p| self.ty(p)).collect(),
                ret: Box::new(self.ty(ret)),
                effects: self.row(effects),
            },
            // A `BTreeMap` iterates in key order, so the traversal does not depend on how the
            // record was built.
            Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|(name, t)| (name.clone(), self.ty(t)))
                    .collect(),
            ),
        }
    }

    fn row(&mut self, row: &Row) -> Row {
        Row {
            atoms: row.atoms.clone(),
            tail: row.tail.map(|t| self.row_var(t)),
        }
    }

    fn scheme(&mut self, scheme: &Scheme) -> Scheme {
        // The body first: a variable's canonical number is where it is *used*, so that a quantifier
        // list in a different order cannot change it.
        let ty = self.ty(&scheme.ty);
        let mut ty_vars: Vec<TyVar> = scheme.ty_vars.iter().map(|v| self.ty_var(*v)).collect();
        let mut row_vars: Vec<RowVar> = scheme.row_vars.iter().map(|v| self.row_var(*v)).collect();
        ty_vars.sort_unstable();
        ty_vars.dedup();
        row_vars.sort_unstable();
        row_vars.dedup();
        Scheme {
            ty_vars,
            row_vars,
            ty,
        }
    }

    fn decl_body(&mut self, body: &DeclBody) -> DeclBody {
        match body {
            DeclBody::Type { arity, ctors } => DeclBody::Type {
                arity: *arity,
                ctors: ctors
                    .iter()
                    .map(|c| CachedCtor {
                        fields: c.fields.iter().map(|f| self.ty(f)).collect(),
                        scheme: self.scheme(&c.scheme),
                    })
                    .collect(),
            },
            DeclBody::Effect { nondet, ops } => DeclBody::Effect {
                nondet: *nondet,
                ops: ops
                    .iter()
                    .map(|op| CachedOp {
                        name: op.name.clone(),
                        mode: op.mode,
                        resource_param: op.resource_param,
                        params: op.params.iter().map(|p| self.ty(p)).collect(),
                        ret: self.ty(&op.ret),
                    })
                    .collect(),
            },
        }
    }
}

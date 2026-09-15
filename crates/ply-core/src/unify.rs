//! Solving `{A | ρ1} ~ {B | ρ2}` as `ρ1 := (B\A) ∪ ρ3`, `ρ2 := (A\B) ∪ ρ3` is most general because
//! it expands both sides to exactly `A ∪ B ∪ ρ3`.

use crate::ty::{EffectAtom, Row, RowVar, TyVar, Type};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default)]
pub struct Fresh {
    next_ty: u32,
    next_row: u32,
}

impl Fresh {
    pub fn ty_var(&mut self) -> TyVar {
        let v = TyVar(self.next_ty);
        self.next_ty += 1;
        v
    }

    pub fn ty(&mut self) -> Type {
        Type::Var(self.ty_var())
    }

    pub fn row_var(&mut self) -> RowVar {
        let v = RowVar(self.next_row);
        self.next_row += 1;
        v
    }

    pub fn row(&mut self) -> Row {
        Row::open(self.row_var())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Subst {
    ty: FxHashMap<TyVar, Type>,
    row: FxHashMap<RowVar, Row>,
    rigid_ty: FxHashSet<TyVar>,
    rigid_row: FxHashSet<RowVar>,
}

/// Boxed at the `Result` boundary: a mismatch carries whole types, and every unification on the
/// success path would otherwise pay for that.
pub type UnifyResult = Result<(), Box<UnifyError>>;

#[derive(Clone, Debug)]
pub enum UnifyError {
    Mismatch { expected: Type, found: Type },
    Arity { expected: usize, found: usize },
    OccursTy { var: TyVar, ty: Type },
    OccursRow { var: RowVar, row: Row },
    RowMismatch { expected: Row, found: Row },
}

impl Subst {
    pub fn new() -> Self {
        Subst::default()
    }

    pub fn mark_rigid_ty(&mut self, v: TyVar) {
        self.rigid_ty.insert(v);
    }

    pub fn mark_rigid_row(&mut self, v: RowVar) {
        self.rigid_row.insert(v);
    }

    pub fn is_rigid_ty(&self, v: TyVar) -> bool {
        self.rigid_ty.contains(&v)
    }

    pub fn is_rigid_row(&self, v: RowVar) -> bool {
        self.rigid_row.contains(&v)
    }

    /// Borrowed rather than cloned: the traversals below visit every node of every type in the
    /// program, and a clone per node makes each of them quadratic in the size of a type.
    fn shallow_ref<'a>(&'a self, t: &'a Type) -> &'a Type {
        let mut cur = t;
        while let Type::Var(v) = cur {
            match self.ty.get(v) {
                Some(next) => cur = next,
                None => break,
            }
        }
        cur
    }

    pub fn shallow_ty(&self, t: &Type) -> Type {
        self.shallow_ref(t).clone()
    }

    pub fn resolve_ty(&self, t: &Type) -> Type {
        match self.shallow_ref(t) {
            Type::Var(v) => Type::Var(*v),
            Type::Con(name, args) => Type::Con(
                name.clone(),
                args.iter().map(|a| self.resolve_ty(a)).collect(),
            ),
            Type::Fn {
                params,
                ret,
                effects,
            } => Type::Fn {
                params: params.iter().map(|p| self.resolve_ty(p)).collect(),
                ret: Box::new(self.resolve_ty(ret)),
                effects: self.resolve_row(effects),
            },
            Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|(k, v)| (k.clone(), self.resolve_ty(v)))
                    .collect(),
            ),
        }
    }

    /// The tail a row ends in once its variables are followed.
    pub fn resolved_tail(&self, r: &Row) -> Option<RowVar> {
        let mut tail = r.tail;
        let mut hops = 0usize;
        while let Some(v) = tail {
            match self.row.get(&v) {
                Some(next) if next.tail != Some(v) => tail = next.tail,
                _ => return Some(v),
            }
            // A row chain is acyclic by construction, but a corrupt one would hang inference rather
            // than fail it.
            hops += 1;
            if hops > self.row.len() {
                return tail;
            }
        }
        None
    }

    pub fn resolve_row(&self, r: &Row) -> Row {
        let mut atoms = r.atoms.clone();
        let mut tail = r.tail;
        let mut seen: Vec<RowVar> = Vec::new();
        while let Some(v) = tail {
            if seen.contains(&v) {
                break;
            }
            seen.push(v);
            match self.row.get(&v) {
                Some(next) => {
                    atoms.extend(next.atoms.iter().cloned());
                    tail = next.tail;
                }
                None => break,
            }
        }
        Row { atoms, tail }
    }

    fn bind_ty(&mut self, v: TyVar, t: &Type) -> UnifyResult {
        let t = self.resolve_ty(t);
        if t == Type::Var(v) {
            return Ok(());
        }
        if occurs_ty(v, &t) {
            return Err(Box::new(UnifyError::OccursTy { var: v, ty: t }));
        }
        self.ty.insert(v, t);
        Ok(())
    }

    fn bind_row(&mut self, v: RowVar, r: Row) -> UnifyResult {
        if r.atoms.is_empty() && r.tail == Some(v) {
            return Ok(());
        }
        if r.tail == Some(v) {
            return Err(Box::new(UnifyError::OccursRow { var: v, row: r }));
        }
        self.row.insert(v, r);
        Ok(())
    }

    /// Solves the one row variable a self-occurrence is sound for: the row of a `handle`'s
    /// continuation, which is by construction the residual row of the `handle` itself.
    pub fn solve_continuation_row(&mut self, v: RowVar, row: &Row) {
        if self.row.contains_key(&v) {
            return;
        }
        let mut solved = self.resolve_row(row);
        if solved.tail == Some(v) {
            solved.tail = None;
        }
        self.row.insert(v, solved);
    }

    pub fn free_vars(&self, t: &Type, tys: &mut BTreeSet<TyVar>, rows: &mut BTreeSet<RowVar>) {
        match self.shallow_ref(t) {
            Type::Var(v) => {
                tys.insert(*v);
            }
            Type::Con(_, args) => {
                for a in args {
                    self.free_vars(a, tys, rows);
                }
            }
            Type::Fn {
                params,
                ret,
                effects,
            } => {
                for p in params {
                    self.free_vars(p, tys, rows);
                }
                self.free_vars(ret, tys, rows);
                if let Some(v) = self.resolved_tail(effects) {
                    rows.insert(v);
                }
            }
            Type::Record(fields) => {
                for v in fields.values() {
                    self.free_vars(v, tys, rows);
                }
            }
        }
    }
}

fn occurs_ty(v: TyVar, t: &Type) -> bool {
    match t {
        Type::Var(w) => *w == v,
        Type::Con(_, args) => args.iter().any(|a| occurs_ty(v, a)),
        Type::Fn { params, ret, .. } => params.iter().any(|p| occurs_ty(v, p)) || occurs_ty(v, ret),
        Type::Record(fields) => fields.values().any(|f| occurs_ty(v, f)),
    }
}

pub fn unify(s: &mut Subst, f: &mut Fresh, expected: &Type, found: &Type) -> UnifyResult {
    let a = s.shallow_ty(expected);
    let b = s.shallow_ty(found);
    let mismatch = || {
        Box::new(UnifyError::Mismatch {
            expected: a.clone(),
            found: b.clone(),
        })
    };
    match (&a, &b) {
        (Type::Var(x), Type::Var(y)) if x == y => Ok(()),
        (Type::Var(x), _) if !s.is_rigid_ty(*x) => s.bind_ty(*x, &b),
        (_, Type::Var(y)) if !s.is_rigid_ty(*y) => s.bind_ty(*y, &a),
        (Type::Var(_), _) | (_, Type::Var(_)) => Err(mismatch()),

        (Type::Con(n1, a1), Type::Con(n2, a2)) => {
            if n1 != n2 || a1.len() != a2.len() {
                return Err(mismatch());
            }
            for (x, y) in a1.iter().zip(a2) {
                unify(s, f, x, y)?;
            }
            Ok(())
        }

        (
            Type::Fn {
                params: p1,
                ret: r1,
                effects: e1,
            },
            Type::Fn {
                params: p2,
                ret: r2,
                effects: e2,
            },
        ) => {
            if p1.len() != p2.len() {
                return Err(Box::new(UnifyError::Arity {
                    expected: p1.len(),
                    found: p2.len(),
                }));
            }
            for (x, y) in p1.iter().zip(p2) {
                unify(s, f, x, y)?;
            }
            unify(s, f, r1, r2)?;
            unify_row(s, f, e1, e2)
        }

        (Type::Record(f1), Type::Record(f2)) => {
            if f1.len() != f2.len() || f1.keys().ne(f2.keys()) {
                return Err(mismatch());
            }
            for (k, v1) in f1 {
                unify(s, f, v1, &f2[k])?;
            }
            Ok(())
        }

        _ => Err(mismatch()),
    }
}

pub fn unify_row(s: &mut Subst, f: &mut Fresh, expected: &Row, found: &Row) -> UnifyResult {
    let a = s.resolve_row(expected);
    let b = s.resolve_row(found);
    let only_a: BTreeSet<EffectAtom> = a.atoms.difference(&b.atoms).cloned().collect();
    let only_b: BTreeSet<EffectAtom> = b.atoms.difference(&a.atoms).cloned().collect();
    let bad = || {
        Box::new(UnifyError::RowMismatch {
            expected: a.clone(),
            found: b.clone(),
        })
    };

    match (a.tail, b.tail) {
        (None, None) => {
            if only_a.is_empty() && only_b.is_empty() {
                Ok(())
            } else {
                Err(bad())
            }
        }
        (None, Some(v)) => {
            if !only_b.is_empty() || s.is_rigid_row(v) {
                return Err(bad());
            }
            s.bind_row(v, Row::closed(only_a))
        }
        (Some(v), None) => {
            if !only_a.is_empty() || s.is_rigid_row(v) {
                return Err(bad());
            }
            s.bind_row(v, Row::closed(only_b))
        }
        (Some(v1), Some(v2)) if v1 == v2 => {
            if only_a.is_empty() && only_b.is_empty() {
                return Ok(());
            }
            if s.is_rigid_row(v1) {
                return Err(bad());
            }
            let v3 = f.row_var();
            let mut atoms = only_a;
            atoms.extend(only_b);
            s.bind_row(
                v1,
                Row {
                    atoms,
                    tail: Some(v3),
                },
            )
        }
        (Some(v1), Some(v2)) => match (s.is_rigid_row(v1), s.is_rigid_row(v2)) {
            (true, true) => Err(bad()),
            (true, false) => {
                if !only_b.is_empty() {
                    return Err(bad());
                }
                s.bind_row(
                    v2,
                    Row {
                        atoms: only_a,
                        tail: Some(v1),
                    },
                )
            }
            (false, true) => {
                if !only_a.is_empty() {
                    return Err(bad());
                }
                s.bind_row(
                    v1,
                    Row {
                        atoms: only_b,
                        tail: Some(v2),
                    },
                )
            }
            (false, false) => {
                let v3 = f.row_var();
                s.bind_row(
                    v1,
                    Row {
                        atoms: only_b,
                        tail: Some(v3),
                    },
                )?;
                s.bind_row(
                    v2,
                    Row {
                        atoms: only_a,
                        tail: Some(v3),
                    },
                )
            }
        },
    }
}

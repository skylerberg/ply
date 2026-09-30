//! The world of a checked fixture, read the way `proof.world` reads the compiler's answer: its
//! types from their constructors, its signatures, and a claim's binders numbered together. A test
//! here has no program to build one for it.

use ply_prove::world::{Decl, Signature, Variant, World};
use ply_prove::{Binder, Sort};
use ply_span::Symbol;
use ply_ty::{CheckOutput, TyVar, Type};

/// Each variable once, where it first appears.
fn met(ty: &Type, seen: &mut Vec<TyVar>) {
    match ty {
        Type::Var(v) => {
            if !seen.contains(v) {
                seen.push(*v);
            }
        }
        Type::Con(_, args) => args.iter().for_each(|a| met(a, seen)),
        Type::Fn { params, ret, .. } => {
            params.iter().for_each(|p| met(p, seen));
            met(ret, seen);
        }
        Type::Record(fields) => fields.values().for_each(|f| met(f, seen)),
    }
}

/// A variable the numbering does not hold is numbered past all of them, as `proof.world` does.
pub fn sort_of(ty: &Type, vars: &[TyVar]) -> Sort {
    match ty {
        Type::Var(v) => Sort::Var(vars.iter().position(|x| x == v).unwrap_or(vars.len()) as u32),
        Type::Con(name, args) => Sort::Con(
            name.clone(),
            args.iter().map(|a| sort_of(a, vars)).collect(),
        ),
        Type::Fn {
            params,
            ret,
            effects,
        } => Sort::func(
            params.iter().map(|p| sort_of(p, vars)).collect(),
            sort_of(ret, vars),
            effects.is_pure(),
        ),
        Type::Record(fields) => Sort::record(
            fields
                .iter()
                .map(|(name, f)| (name.clone(), sort_of(f, vars))),
        ),
    }
}

/// One claim's binders, numbered together.
pub fn binders_of<'t>(named: impl IntoIterator<Item = (Symbol, &'t Type)>) -> Vec<Binder> {
    let named: Vec<(Symbol, &Type)> = named.into_iter().collect();
    let mut vars = Vec::new();
    for (_, ty) in &named {
        met(ty, &mut vars);
    }
    named
        .into_iter()
        .map(|(name, ty)| {
            let sort = sort_of(ty, &vars);
            Binder {
                name,
                text: sort.to_string(),
                sort,
            }
        })
        .collect()
}

pub fn world_of(check: &CheckOutput) -> World {
    let mut decls: Vec<Decl> = Vec::new();
    for ctor in check.ctors.values() {
        let answer = match &ctor.scheme.ty {
            Type::Fn { ret, .. } => ret.as_ref(),
            other => other,
        };
        let params: Vec<TyVar> = match answer {
            Type::Con(_, args) => args
                .iter()
                .map(|a| match a {
                    Type::Var(v) => *v,
                    _ => TyVar(u32::MAX),
                })
                .collect(),
            _ => Vec::new(),
        };
        let variant = Variant {
            name: ctor.name.clone(),
            index: ctor.index,
            fields: ctor.fields.iter().map(|f| sort_of(f, &params)).collect(),
            depth: None,
        };
        match decls.iter_mut().find(|d| d.name == ctor.type_name) {
            Some(decl) => decl.variants.push(variant),
            None => decls.push(Decl {
                name: ctor.type_name.clone(),
                params: params.len(),
                variants: vec![variant],
                depth: None,
            }),
        }
    }
    for decl in &mut decls {
        decl.variants.sort_by_key(|v| v.index);
    }
    let signatures = check.defs.values().map(|def| {
        let mut vars = Vec::new();
        met(&def.scheme.ty, &mut vars);
        Signature {
            name: def.name.clone(),
            sort: sort_of(&def.scheme.ty, &vars),
            pure: def.footprint.is_empty(),
        }
    });
    World::new(decls, signatures)
}

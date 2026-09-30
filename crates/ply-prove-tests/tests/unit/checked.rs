//! The world of a checked fixture, read off the compiler's answer the way `proof.world` reads the
//! checker's tables: its types from their constructors, its signatures, and a claim's binders
//! numbered together. A test here has no program to build one for it.

use ply_eval::Symbol;
use ply_eval::decode::{At, Error};
use ply_prove::world::{Decl, Signature, Variant, World};
use ply_prove::{Binder, Sort};

/// A record's fields in the order the printer reads them: a tuple's by position.
fn fields(list: At<'_>) -> Result<Vec<(&str, At<'_>)>, Error> {
    let fields: Vec<(&str, At<'_>)> =
        list.items(|f| Ok((f.field("name")?.utf8()?, f.field("ty")?)))?;
    let position = |i: usize| fields.iter().position(|(name, _)| *name == format!("_{i}"));
    if fields.len() >= 2 && (0..fields.len()).all(|i| position(i).is_some()) {
        return Ok((0..fields.len())
            .filter_map(position)
            .map(|at| fields[at])
            .collect());
    }
    Ok(fields)
}

/// Each variable of a `tycore.Type` once, where the printer meets it, after the ones already met.
fn met(ty: At<'_>, seen: &mut Vec<i64>) -> Result<(), Error> {
    let c = ty.ctor()?;
    match c.name() {
        "TyVar" => {
            let v = c.arg(0)?.int()?;
            if !seen.contains(&v) {
                seen.push(v);
            }
        }
        "TyCon" => {
            for arg in c.arg(0)?.field("args")?.list()? {
                met(arg, seen)?;
            }
        }
        "TyFn" => {
            let f = c.arg(0)?;
            for param in f.field("params")?.list()? {
                met(param, seen)?;
            }
            met(f.field("ret")?, seen)?;
        }
        "TyRecord" => {
            for (_, field) in fields(c.arg(0)?)? {
                met(field, seen)?;
            }
        }
        _ => return Err(c.unknown()),
    }
    Ok(())
}

/// A variable the numbering does not hold is numbered past all of them, as `proof.world` does.
fn sort_of(ty: At<'_>, vars: &[i64]) -> Result<Sort, Error> {
    let c = ty.ctor()?;
    Ok(match c.name() {
        "TyVar" => {
            let v = c.arg(0)?.int()?;
            Sort::Var(vars.iter().position(|x| *x == v).unwrap_or(vars.len()) as u32)
        }
        "TyCon" => {
            let con = c.arg(0)?;
            Sort::Con(
                Symbol::new(con.field("name")?.utf8()?),
                con.field("args")?.items(|a| sort_of(a, vars))?,
            )
        }
        "TyFn" => {
            let f = c.arg(0)?;
            let effects = f.field("effects")?;
            let pure = effects.field("atoms")?.list()?.len() == 0
                && effects.field("tail")?.option()?.is_none();
            Sort::func(
                f.field("params")?.items(|p| sort_of(p, vars))?,
                sort_of(f.field("ret")?, vars)?,
                pure,
            )
        }
        "TyRecord" => Sort::record(
            fields(c.arg(0)?)?
                .into_iter()
                .map(|(name, field)| Ok((Symbol::new(name), sort_of(field, vars)?)))
                .collect::<Result<Vec<_>, Error>>()?,
        ),
        _ => return Err(c.unknown()),
    })
}

/// One claim's binders, numbered together. The static prover prints nothing, so a binder's text,
/// which is the compiler's printing, is left empty.
pub fn binders_of(named: &[(Symbol, At<'_>)]) -> Vec<Binder> {
    let mut vars = Vec::new();
    for (_, ty) in named {
        met(*ty, &mut vars).unwrap_or_else(|e| panic!("{e}"));
    }
    named
        .iter()
        .map(|(name, ty)| Binder {
            name: name.clone(),
            sort: sort_of(*ty, &vars).unwrap_or_else(|e| panic!("{e}")),
            text: String::new(),
        })
        .collect()
}

/// The type's parameters as one constructor's scheme binds them: the arguments its answer is
/// applied to. An argument that is not a variable holds a place no variable is.
fn params_of(scheme: At<'_>) -> Result<Vec<i64>, Error> {
    let ty = scheme.field("ty")?;
    let c = ty.ctor()?;
    let answer = if c.name() == "TyFn" {
        c.arg(0)?.field("ret")?
    } else {
        ty
    };
    let c = answer.ctor()?;
    if c.name() != "TyCon" {
        return Ok(Vec::new());
    }
    c.arg(0)?.field("args")?.items(|arg| {
        let arg = arg.ctor()?;
        Ok(if arg.name() == "TyVar" {
            arg.arg(0)?.int()?
        } else {
            -1
        })
    })
}

/// Every type a constructor of the answer belongs to, and every definition's signature.
pub fn world_of(answer: At<'_>) -> World {
    let read = || -> Result<World, Error> {
        let mut decls: Vec<Decl> = Vec::new();
        for ctor in answer.field("ctors")?.list()? {
            let params = params_of(ctor.field("scheme")?)?;
            let type_name = Symbol::new(ctor.field("type_name")?.utf8()?);
            let variant = Variant {
                name: Symbol::new(ctor.field("name")?.utf8()?),
                index: ctor.field("index")?.number()?,
                fields: ctor.field("fields")?.items(|f| sort_of(f, &params))?,
                depth: None,
            };
            match decls.iter_mut().find(|d| d.name == type_name) {
                Some(decl) => decl.variants.push(variant),
                None => decls.push(Decl {
                    name: type_name,
                    params: params.len(),
                    variants: vec![variant],
                    depth: None,
                }),
            }
        }
        for decl in &mut decls {
            decl.variants.sort_by_key(|v| v.index);
        }
        let signatures = answer.field("defs")?.items(|def| {
            let ty = def.field("scheme")?.field("ty")?;
            let mut vars = Vec::new();
            met(ty, &mut vars)?;
            Ok(Signature {
                name: Symbol::new(def.field("name")?.utf8()?),
                sort: sort_of(ty, &vars)?,
                pure: def.field("footprint")?.list()?.len() == 0,
            })
        })?;
        Ok(World::new(decls, signatures))
    };
    read().unwrap_or_else(|e| panic!("{e}"))
}

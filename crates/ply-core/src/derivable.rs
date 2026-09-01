//! `derivable(D, t)` — the predicate a `derive`, a `where` clause and a `Map` key type all ask, in
//! one implementation.

use crate::ty::{TyVar, Type};
use ply_derive::rules::{Refusal, Shape, shape};
use ply_span::Symbol;
use ply_syntax::ast::Deriver;
use rustc_hash::{FxHashMap, FxHashSet};

/// Why a type has no derivation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// A function has no encoding, no equality and no order.
    Function,
    /// A `Cell` names a location in a world; a `Task` names a slot in a scheduler that dies with
    /// its region.
    Handle(&'static str),
    /// `Float` is derivable for `json` and `eq` and never for `ord`: NaN makes `<` non-total, and a
    /// total order that disagrees with `==` on its own keys is a lookup that fails to find what it
    /// just inserted.
    FloatIsNotOrdered,
    /// A type parameter with no `where derivable(D, ·)` on its signature.
    Unconstrained(TyVar),
    /// An `Option` whose payload also encodes as the JSON document `null`, so `None` and `Some(..)`
    /// write the same bytes.
    NullInsideOption,
    /// A `Secret`, for every deriver but `eq`.
    Secret(Deriver),
}

/// The type that has no derivation — the *field*, not the type it sits in, so a diagnostic can
/// point at the thing the user can change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocked {
    pub ty: Type,
    pub why: Why,
}

/// A sum type's declaration, flattened: the parameters it is written in terms of, and every field
/// of every variant.
pub struct Adt {
    pub params: Vec<TyVar>,
    pub fields: Vec<Type>,
}

/// What the predicate needs of its surroundings, and nothing more.
pub struct Context<'a> {
    /// The declaration of a named sum type.
    pub adt: &'a dyn Fn(&Symbol) -> Option<Adt>,
    /// The type parameters the enclosing signature declared derivable for this deriver.
    pub assumed: &'a FxHashSet<TyVar>,
}

pub fn derivable(d: Deriver, ty: &Type, cx: &Context<'_>) -> Result<(), Blocked> {
    let mut visiting = Vec::new();
    walk(d, ty, cx, &mut visiting)
}

/// Whether a type may be a `Map` key.
pub fn ordered(ty: &Type, cx: &Context<'_>) -> Result<(), Blocked> {
    derivable(Deriver::Ord, ty, cx)
}

fn walk(
    d: Deriver,
    ty: &Type,
    cx: &Context<'_>,
    visiting: &mut Vec<Symbol>,
) -> Result<(), Blocked> {
    match ty {
        Type::Var(v) => {
            if cx.assumed.contains(v) {
                Ok(())
            } else {
                Err(Blocked {
                    ty: ty.clone(),
                    why: Why::Unconstrained(*v),
                })
            }
        }
        Type::Fn { .. } => Err(Blocked {
            ty: ty.clone(),
            why: Why::Function,
        }),
        Type::Record(fields) => {
            for f in fields.values() {
                walk(d, f, cx, visiting)?;
            }
            Ok(())
        }
        Type::Con(name, args) => con(d, ty, name, args, cx, visiting),
    }
}

fn con(
    d: Deriver,
    ty: &Type,
    name: &Symbol,
    args: &[Type],
    cx: &Context<'_>,
    visiting: &mut Vec<Symbol>,
) -> Result<(), Blocked> {
    // Which constructors are leaves, structural or refused is `ply_derive`'s table, because the
    // deriver has to agree with this predicate exactly: one walks a type to *generate* a dictionary
    // and the other to decide whether it may, and a type the first can encode and the second
    // refuses is a contradiction the user cannot resolve.
    match shape(d, name.as_str()) {
        Shape::Leaf => return Ok(()),
        Shape::Refused(Refusal::FloatIsNotOrdered) => {
            return Err(Blocked {
                ty: ty.clone(),
                why: Why::FloatIsNotOrdered,
            });
        }
        Shape::Refused(Refusal::Handle(what)) => {
            return Err(Blocked {
                ty: ty.clone(),
                why: Why::Handle(what),
            });
        }
        Shape::Refused(Refusal::Secret(deriver)) => {
            return Err(Blocked {
                ty: ty.clone(),
                why: Why::Secret(deriver),
            });
        }
        Shape::Structural(_) => {
            // `option_json` writes `None` as `null` and `Some(x)` as `x`, so an inner encoding that
            // can be `null` collapses the two.
            if d == Deriver::Json
                && name.as_str() == ply_derive::rules::OPTION
                && args.first().is_some_and(json_null_encoded)
            {
                return Err(Blocked {
                    ty: ty.clone(),
                    why: Why::NullInsideOption,
                });
            }
            for a in args {
                walk(d, a, cx, visiting)?;
            }
            return Ok(());
        }
        Shape::Nominal => {}
    }
    // A recursive type is derivable exactly when its non-recursive parts are: the codec for `Tree`
    // calls itself, which terminates on a finite value, so revisiting the type under itself is not
    // a reason to refuse.
    if visiting.contains(name) {
        return Ok(());
    }
    let Some(adt) = (cx.adt)(name) else {
        return Ok(());
    };
    let subst: FxHashMap<TyVar, Type> = adt
        .params
        .iter()
        .copied()
        .zip(args.iter().cloned())
        .collect();
    visiting.push(name.clone());
    let result = (|| {
        for f in &adt.fields {
            walk(d, &substitute(f, &subst), cx, visiting)?;
        }
        Ok(())
    })();
    visiting.pop();
    result
}

/// Whether a solved type's JSON encoding is `null` for some value.
fn json_null_encoded(ty: &Type) -> bool {
    matches!(ty, Type::Con(name, _) if ply_derive::rules::json_null_encoded(name.as_str()))
}

fn substitute(ty: &Type, subst: &FxHashMap<TyVar, Type>) -> Type {
    match ty {
        Type::Var(v) => subst.get(v).cloned().unwrap_or_else(|| ty.clone()),
        Type::Con(name, args) => Type::Con(
            name.clone(),
            args.iter().map(|a| substitute(a, subst)).collect(),
        ),
        Type::Fn {
            params,
            ret,
            effects,
        } => Type::Fn {
            params: params.iter().map(|p| substitute(p, subst)).collect(),
            ret: Box::new(substitute(ret, subst)),
            effects: effects.clone(),
        },
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), substitute(v, subst)))
                .collect(),
        ),
    }
}

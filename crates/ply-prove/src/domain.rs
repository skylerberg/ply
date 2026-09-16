//! Finite domains, and the proof that comes from covering one.

use crate::ENUMERATION_BOUND;
use crate::property::TypeWorld;
use ply_eval::{Fixed, Value};
use ply_span::Symbol;
use ply_ty::IntTy;
use ply_ty::{LawBinder, Type};
use std::collections::BTreeMap;

/// A domain small enough to walk, and how to walk it.
#[derive(Clone, Debug)]
pub struct Finite {
    pub types: Vec<Type>,
    /// Cardinality of each binder's type, parallel to `types`.
    sizes: Vec<u64>,
    pub points: u64,
}

impl Finite {
    /// The binders' types rendered as the product they are, for the certificate.
    pub fn name(&self) -> Symbol {
        if self.types.is_empty() {
            return Symbol::new("unit");
        }
        let parts: Vec<String> = self.types.iter().map(|t| t.to_string()).collect();
        Symbol::new(parts.join(" × "))
    }

    /// The `index`-th point, in a fixed order — the first binder varying slowest.
    pub fn point(&self, world: &TypeWorld, index: u64) -> Option<Vec<Value>> {
        let mut rest = index;
        let mut out = Vec::with_capacity(self.types.len());
        for (ty, size) in self.types.iter().zip(&self.sizes).rev() {
            out.push(value_at(ty, world, rest % size)?);
            rest /= size;
        }
        out.reverse();
        Some(out)
    }
}

/// The binders' domain, when every one of them is finite and the product is within budget.
pub fn finite(binders: &[LawBinder], world: &TypeWorld) -> Option<Finite> {
    let mut sizes = Vec::with_capacity(binders.len());
    let mut points: u64 = 1;
    for binder in binders {
        let size = cardinality(&binder.ty, world)?;
        // An empty type makes the whole product empty, and a domain of no points is a vacuity
        // rather than a proof.
        if size == 0 {
            return None;
        }
        points = points.checked_mul(size)?;
        if points > ENUMERATION_BOUND {
            return None;
        }
        sizes.push(size);
    }
    Some(Finite {
        types: binders.iter().map(|b| b.ty.clone()).collect(),
        sizes,
        points,
    })
}

/// How many values inhabit a type, or `None` when it is infinite, unknown, or larger than
/// [`ENUMERATION_BOUND`].
pub fn cardinality(ty: &Type, world: &TypeWorld) -> Option<u64> {
    size_of(ty, world, &mut Vec::new())
}

fn size_of(ty: &Type, world: &TypeWorld, open: &mut Vec<Symbol>) -> Option<u64> {
    match ty {
        // An uninterpreted sort has no cardinality.
        Type::Var(_) => None,
        Type::Fn { .. } => None,
        Type::Record(fields) => fields.values().try_fold(1u64, |acc, f| {
            acc.checked_mul(size_of(f, world, open)?)
                .filter(|n| *n <= ENUMERATION_BOUND)
        }),
        Type::Con(name, args) => match name.as_str() {
            "Unit" => Some(1),
            "Bool" => Some(2),
            // `Float` and `Decimal` are finite sets of machine values and are still not enumerable:
            // a proof by covering 2^64 points is not a proof anybody runs, and claiming a
            // cardinality here would put the whole domain inside `ENUMERATION_BOUND`'s arithmetic.
            "Int" | "String" | "Bytes" | "List" | "Float" | "Decimal" | "Map" => None,
            // A fixed width *is* a finite set, and a small one at the narrow types: `U8` has 256
            // values, which is inside `ENUMERATION_BOUND`, so `forall (b: U8)` is discharged by
            // covering every byte rather than by sampling. Sixty-four bits is a cardinality no
            // `u64` holds and no run would finish.
            n if IntTy::from_name(n).is_some_and(|t| t.bits() < 64) => {
                Some(1u64 << IntTy::from_name(n).expect("just checked").bits())
            }
            n if IntTy::from_name(n).is_some() => None,
            _ => {
                if open.contains(name) {
                    return None;
                }
                let variants = world.variants(name)?;
                open.push(name.clone());
                let total = variants.iter().try_fold(0u64, |acc, variant| {
                    world
                        .fields(name, variant, args)
                        .iter()
                        .try_fold(1u64, |product, field| {
                            product.checked_mul(size_of(field, world, open)?)
                        })
                        .and_then(|n| acc.checked_add(n))
                        .filter(|n| *n <= ENUMERATION_BOUND)
                });
                open.pop();
                total
            }
        },
    }
}

/// The `index`-th value of a finite type, in the order [`cardinality`] counts: constructors in
/// declaration order, then fields left to right with the last varying fastest.
fn value_at(ty: &Type, world: &TypeWorld, index: u64) -> Option<Value> {
    match ty {
        Type::Con(name, args) => match name.as_str() {
            "Unit" => Some(Value::Unit),
            "Bool" => Some(Value::Bool(index == 1)),
            "Int" | "String" | "Bytes" | "List" | "Float" | "Decimal" | "Map" => None,
            // Ascending from the type's smallest value, so an exhaustive run walks the type in
            // its own order.
            n if IntTy::from_name(n).is_some_and(|t| t.bits() < 64) => {
                let t = IntTy::from_name(n).expect("just checked");
                Fixed::of(t, t.min() + i128::from(index)).map(Value::Fixed)
            }
            _ => {
                let variants = world.variants(name)?;
                let mut rest = index;
                for variant in variants {
                    let fields = world.fields(name, variant, args);
                    let size = fields
                        .iter()
                        .try_fold(1u64, |acc, f| acc.checked_mul(cardinality(f, world)?))?;
                    if rest < size {
                        return Some(Value::Ctor {
                            name: variant.name.clone(),
                            args: std::sync::Arc::new(tuple_at(&fields, world, rest)?),
                        });
                    }
                    rest -= size;
                }
                None
            }
        },
        Type::Record(fields) => {
            let types: Vec<Type> = fields.values().cloned().collect();
            let values = tuple_at(&types, world, index)?;
            let map: BTreeMap<Symbol, Value> = fields.keys().cloned().zip(values).collect();
            Some(Value::Record(std::sync::Arc::new(
                map.into_iter().collect(),
            )))
        }
        Type::Var(_) | Type::Fn { .. } => None,
    }
}

/// One point of a product of finite types, with the last varying fastest.
fn tuple_at(types: &[Type], world: &TypeWorld, index: u64) -> Option<Vec<Value>> {
    let mut rest = index;
    let mut out = Vec::with_capacity(types.len());
    for ty in types.iter().rev() {
        let size = cardinality(ty, world)?;
        out.push(value_at(ty, world, rest % size)?);
        rest /= size;
    }
    out.reverse();
    Some(out)
}

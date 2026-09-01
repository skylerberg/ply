//! Finite domains, and the proof that comes from covering one.

use crate::ENUMERATION_BOUND;
use crate::property::TypeWorld;
use ply_core::{LawBinder, Type};
use ply_eval::Value;
use ply_span::Symbol;
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
            Some(Value::Record(std::sync::Arc::new(map)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use ply_core::{CtorInfo, Scheme};
    use ply_span::Span;

    fn con(name: &str) -> Type {
        Type::Con(Symbol::new(name), Vec::new())
    }

    fn ctor(ty: &str, name: &str, index: usize, fields: Vec<Type>) -> CtorInfo {
        CtorInfo {
            name: Symbol::new(name),
            module: ply_syntax::ast::ModuleName::anonymous(),
            simple_name: Symbol::new(name),
            type_name: Symbol::new(ty),
            index,
            arity: fields.len(),
            fields,
            scheme: Scheme::mono(Type::Con(Symbol::new(ty), Vec::new())),
            span: Span::DUMMY,
        }
    }

    fn binder(name: &str, ty: Type) -> LawBinder {
        LawBinder {
            name: Symbol::new(name),
            ty,
            span: Span::DUMMY,
        }
    }

    fn kinds() -> TypeWorld {
        let ctors = vec![
            ctor("Kind", "Asset", 0, Vec::new()),
            ctor("Kind", "Liability", 1, Vec::new()),
            ctor("Kind", "Equity", 2, Vec::new()),
        ];
        TypeWorld::new(&ctors)
    }

    #[test]
    fn a_nullary_enum_and_bool_are_finite_and_everything_unbounded_is_not() {
        let world = kinds();
        assert_eq!(cardinality(&con("Bool"), &world), Some(2));
        assert_eq!(cardinality(&con("Unit"), &world), Some(1));
        assert_eq!(cardinality(&con("Kind"), &world), Some(3));
        assert_eq!(cardinality(&con("Int"), &world), None);
        assert_eq!(cardinality(&con("String"), &world), None);
        assert_eq!(
            cardinality(&Type::Con(Symbol::new("List"), vec![con("Bool")]), &world),
            None
        );
    }

    /// An uninterpreted sort has no cardinality.
    #[test]
    fn a_type_variable_is_never_finite() {
        assert_eq!(cardinality(&Type::Var(ply_core::TyVar(0)), &kinds()), None);
    }

    /// A type in a constructor cycle has values of every nesting depth, so it has no finite domain
    /// to cover — which is exactly where induction would be needed and is not available.
    #[test]
    fn a_recursive_type_is_infinite_even_with_a_nullary_base_case() {
        let ctors = vec![
            ctor("Nat", "Zero", 0, Vec::new()),
            ctor("Nat", "Succ", 1, vec![con("Nat")]),
        ];
        assert_eq!(cardinality(&con("Nat"), &TypeWorld::new(&ctors)), None);
    }

    #[test]
    fn a_product_beyond_the_bound_is_refused_rather_than_walked() {
        let world = kinds();
        let wide: Vec<LawBinder> = (0..16)
            .map(|i| binder(&format!("b{i}"), con("Kind")))
            .collect();
        assert!(finite(&wide, &world).is_none(), "3^16 is past the bound");

        let narrow: Vec<LawBinder> = (0..4)
            .map(|i| binder(&format!("b{i}"), con("Kind")))
            .collect();
        assert_eq!(finite(&narrow, &world).map(|d| d.points), Some(81));
    }

    /// A ground claim is the degenerate finite domain: one point, the empty tuple, and no way to
    /// miss any of it.
    #[test]
    fn a_ground_claim_has_exactly_one_point() {
        let domain = finite(&[], &kinds()).expect("no binders is a finite domain");
        assert_eq!(domain.points, 1);
        assert_eq!(domain.name().as_str(), "unit");
        assert_eq!(domain.point(&kinds(), 0).map(|p| p.len()), Some(0));
    }

    /// Every point exactly once, in an order two runs agree on — a refutation found here reports
    /// its point as the counterexample with no shrinking.
    #[test]
    fn enumeration_covers_the_domain_once_each_in_a_fixed_order() {
        let world = kinds();
        let binders = [binder("b", con("Bool")), binder("k", con("Kind"))];
        let domain = finite(&binders, &world).expect("both types are finite");
        assert_eq!(domain.points, 6);

        let points: Vec<Vec<Value>> = (0..domain.points)
            .map(|i| domain.point(&world, i).expect("within the domain"))
            .collect();
        let show = |p: &Vec<Value>| p.iter().map(Value::render).collect::<Vec<_>>().join(", ");
        assert_eq!(show(&points[0]), "false, Asset");
        assert_eq!(show(&points[3]), "true, Asset");
        assert_eq!(show(&points[5]), "true, Equity");

        let mut rendered: Vec<String> = points.iter().map(show).collect();
        rendered.sort();
        rendered.dedup();
        assert_eq!(rendered.len(), 6, "a point was visited twice");
    }
}

//! A finite domain as `proof.domain` measured it, and the values its points are.

use ply_eval::decode::{At, Error};
use ply_eval::{Fixed, IntTy, Symbol, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What a program measured of one binder's type: how its values are built from an index, and how
/// many each node holds. The sizes are the program's decision; this side only decodes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A builtin by name: `Unit`, `Bool` or a fixed-width integer.
    Scalar { name: String, size: u64 },
    /// A declared type's constructors, in declaration order.
    Cases { size: u64, cases: Vec<Case> },
    /// A record's fields, in the order the program measured them.
    Fields {
        size: u64,
        fields: Vec<(Symbol, Shape)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    pub name: Symbol,
    pub size: u64,
    pub fields: Vec<Shape>,
}

impl Shape {
    pub fn size(&self) -> u64 {
        match self {
            Shape::Scalar { size, .. } | Shape::Cases { size, .. } | Shape::Fields { size, .. } => {
                *size
            }
        }
    }

    /// A `proof.domain.Shape`.
    pub fn decode(at: At<'_>) -> Result<Shape, Error> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            let shape = at.ctor()?;
            match shape.name() {
                "Scalar" => Ok(Shape::Scalar {
                    name: shape.arg(0)?.str()?.to_string(),
                    size: shape.arg(1)?.number()?,
                }),
                "Cases" => Ok(Shape::Cases {
                    size: shape.arg(0)?.number()?,
                    cases: shape.arg(1)?.items(|case| {
                        Ok(Case {
                            name: Symbol::new(case.field("name")?.str()?),
                            size: case.field("size")?.number()?,
                            fields: case.field("fields")?.items(Shape::decode)?,
                        })
                    })?,
                }),
                "Fields" => Ok(Shape::Fields {
                    size: shape.arg(0)?.number()?,
                    fields: shape.arg(1)?.items(|field| {
                        Ok((
                            Symbol::new(field.field("name")?.str()?),
                            Shape::decode(field.field("shape")?)?,
                        ))
                    })?,
                }),
                _ => Err(shape.unknown()),
            }
        })
    }

    /// The `index`-th value, in the order the program counted: constructors in declaration order,
    /// then fields left to right with the last varying fastest.
    fn value_at(&self, index: u64) -> Option<Value> {
        match self {
            Shape::Scalar { name, .. } => match name.as_str() {
                "Unit" => Some(Value::Unit),
                "Bool" => Some(Value::Bool(index == 1)),
                width => {
                    let t = IntTy::from_name(width)?;
                    Fixed::of(t, t.min() + i128::from(index)).map(Value::Fixed)
                }
            },
            Shape::Cases { cases, .. } => {
                let mut rest = index;
                for case in cases {
                    if rest < case.size {
                        return Some(Value::Ctor {
                            name: case.name.clone(),
                            args: Arc::new(tuple_at(&case.fields, rest)?),
                        });
                    }
                    rest -= case.size;
                }
                None
            }
            Shape::Fields { fields, .. } => {
                let shapes: Vec<Shape> = fields.iter().map(|(_, s)| s.clone()).collect();
                let values = tuple_at(&shapes, index)?;
                let map: BTreeMap<Symbol, Value> = fields
                    .iter()
                    .map(|(name, _)| name.clone())
                    .zip(values)
                    .collect();
                Some(Value::Record(Arc::new(map.into_iter().collect())))
            }
        }
    }
}

/// A domain the program decided to walk: each binder's shape, how many points there are, and
/// what an artifact calls it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finite {
    /// One per binder.
    pub shapes: Vec<Shape>,
    pub points: u64,
    pub name: Symbol,
}

impl Finite {
    /// A `proof.domain.Domain` and its name. A count the shapes do not multiply out to would walk a
    /// point twice or miss one, so it is refused rather than walked.
    pub fn decode(domain: At<'_>, name: At<'_>) -> Result<Finite, Error> {
        let shapes = domain.field("shapes")?.items(Shape::decode)?;
        let count = domain.field("points")?;
        let points: u64 = count.number()?;
        let product = shapes
            .iter()
            .try_fold(1u64, |acc, shape| acc.checked_mul(shape.size()));
        if points == 0 {
            return Err(count.error("a domain of no points"));
        }
        if product != Some(points) {
            return Err(count.error(format!(
                "{points} points, which the binders' shapes do not multiply out to"
            )));
        }
        Ok(Finite {
            shapes,
            points,
            name: Symbol::new(name.str()?),
        })
    }

    /// The `index`-th point, in a fixed order — the first binder varying slowest — and `None` past
    /// the last, where decoding would wrap round to a point already named.
    pub fn point(&self, index: u64) -> Option<Vec<Value>> {
        if index >= self.points {
            return None;
        }
        tuple_at(&self.shapes, index)
    }
}

fn tuple_at(shapes: &[Shape], index: u64) -> Option<Vec<Value>> {
    let mut rest = index;
    let mut out = Vec::with_capacity(shapes.len());
    for shape in shapes.iter().rev() {
        let size = shape.size();
        if size == 0 {
            return None;
        }
        out.push(shape.value_at(rest % size)?);
        rest /= size;
    }
    out.reverse();
    Some(out)
}

//! Finite domains, and the proof that comes from covering one.

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

#[derive(Clone, Debug)]
pub struct Finite {
    /// One per binder.
    shapes: Vec<Shape>,
    pub points: u64,
}

impl Finite {
    /// The domain the program measured; `None` when it has no points, which is a vacuity rather than
    /// a domain.
    pub fn of_shapes(shapes: Vec<Shape>) -> Option<Finite> {
        let points = shapes
            .iter()
            .try_fold(1u64, |acc, s| acc.checked_mul(s.size()))?;
        if points == 0 {
            return None;
        }
        Some(Finite { shapes, points })
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

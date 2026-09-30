//! What the prover reasons a value's type with, as `proof.world` hands one over.

use ply_eval::Symbol;
use ply_eval::decode::{At, Error};

/// A type as the prover reads one. A variable is numbered by where it first appears in the item it
/// belongs to, so one law's `Var(0)` and one signature's are unrelated.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Sort {
    Var(u32),
    Con(Symbol, Vec<Sort>),
    Fn {
        params: Vec<Sort>,
        ret: Box<Sort>,
        /// The function's row is empty.
        pure: bool,
    },
    /// Ascending by field name, each name once.
    Record(Vec<(Symbol, Sort)>),
}

impl Sort {
    pub fn con(name: &str) -> Sort {
        Sort::Con(Symbol::new(name), Vec::new())
    }

    pub fn int() -> Sort {
        Sort::con("Int")
    }

    pub fn bool() -> Sort {
        Sort::con("Bool")
    }

    pub fn string() -> Sort {
        Sort::con("String")
    }

    pub fn bytes() -> Sort {
        Sort::con("Bytes")
    }

    pub fn unit() -> Sort {
        Sort::con("Unit")
    }

    pub fn float() -> Sort {
        Sort::con("Float")
    }

    pub fn decimal() -> Sort {
        Sort::con("Decimal")
    }

    pub fn list(elem: Sort) -> Sort {
        Sort::Con(Symbol::new("List"), vec![elem])
    }

    pub fn map(key: Sort, value: Sort) -> Sort {
        Sort::Con(Symbol::new("Map"), vec![key, value])
    }

    pub fn func(params: Vec<Sort>, ret: Sort, pure: bool) -> Sort {
        Sort::Fn {
            params,
            ret: Box::new(ret),
            pure,
        }
    }

    /// Sorted by name; a later field of a name replaces an earlier one.
    pub fn record(fields: impl IntoIterator<Item = (Symbol, Sort)>) -> Sort {
        let mut out: Vec<(Symbol, Sort)> = Vec::new();
        for (name, sort) in fields {
            match out.binary_search_by(|(held, _)| held.cmp(&name)) {
                Ok(at) => out[at].1 = sort,
                Err(at) => out.insert(at, (name, sort)),
            }
        }
        Sort::Record(out)
    }

    /// A named type of no arguments called `name`.
    pub fn is_con(&self, name: &str) -> bool {
        matches!(self, Sort::Con(n, args) if n.as_str() == name && args.is_empty())
    }

    pub fn list_elem(&self) -> Option<&Sort> {
        match self {
            Sort::Con(name, args) if name.as_str() == "List" && args.len() == 1 => Some(&args[0]),
            _ => None,
        }
    }

    pub fn field(&self, name: &Symbol) -> Option<&Sort> {
        match self {
            Sort::Record(fields) => fields.iter().find(|(n, _)| n == name).map(|(_, s)| s),
            _ => None,
        }
    }

    /// Each variable once, in the order they first appear.
    pub fn vars(&self, out: &mut Vec<u32>) {
        match self {
            Sort::Var(v) => {
                if !out.contains(v) {
                    out.push(*v);
                }
            }
            Sort::Con(_, args) => args.iter().for_each(|a| a.vars(out)),
            Sort::Fn { params, ret, .. } => {
                params.iter().for_each(|p| p.vars(out));
                ret.vars(out);
            }
            Sort::Record(fields) => fields.iter().for_each(|(_, f)| f.vars(out)),
        }
    }

    /// `args` in place of the variables they number; a variable past them stays.
    pub fn substituted(&self, args: &[Sort]) -> Sort {
        match self {
            Sort::Var(v) => args.get(*v as usize).cloned().unwrap_or(Sort::Var(*v)),
            Sort::Con(name, xs) => Sort::Con(
                name.clone(),
                xs.iter().map(|x| x.substituted(args)).collect(),
            ),
            Sort::Fn { params, ret, pure } => Sort::Fn {
                params: params.iter().map(|p| p.substituted(args)).collect(),
                ret: Box::new(ret.substituted(args)),
                pure: *pure,
            },
            Sort::Record(fields) => Sort::Record(
                fields
                    .iter()
                    .map(|(n, f)| (n.clone(), f.substituted(args)))
                    .collect(),
            ),
        }
    }

    /// A `proof.domain.Ty`.
    pub fn decode(at: At<'_>) -> Result<Sort, Error> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            let ty = at.ctor()?;
            match ty.name() {
                "Var" => Ok(Sort::Var(ty.arg(0)?.number()?)),
                "Con" => Ok(Sort::Con(
                    Symbol::new(ty.arg(0)?.str()?),
                    ty.arg(1)?.items(Sort::decode)?,
                )),
                "Fn" => Ok(Sort::func(
                    ty.arg(0)?.items(Sort::decode)?,
                    Sort::decode(ty.arg(1)?)?,
                    ty.arg(2)?.bool()?,
                )),
                "Record" => {
                    let listed = ty.arg(0)?;
                    let mut fields: Vec<(Symbol, Sort)> = Vec::new();
                    for field in listed.list()? {
                        let name = Symbol::new(field.field("name")?.str()?);
                        if fields.iter().any(|(held, _)| *held == name) {
                            return Err(field.error(format!("a second field named `{name}`")));
                        }
                        fields.push((name, Sort::decode(field.field("ty")?)?));
                    }
                    Ok(Sort::record(fields))
                }
                _ => Err(ty.unknown()),
            }
        })
    }
}

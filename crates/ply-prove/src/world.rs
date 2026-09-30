//! The program as the prover reads it — its declared types, its definitions' signatures and the
//! obligations it owes — as `proof.world` builds it.

use crate::sort::Sort;
use crate::{Binder, Obligation, ObligationKind};
use ply_eval::decode::{At, Error};
use ply_eval::{IntTy, SECRET, TASK_TYPE};
use ply_span::{SourceId, Span, Symbol};
use ply_ty::DefHash;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct Variant {
    pub name: Symbol,
    /// Its place among its type's variants, which is declaration order.
    pub index: usize,
    /// In the owning type's parameters: `Var(i)` is the `i`th.
    pub fields: Vec<Sort>,
    /// Nested constructor applications a value of this variant needs.
    pub depth: Option<u64>,
}

/// A declared type: every variant its constructors belong to.
#[derive(Clone, Debug)]
pub struct Decl {
    pub name: Symbol,
    pub params: usize,
    pub variants: Vec<Variant>,
    pub depth: Option<u64>,
}

impl Decl {
    /// The variants in declaration order, each a name and its fields.
    pub fn new(name: &str, params: usize, variants: Vec<(&str, Vec<Sort>)>) -> Decl {
        Decl {
            name: Symbol::new(name),
            params,
            variants: variants
                .into_iter()
                .enumerate()
                .map(|(index, (variant, fields))| Variant {
                    name: Symbol::new(variant),
                    index,
                    fields,
                    depth: None,
                })
                .collect(),
            depth: None,
        }
    }

    /// The type over its own parameters.
    pub fn sort(&self) -> Sort {
        Sort::Con(
            self.name.clone(),
            (0..self.params as u32).map(Sort::Var).collect(),
        )
    }
}

/// A constructor, beside the type it builds.
#[derive(Clone, Copy, Debug)]
pub struct Ctor<'w> {
    pub decl: &'w Decl,
    pub variant: &'w Variant,
}

impl Ctor<'_> {
    pub fn arity(&self) -> usize {
        self.variant.fields.len()
    }

    /// As a value: the type it builds, or a pure function to it from its fields.
    pub fn sort(&self) -> Sort {
        if self.variant.fields.is_empty() {
            self.decl.sort()
        } else {
            Sort::func(self.variant.fields.clone(), self.decl.sort(), true)
        }
    }
}

/// A definition as a callee.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Signature {
    pub name: Symbol,
    pub sort: Sort,
    /// Its published row is empty.
    pub pure: bool,
}

#[derive(Clone, Debug, Default)]
pub struct World {
    types: BTreeMap<Symbol, Decl>,
    /// Each constructor's type, and its place among that type's variants.
    ctors: BTreeMap<Symbol, (Symbol, usize)>,
    signatures: BTreeMap<Symbol, Signature>,
}

impl World {
    pub fn new(
        decls: impl IntoIterator<Item = Decl>,
        signatures: impl IntoIterator<Item = Signature>,
    ) -> World {
        let mut world = World::default();
        for mut decl in decls {
            decl.depth = None;
            for (index, variant) in decl.variants.iter_mut().enumerate() {
                variant.index = index;
                variant.depth = None;
                world
                    .ctors
                    .insert(variant.name.clone(), (decl.name.clone(), index));
            }
            world.types.insert(decl.name.clone(), decl);
        }
        for signature in signatures {
            world.signatures.insert(signature.name.clone(), signature);
        }
        world.solve_depths();
        world
    }

    /// A `proof.world.World`, and its obligations in the order the program listed them, which is
    /// the order it names them by.
    pub fn decode(at: At<'_>) -> Result<(World, Vec<Obligation>), Error> {
        let mut decls: Vec<Decl> = Vec::new();
        let mut types: BTreeSet<Symbol> = BTreeSet::new();
        let mut ctors: BTreeSet<Symbol> = BTreeSet::new();
        for item in at.field("decls")?.list()? {
            let decl = decl(item)?;
            if !types.insert(decl.name.clone()) {
                return Err(item.error(format!("a second declaration of `{}`", decl.name)));
            }
            for variant in &decl.variants {
                if !ctors.insert(variant.name.clone()) {
                    return Err(item.error(format!(
                        "`{}` is a constructor of a type declared before it",
                        variant.name
                    )));
                }
            }
            decls.push(decl);
        }
        let mut signatures: Vec<Signature> = Vec::new();
        let mut named: BTreeSet<Symbol> = BTreeSet::new();
        for item in at.field("signatures")?.list()? {
            let signature = Signature {
                name: Symbol::new(item.field("name")?.str()?),
                sort: Sort::decode(item.field("ty")?)?,
                pure: item.field("pure")?.bool()?,
            };
            if !named.insert(signature.name.clone()) {
                return Err(item.error(format!("a second signature of `{}`", signature.name)));
            }
            signatures.push(signature);
        }
        let obligations = at.field("obligations")?.items(obligation)?;
        Ok((World::new(decls, signatures), obligations))
    }

    pub fn decls(&self) -> impl Iterator<Item = &Decl> {
        self.types.values()
    }

    pub fn decl(&self, ty: &Symbol) -> Option<&Decl> {
        self.types.get(ty)
    }

    pub fn variants(&self, ty: &Symbol) -> Option<&[Variant]> {
        self.types.get(ty).map(|d| d.variants.as_slice())
    }

    pub fn ctor(&self, name: &Symbol) -> Option<Ctor<'_>> {
        let (ty, index) = self.ctors.get(name)?;
        let decl = self.types.get(ty)?;
        Some(Ctor {
            decl,
            variant: decl.variants.get(*index)?,
        })
    }

    /// The variant's fields with `args` in place of the type's parameters.
    pub fn fields(&self, variant: &Variant, args: &[Sort]) -> Vec<Sort> {
        variant.fields.iter().map(|f| f.substituted(args)).collect()
    }

    pub fn signature(&self, name: &Symbol) -> Option<&Signature> {
        self.signatures.get(name)
    }

    fn solve_depths(&mut self) {
        let names: Vec<Symbol> = self.types.keys().cloned().collect();
        // Each round settles at least one type, so one round per type suffices.
        for _ in 0..=names.len() {
            let mut changed = false;
            for name in &names {
                let depths: Vec<Option<u64>> = self.types[name]
                    .variants
                    .iter()
                    .map(|variant| {
                        variant
                            .fields
                            .iter()
                            .try_fold(0u64, |acc, field| self.depth(field).map(|d| acc.max(d)))
                            .map(|d| d.saturating_add(1))
                    })
                    .collect();
                let best = depths.iter().flatten().copied().min();
                let decl = self
                    .types
                    .get_mut(name)
                    .expect("the name came from this map");
                if decl.depth != best {
                    decl.depth = best;
                    changed = true;
                }
                for (variant, depth) in decl.variants.iter_mut().zip(depths) {
                    if variant.depth != depth {
                        variant.depth = depth;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Nested constructor applications a value of `sort` needs, or `None` when nothing finite
    /// inhabits it.
    pub fn depth(&self, sort: &Sort) -> Option<u64> {
        match sort {
            Sort::Var(_) => Some(0),
            Sort::Record(fields) => fields
                .iter()
                .try_fold(0u64, |acc, (_, f)| self.depth(f).map(|d| acc.max(d))),
            Sort::Fn {
                ret, pure: true, ..
            } => self.depth(ret),
            Sort::Fn { .. } => None,
            Sort::Con(name, _) => match name.as_str() {
                "Int" | "Bool" | "String" | "Bytes" | "Unit" | "Float" | "Decimal" => Some(0),
                n if IntTy::from_name(n).is_some() => Some(0),
                "List" | "Map" => Some(0),
                "Cell" => None,
                n if n == TASK_TYPE || n == SECRET => None,
                _ => self.types.get(name).and_then(|d| d.depth),
            },
        }
    }
}

/// A `proof.domain.Decl`: its variants' fields name only its own parameters.
fn decl(at: At<'_>) -> Result<Decl, Error> {
    let params: usize = at.field("params")?.number()?;
    let mut variants = Vec::new();
    for (index, item) in at.field("variants")?.list()?.enumerate() {
        let mut fields = Vec::new();
        for field in item.field("fields")?.list()? {
            let sort = Sort::decode(field)?;
            let mut vars = Vec::new();
            sort.vars(&mut vars);
            if let Some(v) = vars.iter().find(|v| **v as usize >= params) {
                return Err(field.error(format!(
                    "a field over parameter {v} of a type that takes {params}"
                )));
            }
            fields.push(sort);
        }
        variants.push(Variant {
            name: Symbol::new(item.field("name")?.str()?),
            index,
            fields,
            depth: None,
        });
    }
    Ok(Decl {
        name: Symbol::new(at.field("name")?.str()?),
        params,
        variants,
        depth: None,
    })
}

/// A `proof.world.Obligation`.
fn obligation(at: At<'_>) -> Result<Obligation, Error> {
    let key = at.field("key")?;
    let text = key.str()?;
    let kind = at.field("kind")?.ctor()?;
    let place = at.field("at")?;
    Ok(Obligation {
        key: DefHash::from_hex(text)
            .ok_or_else(|| key.error(format!("`{text}` is not a hash in hex")))?,
        owner: Symbol::new(at.field("owner")?.str()?),
        kind: match kind.name() {
            "Ensures" => ObligationKind::Ensures {
                index: kind.arg(0)?.number()?,
            },
            "Law" => ObligationKind::Law,
            _ => return Err(kind.unknown()),
        },
        span: Span::new(
            SourceId(place.field("module")?.number()?),
            place.field("start")?.number()?,
            place.field("end")?.number()?,
        ),
        binders: at.field("binders")?.items(|b| {
            Ok(Binder {
                name: Symbol::new(b.field("name")?.str()?),
                sort: Sort::decode(b.field("ty")?)?,
                text: b.field("text")?.str()?.to_string(),
            })
        })?,
        guarded: at.field("guarded")?.bool()?,
        host: at.field("host")?.bool()?,
        footprint: match at.field("footprint")?.option()? {
            Some(row) => Some(row.str()?.to_string()),
            None => None,
        },
    })
}

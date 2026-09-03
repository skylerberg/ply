//! The property tier: drawing a value of every Ply type from a seed, running an obligation's cases
//! against its guard, and reporting what the guard let through.

#[cfg(test)]
pub(crate) mod tests;

use crate::shrink::{self, Target};
use crate::{
    Binding, CaseReport, Counterexample, Discharge, Evidence, GEN_DEPTH, Gap, ProvePlan, Vacuity,
    VacuityKind,
};
use ply_core::ty::IntTy;
use ply_core::{CtorInfo, Row, TyVar, Type};
use ply_core::{LawBinder, prelude};
use ply_eval::{Closure, ClosureKind, Decimal, Fixed, Value};
use ply_hash::DefHash;
use ply_span::{Diagnostic, Span, Symbol};
use ply_syntax::ast::{BinOp, Expr, ExprKind, Ident, QName};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

/// Case indices below this draw the **edge** of their type rather than a biased sample: an `Int`
/// takes each of [`EDGE_INTS`] in turn.
pub const EDGE_CASES: u32 = 5;

/// Drawn first, in this order, one per edge case index.
pub const EDGE_INTS: [i64; 5] = [0, 1, -1, i64::MIN, i64::MAX];

/// The `Float` edge, and the *whole* edge on purpose.
pub const EDGE_FLOATS: [f64; 8] = [
    f64::NAN,
    0.0,
    -0.0,
    1.0,
    -1.0,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::MAX,
];

/// How many `Decimal` edge points [`edge_decimal`] offers.
const EDGE_DECIMAL_COUNT: usize = 6;

/// The `Decimal` edge: zero at two scales, one, minus one, and the ends of the range where an exact
/// addition overflows.
fn edge_decimal(index: usize) -> Decimal {
    match index {
        0 => Decimal::ZERO,
        1 => Decimal::ONE,
        2 => -Decimal::ONE,
        // `0.00m`, so the difference between a value and its scale is drawn.
        3 => Decimal::new(0, 2),
        4 => Decimal::MAX,
        _ => Decimal::MIN,
    }
}

/// Sixteen characters starting at `'a'`, because the shrinker lowers a character toward `'a'` and a
/// character outside the alphabet would shrink into it.
pub const GEN_ALPHABET: &[u8; 16] = b"abcdefghijklmnop";

/// The longest `List` or `String` a draw produces.
pub const MAX_GEN_LEN: u64 = 16;

/// The most entries a generated `Map` holds.
pub const MAX_GEN_ENTRIES: usize = 8;

/// An absolute ceiling on how deep generation may nest, independent of [`GEN_DEPTH`].
pub const HARD_GEN_DEPTH: u32 = 64;

const GEN_DOMAIN: &[u8] = b"ply.gen.stream.1";

/// Env slots on a generated function value.
const FN_SIZE: &str = "#size";
const FN_CONST: &str = "#c";
const FN_DEFAULT: &str = "#d";

/// The value source: counter-mode BLAKE3, keyed by the root **and** the obligation.
#[derive(Clone, Debug)]
pub struct GenStream {
    root: u64,
    key: DefHash,
    counter: u64,
}

impl GenStream {
    pub fn new(root: u64, key: DefHash) -> GenStream {
        GenStream::at(root, key, 0)
    }

    /// A stream that has already served `counter` draws.
    pub fn at(root: u64, key: DefHash, counter: u64) -> GenStream {
        GenStream { root, key, counter }
    }

    pub fn next_u64(&mut self) -> u64 {
        let value = GenStream::draw(self.root, &self.key, self.counter);
        self.counter = self.counter.wrapping_add(1);
        value
    }

    /// Uniform over `0..n` by rejection, specified exactly rather than described as unbiased: a
    /// different unbiased rule is a different sequence, and every `(root, case)` ever printed would
    /// name a different tuple.
    pub fn below(&mut self, n: u64) -> Option<u64> {
        if n == 0 {
            return None;
        }
        let limit = (u64::MAX / n) * n;
        loop {
            let x = self.next_u64();
            if x < limit {
                return Some(x % n);
            }
        }
    }

    pub fn drawn(&self) -> u64 {
        self.counter
    }

    pub fn root(&self) -> u64 {
        self.root
    }

    /// Pure, so a caller replaying a recorded run can ask for draw *i* without having served the
    /// ones before it.
    pub fn draw(root: u64, key: &DefHash, counter: u64) -> u64 {
        let mut hasher = blake3::Hasher::new();
        hasher.update(GEN_DOMAIN);
        hasher.update(&root.to_le_bytes());
        hasher.update(&key.0);
        hasher.update(&counter.to_le_bytes());
        let bytes = hasher.finalize();
        u64::from_le_bytes(
            bytes.as_bytes()[..8]
                .try_into()
                .expect("blake3 is 32 bytes"),
        )
    }
}

/// One variant of a sum type, as generation and shrinking need it.
#[derive(Clone, Debug)]
pub struct Variant {
    /// Program-wide constructor name, which is what a [`Value::Ctor`] carries.
    pub name: Symbol,
    /// Position among the owning type's variants, in declaration order.
    pub index: usize,
    /// Declared field types, still written in the owning type's parameters.
    pub fields: Vec<Type>,
    /// Nested constructor applications a value of this variant needs, at declaration level.
    pub depth: Option<u64>,
}

#[derive(Clone, Debug)]
struct TypeDecl {
    /// The owning type's parameters, in declaration order, so a `Type::Con`'s arguments can be
    /// substituted into a variant's field types.
    params: Vec<TyVar>,
    variants: Vec<Variant>,
    depth: Option<u64>,
}

/// What generation and shrinking need to know about the program's sum types.
#[derive(Clone, Debug, Default)]
pub struct TypeWorld {
    types: BTreeMap<Symbol, TypeDecl>,
    /// Program-wide constructor name -> (owning type, position).
    ctors: BTreeMap<Symbol, (Symbol, usize)>,
}

impl TypeWorld {
    pub fn new<'a>(ctors: impl IntoIterator<Item = &'a CtorInfo>) -> TypeWorld {
        let mut world = TypeWorld::default();
        for info in ctors {
            let decl = world
                .types
                .entry(info.type_name.clone())
                .or_insert_with(|| TypeDecl {
                    params: info.scheme.ty_vars.clone(),
                    variants: Vec::new(),
                    depth: None,
                });
            decl.variants.push(Variant {
                name: info.name.clone(),
                index: info.index,
                fields: info.fields.clone(),
                depth: None,
            });
            world
                .ctors
                .insert(info.name.clone(), (info.type_name.clone(), info.index));
        }
        for decl in world.types.values_mut() {
            decl.variants.sort_by_key(|v| v.index);
        }
        world.solve_depths();
        world
    }

    /// Least fixpoint of "how deep must a value of this type nest".
    fn solve_depths(&mut self) {
        let names: Vec<Symbol> = self.types.keys().cloned().collect();
        // Each round settles at least one type or the fixpoint is reached, so one round per type is
        // enough.
        for _ in 0..=names.len() {
            let mut changed = false;
            for name in &names {
                let variants = self.types[name].variants.clone();
                let mut best: Option<u64> = None;
                let mut depths: Vec<Option<u64>> = Vec::with_capacity(variants.len());
                for variant in &variants {
                    let depth = variant
                        .fields
                        .iter()
                        .try_fold(0u64, |acc, field| {
                            self.type_depth(field).map(|d| acc.max(d))
                        })
                        .map(|d| d.saturating_add(1));
                    depths.push(depth);
                    if let Some(d) = depth {
                        best = Some(best.map_or(d, |b: u64| b.min(d)));
                    }
                }
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

    /// Nested constructor applications a value of this type needs, given what is known so far.
    fn type_depth(&self, ty: &Type) -> Option<u64> {
        match ty {
            Type::Var(_) => Some(0),
            Type::Record(fields) => fields
                .values()
                .try_fold(0u64, |acc, f| self.type_depth(f).map(|d| acc.max(d))),
            Type::Fn { ret, effects, .. } if effects.is_pure() => self.type_depth(ret),
            Type::Fn { .. } => None,
            Type::Con(name, _) => match name.as_str() {
                "Int" | "Bool" | "String" | "Bytes" | "Unit" | "Float" | "Decimal" => Some(0),
                n if IntTy::from_name(n).is_some() => Some(0),
                // The empty collection needs nothing, whatever it holds.
                "List" | "Map" => Some(0),
                "Cell" => None,
                _ if name.as_str() == prelude::TASK_TYPE => None,
                _ if name.as_str() == ply_core::ty::SECRET => None,
                _ => self.types.get(name).and_then(|d| d.depth),
            },
        }
    }

    pub fn variants(&self, ty: &Symbol) -> Option<&[Variant]> {
        self.types.get(ty).map(|d| d.variants.as_slice())
    }

    /// The owning type and the position of a constructor, by its program-wide name — what a
    /// [`Value::Ctor`] carries and the shrinker needs back.
    pub fn ctor(&self, name: &Symbol) -> Option<(&Symbol, usize)> {
        self.ctors.get(name).map(|(ty, index)| (ty, *index))
    }

    /// A variant's field types with a `Type::Con`'s arguments substituted for the owning type's
    /// parameters.
    pub fn fields(&self, ty: &Symbol, variant: &Variant, args: &[Type]) -> Vec<Type> {
        let Some(decl) = self.types.get(ty) else {
            return variant.fields.clone();
        };
        let subst: BTreeMap<TyVar, Type> = decl
            .params
            .iter()
            .copied()
            .zip(args.iter().cloned())
            .collect();
        variant
            .fields
            .iter()
            .map(|f| substitute(f, &subst))
            .collect()
    }
}

fn substitute(ty: &Type, subst: &BTreeMap<TyVar, Type>) -> Type {
    match ty {
        Type::Var(v) => subst.get(v).cloned().unwrap_or_else(|| ty.clone()),
        Type::Con(name, args) => Type::Con(
            name.clone(),
            args.iter().map(|a| substitute(a, subst)).collect(),
        ),
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), substitute(v, subst)))
                .collect(),
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
    }
}

/// Why no value of a type can be drawn.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ungeneratable {
    /// A cell belongs to the region that opened it, and an obligation opens none.
    Cell,
    /// A task belongs to a `simulate` region, and a binder is not one.
    Task,
    /// A credential.
    Secret,
    /// A function type with a non-empty row: applying it inside a spec would make the spec impure,
    /// so the binder would be unusable.
    Effectful(Row),
    /// An unsolved effect-row tail, which is not pure for every instantiation.
    RowVariable,
    /// Every constructor of this type needs a value of a type that needs it, so no finite value
    /// inhabits it.
    Uninhabited(Symbol),
    /// A type constructor nothing declares.
    Unknown(Symbol),
    /// Nesting reached [`HARD_GEN_DEPTH`].
    TooDeep,
}

impl fmt::Display for Ungeneratable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ungeneratable::Cell => f.write_str("a `Cell` belongs to the region that opened it"),
            Ungeneratable::Task => f.write_str("a `Task` belongs to a `simulate` region"),
            Ungeneratable::Secret => {
                f.write_str("a `Secret` is a credential, and nothing may generate one")
            }
            Ungeneratable::Effectful(row) => {
                write!(f, "a function performing {row} cannot be applied in a spec")
            }
            Ungeneratable::RowVariable => {
                f.write_str("an effect-row variable is not pure for every instantiation")
            }
            Ungeneratable::Uninhabited(name) => write!(f, "no finite value inhabits `{name}`"),
            Ungeneratable::Unknown(name) => write!(f, "no type named `{name}` is declared"),
            Ungeneratable::TooDeep => write!(f, "nesting reached {HARD_GEN_DEPTH} levels"),
        }
    }
}

/// Whether a value of this type can be drawn at all.
pub fn generatable(ty: &Type, world: &TypeWorld) -> Result<(), Ungeneratable> {
    match ty {
        // Monomorphised to `Int`, and recorded in `CaseReport::instantiations` so a sampled
        // polymorphic law says which instantiation it is about.
        Type::Var(_) => Ok(()),
        Type::Record(fields) => fields.values().try_for_each(|f| generatable(f, world)),
        Type::Fn {
            params,
            ret,
            effects,
        } => {
            if effects.tail.is_some() {
                return Err(Ungeneratable::RowVariable);
            }
            if !effects.atoms.is_empty() {
                return Err(Ungeneratable::Effectful(effects.clone()));
            }
            params.iter().try_for_each(|p| generatable(p, world))?;
            generatable(ret, world)
        }
        Type::Con(name, args) => match name.as_str() {
            "Int" | "Bool" | "String" | "Bytes" | "Unit" | "Float" | "Decimal" => Ok(()),
            n if IntTy::from_name(n).is_some() => Ok(()),
            "List" | "Map" => args.iter().try_for_each(|a| generatable(a, world)),
            "Cell" => Err(Ungeneratable::Cell),
            _ if name.as_str() == prelude::TASK_TYPE => Err(Ungeneratable::Task),
            _ if name.as_str() == ply_core::ty::SECRET => Err(Ungeneratable::Secret),
            _ => {
                let Some(decl) = world.types.get(name) else {
                    return Err(Ungeneratable::Unknown(name.clone()));
                };
                if decl.depth.is_none() {
                    return Err(Ungeneratable::Uninhabited(name.clone()));
                }
                args.iter().try_for_each(|a| generatable(a, world))
            }
        },
    }
}

/// Draw one value of `ty` for case `case`.
pub fn generate(
    ty: &Type,
    world: &TypeWorld,
    stream: &mut GenStream,
    case: u32,
) -> Result<Value, Ungeneratable> {
    let mut draw = Gen {
        world,
        stream,
        size: size_for(case),
        edge: edge_for(case),
    };
    draw.value(ty, 0)
}

/// Every tuple a root draws, in order.
pub fn draw_cases(
    binders: &[LawBinder],
    world: &TypeWorld,
    key: DefHash,
    root: u64,
    cases: u32,
) -> Result<Vec<Vec<Value>>, Ungeneratable> {
    let mut stream = GenStream::new(root, key);
    let mut out = Vec::with_capacity(cases as usize);
    for case in 0..cases {
        let mut tuple = Vec::with_capacity(binders.len());
        for binder in binders {
            tuple.push(generate(&binder.ty, world, &mut stream, case)?);
        }
        out.push(tuple);
    }
    Ok(out)
}

/// The size parameter, growing with the case index and then flat.
fn size_for(case: u32) -> u64 {
    (case as u64).min(63)
}

fn edge_for(case: u32) -> Option<u32> {
    (case < EDGE_CASES).then_some(case)
}

struct Gen<'a> {
    world: &'a TypeWorld,
    stream: &'a mut GenStream,
    size: u64,
    /// `Some(i)` for an edge case: leaves take their `i`th edge point rather than a draw, and
    /// collections take a short fixed length.
    edge: Option<u32>,
}

impl Gen<'_> {
    fn value(&mut self, ty: &Type, depth: u32) -> Result<Value, Ungeneratable> {
        if depth >= HARD_GEN_DEPTH {
            return Err(Ungeneratable::TooDeep);
        }
        match ty {
            Type::Var(_) => self.int(),
            Type::Record(fields) => {
                let mut out = BTreeMap::new();
                for (name, field) in fields {
                    out.insert(name.clone(), self.value(field, depth + 1)?);
                }
                Ok(Value::Record(Arc::new(out.into_iter().collect())))
            }
            Type::Fn {
                params,
                ret,
                effects,
            } => {
                if effects.tail.is_some() {
                    return Err(Ungeneratable::RowVariable);
                }
                if !effects.atoms.is_empty() {
                    return Err(Ungeneratable::Effectful(effects.clone()));
                }
                self.function(params, ret, depth)
            }
            Type::Con(name, args) => match name.as_str() {
                "Int" => self.int(),
                n if IntTy::from_name(n).is_some() => {
                    let t = IntTy::from_name(n).expect("just checked");
                    Ok(Value::Fixed(self.fixed(t)))
                }
                "Float" => Ok(Value::Float(self.float())),
                "Decimal" => Ok(Value::Decimal(self.decimal())),
                "Bool" => Ok(Value::Bool(self.bool())),
                "String" => Ok(self.string()),
                "Bytes" => Ok(self.bytes()),
                "Unit" => Ok(Value::Unit),
                "List" => {
                    let elem = args.first().cloned().unwrap_or_else(Type::int);
                    self.list(&elem, depth)
                }
                "Map" => {
                    let key = args.first().cloned().unwrap_or_else(Type::int);
                    let value = args.get(1).cloned().unwrap_or_else(Type::int);
                    self.map(&key, &value, depth)
                }
                "Cell" => Err(Ungeneratable::Cell),
                _ if name.as_str() == prelude::TASK_TYPE => Err(Ungeneratable::Task),
                _ if name.as_str() == ply_core::ty::SECRET => Err(Ungeneratable::Secret),
                _ => self.adt(name, args, depth),
            },
        }
    }

    /// The same shape as [`Self::int`], reduced to the type: the edges of the *type* rather than
    /// of `Int`, because `0`, `1`, the minimum and the maximum are where a fixed width breaks.
    fn fixed(&mut self, t: IntTy) -> Fixed {
        let edges = [0i128, 1, -1, t.min(), t.max() as i128, t.max() as i128 - 1];
        let pick = |i: u64| {
            let v = edges[i as usize % edges.len()];
            Fixed::of(t, v).unwrap_or_else(|| Fixed::new(t, v as u64))
        };
        match self.edge {
            Some(i) => pick(u64::from(i)),
            None => {
                let selector = self.stream.next_u64() % 32;
                if (selector as usize) < edges.len() {
                    pick(selector)
                } else {
                    Fixed::new(t, self.stream.next_u64())
                }
            }
        }
    }

    fn int(&mut self) -> Result<Value, Ungeneratable> {
        Ok(Value::Int(match self.edge {
            Some(i) => EDGE_INTS[i as usize % EDGE_INTS.len()],
            None => {
                let selector = self.stream.next_u64() % 32;
                if (selector as usize) < EDGE_INTS.len() {
                    EDGE_INTS[selector as usize]
                } else {
                    let x = self.stream.next_u64();
                    let bits = 8 + self.size.min(54);
                    let magnitude = ((x >> 1) & ((1u64 << bits) - 1)) as i64;
                    if x & 1 == 0 { magnitude } else { -magnitude }
                }
            }
        }))
    }

    /// Finite values **and the specials**.
    fn float(&mut self) -> f64 {
        if let Some(i) = self.edge {
            return EDGE_FLOATS[i as usize % EDGE_FLOATS.len()];
        }
        let selector = self.stream.next_u64() % 32;
        if (selector as usize) < EDGE_FLOATS.len() {
            return EDGE_FLOATS[selector as usize];
        }
        // A draw over the bit pattern would be almost all NaN; a draw over a bounded mantissa and a
        // bounded exponent reaches both the ordinary scale a program works at and the ends of the
        // range.
        let mantissa = (self.stream.next_u64() >> 11) as f64;
        let exponent = (self.stream.next_u64() % (1 + self.size.min(60))) as i32 - 30;
        let sign = if self.stream.next_u64() & 1 == 0 {
            1.0
        } else {
            -1.0
        };
        sign * mantissa * 2f64.powi(exponent)
    }

    /// Scale `0..=6` around zero, plus the ends of the range.
    fn decimal(&mut self) -> Decimal {
        if let Some(i) = self.edge {
            return edge_decimal(i as usize % EDGE_DECIMAL_COUNT);
        }
        let selector = self.stream.next_u64() % 32;
        if (selector as usize) < EDGE_DECIMAL_COUNT {
            return edge_decimal(selector as usize);
        }
        let scale = (self.stream.next_u64() % 7) as u32;
        let bits = 8 + self.size.min(54);
        let x = self.stream.next_u64();
        let magnitude = ((x >> 1) & ((1u64 << bits) - 1)) as i64;
        let mantissa = if x & 1 == 0 { magnitude } else { -magnitude };
        Decimal::try_from_i128_with_scale(mantissa as i128, scale).unwrap_or(Decimal::ZERO)
    }

    fn bool(&mut self) -> bool {
        match self.edge {
            Some(i) => i % 2 == 1,
            None => self.stream.next_u64() & 1 == 1,
        }
    }

    /// Length biased small by taking the lesser of two draws, then capped by the size parameter, so
    /// a case-3 counterexample is short and a case-200 one still reaches the full width.
    fn length(&mut self) -> usize {
        if let Some(i) = self.edge {
            return (i % 3) as usize;
        }
        let cap = MAX_GEN_LEN.min(1 + self.size / 4);
        let a = self.stream.next_u64() % (cap + 1);
        let b = self.stream.next_u64() % (cap + 1);
        a.min(b) as usize
    }

    fn string(&mut self) -> Value {
        let len = self.length();
        let mut out = String::with_capacity(len);
        for _ in 0..len {
            let index = (self.stream.next_u64() % GEN_ALPHABET.len() as u64) as usize;
            out.push(GEN_ALPHABET[index] as char);
        }
        Value::str(out)
    }

    /// The whole byte range, unlike [`Generator::string`]'s alphabet: a `Bytes` that never contains
    /// `0x00` or `0xff` is a `Bytes` whose laws are checked over the cases that never break.
    fn bytes(&mut self) -> Value {
        let len = self.length();
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push((self.stream.next_u64() & 0xff) as u8);
        }
        Value::bytes(out)
    }

    fn list(&mut self, elem: &Type, depth: u32) -> Result<Value, Ungeneratable> {
        // Past `GEN_DEPTH` a collection is empty.
        let len = if depth >= GEN_DEPTH { 0 } else { self.length() };
        let mut items = Vec::with_capacity(len);
        for _ in 0..len {
            items.push(self.value(elem, depth + 1)?);
        }
        Ok(Value::list(items))
    }

    /// At most [`MAX_GEN_ENTRIES`] entries, drawn from the key and value generators.
    fn map(&mut self, key: &Type, value: &Type, depth: u32) -> Result<Value, Ungeneratable> {
        let len = if depth >= GEN_DEPTH {
            0
        } else {
            self.length().min(MAX_GEN_ENTRIES)
        };
        let mut entries = Vec::with_capacity(len);
        for _ in 0..len {
            let k = self.value(key, depth + 1)?;
            let v = self.value(value, depth + 1)?;
            entries.push((k, v));
        }
        Ok(Value::map(entries))
    }

    fn adt(&mut self, name: &Symbol, args: &[Type], depth: u32) -> Result<Value, Ungeneratable> {
        let Some(decl) = self.world.types.get(name) else {
            return Err(Ungeneratable::Unknown(name.clone()));
        };
        let variants = decl.variants.clone();

        // Only variants every one of whose *substituted* fields can be drawn: `type Box<a> = B(a)`
        // is generatable at `Box<Int>` and not at `Box<Cell<Int>>`, and the declaration alone
        // cannot tell the two apart.
        let mut usable: Vec<(&Variant, Vec<Type>)> = Vec::new();
        for variant in &variants {
            let fields = self.world.fields(name, variant, args);
            if fields.iter().all(|f| generatable(f, self.world).is_ok()) {
                usable.push((variant, fields));
            }
        }
        if usable.is_empty() {
            return Err(Ungeneratable::Uninhabited(name.clone()));
        }

        // Past `GEN_DEPTH`, only the shallowest constructors.
        if depth >= GEN_DEPTH {
            let shallowest = usable
                .iter()
                .filter_map(|(v, _)| v.depth)
                .min()
                .unwrap_or(u64::MAX);
            usable.retain(|(v, _)| v.depth == Some(shallowest));
            if usable.is_empty() {
                return Err(Ungeneratable::Uninhabited(name.clone()));
            }
        }

        let pick = match self.edge {
            Some(i) => i as usize % usable.len(),
            None => (self.stream.next_u64() % usable.len() as u64) as usize,
        };
        let (variant, fields) = &usable[pick];
        let ctor = variant.name.clone();
        let fields = fields.clone();
        let mut out = Vec::with_capacity(fields.len());
        for field in &fields {
            out.push(self.value(field, depth + 1)?);
        }
        Ok(Value::ctor(ctor, out))
    }

    /// A member of a fixed family, every one of which is pure, total, extensionally deterministic
    /// and printable — so a counterexample naming a function names something a reader can act on
    /// rather than `<fn>`.
    fn function(
        &mut self,
        params: &[Type],
        ret: &Type,
        depth: u32,
    ) -> Result<Value, Ungeneratable> {
        let projection = params.iter().position(|p| p == ret);
        let tabulatable = params.first().is_some_and(comparable);
        let choice = match self.edge {
            Some(_) => 0,
            None => self.stream.next_u64() % 4,
        };
        match choice {
            1 if projection.is_some() => {
                let index = projection.expect("guarded by the arm");
                Ok(projection_fn(params.len(), index))
            }
            2 | 3 if tabulatable => {
                let entries = 1 + (self.stream.next_u64() % 2) as usize;
                let mut table = Vec::with_capacity(entries);
                for _ in 0..entries {
                    let key = self.value(&params[0], depth + 1)?;
                    let value = self.value(ret, depth + 1)?;
                    table.push((key, value));
                }
                let default = self.value(ret, depth + 1)?;
                Ok(table_fn(params.len(), table, default, self.world))
            }
            _ => {
                let value = self.value(ret, depth + 1)?;
                Ok(const_fn(params.len(), value, self.world))
            }
        }
    }
}

/// Whether `==` on this type answers rather than raising.
fn comparable(ty: &Type) -> bool {
    match ty {
        Type::Fn { .. } => false,
        Type::Var(_) => true,
        Type::Record(fields) => fields.values().all(comparable),
        Type::Con(_, args) => args.iter().all(comparable),
    }
}

fn ident(name: &str) -> Ident {
    Ident::new(name, Span::DUMMY)
}

fn var(name: &str) -> Expr {
    Expr {
        kind: ExprKind::Var(QName::bare(ident(name))),
        span: Span::DUMMY,
    }
}

fn param_names(arity: usize) -> Vec<Symbol> {
    (0..arity).map(|i| Symbol::new(format!("x{i}"))).collect()
}

/// The synthesized body names only its own parameters and the values bound beside it, so the module
/// scope is never consulted and index 0 is a placeholder rather than a claim about which module
/// this came from.
fn closure(
    params: Vec<Symbol>,
    body: Expr,
    bindings: Vec<(Symbol, Value)>,
    description: String,
) -> Value {
    Value::Closure(Arc::new(Closure {
        name: Some(Symbol::new(description)),
        kind: ClosureKind::Fn {
            params,
            body: Arc::new(body),
            bindings,
            module: 0,
        },
    }))
}

fn binder_list(arity: usize, names: &[Symbol]) -> String {
    if names.is_empty() {
        return (0..arity).map(|_| "_").collect::<Vec<_>>().join(", ");
    }
    names
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `\(..) -> c`.
pub(crate) fn const_fn(arity: usize, value: Value, world: &TypeWorld) -> Value {
    let description = format!("|{}| {}", binder_list(arity, &[]), value.render());
    let bindings = vec![
        (Symbol::new(FN_CONST), value.clone()),
        (
            Symbol::new(FN_SIZE),
            Value::Int(saturating_i64(
                2u64.saturating_add(shrink::size(&value, world)),
            )),
        ),
    ];
    closure(param_names(arity), var(FN_CONST), bindings, description)
}

/// `\(x0, ..) -> xi` where `xi` has the return type.
fn projection_fn(arity: usize, index: usize) -> Value {
    let names = param_names(arity);
    let picked = names[index].to_string();
    let description = format!("|{}| {picked}", binder_list(arity, &names));
    let bindings = vec![(Symbol::new(FN_SIZE), Value::Int(1))];
    closure(names, var(&picked), bindings, description)
}

/// `\(x0, ..) -> if x0 == k { v } else .. else d`: a lookup table over the first parameter with a
/// default.
fn table_fn(arity: usize, table: Vec<(Value, Value)>, default: Value, world: &TypeWorld) -> Value {
    let names = param_names(arity);
    let subject = names[0].to_string();
    let mut bindings = vec![(Symbol::new(FN_DEFAULT), default.clone())];
    let mut size = 4u64.saturating_add(shrink::size(&default, world));
    let mut body = var(FN_DEFAULT);
    let mut description = default.render();
    for (i, (key, value)) in table.iter().enumerate().rev() {
        let key_slot = format!("#k{i}");
        let value_slot = format!("#v{i}");
        bindings.push((Symbol::new(&key_slot), key.clone()));
        bindings.push((Symbol::new(&value_slot), value.clone()));
        size = size
            .saturating_add(shrink::size(key, world))
            .saturating_add(shrink::size(value, world));
        description = format!(
            "if {subject} == {} {{ {} }} else {{ {description} }}",
            key.render(),
            value.render()
        );
        body = Expr {
            kind: ExprKind::If {
                cond: Box::new(Expr {
                    kind: ExprKind::Binary {
                        op: BinOp::Eq,
                        lhs: Box::new(var(&subject)),
                        rhs: Box::new(var(&key_slot)),
                    },
                    span: Span::DUMMY,
                }),
                then_branch: Box::new(var(&value_slot)),
                else_branch: Box::new(body),
            },
            span: Span::DUMMY,
        };
    }
    bindings.push((Symbol::new(FN_SIZE), Value::Int(saturating_i64(size))));
    let description = format!("|{}| {description}", binder_list(arity, &names));
    closure(names, body, bindings, description)
}

fn saturating_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The size a generated function value was built with.
pub(crate) fn fn_size(value: &Value) -> Option<u64> {
    let Value::Closure(closure) = value else {
        return None;
    };
    let ClosureKind::Fn { bindings, .. } = &closure.kind else {
        return None;
    };
    let size = Symbol::new(FN_SIZE);
    match bindings.iter().find(|(n, _)| *n == size) {
        Some((_, Value::Int(n))) => Some((*n).max(0) as u64),
        _ => None,
    }
}

/// What one case did.
#[derive(Debug)]
pub enum Outcome {
    /// The guard did not admit this tuple.
    Rejected,
    Held,
    Failed,
    /// The guard or the body raised.
    Raised(Diagnostic),
}

impl Outcome {
    pub(crate) fn matches(&self, target: Target) -> bool {
        matches!(
            (self, target),
            (Outcome::Failed, Target::Falsifies) | (Outcome::Raised(_), Target::Raises)
        )
    }
}

/// How the property tier asks about one tuple of binder values.
pub trait Judge {
    /// `Ok(false)` means the guard rejected this tuple.
    fn guard(&mut self, values: &[Value]) -> Result<bool, Diagnostic>;
    /// `Ok(true)` means the obligation held at this tuple.
    fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic>;
}

impl<T: Judge + ?Sized> Judge for &mut T {
    fn guard(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        (**self).guard(values)
    }
    fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        (**self).body(values)
    }
}

/// Guard first, always.
pub fn judge_case(judge: &mut dyn Judge, values: &[Value]) -> Outcome {
    match judge.guard(values) {
        Err(d) => Outcome::Raised(d),
        Ok(false) => Outcome::Rejected,
        Ok(true) => match judge.body(values) {
            Err(d) => Outcome::Raised(d),
            Ok(true) => Outcome::Held,
            Ok(false) => Outcome::Failed,
        },
    }
}

/// Draw, filter by the guard, evaluate, and report what happened — including the two outcomes that
/// are not tiers.
pub fn run_property(
    key: DefHash,
    binders: &[LawBinder],
    world: &TypeWorld,
    plan: &ProvePlan,
    guard_span: Span,
    judge: &mut dyn Judge,
) -> Discharge {
    for binder in binders {
        if generatable(&binder.ty, world).is_err() {
            return Discharge::Unattempted(Gap::Ungeneratable {
                param: binder.name.clone(),
                ty: binder.ty.clone(),
            });
        }
    }

    let plan = plan.clone().normalized();
    let types: Vec<Type> = binders.iter().map(|b| b.ty.clone()).collect();
    let mut generated: u32 = 0;
    let mut kept: u32 = 0;

    for &root in &plan.roots {
        let mut stream = GenStream::new(root, key);
        for case in 0..plan.cases {
            let mut values = Vec::with_capacity(binders.len());
            for binder in binders {
                match generate(&binder.ty, world, &mut stream, case) {
                    Ok(v) => values.push(v),
                    Err(_) => {
                        return Discharge::Unattempted(Gap::Ungeneratable {
                            param: binder.name.clone(),
                            ty: binder.ty.clone(),
                        });
                    }
                }
            }
            generated = generated.saturating_add(1);
            let outcome = judge_case(judge, &values);
            match outcome {
                Outcome::Rejected => {}
                Outcome::Held => kept = kept.saturating_add(1),
                Outcome::Failed => {
                    let shrunk = shrink::shrink(
                        &values,
                        &types,
                        world,
                        judge,
                        Target::Falsifies,
                        plan.shrink_budget,
                    );
                    return Discharge::Refuted(Counterexample {
                        bindings: bindings(binders, &shrunk.values),
                        original: bindings(binders, &values),
                        shrinks: shrunk.steps,
                        root,
                        case,
                        race: None,
                        sim_seed: None,
                    });
                }
                Outcome::Raised(diagnostic) => {
                    // A minimal raising input is worth exactly what a minimal falsifying one is, so
                    // it gets the same treatment.
                    let shrunk = shrink::shrink(
                        &values,
                        &types,
                        world,
                        judge,
                        Target::Raises,
                        plan.shrink_budget,
                    );
                    return Discharge::Unattempted(Gap::Raised {
                        bindings: bindings(binders, &shrunk.values),
                        diagnostic: shrunk.diagnostic.unwrap_or(diagnostic),
                    });
                }
            }
        }
    }

    if kept == 0 {
        // Never a pass.
        return Discharge::Vacuous(Vacuity {
            guard: guard_span,
            kind: VacuityKind::NoCaseKept { generated },
        });
    }

    Discharge::Held(Evidence::Cases(CaseReport {
        generated,
        kept,
        rejected: generated - kept,
        roots: plan.roots.clone(),
        instantiations: instantiations(&types),
    }))
}

fn bindings(binders: &[LawBinder], values: &[Value]) -> Vec<Binding> {
    binders
        .iter()
        .zip(values)
        .map(|(binder, value)| Binding {
            name: binder.name.clone(),
            ty: binder.ty.clone(),
            rendered: value.render(),
        })
        .collect()
}

/// Every type variable the binders mention, monomorphised to `Int`.
pub fn instantiations(types: &[Type]) -> Vec<(Symbol, Type)> {
    let mut seen: BTreeSet<TyVar> = BTreeSet::new();
    let mut out = Vec::new();
    for ty in types {
        collect_vars(ty, &mut seen, &mut out);
    }
    out
}

fn collect_vars(ty: &Type, seen: &mut BTreeSet<TyVar>, out: &mut Vec<(Symbol, Type)>) {
    match ty {
        Type::Var(v) => {
            if seen.insert(*v) {
                out.push((Symbol::new(Type::Var(*v).to_string()), Type::int()));
            }
        }
        Type::Con(_, args) => args.iter().for_each(|a| collect_vars(a, seen, out)),
        Type::Record(fields) => fields.values().for_each(|f| collect_vars(f, seen, out)),
        Type::Fn { params, ret, .. } => {
            params.iter().for_each(|p| collect_vars(p, seen, out));
            collect_vars(ret, seen, out);
        }
    }
}

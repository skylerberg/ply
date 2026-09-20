//! The property tier: seeded generation of every Ply type, run against an obligation's guard.

use crate::shrink::{self, Target};
use crate::{
    Binding, CaseReport, Counterexample, Discharge, Evidence, GEN_DEPTH, Gap, ProvePlan, Vacuity,
    VacuityKind,
};
use ply_eval::{Closure, ClosureKind, Decimal, Fixed, Synth, Value};
use ply_span::{Diagnostic, Span, Symbol};
use ply_ty::DefHash;
use ply_ty::prelude;
use ply_ty::{CtorInfo, IntTy, LawBinder, Row, TyVar, Type};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

/// Case indices below this draw their type's edge values rather than a sample.
pub const EDGE_CASES: u32 = 5;

pub const EDGE_INTS: [i64; 5] = [0, 1, -1, i64::MIN, i64::MAX];

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

const EDGE_DECIMAL_COUNT: usize = 6;

fn edge_decimal(index: usize) -> Decimal {
    match index {
        0 => Decimal::ZERO,
        1 => Decimal::ONE,
        2 => -Decimal::ONE,
        // `0.00m`: equal to zero at a different scale.
        3 => Decimal::new(0, 2),
        4 => Decimal::MAX,
        _ => Decimal::MIN,
    }
}

/// Starts at `'a'` because the shrinker lowers characters toward `'a'`.
pub const GEN_ALPHABET: &[u8; 16] = b"abcdefghijklmnop";

pub const MAX_GEN_LEN: u64 = 16;

pub const MAX_GEN_ENTRIES: usize = 8;

pub const HARD_GEN_DEPTH: u32 = 64;

const GEN_DOMAIN: &[u8] = b"ply.gen.stream.1";

/// Counter-mode BLAKE3, keyed by the root and the obligation.
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

    pub fn at(root: u64, key: DefHash, counter: u64) -> GenStream {
        GenStream { root, key, counter }
    }

    pub fn next_u64(&mut self) -> u64 {
        let value = GenStream::draw(self.root, &self.key, self.counter);
        self.counter = self.counter.wrapping_add(1);
        value
    }

    /// Uniform by rejection. Do not change the rule: printed `(root, case)` pairs would shift.
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

    /// Pure, so a replay can ask for draw *i* directly.
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

#[derive(Clone, Debug)]
pub struct Variant {
    pub name: Symbol,
    pub index: usize,
    /// Written in the owning type's parameters.
    pub fields: Vec<Type>,
    /// Nested constructor applications a value of this variant needs.
    pub depth: Option<u64>,
}

#[derive(Clone, Debug)]
struct TypeDecl {
    params: Vec<TyVar>,
    variants: Vec<Variant>,
    depth: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct TypeWorld {
    types: BTreeMap<Symbol, TypeDecl>,
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

    fn solve_depths(&mut self) {
        let names: Vec<Symbol> = self.types.keys().cloned().collect();
        // Each round settles at least one type, so one round per type suffices.
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
                "List" | "Map" => Some(0),
                "Cell" => None,
                _ if name.as_str() == prelude::TASK_TYPE => None,
                _ if name.as_str() == ply_ty::SECRET => None,
                _ => self.types.get(name).and_then(|d| d.depth),
            },
        }
    }

    pub fn variants(&self, ty: &Symbol) -> Option<&[Variant]> {
        self.types.get(ty).map(|d| d.variants.as_slice())
    }

    pub fn ctor(&self, name: &Symbol) -> Option<(&Symbol, usize)> {
        self.ctors.get(name).map(|(ty, index)| (ty, *index))
    }

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

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ungeneratable {
    Cell,
    Task,
    Secret,
    /// Applying it would make the spec impure.
    Effectful(Row),
    RowVariable,
    Uninhabited(Symbol),
    Unknown(Symbol),
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

pub fn generatable(ty: &Type, world: &TypeWorld) -> Result<(), Ungeneratable> {
    match ty {
        // Monomorphised to `Int` and recorded in `CaseReport::instantiations`.
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
            _ if name.as_str() == ply_ty::SECRET => Err(Ungeneratable::Secret),
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
    /// `Some(i)` for an edge case: leaves take their `i`th edge point.
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
                _ if name.as_str() == ply_ty::SECRET => Err(Ungeneratable::Secret),
                _ => self.adt(name, args, depth),
            },
        }
    }

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

    fn float(&mut self) -> f64 {
        if let Some(i) = self.edge {
            return EDGE_FLOATS[i as usize % EDGE_FLOATS.len()];
        }
        let selector = self.stream.next_u64() % 32;
        if (selector as usize) < EDGE_FLOATS.len() {
            return EDGE_FLOATS[selector as usize];
        }
        // Not over the bit pattern: a bounded mantissa and exponent reach ordinary scales too.
        let mantissa = (self.stream.next_u64() >> 11) as f64;
        let exponent = (self.stream.next_u64() % (1 + self.size.min(60))) as i32 - 30;
        let sign = if self.stream.next_u64() & 1 == 0 {
            1.0
        } else {
            -1.0
        };
        sign * mantissa * 2f64.powi(exponent)
    }

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

    /// Length is the lesser of two draws, capped by the size parameter, so early cases are short.
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

    fn bytes(&mut self) -> Value {
        let len = self.length();
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push((self.stream.next_u64() & 0xff) as u8);
        }
        Value::bytes(out)
    }

    fn list(&mut self, elem: &Type, depth: u32) -> Result<Value, Ungeneratable> {
        let len = if depth >= GEN_DEPTH { 0 } else { self.length() };
        let mut items = Vec::with_capacity(len);
        for _ in 0..len {
            items.push(self.value(elem, depth + 1)?);
        }
        Ok(Value::list(items))
    }

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

        // Check substituted fields: `Box<a>` is generatable at `Box<Int>`, not `Box<Cell<Int>>`.
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

    /// From a fixed family of pure, total, printable functions, so counterexamples are readable.
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
                Ok(table_fn(params.len(), table, default))
            }
            _ => {
                let value = self.value(ret, depth + 1)?;
                Ok(const_fn(params.len(), value))
            }
        }
    }
}

fn comparable(ty: &Type) -> bool {
    match ty {
        Type::Fn { .. } => false,
        Type::Var(_) => true,
        Type::Record(fields) => fields.values().all(comparable),
        Type::Con(_, args) => args.iter().all(comparable),
    }
}

fn param_names(arity: usize) -> Vec<Symbol> {
    (0..arity).map(|i| Symbol::new(format!("x{i}"))).collect()
}

fn closure(arity: usize, rule: Synth, description: String) -> Value {
    Value::Closure(Arc::new(Closure {
        name: Some(Symbol::new(description)),
        kind: ClosureKind::Synth { arity, rule },
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

pub(crate) fn const_fn(arity: usize, value: Value) -> Value {
    let description = format!("|{}| {}", binder_list(arity, &[]), value.render());
    closure(arity, Synth::Const(value), description)
}

fn projection_fn(arity: usize, index: usize) -> Value {
    let names = param_names(arity);
    let description = format!("|{}| {}", binder_list(arity, &names), names[index]);
    closure(arity, Synth::Project(index), description)
}

fn table_fn(arity: usize, entries: Vec<(Value, Value)>, default: Value) -> Value {
    let names = param_names(arity);
    let subject = &names[0];
    let mut description = default.render();
    for (key, value) in entries.iter().rev() {
        description = format!(
            "if {subject} == {} {{ {} }} else {{ {description} }}",
            key.render(),
            value.render()
        );
    }
    let description = format!("|{}| {description}", binder_list(arity, &names));
    closure(arity, Synth::Table { entries, default }, description)
}

pub(crate) fn fn_size(value: &Value, world: &TypeWorld) -> Option<u64> {
    let Value::Closure(closure) = value else {
        return None;
    };
    let ClosureKind::Synth { rule, .. } = &closure.kind else {
        return None;
    };
    let own: u64 = match rule {
        Synth::Const(_) => 2,
        Synth::Project(_) => 1,
        Synth::Table { .. } => 4,
    };
    Some(
        rule.values()
            .into_iter()
            .fold(own, |acc, v| acc.saturating_add(shrink::size(v, world))),
    )
}

#[derive(Debug)]
pub enum Outcome {
    Rejected,
    Held,
    Failed,
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
                    // An evaluation that ran out of time is not shrunk: every candidate would
                    // spend the whole budget again.
                    if diagnostic.code == ply_span::codes::TIME_BUDGET {
                        return Discharge::Unattempted(Gap::Raised {
                            bindings: bindings(binders, &values),
                            diagnostic: Box::new(diagnostic),
                        });
                    }
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
                        diagnostic: Box::new(shrunk.diagnostic.unwrap_or(diagnostic)),
                    });
                }
            }
        }
    }

    if kept == 0 {
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

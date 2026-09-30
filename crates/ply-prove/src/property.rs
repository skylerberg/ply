//! The property tier: seeded generation of every Ply type, run against an obligation's guard.

use crate::shrink::{self, Target};
use crate::sort::Sort;
use crate::world::{Variant, World};
use crate::{
    Binder, Binding, CaseReport, Counterexample, Discharge, Evidence, Fault, GEN_DEPTH, Gap,
    ProvePlan, Vacuity, VacuityKind,
};
use ply_eval::{
    Closure, ClosureKind, Decimal, DefHash, Diagnostic, Fixed, IntTy, SECRET, Span, Symbol, Synth,
    TASK_TYPE, Value,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Case indices below this draw their type's edge values rather than a sample.
pub const EDGE_CASES: u32 = 5;

pub const EDGE_INTS: [i64; 5] = [0, 1, -1, i64::MIN, i64::MAX];

const EDGE_FLOATS: [f64; 8] = [
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
const GEN_ALPHABET: &[u8; 16] = b"abcdefghijklmnop";

const MAX_GEN_LEN: u64 = 16;

const MAX_GEN_ENTRIES: usize = 8;

pub(crate) const HARD_GEN_DEPTH: u32 = 64;

/// What every type variable is drawn as, as a report names the type.
const VARIABLE_DRAWN_AS: &str = "Int";

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
        GenStream {
            root,
            key,
            counter: 0,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let value = GenStream::draw(self.root, &self.key, self.counter);
        self.counter = self.counter.wrapping_add(1);
        value
    }

    /// Pure: a draw is its root, its obligation and its place, and nothing a stream remembers.
    fn draw(root: u64, key: &DefHash, counter: u64) -> u64 {
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ungeneratable {
    Cell,
    Task,
    Secret,
    /// Its row is not empty, so applying it would make the spec impure.
    Effectful,
    Uninhabited(Symbol),
    Unknown(Symbol),
    TooDeep,
}

pub fn generatable(sort: &Sort, world: &World) -> Result<(), Ungeneratable> {
    match sort {
        // Monomorphised to `Int` and recorded in `CaseReport::instantiations`.
        Sort::Var(_) => Ok(()),
        Sort::Record(fields) => fields.iter().try_for_each(|(_, f)| generatable(f, world)),
        Sort::Fn { pure: false, .. } => Err(Ungeneratable::Effectful),
        Sort::Fn { params, ret, .. } => {
            params.iter().try_for_each(|p| generatable(p, world))?;
            generatable(ret, world)
        }
        Sort::Con(name, args) => match name.as_str() {
            "Int" | "Bool" | "String" | "Bytes" | "Unit" | "Float" | "Decimal" => Ok(()),
            n if IntTy::from_name(n).is_some() => Ok(()),
            "List" | "Map" => args.iter().try_for_each(|a| generatable(a, world)),
            "Cell" => Err(Ungeneratable::Cell),
            _ if name.as_str() == TASK_TYPE => Err(Ungeneratable::Task),
            _ if name.as_str() == SECRET => Err(Ungeneratable::Secret),
            _ => {
                let Some(decl) = world.decl(name) else {
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
    sort: &Sort,
    world: &World,
    stream: &mut GenStream,
    case: u32,
) -> Result<Value, Ungeneratable> {
    let mut draw = Gen {
        world,
        stream,
        size: size_for(case),
        edge: edge_for(case),
    };
    draw.value(sort, 0)
}

fn size_for(case: u32) -> u64 {
    (case as u64).min(63)
}

fn edge_for(case: u32) -> Option<u32> {
    (case < EDGE_CASES).then_some(case)
}

struct Gen<'a> {
    world: &'a World,
    stream: &'a mut GenStream,
    size: u64,
    /// `Some(i)` for an edge case: leaves take their `i`th edge point.
    edge: Option<u32>,
}

impl Gen<'_> {
    fn value(&mut self, sort: &Sort, depth: u32) -> Result<Value, Ungeneratable> {
        if depth >= HARD_GEN_DEPTH {
            return Err(Ungeneratable::TooDeep);
        }
        match sort {
            Sort::Var(_) => self.int(),
            Sort::Record(fields) => {
                let mut out = BTreeMap::new();
                for (name, field) in fields {
                    out.insert(name.clone(), self.value(field, depth + 1)?);
                }
                Ok(Value::Record(Arc::new(out.into_iter().collect())))
            }
            Sort::Fn { pure: false, .. } => Err(Ungeneratable::Effectful),
            Sort::Fn { params, ret, .. } => self.function(params, ret, depth),
            Sort::Con(name, args) => match name.as_str() {
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
                    let elem = args.first().cloned().unwrap_or_else(Sort::int);
                    self.list(&elem, depth)
                }
                "Map" => {
                    let key = args.first().cloned().unwrap_or_else(Sort::int);
                    let value = args.get(1).cloned().unwrap_or_else(Sort::int);
                    self.map(&key, &value, depth)
                }
                "Cell" => Err(Ungeneratable::Cell),
                _ if name.as_str() == TASK_TYPE => Err(Ungeneratable::Task),
                _ if name.as_str() == SECRET => Err(Ungeneratable::Secret),
                _ => self.adt(name, args, depth),
            },
        }
    }

    fn fixed(&mut self, t: IntTy) -> Fixed {
        // `-1` is all ones, which an unsigned width reads as its largest value.
        let edges = [
            Fixed::new(t, 0),
            Fixed::new(t, 1),
            Fixed::new(t, u128::MAX),
            Fixed::new(t, t.min() as u128),
            Fixed::new(t, t.max()),
            Fixed::new(t, t.max() - 1),
        ];
        let pick = |i: u64| edges[i as usize % edges.len()];
        match self.edge {
            Some(i) => pick(u64::from(i)),
            None => {
                let selector = self.stream.next_u64() % 32;
                if (selector as usize) < edges.len() {
                    pick(selector)
                } else if t.bits() == 128 {
                    let high = u128::from(self.stream.next_u64());
                    Fixed::new(t, high << 64 | u128::from(self.stream.next_u64()))
                } else {
                    Fixed::new(t, u128::from(self.stream.next_u64()))
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

    fn list(&mut self, elem: &Sort, depth: u32) -> Result<Value, Ungeneratable> {
        let len = if depth >= GEN_DEPTH { 0 } else { self.length() };
        let mut items = Vec::with_capacity(len);
        for _ in 0..len {
            items.push(self.value(elem, depth + 1)?);
        }
        Ok(Value::list(items))
    }

    fn map(&mut self, key: &Sort, value: &Sort, depth: u32) -> Result<Value, Ungeneratable> {
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

    fn adt(&mut self, name: &Symbol, args: &[Sort], depth: u32) -> Result<Value, Ungeneratable> {
        let world = self.world;
        let Some(decl) = world.decl(name) else {
            return Err(Ungeneratable::Unknown(name.clone()));
        };

        // Check substituted fields: `Box<a>` is generatable at `Box<Int>`, not `Box<Cell<Int>>`.
        let mut usable: Vec<(&Variant, Vec<Sort>)> = Vec::new();
        for variant in &decl.variants {
            let fields = world.fields(variant, args);
            if fields.iter().all(|f| generatable(f, world).is_ok()) {
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
        params: &[Sort],
        ret: &Sort,
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

fn comparable(sort: &Sort) -> bool {
    match sort {
        Sort::Fn { .. } => false,
        Sort::Var(_) => true,
        Sort::Record(fields) => fields.iter().all(|(_, f)| comparable(f)),
        Sort::Con(_, args) => args.iter().all(comparable),
    }
}

/// Unnamed: `std.value.render` spells a generated function from its rule.
fn closure(arity: usize, rule: Synth) -> Value {
    Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Synth { arity, rule },
    }))
}

pub(crate) fn const_fn(arity: usize, value: Value) -> Value {
    closure(arity, Synth::Const(value))
}

fn projection_fn(arity: usize, index: usize) -> Value {
    closure(arity, Synth::Project(index))
}

fn table_fn(arity: usize, entries: Vec<(Value, Value)>, default: Value) -> Value {
    closure(arity, Synth::Table { entries, default })
}

pub(crate) fn fn_size(value: &Value, world: &World) -> Option<u64> {
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
    /// The program raised.
    Raised(Diagnostic),
    /// Ply failed rather than the program, so the tuple says nothing about the claim.
    Faulted(Diagnostic),
}

impl Outcome {
    /// A judgement that stopped on `diagnostic`: the program's raise unless its code is Ply's own.
    pub fn stopped(diagnostic: Diagnostic) -> Outcome {
        if ply_eval::codes::is_defect(diagnostic.code) {
            Outcome::Faulted(diagnostic)
        } else {
            Outcome::Raised(diagnostic)
        }
    }

    /// Ply's failure matches no target, so no walk takes a candidate that only makes Ply fail.
    pub fn matches(&self, target: Target) -> bool {
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
        Err(d) => Outcome::stopped(d),
        Ok(false) => Outcome::Rejected,
        Ok(true) => match judge.body(values) {
            Err(d) => Outcome::stopped(d),
            Ok(true) => Outcome::Held,
            Ok(false) => Outcome::Failed,
        },
    }
}

/// Why a binder has no value to draw: its type, as a report prints it.
pub fn ungeneratable(binder: &Binder) -> Gap {
    Gap::Ungeneratable {
        param: binder.name.clone(),
        ty: binder.text.clone(),
    }
}

/// `variables` names each type variable of the binders by its number, as the claim prints it.
pub fn run_property(
    key: DefHash,
    binders: &[Binder],
    variables: &[Symbol],
    world: &World,
    plan: &ProvePlan,
    guard_span: Span,
    judge: &mut dyn Judge,
) -> Discharge {
    if let Some(binder) = binders
        .iter()
        .find(|b| generatable(&b.sort, world).is_err())
    {
        return Discharge::Unattempted(ungeneratable(binder));
    }

    let plan = plan.clone().normalized();
    let mut generated: u32 = 0;
    let mut kept: u32 = 0;

    for &root in &plan.roots {
        let mut stream = GenStream::new(root, key);
        for case in 0..plan.cases {
            let mut values = Vec::with_capacity(binders.len());
            for binder in binders {
                match generate(&binder.sort, world, &mut stream, case) {
                    Ok(v) => values.push(v),
                    Err(_) => return Discharge::Unattempted(ungeneratable(binder)),
                }
            }
            generated = generated.saturating_add(1);
            let outcome = judge_case(judge, &values);
            match outcome {
                Outcome::Rejected => {}
                Outcome::Held => kept = kept.saturating_add(1),
                Outcome::Failed => {
                    // Unshrunk: the walk that makes a counterexample small is the program's now, and
                    // it drives it through the offers/holds operations. The values ride along so
                    // that something still holds them.
                    return Discharge::Refuted(Counterexample {
                        bindings: bindings(binders, &values),
                        original: bindings(binders, &values),
                        shrinks: 0,
                        root,
                        case,
                        race: None,
                        sim_seed: None,
                    });
                }
                Outcome::Raised(diagnostic) => {
                    return Discharge::Unattempted(Gap::Raised {
                        bindings: bindings(binders, &values),
                        diagnostic: Box::new(diagnostic),
                        root,
                        case,
                    });
                }
                Outcome::Faulted(diagnostic) => {
                    return Discharge::Faulted(Fault {
                        bindings: bindings(binders, &values),
                        diagnostic: Box::new(diagnostic),
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
        instantiations: instantiations(binders, variables),
    }))
}

/// Each binder beside the value it was given, as a report prints them.
pub fn bindings(binders: &[Binder], values: &[Value]) -> Vec<Binding> {
    binders
        .iter()
        .zip(values)
        .map(|(binder, value)| Binding {
            name: binder.name.clone(),
            ty: binder.text.clone(),
            value: ply_eval::Plain::shown(value),
        })
        .collect()
}

/// Every variable of the binders, in the order they first appear, named as `variables` numbers
/// them, beside the type each is drawn as.
pub fn instantiations(binders: &[Binder], variables: &[Symbol]) -> Vec<(Symbol, String)> {
    let mut vars: Vec<u32> = Vec::new();
    for binder in binders {
        binder.sort.vars(&mut vars);
    }
    vars.into_iter()
        .map(|v| (variables[v as usize].clone(), VARIABLE_DRAWN_AS.to_string()))
        .collect()
}

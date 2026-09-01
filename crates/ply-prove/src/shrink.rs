//! Shrinking a counterexample.

#[cfg(test)]
mod tests;

use crate::property::{
    HARD_GEN_DEPTH, Judge, Outcome, TypeWorld, Ungeneratable, const_fn, fn_size, judge_case,
};
use ply_core::{Type, prelude};
use ply_eval::{Decimal, List, Value};
use ply_span::{Diagnostic, Symbol};
use rust_decimal::RoundingStrategy;
use rust_decimal::prelude::ToPrimitive;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The property the original input had, and which every accepted candidate must still have.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Falsifies,
    Raises,
}

/// What the walk arrived at.
#[derive(Debug)]
pub struct Shrunk {
    pub values: Vec<Value>,
    /// Accepted steps.
    pub steps: u32,
    /// Candidates actually evaluated, against `--shrink-budget`.
    pub evaluations: u32,
    /// For [`Target::Raises`], what the last accepted candidate raised.
    pub diagnostic: Option<Diagnostic>,
}

/// Greedy descent: the first accepted candidate becomes the new value and the walk restarts.
pub fn shrink(
    values: &[Value],
    types: &[Type],
    world: &TypeWorld,
    judge: &mut dyn Judge,
    target: Target,
    budget: u32,
) -> Shrunk {
    let mut current: Vec<Value> = values.to_vec();
    let mut steps: u32 = 0;
    let mut evaluations: u32 = 0;
    let mut diagnostic: Option<Diagnostic> = None;

    'restart: loop {
        for index in 0..current.len() {
            let Some(ty) = types.get(index) else { continue };
            let here = size(&current[index], world);
            for candidate in candidates(&current[index], ty, world) {
                if size(&candidate, world) >= here {
                    continue;
                }
                if evaluations >= budget {
                    break 'restart;
                }
                let mut next = current.clone();
                next[index] = candidate;
                evaluations = evaluations.saturating_add(1);
                let outcome = judge_case(judge, &next);
                if outcome.matches(target) {
                    if let Outcome::Raised(d) = outcome {
                        diagnostic = Some(d);
                    }
                    current = next;
                    steps = steps.saturating_add(1);
                    continue 'restart;
                }
            }
        }
        break;
    }

    Shrunk {
        values: current,
        steps,
        evaluations,
        diagnostic,
    }
}

/// A saturating structural measure, and the reason the walk terminates.
pub fn size(value: &Value, world: &TypeWorld) -> u64 {
    let mut total: u64 = 0;
    let mut pending: Vec<&Value> = vec![value];
    while let Some(v) = pending.pop() {
        let here = match v {
            Value::Int(n) => int_size(*n),
            Value::Bool(b) => u64::from(*b),
            Value::Unit => 0,
            // Ordered so that every candidate below is a strict descent: toward zero, and positive
            // before its negation.
            Value::Float(f) => float_size(*f),
            Value::Decimal(d) => decimal_size(*d),
            Value::Str(s) => s
                .chars()
                .fold(0u64, |acc, c| acc.saturating_add(1 + c as u64)),
            Value::Bytes(b) => b
                .iter()
                .fold(0u64, |acc, byte| acc.saturating_add(1 + u64::from(*byte))),
            Value::List(items) => {
                pending.extend(items.iter());
                items.len() as u64
            }
            // Keys count as well as values: two maps with one entry each are ordered by what is in
            // them, which is what makes removing an entry a strict shrink and replacing a key with
            // a smaller one another.
            Value::Map(entries) => {
                pending.extend(entries.keys());
                pending.extend(entries.values());
                entries.size() as u64
            }
            Value::Record(fields) => {
                pending.extend(fields.values());
                0
            }
            Value::Ctor { name, args } => {
                pending.extend(args.iter());
                let index = world.ctor(name).map(|(_, i)| i).unwrap_or(0) as u64;
                1u64.saturating_add(index).saturating_add(args.len() as u64)
            }
            // A closure this crate generated carries the size it was built with; anything else is
            // left alone rather than guessed at.
            Value::Closure(_) => fn_size(v).unwrap_or(1),
            // A `Secret` is unreachable here: `forall (s: Secret<a>)` is `E0418`, so the generator
            // never mints one and no counterexample holds one.
            Value::Cell(_) | Value::Task(_) | Value::Continuation(_) | Value::Secret(_) => 0,
        };
        total = total.saturating_add(here);
    }
    total
}

/// Finite values by magnitude, with a negative outweighing its absolute value, and everything
/// non-finite above every finite value.
fn float_size(f: f64) -> u64 {
    if f.is_nan() {
        return u64::MAX;
    }
    if f.is_infinite() {
        return u64::MAX - 1;
    }
    let magnitude = f.abs();
    // A `Float` spans more magnitudes than a `u64` counts, so the measure is over the exponent and
    // the mantissa rather than over the value: it only has to order candidates, and a saturating
    // cast would make every large float one size and stall the walk.
    let bits = magnitude.to_bits();
    bits.saturating_mul(2)
        .saturating_add(u64::from(f.is_sign_negative()))
}

/// Magnitude, then whether there is a fraction at all, then the scale, then the sign — strictly
/// layered, so each is a tiebreak on the one above it.
fn decimal_size(d: Decimal) -> u64 {
    // The layers: a magnitude step is worth more than every tiebreak together, and having a
    // fraction is worth more than the scale it is written at.
    const MAGNITUDE: u64 = 64;
    const FRACTION: u64 = 32;
    let magnitude = d.trunc().abs().to_u64().unwrap_or(u64::MAX / MAGNITUDE);
    magnitude
        .saturating_mul(MAGNITUDE)
        .saturating_add(if d.fract().is_zero() { 0 } else { FRACTION })
        .saturating_add(u64::from(d.scale()))
        .saturating_add(u64::from(d.is_sign_negative()))
}

/// `-5` is larger than `5`, which is what makes negating a negative a shrink.
fn int_size(n: i64) -> u64 {
    n.unsigned_abs()
        .saturating_mul(2)
        .saturating_add(u64::from(n < 0))
}

/// The smallest value of a type: the shrinker's floor, and what fills a field a candidate
/// constructor has and the current value does not.
pub fn minimal(ty: &Type, world: &TypeWorld) -> Result<Value, Ungeneratable> {
    minimal_at(ty, world, 0)
}

fn minimal_at(ty: &Type, world: &TypeWorld, depth: u32) -> Result<Value, Ungeneratable> {
    if depth >= HARD_GEN_DEPTH {
        return Err(Ungeneratable::TooDeep);
    }
    match ty {
        Type::Var(_) => Ok(Value::Int(0)),
        Type::Record(fields) => {
            let mut out = BTreeMap::new();
            for (name, field) in fields {
                out.insert(name.clone(), minimal_at(field, world, depth + 1)?);
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
            let value = minimal_at(ret, world, depth + 1)?;
            Ok(const_fn(params.len(), value, world))
        }
        Type::Con(name, args) => match name.as_str() {
            "Int" => Ok(Value::Int(0)),
            // `0.0`, not `-0.0`: the two are different values and the positive one is the floor.
            "Float" => Ok(Value::Float(0.0)),
            "Decimal" => Ok(Value::Decimal(Decimal::ZERO)),
            "Bool" => Ok(Value::Bool(false)),
            "String" => Ok(Value::str("")),
            "Bytes" => Ok(Value::bytes([])),
            "Unit" => Ok(Value::Unit),
            "List" => Ok(Value::list(Vec::new())),
            // `map_new()`, which is the floor the contract names.
            "Map" => Ok(Value::empty_map()),
            "Cell" => Err(Ungeneratable::Cell),
            _ if name.as_str() == prelude::TASK_TYPE => Err(Ungeneratable::Task),
            _ if name.as_str() == ply_core::ty::SECRET => Err(Ungeneratable::Secret),
            _ => {
                let Some((variant, fields)) = shallowest(name, args, world) else {
                    return Err(Ungeneratable::Uninhabited(name.clone()));
                };
                let mut out = Vec::with_capacity(fields.len());
                for field in &fields {
                    out.push(minimal_at(field, world, depth + 1)?);
                }
                Ok(Value::ctor(variant, out))
            }
        },
    }
}

/// The variant a minimal value of `Con(name, args)` uses: fewest nested constructors, then lowest
/// declaration index.
fn shallowest(name: &Symbol, args: &[Type], world: &TypeWorld) -> Option<(Symbol, Vec<Type>)> {
    let variants = world.variants(name)?;
    variants
        .iter()
        .filter_map(|v| {
            let fields = world.fields(name, v, args);
            let usable = fields
                .iter()
                .all(|f| crate::property::generatable(f, world).is_ok());
            (usable && v.depth.is_some()).then_some((v, fields))
        })
        .min_by_key(|(v, _)| (v.depth.unwrap_or(u64::MAX), v.index))
        .map(|(v, fields)| (v.name.clone(), fields))
}

/// Candidates for one value, in the order two runs must agree on.
pub fn candidates(value: &Value, ty: &Type, world: &TypeWorld) -> Vec<Value> {
    candidates_at(value, ty, world, 0)
}

fn candidates_at(value: &Value, ty: &Type, world: &TypeWorld, depth: u32) -> Vec<Value> {
    if depth >= HARD_GEN_DEPTH {
        return Vec::new();
    }
    match (value, ty) {
        (Value::Int(n), _) => int_candidates(*n),
        (Value::Float(f), _) => float_candidates(*f),
        (Value::Decimal(d), _) => decimal_candidates(*d),
        (Value::Bool(true), _) => vec![Value::Bool(false)],
        (Value::Bool(false), _) | (Value::Unit, _) => Vec::new(),
        (Value::Str(s), _) => string_candidates(s),
        (Value::Bytes(b), _) => bytes_candidates(b),
        (Value::List(items), _) => {
            let elem = match ty {
                Type::Con(name, args) if name.as_str() == "List" => {
                    args.first().cloned().unwrap_or_else(Type::int)
                }
                _ => Type::int(),
            };
            list_candidates(items, &elem, world, depth)
        }
        (Value::Map(entries), _) => {
            let (key, value) = match ty {
                Type::Con(name, args) if name.as_str() == "Map" && args.len() == 2 => {
                    (args[0].clone(), args[1].clone())
                }
                _ => (Type::int(), Type::int()),
            };
            map_candidates(entries, &key, &value, world, depth)
        }
        (Value::Record(fields), Type::Record(types)) => {
            let mut out = Vec::new();
            for (name, field) in fields.iter() {
                let Some(field_ty) = types.get(name) else {
                    continue;
                };
                for candidate in candidates_at(field, field_ty, world, depth + 1) {
                    let mut next = (**fields).clone();
                    next.insert(name.clone(), candidate);
                    out.push(Value::Record(Arc::new(next)));
                }
            }
            out
        }
        (Value::Ctor { name, args }, Type::Con(ty_name, ty_args)) => {
            ctor_candidates(name, args, ty_name, ty_args, ty, world, depth)
        }
        // Toward the constant function returning the smallest value of the return type, which is
        // the family's floor, so a second application proposes the same value and the size test
        // ends the walk.
        (Value::Closure(_), Type::Fn { params, ret, .. }) => minimal(ret, world)
            .map(|v| vec![const_fn(params.len(), v, world)])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// `0`, then halving toward zero, then one step toward zero, then the positive of a negative.
fn int_candidates(n: i64) -> Vec<Value> {
    if n == 0 {
        return Vec::new();
    }
    let mut out: Vec<i64> = vec![0];
    let mut half = n / 2;
    while half != 0 {
        out.push(half);
        half /= 2;
    }
    if let Some(step) = n.checked_sub(n.signum()) {
        out.push(step);
    }
    if n < 0
        && let Some(positive) = n.checked_neg()
    {
        out.push(positive);
    }
    out.retain(|c| *c != n);
    out.dedup();
    out.into_iter().map(Value::Int).collect()
}

/// Toward `0.0`, and a specific value before a special one.
fn float_candidates(f: f64) -> Vec<Value> {
    if f == 0.0 && f.is_sign_positive() {
        return Vec::new();
    }
    if !f.is_finite() {
        return vec![Value::Float(0.0)];
    }
    let mut out: Vec<f64> = vec![0.0];
    // The truncation is the point: a fraction shrinks to the whole number below it, which is what
    // makes `0.30000000000000004` reach `0.0` in two steps.
    let truncated = f.trunc();
    if truncated != f {
        out.push(truncated);
    }
    let mut half = f / 2.0;
    for _ in 0..64 {
        if half == 0.0 || !half.is_finite() {
            break;
        }
        out.push(half);
        half /= 2.0;
    }
    if f.is_sign_negative() {
        out.push(-f);
    }
    // `!=` and not `total_cmp`, so `-0.0` is dropped against a `0.0` candidate and the walk cannot
    // cycle between two values the language calls equal.
    out.retain(|c| *c != f);
    out.into_iter().map(Value::Float).collect()
}

/// Toward `0m`, and toward scale 0 — the trailing zeros go before the digits do, so a witness reads
/// `1.5m` rather than `1.500000m`.
fn decimal_candidates(d: Decimal) -> Vec<Value> {
    if d.is_zero() && d.scale() == 0 {
        return Vec::new();
    }
    let mut out: Vec<Decimal> = vec![Decimal::ZERO];
    let normalized = d.normalize();
    if normalized.scale() != d.scale() {
        out.push(normalized);
    }
    // Truncation, shortest first: `12.345m` offers `12m`, then `12.3m`, then `12.34m`.
    for places in 0..d.scale() {
        out.push(d.round_dp_with_strategy(places, RoundingStrategy::ToZero));
    }
    // Then the magnitude, which is what moves an integer witness.
    let mut half = d;
    for _ in 0..96 {
        match half.checked_div(Decimal::TWO) {
            Some(next) if !next.is_zero() && next.scale() <= d.scale() => {
                half = next;
                out.push(half);
            }
            _ => break,
        }
    }
    if d.is_sign_negative() {
        out.push(-d);
    }
    let here = decimal_size(d);
    out.retain(|c| decimal_size(*c) < here);
    out.dedup();
    out.into_iter().map(Value::Decimal).collect()
}

/// Length before content, which is the order that makes a minimal witness readable: `b""`, then the
/// two halves, then each byte lowered toward zero.
fn bytes_candidates(b: &[u8]) -> Vec<Value> {
    if b.is_empty() {
        return Vec::new();
    }
    let mut out = vec![Value::bytes([])];
    if b.len() >= 2 {
        let mid = b.len() / 2;
        out.push(Value::bytes(&b[..mid]));
        out.push(Value::bytes(&b[mid..]));
    }
    for (i, byte) in b.iter().enumerate() {
        for lowered in [0, byte / 2] {
            if lowered == *byte {
                continue;
            }
            let mut next = b.to_vec();
            next[i] = lowered;
            out.push(Value::bytes(next));
        }
    }
    out
}

/// `""`, the two halves, then each character lowered toward `'a'`, left to right.
fn string_candidates(s: &str) -> Vec<Value> {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut out = vec![Value::str("")];
    if chars.len() >= 2 {
        let mid = chars.len() / 2;
        out.push(Value::str(chars[..mid].iter().collect::<String>()));
        out.push(Value::str(chars[mid..].iter().collect::<String>()));
    }
    for (i, c) in chars.iter().enumerate() {
        let code = *c as u32;
        let floor = 'a' as u32;
        if code <= floor {
            continue;
        }
        let midpoint = floor + (code - floor) / 2;
        for lowered in [floor, midpoint] {
            if lowered == code {
                continue;
            }
            let Some(lowered) = char::from_u32(lowered) else {
                continue;
            };
            let mut next = chars.clone();
            next[i] = lowered;
            out.push(Value::str(next.into_iter().collect::<String>()));
        }
    }
    out
}

/// `[]`, the two halves, each single element removed, then each element shrunk in place.
fn list_candidates(items: &List, elem: &Type, world: &TypeWorld, depth: u32) -> Vec<Value> {
    if items.is_empty() {
        return Vec::new();
    }
    let mut out = vec![Value::list(Vec::new())];
    if items.len() >= 2 {
        let mid = items.len() / 2;
        out.push(Value::list(items.iter().take(mid).cloned().collect()));
        out.push(Value::list(items.iter().skip(mid).cloned().collect()));
    }
    for i in 0..items.len() {
        let mut next = items.to_vec();
        next.remove(i);
        out.push(Value::list(next));
    }
    for (i, item) in items.iter().enumerate() {
        for candidate in candidates_at(item, elem, world, depth + 1) {
            let mut next = items.to_vec();
            next[i] = candidate;
            out.push(Value::list(next));
        }
    }
    out
}

/// The empty map, then each entry dropped, then each value shrunk, then each key shrunk.
fn map_candidates(
    entries: &ply_eval::Map,
    key: &Type,
    value: &Type,
    world: &TypeWorld,
    depth: u32,
) -> Vec<Value> {
    if entries.is_empty() {
        return Vec::new();
    }
    let pairs: Vec<(Value, Value)> = entries
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut out = vec![Value::empty_map()];
    for i in 0..pairs.len() {
        let mut next = pairs.clone();
        next.remove(i);
        out.push(Value::map(next));
    }
    for i in 0..pairs.len() {
        for candidate in candidates_at(&pairs[i].1, value, world, depth + 1) {
            let mut next = pairs.clone();
            next[i].1 = candidate;
            out.push(Value::map(next));
        }
    }
    for i in 0..pairs.len() {
        for candidate in candidates_at(&pairs[i].0, key, world, depth + 1) {
            let mut next = pairs.clone();
            next[i].0 = candidate;
            out.push(Value::map(next));
        }
    }
    out
}

/// A recursive field, then a lower-index constructor, then each field shrunk in place.
fn ctor_candidates(
    ctor: &Symbol,
    args: &Arc<Vec<Value>>,
    ty_name: &Symbol,
    ty_args: &[Type],
    ty: &Type,
    world: &TypeWorld,
    depth: u32,
) -> Vec<Value> {
    let Some(variants) = world.variants(ty_name) else {
        return Vec::new();
    };
    let Some(variant) = variants.iter().find(|v| &v.name == ctor) else {
        return Vec::new();
    };
    let fields = world.fields(ty_name, variant, ty_args);
    if fields.len() != args.len() {
        return Vec::new();
    }
    let mut out = Vec::new();

    for (i, field) in fields.iter().enumerate() {
        if field == ty {
            out.push(args[i].clone());
        }
    }

    for lower in variants.iter().filter(|v| v.index < variant.index) {
        let wanted = world.fields(ty_name, lower, ty_args);
        let mut used = vec![false; args.len()];
        let mut filled = Vec::with_capacity(wanted.len());
        let mut buildable = true;
        for want in &wanted {
            match fields
                .iter()
                .enumerate()
                .find(|(j, have)| !used[*j] && *have == want)
            {
                Some((j, _)) => {
                    used[j] = true;
                    filled.push(args[j].clone());
                }
                None => match minimal(want, world) {
                    Ok(v) => filled.push(v),
                    Err(_) => {
                        buildable = false;
                        break;
                    }
                },
            }
        }
        if buildable {
            out.push(Value::ctor(lower.name.clone(), filled));
        }
    }

    for (i, field) in fields.iter().enumerate() {
        for candidate in candidates_at(&args[i], field, world, depth + 1) {
            let mut next = (**args).clone();
            next[i] = candidate;
            out.push(Value::ctor(ctor.clone(), next));
        }
    }

    out
}

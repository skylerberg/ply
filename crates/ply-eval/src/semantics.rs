//! A node's meaning and diagnostics, shared by every evaluation strategy.

use crate::handler::OpDecl;
use crate::value::{Closure, ClosureKind, Decimal, Fixed, Value, type_error, values_equal};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Ident, QName};
use ply_ty::{BinOp, Lit, Mode, UnOp};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::sync::Arc;

/// Keyed by program-wide effect name and operation name.
pub(crate) type OpTable = FxHashMap<(Symbol, Symbol), (bool, Mode)>;

pub(crate) fn op_decl(ops: &OpTable, effect: &Symbol, op: &Symbol) -> OpDecl {
    match ops.get(&(effect.clone(), op.clone())) {
        Some(&(resource_param, mode)) => OpDecl::Declared {
            resource_param,
            mode,
        },
        None if ops.keys().any(|(e, _)| e == effect) => OpDecl::NoSuchOp,
        None => OpDecl::UnknownEffect,
    }
}

pub(crate) fn literal(lit: &Lit) -> Value {
    match lit {
        Lit::Int(i) => Value::Int(*i),
        Lit::Fixed { ty, bits } => Value::Fixed(Fixed::new(*ty, *bits)),
        Lit::Bool(b) => Value::Bool(*b),
        Lit::Str(s) => Value::str(s),
        Lit::Bytes(b) => Value::bytes(b),
        Lit::Float(f) => Value::Float(*f),
        Lit::Decimal { mantissa, scale } => Value::Decimal(decimal_lit(*mantissa, *scale)),
        Lit::Unit => Value::Unit,
    }
}

/// The fallback is unreachable: the lexer and body decoder already enforce `Decimal`'s range.
pub(crate) fn decimal_lit(mantissa: i128, scale: u32) -> Decimal {
    Decimal::try_from_i128_with_scale(mantissa, scale).unwrap_or(Decimal::ZERO)
}

/// `Float` matches by IEEE `==`, so a `NaN` pattern matches nothing, not even a NaN scrutinee.
pub fn lit_matches(lit: &Lit, value: &Value) -> bool {
    match (lit, value) {
        (Lit::Int(a), Value::Int(b)) => a == b,
        (Lit::Bool(a), Value::Bool(b)) => a == b,
        (Lit::Str(a), Value::Str(b)) => a.as_str() == b.as_ref(),
        (Lit::Bytes(a), Value::Bytes(b)) => a.as_slice() == b.as_ref(),
        (Lit::Float(a), Value::Float(b)) => a == b,
        // By numeric value, matching `==`: a `1.50m` pattern matches `1.5m`.
        (Lit::Decimal { mantissa, scale }, Value::Decimal(b)) => {
            decimal_lit(*mantissa, *scale) == *b
        }
        (Lit::Unit, Value::Unit) => true,
        _ => false,
    }
}

/// Constructor values cached per thread; past the bound the rest are built per mention.
pub const CTOR_CACHE_KEEP: usize = 4096;

thread_local! {
    /// Holds the arity too: two programs on one thread can give one name two arities.
    pub static CTOR_VALUES: RefCell<FxHashMap<Symbol, (usize, Value)>> =
        RefCell::new(FxHashMap::default());
}

/// A constructor mention's value, shared per thread; the value is immutable and identity-free.
pub fn ctor_value(name: &Symbol, arity: usize) -> Value {
    let fresh = || {
        if arity == 0 {
            Value::ctor(name.clone(), Vec::new())
        } else {
            Value::Closure(Arc::new(Closure {
                name: Some(name.clone()),
                kind: ClosureKind::Ctor {
                    name: name.clone(),
                    arity,
                },
            }))
        }
    };
    // `try_with`: a `Value` dropped in thread-local teardown can arrive after the cache is gone.
    CTOR_VALUES
        .try_with(|cache| {
            let mut cache = cache.borrow_mut();
            match cache.get(name) {
                Some((at, value)) if *at == arity => value.clone(),
                Some(_) => {
                    let value = fresh();
                    cache.insert(name.clone(), (arity, value.clone()));
                    value
                }
                None => {
                    let value = fresh();
                    if cache.len() < CTOR_CACHE_KEEP {
                        cache.insert(name.clone(), (arity, value.clone()));
                    }
                    value
                }
            }
        })
        .unwrap_or_else(|_| fresh())
}

#[inline(never)]
pub fn strict_binary(
    op: BinOp,
    l: &Value,
    r: &Value,
    lspan: Span,
    rspan: Span,
    span: Span,
) -> Result<Value, Diagnostic> {
    match op {
        BinOp::Eq => Ok(Value::Bool(values_equal(l, r, span)?)),
        BinOp::Ne => Ok(Value::Bool(!values_equal(l, r, span)?)),
        BinOp::Concat => {
            let a = l.as_str(lspan, "`++`")?;
            let b = r.as_str(rspan, "`++`")?;
            Ok(Value::str(format!("{a}{b}")))
        }
        // IEEE: `NaN < x` and `NaN >= x` are both false; a comparison is not its converse negated.
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            if let (Value::Float(a), Value::Float(b)) = (l, r) {
                return Ok(Value::Bool(match op {
                    BinOp::Lt => a < b,
                    BinOp::Le => a <= b,
                    BinOp::Gt => a > b,
                    _ => a >= b,
                }));
            }
            let ordering = match (l, r) {
                (Value::Int(a), Value::Int(b)) => a.cmp(b),
                // By value, not bits, so `I8` puts `-1` below `0`.
                (Value::Fixed(a), Value::Fixed(b)) if a.ty == b.ty => a.value().cmp(&b.value()),
                (Value::Str(a), Value::Str(b)) => a.as_ref().cmp(b.as_ref()),
                (Value::Decimal(a), Value::Decimal(b)) => a.cmp(b),
                (
                    Value::Int(_)
                    | Value::Fixed(_)
                    | Value::Str(_)
                    | Value::Decimal(_)
                    | Value::Float(_),
                    other,
                ) => {
                    return Err(type_error(rspan, "a comparison", l.type_name(), other));
                }
                (other, _) => {
                    return Err(type_error(
                        lspan,
                        "a comparison",
                        "Int, String, Float or Decimal",
                        other,
                    ));
                }
            };
            Ok(Value::Bool(match op {
                BinOp::Lt => ordering.is_lt(),
                BinOp::Le => ordering.is_le(),
                BinOp::Gt => ordering.is_gt(),
                _ => ordering.is_ge(),
            }))
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            match (l, r) {
                (Value::Float(a), Value::Float(b)) => return float_arithmetic(op, *a, *b, span),
                (Value::Decimal(a), Value::Decimal(b)) => {
                    return decimal_arithmetic(op, *a, *b, rspan, span);
                }
                (Value::Fixed(a), Value::Fixed(b)) if a.ty == b.ty => {
                    return fixed_arithmetic(op, *a, *b, rspan, span);
                }
                _ => {}
            }
            let a = l.as_int(lspan, "arithmetic")?;
            let b = r.as_int(rspan, "arithmetic")?;
            let (result, what) = match op {
                BinOp::Add => (a.checked_add(b), "addition"),
                BinOp::Sub => (a.checked_sub(b), "subtraction"),
                BinOp::Mul => (a.checked_mul(b), "multiplication"),
                BinOp::Div if b == 0 => return Err(err_zero_divisor(rspan, "division")),
                BinOp::Div => (a.checked_div(b), "division"),
                _ if b == 0 => return Err(err_zero_divisor(rspan, "remainder")),
                _ => (a.checked_rem(b), "remainder"),
            };
            match result {
                Some(n) => Ok(Value::Int(n)),
                None => Err(err_overflow(span, what, a, b)),
            }
        }
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
            if let (Value::Fixed(a), Value::Fixed(b)) = (l, r)
                && a.ty == b.ty
            {
                let (x, y) = (a.bits(), b.bits());
                let bits = match op {
                    BinOp::BitAnd => x & y,
                    BinOp::BitOr => x | y,
                    _ => x ^ y,
                };
                return Ok(Value::Fixed(Fixed::new(a.ty, bits)));
            }
            let a = l.as_int(lspan, "a bit operator")?;
            let b = r.as_int(rspan, "a bit operator")?;
            Ok(Value::Int(match op {
                BinOp::BitAnd => a & b,
                BinOp::BitOr => a | b,
                _ => a ^ b,
            }))
        }

        // An out-of-range count raises, but `<<` itself drops the bits that leave.
        BinOp::Shl | BinOp::Shr | BinOp::Ushr => {
            let n = r.as_int(rspan, "a shift")?;
            if let Value::Fixed(a) = l {
                let width = i64::from(a.ty.bits());
                if !(0..width).contains(&n) {
                    return Err(err_shift_count_at(rspan, n, a.ty.name(), width));
                }
                let n = n as u32;
                let raw = a.raw();
                let bits = match op {
                    BinOp::Shl => raw << n,
                    // `value()` is non-negative when unsigned, so this zero-fills there.
                    BinOp::Shr => (a.value() >> n) as u64,
                    _ => raw >> n,
                };
                return Ok(Value::Fixed(Fixed::new(a.ty, bits)));
            }
            let a = l.as_int(lspan, "a shift")?;
            if !(0..64).contains(&n) {
                return Err(err_shift_count(rspan, n));
            }
            let n = n as u32;
            Ok(Value::Int(match op {
                BinOp::Shl => ((a as u64) << n) as i64,
                BinOp::Shr => a >> n,
                _ => ((a as u64) >> n) as i64,
            }))
        }

        BinOp::And | BinOp::Or => Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "internal error: a short-circuiting operator reached strict evaluation",
        )
        .primary(span, "please report this")),
    }
}

/// IEEE-754, unmodified: no overflow or zero-divisor error; `Infinity` and `NaN` are values.
fn float_arithmetic(op: BinOp, a: f64, b: f64, span: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Float(match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => a / b,
        BinOp::Rem => a % b,
        _ => {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "internal error: a non-arithmetic operator reached float arithmetic",
            )
            .primary(span, "please report this"));
        }
    }))
}

/// Exact, or a diagnostic; never a silent wrap or rounding. Inference already refuses `/`.
fn decimal_arithmetic(
    op: BinOp,
    a: Decimal,
    b: Decimal,
    rspan: Span,
    span: Span,
) -> Result<Value, Diagnostic> {
    let (result, what) = match op {
        BinOp::Add => (a.checked_add(b), "addition"),
        BinOp::Sub => (a.checked_sub(b), "subtraction"),
        // Half-to-even past scale 28; a mantissa past 96 bits is `None`, reported not rounded.
        BinOp::Mul => (a.checked_mul(b), "multiplication"),
        BinOp::Rem => {
            if b.is_zero() {
                return Err(err_zero_divisor(rspan, "remainder"));
            }
            (a.checked_rem(b), "remainder")
        }
        BinOp::Div => {
            return Err(Diagnostic::error(
                codes::DECIMAL_DIVISION,
                "`/` is not defined on `Decimal`",
            )
            .primary(span, "the exact quotient of two decimals is not a decimal")
            .note("call `decimal_div(a, b, scale, HalfEven)` and say how to round"));
        }
        _ => {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "internal error: a non-arithmetic operator reached decimal arithmetic",
            )
            .primary(span, "please report this"));
        }
    };
    match result {
        Some(d) => Ok(Value::Decimal(d)),
        None => Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`Decimal` overflow in {what}"),
        )
        .primary(
            span,
            format!("{a} and {b} need more than 96 bits of mantissa"),
        )
        .note("`Decimal` is exact and bounded; it will not round to make room")),
    }
}

pub(crate) fn arity_error(span: Span, what: &str, expected: usize, got: usize) -> Diagnostic {
    Diagnostic::error(
        codes::ARITY_MISMATCH,
        format!(
            "{what} takes {expected} argument{}, but {got} were given",
            plural(expected)
        ),
    )
    .primary(span, format!("{got} argument{} here", plural(got)))
}

#[cold]
#[inline(never)]
pub(crate) fn err_unknown_name(q: &QName) -> Diagnostic {
    Diagnostic::error(
        codes::UNKNOWN_NAME,
        format!("cannot find `{q}` in this scope"),
    )
    .primary(q.span, "not bound here")
}

#[cold]
#[inline(never)]
pub(crate) fn err_not_a_function(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NOT_A_FUNCTION,
        format!("cannot call a value of type {}", v.type_name()),
    )
    .primary(span, format!("this is {}", v.render()))
}

#[cold]
#[inline(never)]
pub(crate) fn err_non_exhaustive(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NON_EXHAUSTIVE_MATCH,
        "no match arm applied to the scrutinee",
    )
    .primary(span, format!("this evaluated to {}", v.render()))
    .note("add an arm covering this value, or a `_` catch-all")
}

#[cold]
#[inline(never)]
pub(crate) fn err_let_mismatch(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NON_EXHAUSTIVE_MATCH,
        "`let` pattern did not match the bound value",
    )
    .primary(span, format!("value was {}", v.render()))
    .note("use `match` when the pattern can fail")
}

#[cold]
#[inline(never)]
pub(crate) fn err_no_such_field(field: &Ident, fields: &crate::value::Fields) -> Diagnostic {
    let known: Vec<String> = fields.keys().map(|k| format!("`{k}`")).collect();
    Diagnostic::error(
        codes::UNKNOWN_NAME,
        format!("record has no field `{}`", field.name),
    )
    .primary(field.span, "no such field")
    .note(if known.is_empty() {
        "the record is empty".to_string()
    } else {
        format!("available fields: {}", known.join(", "))
    })
}

#[cold]
#[inline(never)]
pub(crate) fn err_shift_count(span: Span, n: i64) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, "shift count out of range")
        .primary(span, format!("{n} is not in 0..=63"))
        .note("an `Int` is 64 bits, so no other count names a shift of it")
}

#[cold]
#[inline(never)]
pub(crate) fn err_shift_count_at(span: Span, n: i64, ty: &str, width: i64) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, "shift count out of range")
        .primary(span, format!("{n} is not in 0..={}", width - 1))
        .note(format!(
            "a `{ty}` is {width} bits, so no other count names a shift of it"
        ))
}

#[cold]
#[inline(never)]
pub(crate) fn err_fixed_overflow(span: Span, what: &str, a: Fixed, b: Fixed) -> Diagnostic {
    let detail = if what == "negation" {
        format!("-{a} does not fit in {}", a.ty)
    } else {
        format!("{a} and {b} overflow {}", a.ty)
    };
    Diagnostic::error(codes::RUNTIME_ERROR, format!("integer overflow in {what}"))
        .primary(span, detail)
}

/// Exact or a diagnostic at the operands' width; wrapping is `wrap_add` and its siblings.
fn fixed_arithmetic(
    op: BinOp,
    a: Fixed,
    b: Fixed,
    rspan: Span,
    span: Span,
) -> Result<Value, Diagnostic> {
    let (result, what) = match op {
        BinOp::Add => (a.checked(b, i128::checked_add), "addition"),
        BinOp::Sub => (a.checked(b, i128::checked_sub), "subtraction"),
        // Two large `U64`s overflow `i128` too, which is still an overflow of the narrow type.
        BinOp::Mul => (a.checked(b, i128::checked_mul), "multiplication"),
        BinOp::Div if b.value() == 0 => return Err(err_zero_divisor(rspan, "division")),
        BinOp::Div => (a.checked(b, i128::checked_div), "division"),
        _ if b.value() == 0 => return Err(err_zero_divisor(rspan, "remainder")),
        _ => (a.checked(b, i128::checked_rem), "remainder"),
    };
    match result {
        Some(v) => Ok(Value::Fixed(v)),
        None => Err(err_fixed_overflow(span, what, a, b)),
    }
}

#[cold]
#[inline(never)]
pub(crate) fn err_zero_divisor(span: Span, what: &str) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, format!("{what} by zero"))
        .primary(span, "this divisor is 0")
}

#[cold]
#[inline(never)]
pub(crate) fn err_overflow(span: Span, what: &str, a: i64, b: i64) -> Diagnostic {
    let detail = if what == "negation" {
        format!("-{a} does not fit in Int")
    } else {
        format!("{a} and {b} overflow Int")
    };
    Diagnostic::error(codes::RUNTIME_ERROR, format!("integer overflow in {what}"))
        .primary(span, detail)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

pub(crate) fn apply_unary(
    op: UnOp,
    value: &Value,
    operand_span: Span,
    span: Span,
) -> Result<Value, Diagnostic> {
    match op {
        // Not redundant: negation is how a program reaches `-0.0`.
        UnOp::Neg => match value {
            Value::Float(f) => Ok(Value::Float(-f)),
            Value::Decimal(d) => Ok(Value::Decimal(-*d)),
            // At an unsigned type `-x` overflows for every `x` but zero.
            Value::Fixed(f) => match Fixed::of(f.ty, -f.value()) {
                Some(n) => Ok(Value::Fixed(n)),
                None => Err(err_fixed_overflow(span, "negation", *f, *f)),
            },
            _ => {
                let i = value.as_int(operand_span, "negation")?;
                match i.checked_neg() {
                    Some(n) => Ok(Value::Int(n)),
                    None => Err(err_overflow(span, "negation", i, 0)),
                }
            }
        },
        UnOp::Not => Ok(Value::Bool(!value.as_bool(operand_span, "`!`")?)),
        UnOp::BitNot => match value {
            Value::Fixed(f) => Ok(Value::Fixed(Fixed::new(f.ty, !f.bits()))),
            _ => Ok(Value::Int(!value.as_int(operand_span, "`~`")?)),
        },
    }
}

pub(crate) fn short_circuits(op: BinOp, lhs: bool) -> bool {
    lhs == matches!(op, BinOp::Or)
}

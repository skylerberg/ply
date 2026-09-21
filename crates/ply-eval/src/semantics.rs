//! An operator's meaning and the diagnostics shared by every evaluation strategy.

use crate::value::{Decimal, Fixed, Value, type_error, values_equal};
use ply_span::{Diagnostic, Span, codes};
use ply_ty::BinOp;

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
        // Two strings or two byte strings; the checker refuses one of each, so neither side coerces.
        BinOp::Concat => match (l, r) {
            (Value::Bytes(a), Value::Bytes(b)) => {
                let mut out = Vec::with_capacity(a.len() + b.len());
                out.extend_from_slice(a);
                out.extend_from_slice(b);
                Ok(Value::bytes(out))
            }
            (Value::Bytes(_), other) => Err(type_error(rspan, "`++`", "Bytes", other)),
            (other, Value::Bytes(_)) => Err(type_error(lspan, "`++`", "Bytes", other)),
            _ => {
                let a = l.as_str(lspan, "`++`")?;
                let b = r.as_str(rspan, "`++`")?;
                Ok(Value::str(format!("{a}{b}")))
            }
        },
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

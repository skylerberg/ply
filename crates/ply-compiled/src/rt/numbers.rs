//! Arithmetic over `Float`s' bits, `Decimal`s' and 128-bit widths' words, their boxes, and the
//! failures compiled arithmetic and matching raise.

use super::*;

/// `a % b` over two `Float`s' bits, which C reaches only through `fmod`.
pub unsafe extern "C" fn rt_float_rem(_ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    let (a, b) = (f64::from_bits(a as u64), f64::from_bits(b as u64));
    ply_core::float::bits(a % b) as i64
}

/// The operator codes compiled code hands [`rt_decimal_binary`]; `emit/c.ply`'s `binary_code`
/// must match.
pub(super) const BINOPS: [BinOp; 18] = [
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Rem,
    BinOp::Eq,
    BinOp::Ne,
    BinOp::Lt,
    BinOp::Le,
    BinOp::Gt,
    BinOp::Ge,
    BinOp::Concat,
    BinOp::BitAnd,
    BinOp::BitOr,
    BinOp::BitXor,
    BinOp::Shl,
    BinOp::Shr,
    BinOp::Ushr,
];

/// The box of a 128-bit width, the width at `t` in [`ply_core::INT_TYPES`], whose bits are `lo`
/// and above it `hi`.
pub unsafe extern "C" fn rt_box_wide(ctx: *mut Ctx, lo: I64, hi: I64, t: I64) -> Word {
    let ctx = unsafe { &mut *ctx };
    let wide = usize::try_from(t.0)
        .ok()
        .and_then(|i| ply_core::INT_TYPES.get(i))
        .is_some_and(|ty| ty.bits() == 128);
    if !wide {
        return ctx.fail(error(format!("width {} is no 128-bit width", t.0)));
    }
    two_words(ctx, lo.0, hi.0)
}

/// A block of two scalar words, `lo` and then `hi`, which compiled code releases by its size.
fn two_words(ctx: &mut Ctx, lo: i64, hi: i64) -> Word {
    let o = ctx.heap.raw_alloc(16);
    unsafe {
        set_word(o, 0, lo);
        set_word(o, 1, hi);
    }
    o as Word
}

/// The `Decimal` two words of compiled code's `PlyDec` hold.
pub(super) fn decimal_of(lo: i64, hi: i64) -> ply_core::Decimal {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&lo.to_le_bytes());
    bytes[8..].copy_from_slice(&hi.to_le_bytes());
    ply_core::Decimal::deserialize(bytes)
}

/// Writes `d`'s two words at `out`, compiled code's `PlyDec`.
pub(super) unsafe fn write_decimal(out: i64, d: ply_core::Decimal) {
    let bytes = d.serialize();
    let words = out as *mut i64;
    unsafe {
        words.write(i64::from_le_bytes(
            bytes[..8].try_into().expect("eight bytes"),
        ));
        words.add(1).write(i64::from_le_bytes(
            bytes[8..].try_into().expect("eight bytes"),
        ));
    }
}

/// The box of the `Decimal` whose two words are `lo` and `hi`.
pub unsafe extern "C" fn rt_box_decimal(ctx: *mut Ctx, lo: i64, hi: i64) -> i64 {
    two_words(unsafe { &mut *ctx }, lo, hi)
}

/// An arithmetic operator, `op` in `BINOPS`, over two `Decimal`s' words: its answer's two words
/// written at `out`, raised as the evaluator raises.
pub unsafe extern "C" fn rt_decimal_binary(
    ctx: *mut Ctx,
    op: i64,
    a_lo: i64,
    a_hi: i64,
    b_lo: i64,
    b_hi: i64,
    out: i64,
) {
    let ctx = unsafe { &mut *ctx };
    let Some(op) = usize::try_from(op)
        .ok()
        .and_then(|i| BINOPS.get(i).copied())
    else {
        ctx.fail(error("an operator this runtime has no code for"));
        return;
    };
    let (x, y) = (
        Value::Decimal(decimal_of(a_lo, a_hi)),
        Value::Decimal(decimal_of(b_lo, b_hi)),
    );
    match ply_core::strict_binary(op, &x, &y, Span::DUMMY, Span::DUMMY, Span::DUMMY) {
        Ok(Value::Decimal(d)) => unsafe { write_decimal(out, d) },
        Ok(other) => {
            ctx.fail(error(format!(
                "a `Decimal` operator answered {}",
                other.type_name()
            )));
        }
        Err(d) if matches!(op, BinOp::Div | BinOp::Rem) && is_zero(&y) => {
            raise_error(ctx, d);
        }
        Err(d) => {
            ctx.fail(d);
        }
    }
}

/// How two `Decimal`s' words order as `Decimal`s: -1, 0 or 1, equal at any scale that holds the
/// same number.
pub unsafe extern "C" fn rt_decimal_compare(
    _ctx: *mut Ctx,
    a_lo: i64,
    a_hi: i64,
    b_lo: i64,
    b_hi: i64,
) -> i64 {
    decimal_of(a_lo, a_hi).cmp(&decimal_of(b_lo, b_hi)) as i64
}

/// A `Decimal`'s negation, its two words written at `out`.
pub unsafe extern "C" fn rt_decimal_negate(_ctx: *mut Ctx, lo: i64, hi: i64, out: i64) {
    unsafe { write_decimal(out, -decimal_of(lo, hi)) }
}

/// A divisor `/` and `%` raise on rather than overflow.
pub(super) fn is_zero(v: &Value) -> bool {
    match v {
        Value::Int(n) => *n == 0,
        Value::Fixed(f) => f.raw() == 0,
        Value::Decimal(d) => d.is_zero(),
        _ => false,
    }
}

/// A `match` whose arms did not cover the value.
pub unsafe extern "C" fn rt_no_match(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("no arm of this `match` matched");
    ctx.fail(d);
}

/// A refutable `let` whose pattern did not match the value bound to it.
pub unsafe extern "C" fn rt_let_no_match(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("`let` pattern did not match the bound value");
    raise_error(ctx, d);
}

/// An `Int` divided by zero, raised: `what` 0 a division's, any other a remainder's.
pub unsafe extern "C" fn rt_by_zero(ctx: *mut Ctx, what: i64) {
    let ctx = unsafe { &mut *ctx };
    let name = if what == 0 { "division" } else { "remainder" };
    raise_error(ctx, error(format!("{name} by zero")));
}

/// `Int::of_u64` of `value`'s bits, past what an `Int` holds, raised.
pub unsafe extern "C" fn rt_not_an_int(ctx: *mut Ctx, value: i64) {
    let ctx = unsafe { &mut *ctx };
    let d = error(format!("`Int::of_u64` was given {}", value as u64))
        .note("an `Int` is 64 bits and signed, so it does not hold every `U64`");
    raise_error(ctx, d);
}

/// `Int::of_i128`, where `signed` is not 0, or else `Int::of_u128`, of the 128-bit width whose
/// words are `lo` and `hi`, past what an `Int` holds, raised.
pub unsafe extern "C" fn rt_not_an_int_wide(ctx: *mut Ctx, lo: i64, hi: i64, signed: i64) {
    let ctx = unsafe { &mut *ctx };
    let (ty, b) = if signed != 0 {
        (ply_core::IntTy::I128, Builtin::IntOfI128)
    } else {
        (ply_core::IntTy::U128, Builtin::IntOfU128)
    };
    let x = ply_core::Fixed::new(ty, (u128::from(hi as u64) << 64) | u128::from(lo as u64));
    match ply_core::builtins::call(b, vec![Value::Fixed(x)], Span::DUMMY) {
        Err(d) => {
            raise_error(ctx, d);
        }
        Ok(_) => {
            ctx.fail(error("a 128-bit width an `Int` holds was refused as one"));
        }
    }
}

/// A `Decimal` operator's answer past what a `Decimal` holds exactly; `what` is `emit/c.ply`'s
/// `overflow_code`.
pub unsafe extern "C" fn rt_decimal_overflow(ctx: *mut Ctx, what: i64) {
    let ctx = unsafe { &mut *ctx };
    let name = match what {
        0 => "addition",
        1 => "subtraction",
        3 => "multiplication",
        _ => "remainder",
    };
    let d = error(format!("`Decimal` overflow in {name}"))
        .note("`Decimal` is exact and bounded; it will not round to make room");
    ctx.fail(d);
}

/// `what` is `emit/c.ply`'s `overflow_code`.
pub unsafe extern "C" fn rt_overflow(ctx: *mut Ctx, what: i64) {
    let ctx = unsafe { &mut *ctx };
    let name = match what {
        0 => "addition",
        1 => "subtraction",
        2 => "negation",
        3 => "multiplication",
        4 => "division",
        _ => "remainder",
    };
    let d = error(format!("integer overflow in {name}"));
    ctx.fail(d);
}

/// An `Int` outside the target width; `which` indexes [`ply_core::INT_TYPES`].
pub unsafe extern "C" fn rt_not_that_width(ctx: *mut Ctx, which: i64, value: i64) {
    let ctx = unsafe { &mut *ctx };
    let t = ply_core::INT_TYPES[which as usize];
    let d = error(format!(
        "`{t}::of_int` was given {value}: `{t}` holds {} to {}",
        t.min(),
        t.max()
    ));
    raise_error(ctx, d);
}

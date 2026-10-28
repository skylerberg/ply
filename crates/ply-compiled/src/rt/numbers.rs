//! Arithmetic over `Float`s' bits, `Decimal`s' and 128-bit widths' words, their boxes, and the
//! failures compiled arithmetic and matching raise.

use super::*;

/// `a % b` over two `Float`s' bits, which C reaches only through `fmod`.
pub unsafe extern "C" fn rt_float_rem(_ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    let (a, b) = (f64::from_bits(a as u64), f64::from_bits(b as u64));
    ply_core::float::bits(a % b) as i64
}

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

/// An arithmetic operator over two `Decimal`s' words, as an earlier release's emitter codes it
/// (`0` add, `1` subtract, `2` multiply, `4` remainder): its answer's two words written at `out`.
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
    let (a, b) = (decimal_of(a_lo, a_hi), decimal_of(b_lo, b_hi));
    let (answer, what) = match op {
        0 => (a.checked_add(b), "addition"),
        1 => (a.checked_sub(b), "subtraction"),
        2 => (a.checked_mul(b), "multiplication"),
        4 if b.is_zero() => {
            raise_error(ctx, error("remainder by zero"));
            return;
        }
        4 => (a.checked_rem(b), "remainder"),
        _ => {
            ctx.fail(error("an operator this runtime has no code for"));
            return;
        }
    };
    match answer {
        Some(d) => unsafe { write_decimal(out, d) },
        None => {
            ctx.fail(
                error(format!("`Decimal` overflow in {what}"))
                    .note("`Decimal` is exact and bounded; it will not round to make room"),
            );
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
    let bits = (u128::from(hi as u64) << 64) | u128::from(lo as u64);
    let (name, ty, given) = if signed != 0 {
        ("Int::of_i128", "I128", (bits as i128).to_string())
    } else {
        ("Int::of_u128", "U128", bits.to_string())
    };
    let d = error(format!("`{name}` was given {given}")).note(format!(
        "an `Int` is 64 bits and signed, so it does not hold every `{ty}`"
    ));
    raise_error(ctx, d);
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

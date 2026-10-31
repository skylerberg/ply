use super::*;

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

fn two_words(ctx: &mut Ctx, lo: i64, hi: i64) -> Word {
    let o = ctx.heap.raw_alloc(16);
    unsafe {
        set_word(o, 0, lo);
        set_word(o, 1, hi);
    }
    o as Word
}

pub unsafe extern "C" fn rt_box_decimal(ctx: *mut Ctx, lo: i64, hi: i64) -> i64 {
    two_words(unsafe { &mut *ctx }, lo, hi)
}

pub unsafe extern "C" fn rt_no_match(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("no arm of this `match` matched");
    ctx.fail(d);
}

pub unsafe extern "C" fn rt_let_no_match(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("`let` pattern did not match the bound value");
    raise_error(ctx, d);
}

pub unsafe extern "C" fn rt_by_zero(ctx: *mut Ctx, what: i64) {
    let ctx = unsafe { &mut *ctx };
    let name = if what == 0 { "division" } else { "remainder" };
    raise_error(ctx, error(format!("{name} by zero")));
}

pub unsafe extern "C" fn rt_not_an_int(ctx: *mut Ctx, value: i64) {
    let ctx = unsafe { &mut *ctx };
    let d = error(format!("`Int::of_u64` was given {}", value as u64))
        .note("an `Int` is 64 bits and signed, so it does not hold every `U64`");
    raise_error(ctx, d);
}

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

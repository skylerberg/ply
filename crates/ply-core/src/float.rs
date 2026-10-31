//! What a program can observe of a `Float`, and the functions over one the prelude computes with.
//!
//! A program sees one NaN. Hardware does not agree on the NaN an operation makes, so a value may
//! carry any, and every place its bits become observable — [`bits`], an encoding, an order —
//! reads every NaN as [`CANONICAL_NAN_BITS`]. The math functions are CORE-MATH's, correctly
//! rounded, so each result is the one the specification fixes on every machine.

/// The bits every NaN reads as.
pub const CANONICAL_NAN_BITS: u64 = 0x7FF8_0000_0000_0000;

const EXPONENT: u64 = 0x7FF0_0000_0000_0000;
const MANTISSA: u64 = 0x000F_FFFF_FFFF_FFFF;

/// `x`'s IEEE-754 bits, any NaN read as [`CANONICAL_NAN_BITS`].
#[allow(clippy::disallowed_methods)]
pub fn bits(x: f64) -> u64 {
    // Decided on the bits: the compiler may treat any NaN as any other, so a choice between two
    // NaNs made on floats can be folded away.
    let b = x.to_bits();
    if b & EXPONENT == EXPONENT && b & MANTISSA != 0 {
        CANONICAL_NAN_BITS
    } else {
        b
    }
}

/// `x` with any NaN made the canonical one.
pub fn canonical(x: f64) -> f64 {
    f64::from_bits(bits(x))
}

/// The order a `Float` takes among keys: IEEE-754's total order over [`bits`], so every NaN is one
/// key, above `inf`.
pub fn total_cmp(x: f64, y: f64) -> std::cmp::Ordering {
    // `f64::total_cmp`'s key: a negative value's magnitude bits flipped, so the integers order as
    // the floats do.
    let key = |x: f64| {
        let b = bits(x) as i64;
        b ^ (((b >> 63) as u64) >> 1) as i64
    };
    key(x).cmp(&key(y))
}

/// Each named function, computed by CORE-MATH's correctly rounded one.
macro_rules! correctly_rounded {
    ($($name:ident = $cr:ident($($a:ident),+);)+) => {
        unsafe extern "C" {
            $(safe fn $cr($($a: f64),+) -> f64;)+
        }
        $(pub fn $name($($a: f64),+) -> f64 {
            $cr($($a),+)
        })+
    };
}

correctly_rounded! {
    exp = cr_exp(x);
    ln = cr_log(x);
    log2 = cr_log2(x);
    log10 = cr_log10(x);
    sin = cr_sin(x);
    cos = cr_cos(x);
    tan = cr_tan(x);
    asin = cr_asin(x);
    acos = cr_acos(x);
    atan = cr_atan(x);
    pow = cr_pow(x, y);
    atan2 = cr_atan2(y, x);
    hypot = cr_hypot(x, y);
}

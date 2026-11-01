pub const CANONICAL_NAN_BITS: u64 = 0x7FF8_0000_0000_0000;

const EXPONENT: u64 = 0x7FF0_0000_0000_0000;
const MANTISSA: u64 = 0x000F_FFFF_FFFF_FFFF;

#[allow(clippy::disallowed_methods)]
pub fn bits(x: f64) -> u64 {
    let b = x.to_bits();
    if b & EXPONENT == EXPONENT && b & MANTISSA != 0 {
        CANONICAL_NAN_BITS
    } else {
        b
    }
}

pub fn canonical(x: f64) -> f64 {
    f64::from_bits(bits(x))
}

pub fn total_cmp(x: f64, y: f64) -> std::cmp::Ordering {
    let key = |x: f64| {
        let b = bits(x) as i64;
        b ^ (((b >> 63) as u64) >> 1) as i64
    };
    key(x).cmp(&key(y))
}

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

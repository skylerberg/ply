use astro_float::{BigFloat, Consts, RoundingMode, Sign};
use ply_eval::builtins::{Builtin, call};
use ply_eval::float::{CANONICAL_NAN_BITS, bits, total_cmp};
use ply_eval::{Span, Value};
use std::cmp::Ordering;

const RM: RoundingMode = RoundingMode::ToEven;

/// The `Float` nearest `r`, ties to even, read off its mantissa words: `value = 0.m * 2^e`.
fn nearest(r: &BigFloat) -> f64 {
    if r.is_zero() {
        return 0.0;
    }
    if r.is_inf() {
        return if r.is_inf_pos() {
            f64::INFINITY
        } else {
            f64::NEG_INFINITY
        };
    }
    let (words, _, sign, e, inexact) = r.as_raw_parts().expect("a finite reference");
    let n_words = words.len();
    let m = (u128::from(words[n_words - 1]) << 64) | u128::from(words[n_words - 2]);
    let mut sticky = inexact || words[..n_words - 2].iter().any(|w| *w != 0);
    let e = i64::from(e);
    // How many leading bits the answer keeps: 53, fewer for a subnormal.
    let kept = (e + 1074).min(53);
    let (mut q, half) = if kept < 0 {
        sticky = true;
        (0u128, false)
    } else if kept == 0 {
        sticky |= m << 1 != 0;
        (0, m >> 127 == 1)
    } else {
        let shift = 128 - kept;
        sticky |= m & ((1u128 << (shift - 1)) - 1) != 0;
        (m >> shift, (m >> (shift - 1)) & 1 == 1)
    };
    if half && (sticky || q & 1 == 1) {
        q += 1;
    }
    let (mut q, mut lsb) = (q as u64, e - kept);
    if q == 1 << 53 {
        q >>= 1;
        lsb += 1;
    }
    let magnitude = if q < 1 << 52 {
        q
    } else if lsb + 52 + 1023 >= 2047 {
        0x7FF0_0000_0000_0000
    } else {
        (((lsb + 52 + 1023) as u64) << 52) | (q - (1 << 52))
    };
    let f = f64::from_bits(magnitude);
    if sign == Sign::Neg { -f } else { f }
}

/// The correctly rounded answer `r` approximates to `p` bits, or `None` where it lies too near a
/// midpoint between two `Float`s for `p` bits to say which side.
fn decided(r: &BigFloat, p: usize) -> Option<f64> {
    if r.is_nan() {
        return None;
    }
    if r.is_zero() || r.is_inf() || !r.inexact() {
        return Some(nearest(r));
    }
    let slack = r.abs().mul(
        &BigFloat::from_f64(2f64.powi(16 - p as i32), 64),
        p + 64,
        RM,
    );
    let below = nearest(&r.sub(&slack, p + 64, RM));
    let above = nearest(&r.add(&slack, p + 64, RM));
    (bits(below) == bits(above)).then_some(below)
}

/// Bits enough that argument reduction of `x` keeps 256 of them.
fn precision(x: f64) -> usize {
    256 + (x.abs().log2().max(0.0) as usize)
}

fn reference(b: Builtin, x: f64, y: f64, cc: &mut Consts) -> Option<f64> {
    let p = precision(x.max(y));
    let big = |v: f64| BigFloat::from_f64(v, p);
    let r = match b {
        Builtin::Sqrt => big(x).sqrt(p, RM),
        Builtin::Exp => big(x).exp(p, RM, cc),
        Builtin::Ln => big(x).ln(p, RM, cc),
        Builtin::Log2 => big(x).log2(p, RM, cc),
        Builtin::Log10 => big(x).log10(p, RM, cc),
        Builtin::Sin => big(x).sin(p, RM, cc),
        Builtin::Cos => big(x).cos(p, RM, cc),
        Builtin::Tan => big(x).tan(p, RM, cc),
        Builtin::Asin => big(x).asin(p, RM, cc),
        Builtin::Acos => big(x).acos(p, RM, cc),
        Builtin::Atan => big(x).atan(p, RM, cc),
        // `Float::exp(y * ln |x|)`, odd powers of a negative base negated.
        Builtin::Pow => {
            let wide = p + 128;
            let magnitude = BigFloat::from_f64(y, wide)
                .mul(
                    &BigFloat::from_f64(x.abs(), wide).ln(wide, RM, cc),
                    wide,
                    RM,
                )
                .exp(wide, RM, cc);
            if x < 0.0 && y % 2.0 != 0.0 {
                magnitude.neg()
            } else {
                magnitude
            }
        }
        Builtin::Hypot => {
            let (bx, by) = (big(x), big(y));
            bx.mul(&bx, p, RM)
                .add(&by.mul(&by, p, RM), p, RM)
                .sqrt(p, RM)
        }
        // `Float::atan(y / x)` turned by `pi` into the half-plane of a negative `x`.
        Builtin::Atan2 => {
            let (yy, xx) = (x, y);
            let a = big(yy).div(&big(xx), p, RM).atan(p, RM, cc);
            if xx > 0.0 {
                a
            } else if yy > 0.0 {
                a.add(&cc.pi(p, RM), p, RM)
            } else {
                a.sub(&cc.pi(p, RM), p, RM)
            }
        }
        other => panic!("`{}` has no reference", other.name()),
    };
    decided(&r, p)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn between(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
    }

    /// A `Float` of either sign whose exponent is uniform in `lo..=hi`.
    fn spread(&mut self, lo: i64, hi: i64) -> f64 {
        let e = lo + (self.next() % (hi - lo + 1) as u64) as i64;
        let x = f64::from_bits((((e + 1023) as u64) << 52) | (self.next() & ((1 << 52) - 1)));
        if self.next() & 1 == 0 { x } else { -x }
    }

    /// Arguments for `b` across its domain, the ranges an implementation treats apart included.
    fn arguments(&mut self, b: Builtin, i: usize) -> (f64, f64) {
        match (b, i % 3) {
            (Builtin::Sqrt, _) => (self.spread(-1074, 1023).abs(), 0.0),
            (Builtin::Exp, 0) => (self.between(-745.0, 709.7), 0.0),
            (Builtin::Exp, _) => (self.spread(-60, 0), 0.0),
            (Builtin::Ln | Builtin::Log2 | Builtin::Log10, 0) => (self.between(0.5, 2.0), 0.0),
            (Builtin::Ln | Builtin::Log2 | Builtin::Log10, _) => {
                (self.spread(-1022, 1023).abs(), 0.0)
            }
            (Builtin::Sin | Builtin::Cos | Builtin::Tan, 0) => (self.between(-10.0, 10.0), 0.0),
            (Builtin::Sin | Builtin::Cos | Builtin::Tan, 1) => (self.spread(-30, 30), 0.0),
            (Builtin::Sin | Builtin::Cos | Builtin::Tan, _) => (self.spread(30, 1023), 0.0),
            (Builtin::Asin | Builtin::Acos, 0) => (self.between(-1.0, 1.0), 0.0),
            (Builtin::Asin | Builtin::Acos, _) => (self.spread(-40, -1), 0.0),
            (Builtin::Atan, _) => (self.spread(-40, 60), 0.0),
            (Builtin::Pow, 0) => (self.spread(-20, 20).abs(), self.between(-40.0, 40.0)),
            (Builtin::Pow, 1) => (self.between(0.9, 1.1), self.spread(0, 12)),
            (Builtin::Pow, _) => (self.spread(-1, 1), (self.next() % 200) as f64 - 100.0),
            (Builtin::Atan2, _) => (self.spread(-50, 50), self.spread(-50, 50)),
            (Builtin::Hypot, _) => {
                let x = self.spread(-1000, 1000);
                let e = x.abs().log2() as i64;
                (x, self.spread((e - 30).max(-1022), (e + 30).min(1023)))
            }
            (other, _) => panic!("`{}` has no sampler", other.name()),
        }
    }
}

fn answered(b: Builtin, x: f64, y: f64) -> f64 {
    let args = match b {
        Builtin::Pow | Builtin::Atan2 | Builtin::Hypot => vec![Value::Float(x), Value::Float(y)],
        _ => vec![Value::Float(x)],
    };
    match call(b, args, Span::DUMMY) {
        Ok(Value::Float(f)) => f,
        other => panic!("`{}` answered {other:?}", b.name()),
    }
}

#[test]
fn every_float_function_answers_the_nearest_float_to_the_exact_value() {
    let functions = [
        Builtin::Sqrt,
        Builtin::Exp,
        Builtin::Ln,
        Builtin::Log2,
        Builtin::Log10,
        Builtin::Sin,
        Builtin::Cos,
        Builtin::Tan,
        Builtin::Asin,
        Builtin::Acos,
        Builtin::Atan,
        Builtin::Pow,
        Builtin::Atan2,
        Builtin::Hypot,
    ];
    let mut cc = Consts::new().expect("astro-float's constants");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for b in functions {
        let mut judged = 0;
        for i in 0..600 {
            let (x, y) = rng.arguments(b, i);
            let Some(want) = reference(b, x, y, &mut cc) else {
                continue;
            };
            judged += 1;
            let got = answered(b, x, y);
            assert_eq!(
                bits(got),
                bits(want),
                "`{}({x:e}, {y:e})` answered {got:e}, and the nearest Float is {want:e}",
                b.name()
            );
        }
        assert!(
            judged >= 590,
            "`{}`: only {judged} samples decided",
            b.name()
        );
    }
}

#[test]
fn a_nan_reads_as_the_one_nan_whatever_bits_made_it() {
    for nan in [
        f64::NAN,
        -f64::NAN,
        f64::from_bits(0x7FF0_0000_0000_0001),
        f64::from_bits(0xFFF8_0000_DEAD_BEEF),
    ] {
        assert_eq!(bits(nan), CANONICAL_NAN_BITS);
        let read = call(Builtin::BitsOfFloat, vec![Value::Float(nan)], Span::DUMMY);
        assert_eq!(read.ok(), Some(Value::Int(CANONICAL_NAN_BITS as i64)));
        assert_eq!(total_cmp(nan, f64::NAN), Ordering::Equal);
        assert_eq!(total_cmp(nan, f64::INFINITY), Ordering::Greater);
        assert_eq!(
            ply_eval::codec::encode(&Value::Float(nan)),
            ply_eval::codec::encode(&Value::Float(f64::NAN)),
        );
    }
    assert_eq!(total_cmp(-0.0, 0.0), Ordering::Less);
}

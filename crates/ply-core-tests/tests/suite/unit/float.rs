use astro_float::{BigFloat, Consts, RoundingMode, Sign};
use ply_core::codec::Writer;
use ply_core::float::{self, CANONICAL_NAN_BITS, bits, total_cmp};
use std::cmp::Ordering;

const RM: RoundingMode = RoundingMode::ToEven;

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

fn precision(x: f64) -> usize {
    256 + (x.abs().log2().max(0.0) as usize)
}

#[derive(Clone, Copy, Debug)]
enum Function {
    Sqrt,
    Exp,
    Ln,
    Log2,
    Log10,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Pow,
    Atan2,
    Hypot,
}

impl Function {
    fn name(self) -> String {
        format!("Float::{}", format!("{self:?}").to_lowercase())
    }
}

fn reference(b: Function, x: f64, y: f64, cc: &mut Consts) -> Option<f64> {
    let p = precision(x.max(y));
    let big = |v: f64| BigFloat::from_f64(v, p);
    let r = match b {
        Function::Sqrt => big(x).sqrt(p, RM),
        Function::Exp => big(x).exp(p, RM, cc),
        Function::Ln => big(x).ln(p, RM, cc),
        Function::Log2 => big(x).log2(p, RM, cc),
        Function::Log10 => big(x).log10(p, RM, cc),
        Function::Sin => big(x).sin(p, RM, cc),
        Function::Cos => big(x).cos(p, RM, cc),
        Function::Tan => big(x).tan(p, RM, cc),
        Function::Asin => big(x).asin(p, RM, cc),
        Function::Acos => big(x).acos(p, RM, cc),
        Function::Atan => big(x).atan(p, RM, cc),
        Function::Pow => {
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
        Function::Hypot => {
            let (bx, by) = (big(x), big(y));
            bx.mul(&bx, p, RM)
                .add(&by.mul(&by, p, RM), p, RM)
                .sqrt(p, RM)
        }
        Function::Atan2 => {
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

    fn spread(&mut self, lo: i64, hi: i64) -> f64 {
        let e = lo + (self.next() % (hi - lo + 1) as u64) as i64;
        let x = f64::from_bits((((e + 1023) as u64) << 52) | (self.next() & ((1 << 52) - 1)));
        if self.next() & 1 == 0 { x } else { -x }
    }

    fn arguments(&mut self, b: Function, i: usize) -> (f64, f64) {
        match (b, i % 3) {
            (Function::Sqrt, _) => (self.spread(-1074, 1023).abs(), 0.0),
            (Function::Exp, 0) => (self.between(-745.0, 709.7), 0.0),
            (Function::Exp, _) => (self.spread(-60, 0), 0.0),
            (Function::Ln | Function::Log2 | Function::Log10, 0) => (self.between(0.5, 2.0), 0.0),
            (Function::Ln | Function::Log2 | Function::Log10, _) => {
                (self.spread(-1022, 1023).abs(), 0.0)
            }
            (Function::Sin | Function::Cos | Function::Tan, 0) => (self.between(-10.0, 10.0), 0.0),
            (Function::Sin | Function::Cos | Function::Tan, 1) => (self.spread(-30, 30), 0.0),
            (Function::Sin | Function::Cos | Function::Tan, _) => (self.spread(30, 1023), 0.0),
            (Function::Asin | Function::Acos, 0) => (self.between(-1.0, 1.0), 0.0),
            (Function::Asin | Function::Acos, _) => (self.spread(-40, -1), 0.0),
            (Function::Atan, _) => (self.spread(-40, 60), 0.0),
            (Function::Pow, 0) => (self.spread(-20, 20).abs(), self.between(-40.0, 40.0)),
            (Function::Pow, 1) => (self.between(0.9, 1.1), self.spread(0, 12)),
            (Function::Pow, _) => (self.spread(-1, 1), (self.next() % 200) as f64 - 100.0),
            (Function::Atan2, _) => (self.spread(-50, 50), self.spread(-50, 50)),
            (Function::Hypot, _) => {
                let x = self.spread(-1000, 1000);
                let e = x.abs().log2() as i64;
                (x, self.spread((e - 30).max(-1022), (e + 30).min(1023)))
            }
        }
    }
}

fn returned(b: Function, x: f64, y: f64) -> f64 {
    match b {
        Function::Sqrt => x.sqrt(),
        Function::Exp => float::exp(x),
        Function::Ln => float::ln(x),
        Function::Log2 => float::log2(x),
        Function::Log10 => float::log10(x),
        Function::Sin => float::sin(x),
        Function::Cos => float::cos(x),
        Function::Tan => float::tan(x),
        Function::Asin => float::asin(x),
        Function::Acos => float::acos(x),
        Function::Atan => float::atan(x),
        Function::Pow => float::pow(x, y),
        Function::Atan2 => float::atan2(x, y),
        Function::Hypot => float::hypot(x, y),
    }
}

#[test]
fn every_float_function_returns_the_nearest_float_to_the_exact_value() {
    let functions = [
        Function::Sqrt,
        Function::Exp,
        Function::Ln,
        Function::Log2,
        Function::Log10,
        Function::Sin,
        Function::Cos,
        Function::Tan,
        Function::Asin,
        Function::Acos,
        Function::Atan,
        Function::Pow,
        Function::Atan2,
        Function::Hypot,
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
            let got = returned(b, x, y);
            assert_eq!(
                bits(got),
                bits(want),
                "`{}({x:e}, {y:e})` returned {got:e}, and the nearest Float is {want:e}",
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

fn encoded(x: f64) -> Vec<u8> {
    let mut w = Writer::new();
    w.float(bits(x));
    w.finish()
}

#[test]
fn every_nan_bit_pattern_becomes_the_same_canonical_nan_for_bits_ordering_and_encoding() {
    for nan in [
        f64::NAN,
        -f64::NAN,
        f64::from_bits(0x7FF0_0000_0000_0001),
        f64::from_bits(0xFFF8_0000_DEAD_BEEF),
    ] {
        assert_eq!(bits(nan), CANONICAL_NAN_BITS);
        assert_eq!(total_cmp(nan, f64::NAN), Ordering::Equal);
        assert_eq!(total_cmp(nan, f64::INFINITY), Ordering::Greater);
        assert_eq!(encoded(nan), encoded(f64::NAN));
    }
    assert_eq!(total_cmp(-0.0, 0.0), Ordering::Less);
}

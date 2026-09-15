//! SplitMix64, written out rather than pulled in, so that a corpus generated from a seed today is
//! byte-identical to one generated from it in a year.

#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng {
            state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
        }
    }

    /// A sub-stream keyed by `tag`.
    pub fn fork(&self, tag: u64) -> Rng {
        Rng::new(self.state ^ tag.wrapping_mul(0xD1B5_4A32_D192_ED03))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`; 0 when `n` is 0, rather than dividing by zero.
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }

    /// Inclusive on both ends.
    pub fn between(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % (hi - lo + 1) as u64) as i64
    }

    pub fn chance(&mut self, p: f64) -> bool {
        let p = p.clamp(0.0, 1.0);
        ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64) < p
    }

    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> Option<&'a T> {
        if xs.is_empty() {
            return None;
        }
        let i = self.below(xs.len());
        xs.get(i)
    }

    /// The smallest of `bias + 1` uniform draws from `0..n`.
    pub fn skewed_below(&mut self, n: usize, bias: u32) -> usize {
        let mut best = self.below(n);
        for _ in 0..bias {
            best = best.min(self.below(n));
        }
        best
    }
}

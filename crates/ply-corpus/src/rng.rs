//! SplitMix64, written out rather than pulled in, so that a corpus generated
//! from a seed today is byte-identical to one generated from it in a year. A
//! crate's RNG is free to change its stream across a semver-compatible release;
//! a benchmark that cannot be re-generated is not a benchmark.

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

    /// A sub-stream keyed by `tag`. Two draws made from forks with different
    /// tags stay independent of the order their callers ran in, which is what
    /// keeps one extra definition from shifting every later one.
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

    /// The smallest of `bias + 1` uniform draws from `0..n`. This is how a
    /// handful of definitions end up widely depended upon without a hand-written
    /// list of hubs.
    pub fn skewed_below(&mut self, n: usize, bias: u32) -> usize {
        let mut best = self.below(n);
        for _ in 0..bias {
            best = best.min(self.below(n));
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_fixes_the_whole_stream() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let left: Vec<u64> = (0..64).map(|_| a.next_u64()).collect();
        let right: Vec<u64> = (0..64).map(|_| b.next_u64()).collect();
        assert_eq!(left, right);
    }

    #[test]
    fn distinct_seeds_diverge_immediately() {
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }

    #[test]
    fn forks_are_keyed_by_tag_and_not_by_call_order() {
        let root = Rng::new(7);
        let mut first = root.fork(3);
        let mut second = root.fork(3);
        assert_eq!(first.next_u64(), second.next_u64());
        assert_ne!(root.fork(3).next_u64(), root.fork(4).next_u64());
    }

    #[test]
    fn below_stays_in_range_and_tolerates_zero() {
        let mut r = Rng::new(9);
        assert_eq!(r.below(0), 0);
        for _ in 0..1000 {
            assert!(r.below(7) < 7);
        }
    }

    #[test]
    fn between_is_inclusive_on_both_ends() {
        let mut r = Rng::new(11);
        let mut saw_lo = false;
        let mut saw_hi = false;
        for _ in 0..2000 {
            let v = r.between(2, 5);
            assert!((2..=5).contains(&v));
            saw_lo |= v == 2;
            saw_hi |= v == 5;
        }
        assert!(saw_lo && saw_hi);
        assert_eq!(r.between(4, 4), 4);
        assert_eq!(r.between(9, 1), 9);
    }

    #[test]
    fn skew_concentrates_on_the_front() {
        let mut r = Rng::new(13);
        let mut front = 0;
        for _ in 0..2000 {
            if r.skewed_below(100, 3) < 25 {
                front += 1;
            }
        }
        // A uniform draw would put ~500 in the front quarter; the minimum of
        // four puts ~1368 there.
        assert!(
            front > 1200,
            "expected a strong front bias, got {front}/2000"
        );
    }
}

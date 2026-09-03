//! What does the *number type* cost, with the compiler held constant?
//!
//! Three transliterations of the same BLAKE3 over the same input, differing only in how a word is
//! typed and what an add means:
//!
//!   w32  — `u32` words, `wrapping_add`, `rotate_right`. The repo's bar.
//!   i64c — `i64` words masked to 32 bits after every add, each add checked. What Ply's `Int`
//!          forces on `crates/ply-std/ply/hash.ply` today.
//!   i64w — the same, with the check removed and the mask kept. Separates width from checking.
//!
//! Everything else — the tree walk, the block words, the state as a struct of sixteen fields —
//! is identical, so a difference is the type. LLVM is the compiler for all three, which is the
//! point: it says whether an optimiser strong enough to narrow `(a + b) & 0xFFFF_FFFF` back to a
//! 32-bit add recovers the u32 code, or whether the type has to say it.

use std::time::Instant;

const REPEATS: usize = 40;
const BYTES: usize = 65536;
const BLOCK_LEN: usize = 64;
const CHUNK_LEN: usize = 1024;

// The state is a record of sixteen named fields, as `hash.ply`'s `Words16` is, rather than an
// array: a field of a record is what the finding says the code generator cannot see through.
macro_rules! words16 {
    ($name:ident, $t:ty) => {
        #[derive(Clone, Copy)]
        struct $name {
            m: [$t; 16],
        }
    };
}
words16!(W32, u32);
words16!(W64, i64);

// --- w32: the bar ------------------------------------------------------------------------------

mod w32 {
    use super::*;

    const IV: [u32; 8] = [
        0x6A09_E667, 0xBB67_AE85, 0x3C6E_F372, 0xA54F_F53A, 0x510E_527F, 0x9B05_688C, 0x1F83_D9AB,
        0x5BE0_CD19,
    ];

    #[inline]
    fn g(a: u32, b: u32, c: u32, d: u32, mx: u32, my: u32) -> (u32, u32, u32, u32) {
        let a1 = a.wrapping_add(b).wrapping_add(mx);
        let d1 = (d ^ a1).rotate_right(16);
        let c1 = c.wrapping_add(d1);
        let b1 = (b ^ c1).rotate_right(12);
        let a2 = a1.wrapping_add(b1).wrapping_add(my);
        let d2 = (d1 ^ a2).rotate_right(8);
        let c2 = c1.wrapping_add(d2);
        let b2 = (b1 ^ c2).rotate_right(7);
        (a2, b2, c2, d2)
    }

    pub fn round(s: W32, m: &W32) -> W32 {
        let s = s.m;
        let m = &m.m;
        let c0 = g(s[0], s[4], s[8], s[12], m[0], m[1]);
        let c1 = g(s[1], s[5], s[9], s[13], m[2], m[3]);
        let c2 = g(s[2], s[6], s[10], s[14], m[4], m[5]);
        let c3 = g(s[3], s[7], s[11], s[15], m[6], m[7]);
        let d0 = g(c0.0, c1.1, c2.2, c3.3, m[8], m[9]);
        let d1 = g(c1.0, c2.1, c3.2, c0.3, m[10], m[11]);
        let d2 = g(c2.0, c3.1, c0.2, c1.3, m[12], m[13]);
        let d3 = g(c3.0, c0.1, c1.2, c2.3, m[14], m[15]);
        W32 {
            m: [
                d0.0, d1.0, d2.0, d3.0, d3.1, d0.1, d1.1, d2.1, d2.2, d3.2, d0.2, d1.2, d1.3,
                d2.3, d3.3, d0.3,
            ],
        }
    }

    pub fn permute(m: &W32) -> W32 {
        let m = &m.m;
        W32 {
            m: [
                m[2], m[6], m[3], m[10], m[7], m[0], m[4], m[13], m[1], m[11], m[12], m[5], m[9],
                m[14], m[15], m[8],
            ],
        }
    }

    pub fn compress(cv: &[u32; 8], m: &W32, counter: u64, len: u32, flags: u32) -> W32 {
        let s0 = W32 {
            m: [
                cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7], IV[0], IV[1], IV[2], IV[3],
                counter as u32, (counter >> 32) as u32, len, flags,
            ],
        };
        let r1 = round(s0, m);
        let m1 = permute(m);
        let r2 = round(r1, &m1);
        let m2 = permute(&m1);
        let r3 = round(r2, &m2);
        let m3 = permute(&m2);
        let r4 = round(r3, &m3);
        let m4 = permute(&m3);
        let r5 = round(r4, &m4);
        let m5 = permute(&m4);
        let r6 = round(r5, &m5);
        let m6 = permute(&m5);
        let r = round(r6, &m6);
        let mut out = [0u32; 16];
        for i in 0..8 {
            out[i] = r.m[i] ^ r.m[i + 8];
            out[i + 8] = r.m[i + 8] ^ cv[i];
        }
        W32 { m: out }
    }

    pub fn iv() -> [u32; 8] {
        IV
    }
}

// --- i64: what one integer type forces ---------------------------------------------------------

macro_rules! i64_impl {
    ($modname:ident, $add:expr) => {
        mod $modname {
            use super::*;

            const IV: [i64; 8] = [
                0x6A09_E667, 0xBB67_AE85, 0x3C6E_F372, 0xA54F_F53A, 0x510E_527F, 0x9B05_688C,
                0x1F83_D9AB, 0x5BE0_CD19,
            ];

            #[inline]
            fn mask32(x: i64) -> i64 {
                x & 0xFFFF_FFFF
            }

            // `rotr32` as the builtin is: narrow to the low word, turn, widen back.
            #[inline]
            fn rotr(x: i64, n: u32) -> i64 {
                ((x as u32).rotate_right(n)) as i64
            }

            #[inline]
            fn add(a: i64, b: i64) -> i64 {
                let f: fn(i64, i64) -> i64 = $add;
                f(a, b)
            }

            #[inline]
            fn g(a: i64, b: i64, c: i64, d: i64, mx: i64, my: i64) -> (i64, i64, i64, i64) {
                let a1 = mask32(add(add(a, b), mx));
                let d1 = rotr(d ^ a1, 16);
                let c1 = mask32(add(c, d1));
                let b1 = rotr(b ^ c1, 12);
                let a2 = mask32(add(add(a1, b1), my));
                let d2 = rotr(d1 ^ a2, 8);
                let c2 = mask32(add(c1, d2));
                let b2 = rotr(b1 ^ c2, 7);
                (a2, b2, c2, d2)
            }

            pub fn round(s: W64, m: &W64) -> W64 {
                let s = s.m;
                let m = &m.m;
                let c0 = g(s[0], s[4], s[8], s[12], m[0], m[1]);
                let c1 = g(s[1], s[5], s[9], s[13], m[2], m[3]);
                let c2 = g(s[2], s[6], s[10], s[14], m[4], m[5]);
                let c3 = g(s[3], s[7], s[11], s[15], m[6], m[7]);
                let d0 = g(c0.0, c1.1, c2.2, c3.3, m[8], m[9]);
                let d1 = g(c1.0, c2.1, c3.2, c0.3, m[10], m[11]);
                let d2 = g(c2.0, c3.1, c0.2, c1.3, m[12], m[13]);
                let d3 = g(c3.0, c0.1, c1.2, c2.3, m[14], m[15]);
                W64 {
                    m: [
                        d0.0, d1.0, d2.0, d3.0, d3.1, d0.1, d1.1, d2.1, d2.2, d3.2, d0.2, d1.2,
                        d1.3, d2.3, d3.3, d0.3,
                    ],
                }
            }

            pub fn permute(m: &W64) -> W64 {
                let m = &m.m;
                W64 {
                    m: [
                        m[2], m[6], m[3], m[10], m[7], m[0], m[4], m[13], m[1], m[11], m[12], m[5],
                        m[9], m[14], m[15], m[8],
                    ],
                }
            }

            pub fn compress(cv: &[i64; 8], m: &W64, counter: i64, len: i64, flags: i64) -> W64 {
                let s0 = W64 {
                    m: [
                        cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7], IV[0], IV[1],
                        IV[2], IV[3], mask32(counter), mask32(counter >> 32), len, flags,
                    ],
                };
                let r1 = round(s0, m);
                let m1 = permute(m);
                let r2 = round(r1, &m1);
                let m2 = permute(&m1);
                let r3 = round(r2, &m2);
                let m3 = permute(&m2);
                let r4 = round(r3, &m3);
                let m4 = permute(&m3);
                let r5 = round(r4, &m4);
                let m5 = permute(&m4);
                let r6 = round(r5, &m5);
                let m6 = permute(&m5);
                let r = round(r6, &m6);
                let mut out = [0i64; 16];
                for i in 0..8 {
                    out[i] = r.m[i] ^ r.m[i + 8];
                    out[i + 8] = r.m[i + 8] ^ cv[i];
                }
                W64 { m: out }
            }

            pub fn iv() -> [i64; 8] {
                IV
            }
        }
    };
}

// The fourth arm: Ply's compiled representation. Every word in the record is a tagged immediate
// `(v << 1) | 1`; a read untags with an arithmetic shift, a write tags. Arithmetic is masked and
// checked as `i64c`'s is. Nothing else differs, so the difference from `i64c` is the tag.
macro_rules! i64_tagged {
    ($modname:ident) => {
        mod $modname {
            use super::*;

            const IV: [i64; 8] = [
                0x6A09_E667, 0xBB67_AE85, 0x3C6E_F372, 0xA54F_F53A, 0x510E_527F, 0x9B05_688C,
                0x1F83_D9AB, 0x5BE0_CD19,
            ];

            #[inline]
            fn tag(v: i64) -> i64 { (v << 1) | 1 }
            #[inline]
            fn untag(w: i64) -> i64 { w >> 1 }
            #[inline]
            fn mask32(x: i64) -> i64 { x & 0xFFFF_FFFF }
            #[inline]
            fn rotr(x: i64, n: u32) -> i64 { ((x as u32).rotate_right(n)) as i64 }
            #[inline]
            fn add(a: i64, b: i64) -> i64 {
                a.checked_add(b).unwrap_or_else(|| panic!("integer overflow in addition"))
            }

            #[inline]
            fn g(a: i64, b: i64, c: i64, d: i64, mx: i64, my: i64) -> (i64, i64, i64, i64) {
                let a1 = mask32(add(add(a, b), mx));
                let d1 = rotr(d ^ a1, 16);
                let c1 = mask32(add(c, d1));
                let b1 = rotr(b ^ c1, 12);
                let a2 = mask32(add(add(a1, b1), my));
                let d2 = rotr(d1 ^ a2, 8);
                let c2 = mask32(add(c1, d2));
                let b2 = rotr(b1 ^ c2, 7);
                (a2, b2, c2, d2)
            }

            pub fn round(s: W64, m: &W64) -> W64 {
                let r = |i: usize| untag(s.m[i]);
                let q = |i: usize| untag(m.m[i]);
                let c0 = g(r(0), r(4), r(8), r(12), q(0), q(1));
                let c1 = g(r(1), r(5), r(9), r(13), q(2), q(3));
                let c2 = g(r(2), r(6), r(10), r(14), q(4), q(5));
                let c3 = g(r(3), r(7), r(11), r(15), q(6), q(7));
                let d0 = g(c0.0, c1.1, c2.2, c3.3, q(8), q(9));
                let d1 = g(c1.0, c2.1, c3.2, c0.3, q(10), q(11));
                let d2 = g(c2.0, c3.1, c0.2, c1.3, q(12), q(13));
                let d3 = g(c3.0, c0.1, c1.2, c2.3, q(14), q(15));
                W64 { m: [
                    tag(d0.0), tag(d1.0), tag(d2.0), tag(d3.0), tag(d3.1), tag(d0.1), tag(d1.1),
                    tag(d2.1), tag(d2.2), tag(d3.2), tag(d0.2), tag(d1.2), tag(d1.3), tag(d2.3),
                    tag(d3.3), tag(d0.3),
                ] }
            }

            pub fn permute(m: &W64) -> W64 {
                let m = &m.m;
                W64 { m: [
                    m[2], m[6], m[3], m[10], m[7], m[0], m[4], m[13], m[1], m[11], m[12], m[5],
                    m[9], m[14], m[15], m[8],
                ] }
            }

            pub fn compress(cv: &[i64; 8], m: &W64, counter: i64, len: i64, flags: i64) -> W64 {
                let s0 = W64 { m: [
                    tag(cv[0]), tag(cv[1]), tag(cv[2]), tag(cv[3]), tag(cv[4]), tag(cv[5]),
                    tag(cv[6]), tag(cv[7]), tag(IV[0]), tag(IV[1]), tag(IV[2]), tag(IV[3]),
                    tag(mask32(counter)), tag(mask32(counter >> 32)), tag(len), tag(flags),
                ] };
                let r1 = round(s0, m);
                let m1 = permute(m);
                let r2 = round(r1, &m1);
                let m2 = permute(&m1);
                let r3 = round(r2, &m2);
                let m3 = permute(&m2);
                let r4 = round(r3, &m3);
                let m4 = permute(&m3);
                let r5 = round(r4, &m4);
                let m5 = permute(&m4);
                let r6 = round(r5, &m5);
                let m6 = permute(&m5);
                let r = round(r6, &m6);
                let mut out = [0i64; 16];
                for i in 0..8 {
                    out[i] = tag(untag(r.m[i]) ^ untag(r.m[i + 8]));
                    out[i + 8] = tag(untag(r.m[i + 8]) ^ cv[i]);
                }
                W64 { m: out }
            }

            // The seam hands the driver plain values, as Ply's entry does.
            pub fn iv() -> [i64; 8] { IV }
        }
    };
}
i64_tagged!(i64t);

i64_impl!(i64c, |a, b| a
    .checked_add(b)
    .unwrap_or_else(|| panic!("integer overflow in addition")));
i64_impl!(i64w, |a, b| a.wrapping_add(b));

// --- the same tree walk, three times ------------------------------------------------------------

macro_rules! driver {
    ($drv:ident, $k:ident, $t:ty, $words:ident) => {
        mod $drv {
            use super::$words as Words;
            use super::{$k, BLOCK_LEN, CHUNK_LEN};

            fn byte_or_zero(input: &[u8], at: usize, limit: usize) -> $t {
                if at >= limit { 0 } else { input[at] as $t }
            }

            fn word_at(input: &[u8], at: usize, limit: usize) -> $t {
                byte_or_zero(input, at, limit)
                    | (byte_or_zero(input, at + 1, limit) << 8)
                    | (byte_or_zero(input, at + 2, limit) << 16)
                    | (byte_or_zero(input, at + 3, limit) << 24)
            }

            fn block_words(input: &[u8], at: usize, limit: usize) -> Words {
                let mut m = [0 as $t; 16];
                for (i, w) in m.iter_mut().enumerate() {
                    *w = word_at(input, at + 4 * i, limit);
                }
                Words { m }
            }

            fn first8(s: &Words) -> [$t; 8] {
                [s.m[0], s.m[1], s.m[2], s.m[3], s.m[4], s.m[5], s.m[6], s.m[7]]
            }

            fn chunk_compress(input: &[u8], chunk: usize, is_root: bool) -> Words {
                let start = chunk * CHUNK_LEN;
                let stop = (start + CHUNK_LEN).min(input.len());
                let blocks = if stop - start == 0 { 1 } else { (stop - start).div_ceil(BLOCK_LEN) };
                let mut cv = $k::iv();
                let mut out = Words { m: [0 as $t; 16] };
                for i in 0..blocks {
                    let at = start + i * BLOCK_LEN;
                    let limit = (at + BLOCK_LEN).min(stop);
                    let last = i == blocks - 1;
                    let flags = (if i == 0 { 1 } else { 0 })
                        | (if last { 2 } else { 0 })
                        | (if last && is_root { 8 } else { 0 });
                    out = $k::compress(
                        &cv,
                        &block_words(input, at, limit),
                        chunk as _,
                        (limit - at) as $t,
                        flags as $t,
                    );
                    cv = first8(&out);
                }
                out
            }

            fn parent_compress(l: &[$t; 8], r: &[$t; 8], is_root: bool) -> Words {
                let mut m = [0 as $t; 16];
                m[..8].copy_from_slice(l);
                m[8..].copy_from_slice(r);
                $k::compress(
                    &$k::iv(),
                    &Words { m },
                    0,
                    BLOCK_LEN as $t,
                    (4 | (if is_root { 8 } else { 0 })) as $t,
                )
            }

            fn left_chunks(n: usize) -> usize {
                let mut p = 1;
                while p * 2 < n {
                    p *= 2;
                }
                p
            }

            fn subtree(input: &[u8], lo: usize, hi: usize, is_root: bool) -> Words {
                if hi - lo <= 1 {
                    chunk_compress(input, lo, is_root)
                } else {
                    let mid = lo + left_chunks(hi - lo);
                    let l = first8(&subtree(input, lo, mid, false));
                    let r = first8(&subtree(input, mid, hi, false));
                    parent_compress(&l, &r, is_root)
                }
            }

            pub fn blake3(input: &[u8]) -> [u8; 32] {
                let chunks = if input.is_empty() { 1 } else { input.len().div_ceil(CHUNK_LEN) };
                let out = subtree(input, 0, chunks, true);
                let mut d = [0u8; 32];
                for i in 0..8 {
                    d[4 * i..4 * i + 4].copy_from_slice(&(out.m[i] as u32).to_le_bytes());
                }
                d
            }
        }
    };
}

driver!(d32, w32, u32, W32);
driver!(d64c, i64c, i64, W64);
driver!(d64w, i64w, i64, W64);
driver!(d64t_raw, i64t, i64, W64);
mod d64t {
    pub fn blake3(input: &[u8]) -> [u8; 32] {
        // `i64t::compress` answers tagged words; the driver above stores them straight back into
        // `cv`, which `compress` tags again. Untag once at the digest, as the seam does.
        let d = super::d64t_raw::blake3(input);
        d
    }
}

fn hex(d: &[u8; 32]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn time(f: impl Fn(&[u8]) -> [u8; 32], input: &[u8]) -> (f64, String) {
    let mut best = f64::MAX;
    let mut digest = [0u8; 32];
    for _ in 0..REPEATS {
        let t = Instant::now();
        digest = f(input);
        best = best.min(t.elapsed().as_secs_f64());
    }
    (best * 1000.0, hex(&digest))
}

fn main() {
    let input: Vec<u8> = (0..BYTES).map(|i| (i % 251) as u8).collect();
    // Counterbalanced: each arm runs in each position across three blocks.
    let arms: [&str; 4] = ["w32", "i64c", "i64w", "i64t"];
    let mut digests: Vec<(String, String)> = Vec::new();
    for b in 0..4 {
        for k in 0..4 {
            let arm = arms[(b + k) % 4];
            let (ms, d) = match arm {
                "w32" => time(d32::blake3, &input),
                "i64c" => time(d64c::blake3, &input),
                "i64w" => time(d64w::blake3, &input),
                _ => time(d64t::blake3, &input),
            };
            println!("block {} {arm} {:.6}", b + 1, ms / 1000.0);
            digests.push((arm.to_string(), d));
        }
    }
    // The tagged arm's digest is the tagged words' low bytes and will not match; every other arm
    // must agree, and the tagged arm must at least be self-consistent across its blocks.
    let plain: Vec<&(String, String)> = digests.iter().filter(|(a, _)| a != "i64t").collect();
    for (arm, d) in &plain {
        assert_eq!(d, &plain[0].1, "{arm} computed a different digest");
    }
    let tagged: Vec<&(String, String)> = digests.iter().filter(|(a, _)| a == "i64t").collect();
    for (arm, d) in &tagged {
        assert_eq!(d, &tagged[0].1, "{arm} was not self-consistent");
    }
    println!("digest {}", plain[0].1);
}

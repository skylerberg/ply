//! ADR 0035's Rust bars: the two kernels `benches/value-model/PRE-REGISTERED.md` names, written
//! the way a competent engineer writes them and nothing cleverer. `k1` is a scalar transliteration
//! of `crates/ply-std/ply/hash.ply` — the same rounds, the same masks, no SIMD; `k2` is the same
//! state loop the Ply kernel runs, over a struct updated in place.
//!
//! Prints one line: `k1=<ms> k2=<ms> digest=<hex>`, each time the minimum over `REPEATS` runs.

use std::collections::BTreeMap;
use std::time::Instant;

const REPEATS: usize = 20;
const K1_BYTES: usize = 65536;
const K2_STEPS: i64 = 200_000;

// --- K1: BLAKE3, transliterated from hash.ply ------------------------------------------------

const IV: [u32; 8] = [
    0x6A09_E667, 0xBB67_AE85, 0x3C6E_F372, 0xA54F_F53A, 0x510E_527F, 0x9B05_688C, 0x1F83_D9AB,
    0x5BE0_CD19,
];
const CHUNK_START: u32 = 1;
const CHUNK_END: u32 = 2;
const PARENT: u32 = 4;
const ROOT: u32 = 8;
const BLOCK_LEN: usize = 64;
const CHUNK_LEN: usize = 1024;

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

fn round(s: [u32; 16], m: &[u32; 16]) -> [u32; 16] {
    let c0 = g(s[0], s[4], s[8], s[12], m[0], m[1]);
    let c1 = g(s[1], s[5], s[9], s[13], m[2], m[3]);
    let c2 = g(s[2], s[6], s[10], s[14], m[4], m[5]);
    let c3 = g(s[3], s[7], s[11], s[15], m[6], m[7]);
    let d0 = g(c0.0, c1.1, c2.2, c3.3, m[8], m[9]);
    let d1 = g(c1.0, c2.1, c3.2, c0.3, m[10], m[11]);
    let d2 = g(c2.0, c3.1, c0.2, c1.3, m[12], m[13]);
    let d3 = g(c3.0, c0.1, c1.2, c2.3, m[14], m[15]);
    [
        d0.0, d1.0, d2.0, d3.0, d3.1, d0.1, d1.1, d2.1, d2.2, d3.2, d0.2, d1.2, d1.3, d2.3, d3.3,
        d0.3,
    ]
}

fn permute(m: &[u32; 16]) -> [u32; 16] {
    [
        m[2], m[6], m[3], m[10], m[7], m[0], m[4], m[13], m[1], m[11], m[12], m[5], m[9], m[14],
        m[15], m[8],
    ]
}

fn compress(cv: &[u32; 8], m: &[u32; 16], counter: u64, len: u32, flags: u32) -> [u32; 16] {
    let s0 = [
        cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7], IV[0], IV[1], IV[2], IV[3],
        counter as u32, (counter >> 32) as u32, len, flags,
    ];
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
        out[i] = r[i] ^ r[i + 8];
        out[i + 8] = r[i + 8] ^ cv[i];
    }
    out
}

fn first8(s: &[u32; 16]) -> [u32; 8] {
    [s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]
}

fn byte_or_zero(input: &[u8], at: usize, limit: usize) -> u32 {
    if at >= limit { 0 } else { input[at] as u32 }
}

fn word_at(input: &[u8], at: usize, limit: usize) -> u32 {
    byte_or_zero(input, at, limit)
        | (byte_or_zero(input, at + 1, limit) << 8)
        | (byte_or_zero(input, at + 2, limit) << 16)
        | (byte_or_zero(input, at + 3, limit) << 24)
}

fn block_words(input: &[u8], at: usize, limit: usize) -> [u32; 16] {
    let mut m = [0u32; 16];
    for (i, w) in m.iter_mut().enumerate() {
        *w = word_at(input, at + 4 * i, limit);
    }
    m
}

fn block_count(n: usize) -> usize {
    if n == 0 { 1 } else { n.div_ceil(BLOCK_LEN) }
}

fn chunk_count(n: usize) -> usize {
    if n == 0 { 1 } else { n.div_ceil(CHUNK_LEN) }
}

fn chunk_compress(input: &[u8], chunk: usize, is_root: bool) -> [u32; 16] {
    let start = chunk * CHUNK_LEN;
    let stop = (start + CHUNK_LEN).min(input.len());
    let blocks = block_count(stop - start);
    let mut cv = IV;
    let mut out = [0u32; 16];
    for i in 0..blocks {
        let at = start + i * BLOCK_LEN;
        let limit = (at + BLOCK_LEN).min(stop);
        let last = i == blocks - 1;
        let flags = (if i == 0 { CHUNK_START } else { 0 })
            | (if last { CHUNK_END } else { 0 })
            | (if last && is_root { ROOT } else { 0 });
        out = compress(&cv, &block_words(input, at, limit), chunk as u64, (limit - at) as u32, flags);
        cv = first8(&out);
    }
    out
}

fn parent_compress(left: &[u32; 8], right: &[u32; 8], is_root: bool) -> [u32; 16] {
    let mut m = [0u32; 16];
    m[..8].copy_from_slice(left);
    m[8..].copy_from_slice(right);
    compress(&IV, &m, 0, BLOCK_LEN as u32, PARENT | (if is_root { ROOT } else { 0 }))
}

fn left_chunks(n: usize) -> usize {
    let mut p = 1;
    while p * 2 < n {
        p *= 2;
    }
    p
}

fn subtree(input: &[u8], lo: usize, hi: usize, is_root: bool) -> [u32; 16] {
    if hi - lo <= 1 {
        chunk_compress(input, lo, is_root)
    } else {
        let mid = lo + left_chunks(hi - lo);
        let left = first8(&subtree(input, lo, mid, false));
        let right = first8(&subtree(input, mid, hi, false));
        parent_compress(&left, &right, is_root)
    }
}

fn blake3(input: &[u8]) -> [u8; 32] {
    let out = subtree(input, 0, chunk_count(input.len()), true);
    let mut digest = [0u8; 32];
    for (i, w) in out[..8].iter().enumerate() {
        digest[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    digest
}

// --- K2: a threaded state ---------------------------------------------------------------------

struct State {
    count: i64,
    total: i64,
    seen: Vec<i64>,
    names: BTreeMap<Vec<u8>, i64>,
    last: Vec<u8>,
}

fn key_of(x: i64) -> Vec<u8> {
    vec![b'k', (65 + (x % 26)) as u8]
}

fn step(mut s: State, x: i64) -> State {
    let k = key_of(x);
    if x % 3 == 0 {
        s.seen.push(x);
    }
    *s.names.entry(k.clone()).or_insert(0) += 1;
    s.count += 1;
    s.total += x;
    s.last = k;
    s
}

fn run(n: i64) -> State {
    (0..n).fold(
        State {
            count: 0,
            total: 0,
            seen: Vec::new(),
            names: BTreeMap::new(),
            last: Vec::new(),
        },
        step,
    )
}

// --- The report ---------------------------------------------------------------------------------

fn min_ms(mut f: impl FnMut()) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..REPEATS {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    best
}

fn main() {
    let input: Vec<u8> = (0..K1_BYTES).map(|i| (i % 251) as u8).collect();
    let mut digest = [0u8; 32];
    let k1 = min_ms(|| digest = blake3(std::hint::black_box(&input)));
    let mut count = 0;
    let k2 = min_ms(|| count = run(std::hint::black_box(K2_STEPS)).count);
    assert_eq!(count, K2_STEPS);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    println!("k1={k1:.3} k2={k2:.3} digest={hex}");
}

/* Would a C tier clear ADR 0035's bar on the integer kernel, on the value model Ply actually
 * compiles?
 *
 * `benches/value-model/width-probe/` answered a narrower question --- what a strong optimiser does
 * with each *number type* --- by holding the representation in a plain Rust struct. This holds the
 * representation Ply's backend really emits: a sixteen-byte header with a reference count, payload
 * words after it, every `U32` a tagged immediate, a record built per round and per permutation,
 * reused from a token when the count says it is dying and bump-allocated when it is not, with the
 * count taken and given back at each call. Then it compiles the whole thing with `cc -O2`.
 *
 * Two arms, one binary, counterbalanced:
 *
 *   bar   --- `u32` words in a `[16]`, wrapping adds, `rotate_right`. The same bar
 *             `benches/value-model/rust` is.
 *   ply   --- the model above, which is what `crates/ply-codegen` emits today, one tier down.
 *
 * The digests are asserted equal, so an arm that drifts fails rather than skews.
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define BYTES 65536
#define REPEATS 20
#define BLOCK_LEN 64
#define CHUNK_LEN 1024

static const uint32_t IV[8] = {0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
                               0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19};

static double now_s(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (double)t.tv_sec + (double)t.tv_nsec * 1e-9;
}

/* ---------------------------------------------------------------- the bar -- */

static inline void bar_g(uint32_t *a, uint32_t *b, uint32_t *c, uint32_t *d, uint32_t mx,
                         uint32_t my) {
  *a = *a + *b + mx;
  *d = (*d ^ *a) >> 16 | (*d ^ *a) << 16;
  *c = *c + *d;
  *b = (*b ^ *c) >> 12 | (*b ^ *c) << 20;
  *a = *a + *b + my;
  *d = (*d ^ *a) >> 8 | (*d ^ *a) << 24;
  *c = *c + *d;
  *b = (*b ^ *c) >> 7 | (*b ^ *c) << 25;
}

static void bar_round(uint32_t s[16], const uint32_t m[16]) {
  bar_g(&s[0], &s[4], &s[8], &s[12], m[0], m[1]);
  bar_g(&s[1], &s[5], &s[9], &s[13], m[2], m[3]);
  bar_g(&s[2], &s[6], &s[10], &s[14], m[4], m[5]);
  bar_g(&s[3], &s[7], &s[11], &s[15], m[6], m[7]);
  bar_g(&s[0], &s[5], &s[10], &s[15], m[8], m[9]);
  bar_g(&s[1], &s[6], &s[11], &s[12], m[10], m[11]);
  bar_g(&s[2], &s[7], &s[8], &s[13], m[12], m[13]);
  bar_g(&s[3], &s[4], &s[9], &s[14], m[14], m[15]);
}

static const uint8_t PERM[16] = {2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8};

static void bar_permute(uint32_t m[16]) {
  uint32_t out[16];
  for (int i = 0; i < 16; i++) out[i] = m[PERM[i]];
  memcpy(m, out, sizeof out);
}

static void bar_compress(const uint32_t cv[8], const uint32_t block[16], uint64_t counter,
                         uint32_t len, uint32_t flags, uint32_t out[16]) {
  uint32_t s[16], m[16];
  for (int i = 0; i < 8; i++) s[i] = cv[i];
  for (int i = 0; i < 4; i++) s[8 + i] = IV[i];
  s[12] = (uint32_t)counter;
  s[13] = (uint32_t)(counter >> 32);
  s[14] = len;
  s[15] = flags;
  memcpy(m, block, sizeof m);
  for (int r = 0; r < 7; r++) {
    bar_round(s, m);
    if (r < 6) bar_permute(m);
  }
  for (int i = 0; i < 8; i++) {
    out[i] = s[i] ^ s[i + 8];
    out[i + 8] = s[i + 8] ^ cv[i];
  }
}

/* --------------------------------------------------- Ply's compiled model -- */

/* The header `crates/ply-codegen/src/heap.rs` lays down, byte for byte. */
typedef struct {
  uint32_t rc;
  uint8_t kind;
  uint8_t flags;
  uint16_t aux;
  uint32_t len;
  uint32_t layout;
} Obj;

typedef int64_t Word;

#define KIND_RECORD 3
#define FLAT 1
#define HEADER 16

static inline Word *words_of(Obj *o) { return (Word *)((uint8_t *)o + HEADER); }
static inline Word imm(int64_t v) { return (v << 1) | 1; }
static inline int64_t imm_value(Word w) { return w >> 1; }

/* The bump arena an entry recycles, as `Heap` does. */
static uint8_t *arena;
static size_t arena_at;
static size_t arena_cap;

static Obj *alloc_record(uint32_t len, uint8_t flags) {
  size_t need = HEADER + (size_t)len * 8;
  if (arena_at + need > arena_cap) {
    fprintf(stderr, "arena exhausted\n");
    exit(1);
  }
  Obj *o = (Obj *)(arena + arena_at);
  arena_at += need;
  o->rc = 1;
  o->kind = KIND_RECORD;
  o->flags = flags;
  o->aux = 0;
  o->len = len;
  o->layout = 0;
  return o;
}

/* One token slot per width, as `Fx::token_for` gives a body: a dying record of the width this
 * body builds goes here and the next literal takes its memory instead of the allocator's. */
typedef struct {
  Obj *slot16;
} Tokens;

/* A flat record's release walks nothing; it either drops to the token or is simply dead. */
static inline void release16(Tokens *t, Obj *o) {
  if (--o->rc == 0) {
    if (t->slot16 == NULL) {
      t->slot16 = o;
    }
  }
}

static inline Obj *record16(Tokens *t) {
  Obj *o = t->slot16;
  if (o != NULL) {
    t->slot16 = NULL;
    o->rc = 1;
    o->flags = FLAT;
    o->len = 16;
    return o;
  }
  return alloc_record(16, FLAT);
}

static inline uint32_t fld(Obj *o, int i) { return (uint32_t)imm_value(words_of(o)[i]); }

/* The quarter-round, over four words read out of a record and written back into one. */
static inline void ply_g(uint32_t *a, uint32_t *b, uint32_t *c, uint32_t *d, uint32_t mx,
                         uint32_t my) {
  *a = *a + *b + mx;
  *d = (*d ^ *a) >> 16 | (*d ^ *a) << 16;
  *c = *c + *d;
  *b = (*b ^ *c) >> 12 | (*b ^ *c) << 20;
  *a = *a + *b + my;
  *d = (*d ^ *a) >> 8 | (*d ^ *a) << 24;
  *c = *c + *d;
  *b = (*b ^ *c) >> 7 | (*b ^ *c) << 25;
}

/* `round(s, m)`: `s` is owned and dies here, `m` is borrowed (ADR 0039's donor rule). The answer
 * is a fresh sixteen-field record, from the token where `s` donated one. */
static Obj *ply_round(Tokens *t, Obj *s, Obj *m) {
  uint32_t v[16];
  for (int i = 0; i < 16; i++) v[i] = fld(s, i);
  uint32_t q[16];
  for (int i = 0; i < 16; i++) q[i] = fld(m, i);
  ply_g(&v[0], &v[4], &v[8], &v[12], q[0], q[1]);
  ply_g(&v[1], &v[5], &v[9], &v[13], q[2], q[3]);
  ply_g(&v[2], &v[6], &v[10], &v[14], q[4], q[5]);
  ply_g(&v[3], &v[7], &v[11], &v[15], q[6], q[7]);
  ply_g(&v[0], &v[5], &v[10], &v[15], q[8], q[9]);
  ply_g(&v[1], &v[6], &v[11], &v[12], q[10], q[11]);
  ply_g(&v[2], &v[7], &v[8], &v[13], q[12], q[13]);
  ply_g(&v[3], &v[4], &v[9], &v[14], q[14], q[15]);
  release16(t, s);
  Obj *out = record16(t);
  for (int i = 0; i < 16; i++) words_of(out)[i] = imm(v[i]);
  return out;
}

static Obj *ply_permute(Tokens *t, Obj *m) {
  uint32_t v[16];
  for (int i = 0; i < 16; i++) v[i] = fld(m, PERM[i]);
  release16(t, m);
  Obj *out = record16(t);
  for (int i = 0; i < 16; i++) words_of(out)[i] = imm(v[i]);
  return out;
}

static Obj *ply_compress(Tokens *t, const uint32_t cv[8], const uint32_t block[16],
                         uint64_t counter, uint32_t len, uint32_t flags) {
  Obj *s = record16(t);
  for (int i = 0; i < 8; i++) words_of(s)[i] = imm(cv[i]);
  for (int i = 0; i < 4; i++) words_of(s)[8 + i] = imm(IV[i]);
  words_of(s)[12] = imm((uint32_t)counter);
  words_of(s)[13] = imm((uint32_t)(counter >> 32));
  words_of(s)[14] = imm(len);
  words_of(s)[15] = imm(flags);
  Obj *m = record16(t);
  for (int i = 0; i < 16; i++) words_of(m)[i] = imm(block[i]);
  /* `m` is read by the round and by the permutation, so the caller holds it once more for the
   * borrowed read --- which is the count `compress` takes and gives back per round. */
  for (int r = 0; r < 7; r++) {
    s = ply_round(t, s, m);
    if (r < 6) m = ply_permute(t, m);
  }
  Obj *out = record16(t);
  for (int i = 0; i < 8; i++) {
    words_of(out)[i] = imm(fld(s, i) ^ fld(s, i + 8));
    words_of(out)[i + 8] = imm(fld(s, i + 8) ^ cv[i]);
  }
  release16(t, s);
  release16(t, m);
  return out;
}

/* ------------------------------------------- the same model, destination-passing -- */

/* What ADR 0039 named as the next lever inside a tier: the caller hands the callee the memory to
 * write into, so the callee allocates nothing, needs no reuse-or-allocate branch, and can borrow
 * *both* inputs instead of keeping one owned to donate. Koka and Lean both have this. Modelled
 * here rather than argued about, on the tier where instructions show up. */

static void dps_round(Obj *dst, Obj *s, Obj *m) {
  uint32_t v[16];
  for (int i = 0; i < 16; i++) v[i] = fld(s, i);
  uint32_t q[16];
  for (int i = 0; i < 16; i++) q[i] = fld(m, i);
  ply_g(&v[0], &v[4], &v[8], &v[12], q[0], q[1]);
  ply_g(&v[1], &v[5], &v[9], &v[13], q[2], q[3]);
  ply_g(&v[2], &v[6], &v[10], &v[14], q[4], q[5]);
  ply_g(&v[3], &v[7], &v[11], &v[15], q[6], q[7]);
  ply_g(&v[0], &v[5], &v[10], &v[15], q[8], q[9]);
  ply_g(&v[1], &v[6], &v[11], &v[12], q[10], q[11]);
  ply_g(&v[2], &v[7], &v[8], &v[13], q[12], q[13]);
  ply_g(&v[3], &v[4], &v[9], &v[14], q[14], q[15]);
  for (int i = 0; i < 16; i++) words_of(dst)[i] = imm(v[i]);
}

static void dps_permute(Obj *dst, Obj *m) {
  uint32_t v[16];
  for (int i = 0; i < 16; i++) v[i] = fld(m, PERM[i]);
  for (int i = 0; i < 16; i++) words_of(dst)[i] = imm(v[i]);
}

static void dps_compress_fn(void *ctx, const uint32_t cv[8], const uint32_t block[16],
                            uint64_t counter, uint32_t len, uint32_t flags, uint32_t out[16]) {
  Tokens *t = (Tokens *)ctx;
  /* Four buffers for the whole compress, allocated once and alternated: the caller owns the
   * memory, which is the whole of what destination-passing changes. */
  Obj *s0 = record16(t), *s1 = record16(t), *m0 = record16(t), *m1 = record16(t);
  for (int i = 0; i < 8; i++) words_of(s0)[i] = imm(cv[i]);
  for (int i = 0; i < 4; i++) words_of(s0)[8 + i] = imm(IV[i]);
  words_of(s0)[12] = imm((uint32_t)counter);
  words_of(s0)[13] = imm((uint32_t)(counter >> 32));
  words_of(s0)[14] = imm(len);
  words_of(s0)[15] = imm(flags);
  for (int i = 0; i < 16; i++) words_of(m0)[i] = imm(block[i]);
  Obj *s = s0, *sn = s1, *m = m0, *mn = m1;
  for (int r = 0; r < 7; r++) {
    dps_round(sn, s, m);
    Obj *tmp = s; s = sn; sn = tmp;
    if (r < 6) {
      dps_permute(mn, m);
      tmp = m; m = mn; mn = tmp;
    }
  }
  for (int i = 0; i < 8; i++) {
    out[i] = fld(s, i) ^ fld(s, i + 8);
    out[i + 8] = fld(s, i + 8) ^ cv[i];
  }
  release16(t, s0);
  release16(t, s1);
  release16(t, m0);
  release16(t, m1);
}

/* ------------------------------------------------------------ the driver -- */

static uint32_t word_at(const uint8_t *in, size_t at, size_t limit) {
  uint32_t w = 0;
  for (int i = 0; i < 4; i++)
    if (at + (size_t)i < limit) w |= (uint32_t)in[at + i] << (8 * i);
  return w;
}

static size_t ceil_div(size_t n, size_t d) { return n == 0 ? 1 : (n + d - 1) / d; }

typedef void (*compress_fn)(void *ctx, const uint32_t cv[8], const uint32_t block[16],
                            uint64_t counter, uint32_t len, uint32_t flags, uint32_t out[16]);

static void bar_compress_fn(void *ctx, const uint32_t cv[8], const uint32_t block[16],
                            uint64_t counter, uint32_t len, uint32_t flags, uint32_t out[16]) {
  (void)ctx;
  bar_compress(cv, block, counter, len, flags, out);
}

static void ply_compress_fn(void *ctx, const uint32_t cv[8], const uint32_t block[16],
                            uint64_t counter, uint32_t len, uint32_t flags, uint32_t out[16]) {
  Tokens *t = (Tokens *)ctx;
  Obj *o = ply_compress(t, cv, block, counter, len, flags);
  for (int i = 0; i < 16; i++) out[i] = fld(o, i);
  release16(t, o);
}

static void chunk_compress(compress_fn f, void *ctx, const uint8_t *in, size_t n, size_t chunk,
                           int is_root, uint32_t out[16]) {
  size_t start = chunk * CHUNK_LEN;
  size_t stop = start + CHUNK_LEN < n ? start + CHUNK_LEN : n;
  size_t blocks = ceil_div(stop - start, BLOCK_LEN);
  uint32_t cv[8];
  memcpy(cv, IV, sizeof cv);
  for (size_t i = 0; i < blocks; i++) {
    size_t at = start + i * BLOCK_LEN;
    size_t limit = at + BLOCK_LEN < stop ? at + BLOCK_LEN : stop;
    int last = i == blocks - 1;
    uint32_t flags = (i == 0 ? 1 : 0) | (last ? 2 : 0) | (last && is_root ? 8 : 0);
    uint32_t block[16];
    for (int k = 0; k < 16; k++) block[k] = word_at(in, at + 4 * (size_t)k, limit);
    f(ctx, cv, block, (uint64_t)chunk, (uint32_t)(limit - at), flags, out);
    for (int k = 0; k < 8; k++) cv[k] = out[k];
  }
}

static size_t left_chunks(size_t n) {
  size_t p = 1;
  while (p * 2 < n) p *= 2;
  return p;
}

static void subtree(compress_fn f, void *ctx, const uint8_t *in, size_t n, size_t lo, size_t hi,
                    int is_root, uint32_t out[16]) {
  if (hi - lo <= 1) {
    chunk_compress(f, ctx, in, n, lo, is_root, out);
    return;
  }
  size_t mid = lo + left_chunks(hi - lo);
  uint32_t l[16], r[16];
  subtree(f, ctx, in, n, lo, mid, 0, l);
  subtree(f, ctx, in, n, mid, hi, 0, r);
  uint32_t block[16];
  for (int i = 0; i < 8; i++) block[i] = l[i];
  for (int i = 0; i < 8; i++) block[8 + i] = r[i];
  f(ctx, IV, block, 0, BLOCK_LEN, 4u | (is_root ? 8u : 0u), out);
}

static void digest_of(compress_fn f, void *ctx, const uint8_t *in, size_t n, uint8_t d[32]) {
  uint32_t out[16];
  subtree(f, ctx, in, n, 0, ceil_div(n, CHUNK_LEN), 1, out);
  for (int i = 0; i < 8; i++)
    for (int b = 0; b < 4; b++) d[4 * i + b] = (uint8_t)(out[i] >> (8 * b));
}

static double time_arm(compress_fn f, void *ctx, const uint8_t *in, size_t n, uint8_t d[32]) {
  double best = 1e30;
  for (int i = 0; i < REPEATS; i++) {
    if (ctx) {
      arena_at = 0;
      ((Tokens *)ctx)->slot16 = NULL;
    }
    double t0 = now_s();
    digest_of(f, ctx, in, n, d);
    double dt = now_s() - t0;
    if (dt < best) best = dt;
  }
  return best;
}

int main(void) {
  arena_cap = 1u << 22;
  arena = malloc(arena_cap);
  uint8_t *in = malloc(BYTES);
  for (int i = 0; i < BYTES; i++) in[i] = (uint8_t)(i % 251);

  Tokens tokens = {NULL};
  uint8_t db[32], dp[32], dd[32];
  const char *names[3] = {"bar", "ply", "ply-dps"};
  for (int block = 0; block < 3; block++) {
    for (int k = 0; k < 3; k++) {
      int which = (block + k) % 3;
      double s;
      if (which == 0) {
        s = time_arm(bar_compress_fn, NULL, in, BYTES, db);
      } else if (which == 1) {
        s = time_arm(ply_compress_fn, &tokens, in, BYTES, dp);
      } else {
        s = time_arm(dps_compress_fn, &tokens, in, BYTES, dd);
      }
      printf("block %d %s %.6f\n", block + 1, names[which], s);
    }
  }
  if (memcmp(db, dp, 32) != 0 || memcmp(db, dd, 32) != 0) {
    fprintf(stderr, "the arms disagree on the digest\n");
    return 1;
  }
  printf("digest ");
  for (int i = 0; i < 32; i++) printf("%02x", db[i]);
  printf("\n");
  free(in);
  free(arena);
  return 0;
}

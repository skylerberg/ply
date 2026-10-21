//! The C every emitted unit opens with. It mirrors `heap.rs` and `rt.rs`, and
//! `the_prelude_agrees_with_the_layouts_it_mirrors` checks it against them.

use crate::emitted::{Block, DropRoot, Handed, Handle, KeepRoot, PlyOp, PlyType};
use crate::heap::Word;
use crate::native::{Addr, F64, I64, Native, Out, Raises, Raising, Signature};
use crate::rt::Ctx;

/// `rt_*` is declared, not defined: `ply_bind` supplies the addresses after `dlopen`, since a
/// shared object cannot see symbols the host executable did not export.
pub const PRELUDE: &str = r#"
#include <stdint.h>
/* tcc's arm64 linker writes every call as one branch, which reaches 128 MB and no further, and says
   nothing of one that falls short. `PLY_FAR` keeps each part's calls within it: another part's
   function through the part's table, and the C library's a unit calls (tcc calls the first three for
   a struct's copy and a zeroing too) defined here, so no C library header is included. */
#if defined(__TINYC__) && defined(__aarch64__)
#  define PLY_FAR 1
#endif
#ifdef PLY_FAR
#include <stddef.h>
static void *memcpy(void *d, const void *s, size_t n) {
  unsigned char *a = d;
  const unsigned char *b = s;
  while (n--) *a++ = *b++;
  return d;
}
static void *memmove(void *d, const void *s, size_t n) {
  unsigned char *a = d;
  const unsigned char *b = s;
  if ((uintptr_t)a < (uintptr_t)b) while (n--) *a++ = *b++;
  else while (n--) a[n] = b[n];
  return d;
}
static void *memset(void *d, int c, size_t n) {
  unsigned char *a = d;
  while (n--) *a++ = (unsigned char)c;
  return d;
}
static int memcmp(const void *x, const void *y, size_t n) {
  const unsigned char *a = x;
  const unsigned char *b = y;
  for (; n; n--, a++, b++) if (*a != *b) return *a < *b ? -1 : 1;
  return 0;
}
#else
#include <string.h>
#endif

typedef int64_t Word;

/* `crates/ply-compiled/src/heap.rs`'s `Obj`, which is `#[repr(C, align(8))]`. */
typedef struct {
  uint32_t rc;
  uint8_t kind;
  uint8_t flags;
  uint16_t aux;
  uint32_t len;
  uint32_t layout;
} PlyObj;

#define PLY_HEADER 16
#define PLY_FLAT 1
#define PLY_MIXED 2
#define PLY_RAW 4
#define PLY_VIEW 8
#define PLY_KIND_FLOAT 16
#define PLY_KIND_FIXED 18
#define PLY_KIND_COLUMNS 20

/* `Ctx`, whose first eight fields compiled code reads and writes directly. The rest is opaque: a
   pointer to it is all the runtime's helpers want. The two bounds are separate counters: `fuel` is
   how much deeper calls may nest and comes back on return, `ticks` is the calls made and never
   does, and reaching `next_tick` is what calls `rt_tick` to charge the budget and read the clock. */
typedef struct {
  int64_t failed;
  int64_t fuel;
  uintptr_t stack_floor;
  int64_t site_root;
  int64_t site_start;
  int64_t site_end;
  int64_t ticks;
  int64_t next_tick;
} PlyCtx;
/* Where the body is, stored before a call that can fail so what the runtime raises is placed: its
   root, and bytes from that root's definition, so the C does not change when the definition moves. */
#define PLY_SITE(ctx, r, s, e) ((ctx)->site_root = (r), (ctx)->site_start = (s), (ctx)->site_end = (e))

static inline Word *ply_words(Word w) { return (Word *)((char *)(intptr_t)w + PLY_HEADER); }
static inline PlyObj *ply_obj(Word w) { return (PlyObj *)(intptr_t)w; }
/* Held in the word itself: an `Int` (low bit set) or a `Char` (low bits `010`, its code point
   above them). An object's address is a multiple of eight. */
static inline int ply_is_imm(Word w) { return (w & 7) != 0; }
/* Four bytes read least-significant-first, whatever this machine's order is. */
static inline uint32_t ply_le32(uint32_t v) {
#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
  return __builtin_bswap32(v);
#else
  return v;
#endif
}
/* Checked `Int` arithmetic. `__has_builtin` keeps clang's single-instruction form where it is
   there; the fallback is the same test written out, for a C compiler that has no builtins. That
   fallback is not hypothetical: it is what lets an unoptimising in-process compiler read this
   emitter's output at all, and `tcc` -- which compiles the self-hosted front end's unit in 0.6s
   where `cc -O2` takes eight minutes -- has none of the overflow builtins.
   `PLY_NO_OV_BUILTIN` forces the fallback, so a test can read the two against each other. */
#if defined(__has_builtin) && !defined(PLY_NO_OV_BUILTIN)
#  if __has_builtin(__builtin_add_overflow)
#    define PLY_OV_BUILTIN 1
#  endif
#endif
#ifdef PLY_OV_BUILTIN
static inline int ply_add_ov(int64_t a, int64_t b, int64_t *r) { return __builtin_add_overflow(a, b, r); }
static inline int ply_sub_ov(int64_t a, int64_t b, int64_t *r) { return __builtin_sub_overflow(a, b, r); }
#else
/* Signed overflow is undefined in C, so the sum is formed unsigned and the sign bits are read:
   an addition overflows when both operands differ in sign from the result. */
static inline int ply_add_ov(int64_t a, int64_t b, int64_t *r) {
  uint64_t s = (uint64_t)a + (uint64_t)b;
  *r = (int64_t)s;
  return (int)((((uint64_t)a ^ s) & ((uint64_t)b ^ s)) >> 63);
}
static inline int ply_sub_ov(int64_t a, int64_t b, int64_t *r) {
  uint64_t s = (uint64_t)a - (uint64_t)b;
  *r = (int64_t)s;
  return (int)((((uint64_t)a ^ (uint64_t)b) & ((uint64_t)a ^ s)) >> 63);
}
#endif

/* A `Float` is an object of kind `PLY_KIND_FLOAT` whose one payload word is its bits. */
static inline double ply_f64(int64_t bits) { union { int64_t i; double d; } u; u.i = bits; return u.d; }
static inline int64_t ply_f64_bits(double d) { union { int64_t i; double d; } u; u.d = d; return u.i; }
static inline int ply_is_float(Word w) { return w != 0 && !ply_is_imm(w) && ply_obj(w)->kind == PLY_KIND_FLOAT; }

/* A `Decimal` in registers: the two words of its object, as `rust_decimal` serializes it. */
typedef struct { Word lo, hi; } PlyDec;
/* An `Option` held as its payload's word, zero for `None`. */
typedef Word PlyMaybe;
/* A `Result` as one word: its `Ok`'s argument's, or its `Err`'s object's with the low bits `100`. */
typedef Word PlyTagged;
static inline PlyDec ply_dec_at(Word base, int64_t at) { PlyDec d; d.lo = ply_words(base)[at]; d.hi = ply_words(base)[at + 1]; return d; }
static inline PlyDec ply_dec_of(Word w) { return ply_dec_at(w, 0); }
static inline void ply_set_dec(Word base, int64_t at, PlyDec d) { ply_words(base)[at] = d.lo; ply_words(base)[at + 1] = d.hi; }

static inline Word ply_imm(int64_t v) { return (Word)(((uint64_t)v << 1) | 1); }
static inline int64_t ply_imm_value(Word w) { return w >> 1; }
static inline int ply_fits_imm(int64_t v) { return v >= -(INT64_C(1) << 62) && v < (INT64_C(1) << 62); }

/* An immortal object carries `rc == UINT32_MAX` and is never counted, exactly as `heap.rs` says. */
/* A record or constructor dying at a count of one, with no children to let go of, becomes the
   token the next record or constructor of its size takes, its header as it was, so one released
   unused goes back to its own size. That is the whole of `heap::reset` for
   the shape the kernel builds, and inlining it here is what keeps a record's death off the call
   boundary --- the profile put `heap::reset` beside `round` itself, all of it the sixteen-child
   walk a FLAT record skips. */
static inline Word ply_reset_flat(Word w) {
  if (ply_is_imm(w) || w == 0) return 0;
  PlyObj *o = ply_obj(w);
  if ((o->kind != 3 && o->kind != 4) || o->rc != 1 || !(o->flags & 1)) return 0;
  return w;
}

/* A record held in place at word `at` of `base`'s, its header written as `heap::embed` writes it:
   its count is how many words before it its holder's header starts. */
static inline Word ply_embedded(Word base, int at, uint8_t flags, uint32_t len, uint32_t layout) {
  PlyObj *e = (PlyObj *)&ply_words(base)[at];
  e->rc = UINT32_C(0xC0000000) | (uint32_t)(PLY_HEADER / 8 + at);
  e->kind = 3;
  e->flags = flags;
  e->aux = 0;
  e->len = len;
  e->layout = layout;
  return (Word)(intptr_t)e;
}

/* A string's or bytes value's first byte: its own, or for a view the one its value holds where the
   view begins. */
static inline unsigned char *ply_bytes(Word w) {
  return (ply_obj(w)->flags & PLY_VIEW)
    ? (unsigned char *)ply_words(ply_words(w)[0]) + ply_words(w)[1]
    : (unsigned char *)ply_words(w);
}

/* An array's length: its words a stride at a time where it holds its elements in place. */
static inline int64_t ply_array_len(Word w) {
  PlyObj *o = ply_obj(w);
  return o->aux ? (int64_t)(o->len / o->aux) : (int64_t)o->len;
}

/* The word a field holds, at a statically known offset. */
static inline Word ply_field_at(Word base, int at) { return ply_words(base)[at]; }
static inline void ply_set_field(Word base, int at, Word v) { ply_words(base)[at] = v; }
"#;

/// A runtime helper the emitted C may call: arguments past the context, whether it answers, and
/// the function a loaded unit binds it to.
#[derive(Clone, Copy)]
pub struct Helper {
    pub name: &'static str,
    pub args: usize,
    pub answers: bool,
    pub address: *const (),
}

// SAFETY: an address is read and never written through.
unsafe impl Send for Helper {}
unsafe impl Sync for Helper {}

impl Helper {
    /// The helper `name` bound to `f`, taking and answering what `f`'s type says.
    fn of<F: Native>(name: &'static str, f: F) -> Helper {
        Helper {
            name,
            args: F::SIGNATURE.words(),
            answers: F::SIGNATURE.answers(),
            address: f.address(),
        }
    }
}

/// A native the compiler declares itself over machine data, under the name it declares and with
/// its function's signature; bound as the helper [`builtin_helper_name`] names for it.
pub struct MachineNative {
    pub name: &'static str,
    pub signature: Signature,
    pub helper: Helper,
}

macro_rules! machine {
    ($($name:literal => $f:ident($($t:ty),*) $(-> $r:ty)?;)*) => {
        fn machine_table() -> Vec<MachineNative> {
            vec![$({
                let f: unsafe extern "C" fn(*mut Ctx $(, $t)*) $(-> $r)? = crate::rt::$f;
                let helper = Box::leak(builtin_helper_name($name).into_boxed_str());
                MachineNative {
                    name: $name,
                    signature: signature_of(f),
                    helper: Helper::of(helper, f),
                }
            }),*]
        }
    };
}

fn signature_of<F: Native>(_: F) -> Signature {
    F::SIGNATURE
}

machine! {
    "?out_tag" => rt_out_tag(I64);
    "?out_int" => rt_out_int(I64);
    "?out_float" => rt_out_float(F64);
    "?out_decimal" => rt_out_decimal(I64, I64);
    "?out_fixed" => rt_out_fixed(I64, I64, I64);
    "?out_char" => rt_out_char(I64);
    "?out_str" => rt_out_str(Addr, I64);
    "?out_bytes" => rt_out_bytes(Addr, I64);
    "?out_count" => rt_out_count(I64, I64);
    "?out_ctor" => rt_out_ctor(Addr, I64, I64);
    "?out_name" => rt_out_name(Addr, I64);
    "?out_handle" => rt_out_handle(Handle);
    "?out_carried" => rt_out_carried(Handed, *const PlyType);
    "?out_fun" => rt_out_fun(Addr, I64);
    "?out_again" => rt_out_again(Handed) -> I64;
    "?in_tag" => rt_in_tag() -> I64;
    "?in_int" => rt_in_int() -> I64;
    "?in_float" => rt_in_float() -> F64;
    "?in_decimal" => rt_in_decimal() -> I64;
    "?in_fixed" => rt_in_fixed(I64) -> I64;
    "?in_high" => rt_in_high() -> I64;
    "?in_char" => rt_in_char() -> I64;
    "?in_str" => rt_in_str() -> I64;
    "?in_bytes" => rt_in_bytes() -> I64;
    "?in_count" => rt_in_count(I64) -> I64;
    "?in_field" => rt_in_field(Addr, I64);
    "?in_name_is" => rt_in_name_is(Addr, I64) -> I64;
    "?in_handle" => rt_in_handle() -> Handle;
    "?in_fun" => rt_in_fun(Out) -> I64;
    "?lower_open" => rt_lower_open(I64);
    "?lower_close" => rt_lower_close() -> I64;
    "?lift_open" => rt_lift_open(Addr, I64);
    "?lift_close" => rt_lift_close();
    "?taken" => rt_taken(Addr, I64);
    "?handler_typed" => rt_handler_typed(I64, Handed, *const PlyType);
    "?handler_get" => rt_handler_get(I64) -> Handed;
    "?handler_put" => rt_handler_put(I64, Handed);
    "?cost_mark" => rt_cost_mark(Out);
    "?cost_spent" => rt_cost_spent(Out);
    "?performs_since" => rt_performs_since(I64);
    "?in_carried" => rt_in_carried() -> Handed;
    "?array_bad" => rt_array_bad(I64, I64, I64) -> Raises;
}

/// The natives the compiler declares itself over machine data.
pub fn machine_natives() -> &'static [MachineNative] {
    static ALL: std::sync::OnceLock<Vec<MachineNative>> = std::sync::OnceLock::new();
    ALL.get_or_init(machine_table)
}

/// A helper's parameter as its function takes it: a `Word` unless written otherwise.
macro_rules! taken {
    ($p:ident) => {
        Word
    };
    ($p:ident: $t:ty) => {
        $t
    };
}

macro_rules! helpers {
    ($($f:ident($($p:ident $(: $t:ty)?),*) $(-> $r:ty)?;)*) => {
        /// The helpers that are no builtin's: what the emitted C does to values, frames and
        /// handlers. Each is written as its function's type, which that function must have.
        fn own() -> Vec<Helper> {
            vec![$({
                let f: unsafe extern "C" fn(*mut Ctx $(, taken!($p $(: $t)?))*) $(-> $r)? =
                    crate::rt::$f;
                Helper::of(stringify!($f), f)
            }),*]
        }
    };
}

helpers! {
    rt_dec(w);
    rt_reset(w) -> Word;
    rt_box_int(v) -> Word;
    rt_unbox_int(w) -> Word;
    rt_unbox_bool(w) -> Word;
    rt_box_float(bits) -> Word;
    rt_box_fixed(bits, t) -> Word;
    rt_box_decimal(lo, hi) -> Word;
    rt_decimal_binary(op, a_lo, a_hi, b_lo, b_hi, out);
    rt_decimal_compare(a_lo, a_hi, b_lo, b_hi) -> Word;
    rt_decimal_negate(lo, hi, out);
    rt_columns_field(xs, i, k) -> Word;
    rt_unbox_float(w) -> Word;
    rt_float_rem(a, b) -> Word;
    rt_no_fuel();
    rt_no_stack();
    rt_binary(op, a, b) -> Word;
    rt_wide_binary(op, t, a, b) -> Word;
    rt_negate(a) -> Word;
    rt_arith(op, a, b) -> Word;
    rt_lit(index) -> Word;
    rt_no_match();
    rt_let_no_match();
    rt_overflow(what);
    rt_not_that_width(which, value);
    rt_equal(a, b) -> Word;
    rt_between(v, lo, hi) -> Word;
    rt_concat(a, b) -> Word;
    rt_bytes_join(args: *const Word, n) -> Word;
    rt_builtin_value(index) -> Word;
    rt_ctor_value(index) -> Word;
    rt_constant(index) -> Word;
    rt_call(callee, args: *const Word, n) -> Word;
    rt_closure(index, arity, env: *const Word, n) -> Word;
    rt_lambda_value(index, arity, desc) -> Word;
    rt_iterate(seed, budget, f) -> Raising;
    rt_iterate_bad(what, n);
    rt_shift_count(n, which);
    rt_ctor(index, args: *const Word, n) -> Word;
    rt_field(base, index, own) -> Word;
    rt_list(args: *const Word, n) -> Word;
    rt_record(shape, args: *const Word, n) -> Word;
    rt_record_fits(value, len, exact) -> Word;
    rt_record_has(value, index) -> Word;
    rt_list_fits(value, len, exact) -> Word;
    rt_list_at(value, i) -> Word;
    rt_list_room(n) -> Word;
    rt_list_rest(value, from) -> Word;
    rt_alloc(kind, len, layout, flags) -> Word;
    rt_nullary(index) -> Word;

    rt_handle_push(clauses: *const Word, n, ret) -> Word;
    rt_perform(effect, op, resource, mode, args: *const Word, n) -> Word;
    rt_handle_land(depth, value) -> Word;
    rt_simulate(body) -> Word;
    rt_handle_detached(clauses: *const Word, n, ret, body) -> Word;

    rt_list_lookup(xs, i) -> Word;
    rt_map_lookup(m, k) -> Word;
    rt_maybe_take(w) -> Word;
    rt_maybe_box(w) -> Word;
    rt_maybe_open(w) -> Word;
    rt_tagged_take(w) -> Word;
    rt_tagged_box(w) -> Word;
    rt_tagged_open(w) -> Word;
    rt_tick();
    rt_stack_more() -> Word;
    rt_grow(entry, args) -> Word;
    rt_bitnot(a) -> Word;
    rt_inc_shared(w);
    rt_dec_shared(w);
    rt_parallel(slots, n);
    rt_parallel_past(slots, n, reach: *const Word, count);
    rt_array_lookup(xs, i) -> Word;
    rt_note_update(in_place, copied);
    rt_baked(text, len) -> Word;
    rt_stored(w) -> Word;
    rt_node(kind) -> Word;
    rt_node_writable(n) -> Word;
    rt_list_writable(xs, room) -> Word;
    rt_map_writable(m) -> Word;

    rt_alloc_bytes(bytes) -> Block;
    rt_alloc_kept(bytes) -> Block;
    rt_free(o: Block, bytes);
    rt_dec_shared_last(o: Block) -> Word;
    rt_handle_free(h: Block);
    rt_released_twice(o: Block);
    rt_release_later(w: Handed, root: DropRoot);
    rt_release_drain();
    rt_dispatch(op: *const PlyOp, resource: I64, mode: I64, args: *const Handed) -> Handed;
    rt_handle_install(clauses: *const Word, n: I64, ret: Handed) -> I64;
    rt_handle_install_detached(clauses: *const Word, n: I64, ret: Handed, body: Block) -> Handed;
    rt_with_cleanup(
        acquire: Block,
        body: Block,
        cleanup: Block,
        held: *const PlyType,
        answer: *const PlyType
    ) -> Handed;
    rt_box_wide(lo: I64, hi: I64, t: I64) -> Word;
    rt_builtin_encoded(index: I64);
    rt_region(body: Block, answer: *const PlyType) -> Handed;
    rt_branches(slots: *const Handed, n: I64);
    rt_branches_past(slots: *const Handed, n: I64, reach: *const Word, count: I64);
    rt_memo(index: I64, keep: KeepRoot) -> Handed;
}

/// The helper a builtin is called through, named for it. An elaboration's `?` is spelled out, since
/// no C name holds one, and a function of a type is its type in lower case, `_` and its name.
pub fn builtin_helper_name(builtin: &str) -> String {
    match (builtin.strip_prefix('?'), builtin.split_once("::")) {
        (Some(written), _) => format!("builtin_elaborated_{written}"),
        (None, Some((of, name))) => format!("builtin_{}_{name}", of.to_ascii_lowercase()),
        (None, None) => format!("builtin_{builtin}"),
    }
}

/// Every helper a unit may bind: the runtime's own, then one a builtin, then one a native over
/// machine data.
pub fn helpers() -> &'static [Helper] {
    static ALL: std::sync::OnceLock<Vec<Helper>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let builtins = ply_core::Builtin::all().iter().map(|b| {
            let signature = crate::rt::signature(*b);
            Helper {
                name: Box::leak(builtin_helper_name(b.name()).into_boxed_str()),
                args: signature.words(),
                answers: signature.answers(),
                address: crate::rt::builtin_address(*b),
            }
        });
        let machine = machine_natives().iter().map(|n| n.helper);
        own().into_iter().chain(builtins).chain(machine).collect()
    })
}

/// The line that opens the runtime's definitions: everything from it on is the unit's tail, the
/// one translation unit that defines what [`runtime_header`] declares.
pub const RUNTIME_MARK: &str = "/* --- the runtime, bound at load --- */";

/// A helper's C return type and parameter list.
fn signature(h: &Helper) -> (&'static str, String) {
    let ret = if h.answers { "Word" } else { "void" };
    let mut params = String::from("PlyCtx*");
    for _ in 0..h.args {
        params.push_str(", Word");
    }
    (ret, params)
}

/// What every translation unit of a unit opens with after [`PRELUDE`]: the helper pointers and
/// singletons declared, and `ply_dec` over them. Declared, not defined, so a unit compiled in
/// parts binds one table rather than one per part.
pub fn runtime_header() -> String {
    let mut out = String::from("\n/* --- the runtime, declared --- */\n");
    for h in helpers() {
        let (ret, params) = signature(h);
        out.push_str(&format!(
            "extern {ret} (*{})({params});\n",
            pointer_name(h.name)
        ));
    }
    // Singletons are heap addresses: bound at load, not baked in, or cached objects break.
    out.push_str("extern Word ply_true, ply_false, ply_unit;\n");
    // Trap: `rt_dec` frees unconditionally (it is `release_last`); only call it once `rc == 1`.
    // A count with its top bit set is shared (`heap::SHARED`): tcc has no atomics, so the runtime
    // changes it. One with its top two set is a record's held in place (`heap::EMBEDDED`), which
    // the record holding it counts for it.
    out.push_str(
        "\nstatic inline PlyObj *ply_counted_by(PlyObj *o) {\n\
         \x20 uint32_t rc = o->rc;\n\
         \x20 while (rc >= UINT32_C(0xC0000000) && rc != UINT32_MAX) {\n\
         \x20   o = (PlyObj *)((Word *)o - (rc & UINT32_C(0x3FFFFFFF)));\n\
         \x20   rc = o->rc;\n\
         \x20 }\n\
         \x20 return o;\n\
         }\n\
         \nstatic inline void ply_inc(Word w) {\n\
         \x20 if ((w & 3) != 0 || w == 0) return;\n\
         \x20 PlyObj *o = ply_counted_by(ply_obj(w & ~(Word)7));\n\
         \x20 uint32_t rc = o->rc;\n\
         \x20 if (rc < UINT32_C(0x80000000)) o->rc = rc + 1;\n\
         \x20 else if (rc != UINT32_MAX) rt_inc_shared_p(0, (Word)o);\n\
         }\n\
         \nstatic inline void ply_dec(PlyCtx *ctx, Word w) {\n\
         \x20 if ((w & 3) != 0 || w == 0) return;\n\
         \x20 PlyObj *o = ply_counted_by(ply_obj(w & ~(Word)7));\n\
         \x20 uint32_t rc = o->rc;\n\
         \x20 if (rc == UINT32_MAX) return;\n\
         \x20 if (rc >= UINT32_C(0x80000000)) { rt_dec_shared_p(ctx, (Word)o); return; }\n\
         \x20 if (rc > 1) { o->rc = rc - 1; return; }\n\
         \x20 rt_dec_p(ctx, (Word)o);\n\
         }\n",
    );
    // `heap::embed`: a unique record's words move, so only its shell is let go.
    out.push_str(
        "\nstatic inline void ply_embed(PlyCtx *ctx, Word base, int at, Word child, int tail) {\n\
         \x20 PlyObj *c = ply_obj(child);\n\
         \x20 uint32_t len = c->len;\n\
         \x20 uint8_t flags = c->flags;\n\
         \x20 Word *src = ply_words(child);\n\
         \x20 Word *dst = ply_words(base) + at + PLY_HEADER / 8;\n\
         \x20 memcpy(dst, src, (size_t)len * 8);\n\
         \x20 if (tail) {\n\
         \x20   if (flags & PLY_MIXED) memcpy(dst + len, src + len, (size_t)tail * 8);\n\
         \x20   else memset(dst + len, 0, (size_t)tail * 8);\n\
         \x20 }\n\
         \x20 ply_embedded(base, at, flags, len, c->layout);\n\
         \x20 if (c->rc == 1) c->flags = flags | PLY_FLAT;\n\
         \x20 else if (!(flags & PLY_FLAT))\n\
         \x20   for (uint32_t i = 0; i < len; i++)\n\
         \x20     if (!(flags & PLY_MIXED) || ((uint64_t)src[len + i / 64] >> (i % 64)) & 1) ply_inc(src[i]);\n\
         \x20 ply_dec(ctx, child);\n\
         }\n",
    );
    // What a record held in place counts, let go of before another is written over it.
    out.push_str(
        "\nstatic inline void ply_drop_embedded(PlyCtx *ctx, Word base, int at) {\n\
         \x20 PlyObj *e = (PlyObj *)&ply_words(base)[at];\n\
         \x20 uint32_t len = e->len;\n\
         \x20 uint8_t flags = e->flags;\n\
         \x20 Word *w = ply_words(base) + at + PLY_HEADER / 8;\n\
         \x20 if (flags & PLY_FLAT) return;\n\
         \x20 for (uint32_t i = 0; i < len; i++)\n\
         \x20   if (!(flags & PLY_MIXED) || ((uint64_t)w[len + i / 64] >> (i % 64)) & 1) ply_dec(ctx, w[i]);\n\
         }\n",
    );
    // A field's last read, where the record is its only holder: what it counts moves with it.
    out.push_str(
        "\nstatic inline Word ply_take(PlyCtx *ctx, Word base, int at) {\n\
         \x20 Word inner = (Word)(intptr_t)(ply_words(base) + at);\n\
         \x20 if (ply_obj(base)->rc != 1) { ply_inc(inner); return inner; }\n\
         \x20 PlyObj *e = ply_obj(inner);\n\
         \x20 uint32_t len = e->len;\n\
         \x20 uint8_t flags = e->flags;\n\
         \x20 Word *src = ply_words(inner);\n\
         \x20 Word out = rt_alloc_p(ctx, 3, len, e->layout, flags);\n\
         \x20 memcpy(ply_words(out), src, ((size_t)len + ((flags & PLY_MIXED) ? (len + 63) / 64 : 0)) * 8);\n\
         \x20 if (!(flags & PLY_FLAT))\n\
         \x20   for (uint32_t i = 0; i < len; i++)\n\
         \x20     if (!(flags & PLY_MIXED) || ((uint64_t)src[len + i / 64] >> (i % 64)) & 1) src[i] = 0;\n\
         \x20 return out;\n\
         }\n",
    );
    out
}

/// The runtime's definitions and the exported binders that fill them, generated from
/// [`helpers`]; the unit's tail holds them once.
pub fn runtime_object() -> String {
    let mut out = format!("\n{RUNTIME_MARK}\n");
    for h in helpers() {
        let (ret, params) = signature(h);
        out.push_str(&format!("{ret} (*{})({params});\n", pointer_name(h.name)));
    }
    out.push_str("Word ply_true, ply_false, ply_unit;\n");
    out.push_str(
        "void ply_bind_singletons(Word t, Word f, Word u) { ply_true = t; ply_false = f; ply_unit = u; }\n",
    );
    out.push_str("\nvoid ply_bind(void **fns) {\n");
    for (i, h) in helpers().iter().enumerate() {
        let (ret, params) = signature(h);
        out.push_str(&format!(
            "  {} = ({ret} (*)({params}))fns[{i}];\n",
            pointer_name(h.name)
        ));
    }
    out.push_str("}\n");
    out
}

/// A helper's function-pointer name; distinct from the helper's, so a missed binding fails to link.
pub fn pointer_name(helper: &str) -> String {
    format!("{helper}_p")
}

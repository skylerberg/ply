//! The C every emitted unit opens with. It mirrors `heap.rs` and `rt.rs`, and
//! `the_prelude_agrees_with_the_layouts_it_mirrors` checks it against them.

use crate::emitted::{Block, DropRoot, Handed, Handle, KeepRoot, PlyOp, PlyType};
use crate::heap::Word;
use crate::native::{Addr, F64, I64, Native, Raises, Signature};
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

/* `crates/ply-compiled/src/heap.rs`'s `Obj`, a block's header: its count, in a word of its own. */
typedef struct {
  uint32_t rc;
  uint32_t unused;
} PlyObj;

#define PLY_HEADER 8
#define PLY_RC_FRAME (UINT32_MAX - 1)

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
static inline int ply_mul_ov(int64_t a, int64_t b, int64_t *r) { return __builtin_mul_overflow(a, b, r); }
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
/* The product of the magnitudes, unsigned, overflows where dividing it back does not give the other
   magnitude; then it must fit the sign's range, one more below zero than above. */
static inline int ply_mul_ov(int64_t a, int64_t b, int64_t *r) {
  uint64_t x = a < 0 ? 0 - (uint64_t)a : (uint64_t)a;
  uint64_t y = b < 0 ? 0 - (uint64_t)b : (uint64_t)b;
  uint64_t m = x * y;
  *r = (int64_t)((uint64_t)a * (uint64_t)b);
  if (x != 0 && m / x != y) return 1;
  return m > (uint64_t)INT64_MAX + ((a < 0) != (b < 0));
}
#endif

/* A `Float`'s bits, as a word holds them. */
static inline double ply_f64(int64_t bits) { union { int64_t i; double d; } u; u.i = bits; return u.d; }
static inline int64_t ply_f64_bits(double d) { union { int64_t i; double d; } u; u.d = d; return u.i; }

/* A `Decimal` in registers: the two words of its object, as `rust_decimal` serializes it. */
typedef struct { Word lo, hi; } PlyDec;
/* An `Option` held as its payload's word, zero for `None`. */
typedef Word PlyMaybe;
/* A `Result` as one word: its `Ok`'s argument's, or its `Err`'s object's with the low bits `100`. */
typedef Word PlyTagged;
static inline PlyDec ply_dec_at(Word base, int64_t at) { PlyDec d; d.lo = ply_words(base)[at]; d.hi = ply_words(base)[at + 1]; return d; }
static inline PlyDec ply_dec_of(Word w) { return ply_dec_at(w, 0); }
/* A `U128` or an `I128` in registers: its low word, then its high one. */
typedef struct { Word lo, hi; } PlyWide;
static inline PlyWide ply_wide_at(Word base, int64_t at) { PlyWide x; x.lo = ply_words(base)[at]; x.hi = ply_words(base)[at + 1]; return x; }
static inline PlyWide ply_wide_of(Word w) { return ply_wide_at(w, 0); }
/* -1, 0 or 1 as `a` is below, at or above `b`, their high words signed where `s` is. */
static inline int64_t ply_wide_order(PlyWide a, PlyWide b, int s) {
  if (a.hi != b.hi) return (s ? (int64_t)a.hi < (int64_t)b.hi : (uint64_t)a.hi < (uint64_t)b.hi) ? -1 : 1;
  return ((uint64_t)a.lo > (uint64_t)b.lo) - ((uint64_t)a.lo < (uint64_t)b.lo);
}

static inline Word ply_imm(int64_t v) { return (Word)(((uint64_t)v << 1) | 1); }
static inline int64_t ply_imm_value(Word w) { return w >> 1; }
static inline int ply_fits_imm(int64_t v) { return v >= -(INT64_C(1) << 62) && v < (INT64_C(1) << 62); }
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

/// The native the compiler declares as `name`, bound to `f` and taking and answering what `f`'s
/// type says.
pub(crate) fn machine_native<F: Native>(name: &'static str, f: F) -> MachineNative {
    let helper = Box::leak(builtin_helper_name(name).into_boxed_str());
    MachineNative {
        name,
        signature: F::SIGNATURE,
        helper: Helper::of(helper, f),
    }
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
    "?out_fun" => rt_out_fun(Handed, Addr, I64);
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
    "?lower_open" => rt_lower_open(I64);
    "?lower_close" => rt_lower_close() -> I64;
    "?lift_open" => rt_lift_open(Addr, I64);
    "?lift_close" => rt_lift_close();
    "?taken" => rt_taken(Addr, I64);
    "?handler_typed" => rt_handler_typed(I64, Handed, *const PlyType);
    "?handler_get" => rt_handler_get(I64) -> Handed;
    "?handler_put" => rt_handler_put(I64, Handed);
    "?hold_made" => rt_hold_made(Handed) -> Handle;
    "?hold_read" => rt_hold_read(Handle) -> Handed;
    "?hold_taken" => rt_hold_taken(Handle) -> Handed;
    "?cost_mark" => rt_cost_mark() -> I64;
    "?cost_spent" => rt_cost_spent(I64, I64);
    "?in_carried" => rt_in_carried() -> Handed;
    "?refused" => rt_refused(I64, I64, I64, I64) -> Raises;
}

/// The natives the compiler declares itself over machine data: the crossings' and the rest of
/// this table's, then strings' and byte strings' (`crate::machine`).
pub fn machine_natives() -> &'static [MachineNative] {
    static ALL: std::sync::OnceLock<Vec<MachineNative>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = machine_table();
        all.extend(crate::machine::natives());
        all
    })
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
    rt_box_decimal(lo, hi) -> Word;
    rt_decimal_binary(op, a_lo, a_hi, b_lo, b_hi, out);
    rt_decimal_compare(a_lo, a_hi, b_lo, b_hi) -> Word;
    rt_decimal_negate(lo, hi, out);
    rt_wide_op(code, a_lo, a_hi, b_lo, b_hi, out) -> Word;
    rt_float_rem(a, b) -> Word;
    rt_no_fuel();
    rt_wide_binary(op, t, a, b) -> Word;
    rt_no_match();
    rt_let_no_match();
    rt_overflow(what);
    rt_by_zero(what);
    rt_not_an_int(value);
    rt_not_an_int_wide(lo, hi, signed);
    rt_not_that_width(which, value);
    rt_iterate_bad(what, n);
    rt_shift_count(n, which);

    rt_handle_land(depth, value) -> Word;

    rt_tick();
    rt_stack_more() -> Word;
    rt_inc_shared(w);
    rt_note_update(in_place, copied);

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
    rt_never_lifted();
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

/// Every helper a unit may bind: the runtime's own, then a native over machine data.
pub fn helpers() -> &'static [Helper] {
    static ALL: std::sync::OnceLock<Vec<Helper>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let machine = machine_natives().iter().map(|n| n.helper);
        own().into_iter().chain(machine).collect()
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

/// What every translation unit of a unit opens with after [`PRELUDE`]: the helper pointers
/// declared, not defined, so a unit compiled in parts binds one table rather than one per part.
pub fn runtime_header() -> String {
    let mut out = String::from("\n/* --- the runtime, declared --- */\n");
    for h in helpers() {
        let (ret, params) = signature(h);
        out.push_str(&format!(
            "extern {ret} (*{})({params});\n",
            pointer_name(h.name)
        ));
    }
    out
}

/// The runtime's definitions and the exported binder that fills them, generated from [`helpers`];
/// the unit's tail holds them once.
pub fn runtime_object() -> String {
    let mut out = format!("\n{RUNTIME_MARK}\n");
    for h in helpers() {
        let (ret, params) = signature(h);
        out.push_str(&format!("{ret} (*{})({params});\n", pointer_name(h.name)));
    }
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

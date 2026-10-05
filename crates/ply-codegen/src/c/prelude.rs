//! The C every emitted unit opens with. It mirrors `heap.rs` and `rt.rs`, and
//! `the_prelude_agrees_with_the_layouts_it_mirrors` checks it against them.

/// `rt_*` is declared, not defined: `ply_bind` supplies the addresses after `dlopen`, since a
/// shared object cannot see symbols the host executable did not export.
pub const PRELUDE: &str = r#"
#include <stdint.h>
#include <string.h>

typedef int64_t Word;

/* `crates/ply-codegen/src/heap.rs`'s `Obj`, which is `#[repr(C, align(8))]`. */
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
static inline int ply_is_imm(Word w) { return (w & 1) != 0; }
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

static inline Word ply_imm(int64_t v) { return (Word)(((uint64_t)v << 1) | 1); }
static inline int64_t ply_imm_value(Word w) { return w >> 1; }
static inline int ply_fits_imm(int64_t v) { return v >= -(INT64_C(1) << 62) && v < (INT64_C(1) << 62); }

/* An immortal object carries `rc == UINT32_MAX` and is never counted, exactly as `heap.rs` says. */
/* A record dying at a count of one, with no children to let go of, becomes the token the next
   record of its size takes. That is the whole of `heap::reset` for the shape the kernel builds,
   and inlining it here is what keeps a record's death off the call boundary --- the profile put
   `heap::reset` beside `round` itself, all of it the sixteen-child walk a FLAT record skips. */
static inline Word ply_reset_flat(Word w) {
  if (ply_is_imm(w) || w == 0) return 0;
  PlyObj *o = ply_obj(w);
  if (o->kind != 3 || o->rc != 1 || !(o->flags & 1)) return 0;
  o->len = 0;
  return w;
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

macro_rules! helpers {
    ($(($f:ident, $a:literal, $r:literal)),* $(,)?) => {
        /// The helpers that are no builtin's: what the emitted C does to values, frames and handlers.
        const OWN: &[Helper] = &[$(Helper {
            name: stringify!($f),
            args: $a,
            answers: $r,
            address: crate::rt::$f as *const (),
        }),*];
    };
}

helpers![
    (rt_dec, 1, false),
    (rt_reset, 1, true),
    (rt_box_int, 1, true),
    (rt_unbox_int, 1, true),
    (rt_unbox_bool, 1, true),
    (rt_no_fuel, 0, false),
    (rt_no_stack, 0, false),
    (rt_binary, 3, true),
    (rt_negate, 1, true),
    (rt_arith, 3, true),
    (rt_lit, 1, true),
    (rt_no_match, 0, false),
    (rt_let_no_match, 0, false),
    (rt_overflow, 1, false),
    (rt_not_that_width, 2, false),
    (rt_equal, 2, true),
    (rt_between, 3, true),
    (rt_concat, 2, true),
    (rt_bytes_join, 2, true),
    (rt_builtin_value, 1, true),
    (rt_ctor_value, 1, true),
    (rt_constant, 1, true),
    (rt_call, 3, true),
    (rt_closure, 4, true),
    (rt_iterate, 3, true),
    (rt_iterate_bad, 2, false),
    (rt_shift_count, 2, false),
    (rt_ctor, 3, true),
    (rt_field, 3, true),
    (rt_list, 2, true),
    (rt_record_fits, 3, true),
    (rt_record_has, 2, true),
    (rt_list_fits, 3, true),
    (rt_list_at, 2, true),
    (rt_list_rest, 2, true),
    (rt_alloc, 4, true),
    (rt_nullary, 1, true),
    (rt_cell, 1, true),
    (rt_handle_push, 3, true),
    (rt_perform, 6, true),
    (rt_handle_land, 2, true),
    (rt_simulate, 1, true),
    (rt_handle_detached, 4, true),
    (rt_region, 1, true),
    (rt_region_close, 1, false),
    (rt_list_lookup, 2, true),
    (rt_map_lookup, 2, true),
    (rt_tick, 0, false),
    (rt_grow, 2, true),
    (rt_bitnot, 1, true),
    (rt_inc_shared, 1, false),
    (rt_dec_shared, 1, false),
    (rt_parallel, 2, false),
    (rt_array_lookup, 2, true),
    (rt_baked, 2, true),
    (rt_stored, 1, true),
];

/// The helper a builtin is called through, named for it. An elaboration's `?` is spelled out, since
/// no C name holds one.
pub fn builtin_helper_name(builtin: &str) -> String {
    match builtin.strip_prefix('?') {
        Some(written) => format!("builtin_elaborated_{written}"),
        None => format!("builtin_{builtin}"),
    }
}

/// Every helper a unit may bind: the runtime's own, then one a builtin.
pub fn helpers() -> &'static [Helper] {
    static ALL: std::sync::OnceLock<Vec<Helper>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let builtins = ply_eval::Builtin::all().iter().map(|b| Helper {
            name: Box::leak(builtin_helper_name(b.name()).into_boxed_str()),
            args: b.arity(),
            answers: true,
            address: crate::rt::builtin_address(*b),
        });
        OWN.iter().copied().chain(builtins).collect()
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
    // changes it.
    out.push_str(
        "\nstatic inline void ply_inc(Word w) {\n\
         \x20 if (ply_is_imm(w) || w == 0) return;\n\
         \x20 PlyObj *o = ply_obj(w);\n\
         \x20 uint32_t rc = o->rc;\n\
         \x20 if (rc < UINT32_C(0x80000000)) o->rc = rc + 1;\n\
         \x20 else if (rc != UINT32_MAX) rt_inc_shared_p(0, w);\n\
         }\n\
         \nstatic inline void ply_dec(PlyCtx *ctx, Word w) {\n\
         \x20 if (ply_is_imm(w) || w == 0) return;\n\
         \x20 PlyObj *o = ply_obj(w);\n\
         \x20 uint32_t rc = o->rc;\n\
         \x20 if (rc == UINT32_MAX) return;\n\
         \x20 if (rc >= UINT32_C(0x80000000)) { rt_dec_shared_p(ctx, w); return; }\n\
         \x20 if (rc > 1) { o->rc = rc - 1; return; }\n\
         \x20 rt_dec_p(ctx, w);\n\
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

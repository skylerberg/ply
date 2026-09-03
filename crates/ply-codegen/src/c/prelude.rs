//! The C every emitted unit opens with: the value model's layout, the runtime's declarations, and
//! the handful of operations that must be inline for a C compiler to see through them.
//!
//! Everything here mirrors something in `heap.rs` or `rt.rs` and is checked against it by
//! `crate::c::tests::the_prelude_agrees_with_the_layouts_it_mirrors` — a header that drifts from
//! the Rust it describes is a wrong answer, not a slow one.

/// The layouts, and the inline operations. `rt_*` is declared rather than defined: the loader
/// binds the addresses through [`super::BIND`] after `dlopen`, because a shared object cannot see
/// symbols the host executable did not export.
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

/* `Ctx`, whose first two fields compiled code reads and writes directly. The rest is opaque: a
   pointer to it is all the runtime's helpers want. */
typedef struct {
  int64_t failed;
  int64_t fuel;
} PlyCtx;

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
static inline Word ply_imm(int64_t v) { return (Word)(((uint64_t)v << 1) | 1); }
static inline int64_t ply_imm_value(Word w) { return w >> 1; }
static inline int ply_fits_imm(int64_t v) { return ((v << 1) >> 1) == v; }

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

static inline void ply_inc(Word w) {
  if (!ply_is_imm(w) && w != 0) {
    PlyObj *o = ply_obj(w);
    if (o->rc != UINT32_MAX) o->rc += 1;
  }
}

/* The word a field holds, at a statically known offset. */
static inline Word ply_field_at(Word base, int at) { return ply_words(base)[at]; }
static inline void ply_set_field(Word base, int at, Word v) { ply_words(base)[at] = v; }
"#;

/// One runtime helper the emitted C may call: its name, how many arguments it takes past the
/// context, and whether it answers a word. The table is what both the declarations and the
/// binding are generated from, so a helper cannot be declared and left unbound.
pub struct Helper {
    pub name: &'static str,
    pub args: usize,
    pub answers: bool,
}

macro_rules! helpers {
    ($(($n:literal, $a:literal, $r:literal)),* $(,)?) => {
        pub const HELPERS: &[Helper] = &[$(Helper { name: $n, args: $a, answers: $r }),*];
    };
}

helpers![
    ("rt_dup", 1, true),
    ("rt_dec", 1, false),
    ("rt_reset", 1, true),
    ("rt_box_int", 1, true),
    ("rt_unbox_int", 1, true),
    ("rt_unbox_bool", 1, true),
    ("rt_no_fuel", 0, false),
    ("rt_arith", 3, true),
    ("rt_lit", 1, true),
    ("rt_no_match", 0, false),
    ("rt_overflow", 1, false),
    ("rt_not_that_width", 2, false),
    ("rt_equal", 2, true),
    ("rt_concat", 2, true),
    ("rt_builtin", 3, true),
    ("rt_bytes_join", 2, true),
    ("rt_builtin_value", 1, true),
    ("rt_ctor_value", 1, true),
    ("rt_constant", 1, true),
    ("rt_call", 3, true),
    ("rt_iterate_bad", 2, false),
    ("rt_shift_count", 1, false),
    ("rt_ctor", 3, true),
    ("rt_record", 3, true),
    ("rt_field", 3, true),
    ("rt_list", 2, true),
    ("rt_record_fits", 3, true),
    ("rt_record_has", 2, true),
    ("rt_list_fits", 3, true),
    ("rt_list_at", 2, true),
    ("rt_list_rest", 2, true),
    ("rt_ctor_arg", 3, true),
    ("rt_alloc", 4, true),
    ("rt_list_index", 2, true),
    ("rt_nullary", 1, true),
];

/// The declarations, the function-pointer table and the exported binder, generated from
/// [`HELPERS`] so the three cannot disagree.
pub fn runtime_decls() -> String {
    let mut out = String::from("\n/* --- the runtime, bound at load --- */\n");
    for h in HELPERS {
        let ret = if h.answers { "Word" } else { "void" };
        let mut params = String::from("PlyCtx*");
        for _ in 0..h.args {
            params.push_str(", Word");
        }
        out.push_str(&format!(
            "static {ret} (*{})({params});\n",
            pointer_name(h.name)
        ));
    }
    out.push_str("\nvoid ply_bind(void **fns) {\n");
    for (i, h) in HELPERS.iter().enumerate() {
        let ret = if h.answers { "Word" } else { "void" };
        let mut params = String::from("PlyCtx*");
        for _ in 0..h.args {
            params.push_str(", Word");
        }
        out.push_str(&format!(
            "  {} = ({ret} (*)({params}))fns[{i}];\n",
            pointer_name(h.name)
        ));
    }
    out.push_str("}\n");
    out
}

/// A helper's function-pointer name. The pointer is not the helper's own name, so a unit that
/// forgets to bind one is a link error rather than a call into the host's copy.
pub fn pointer_name(helper: &str) -> String {
    format!("{helper}_p")
}

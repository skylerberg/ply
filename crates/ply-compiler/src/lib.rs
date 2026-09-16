//! The compiler written in Ply: the front end and the emitter the C tier's units come from.
//!
//! This is the self-hosted half of ADR 0042, and under tier-only (ADR 0048) it is not an
//! alternative to anything -- it is the producer every `ply test`, `ply run` and artifact compiles
//! through. `crates/ply-codegen`'s emitter is the fragment it is checked against, byte for byte,
//! by `crates/ply-compiler-diff`.
//!
//! It ships the way `ply-std` ships the standard library: the text is in `ply/`, `include_str!` is
//! what puts it in the binary, and [`MODULES`] is the trusted list read top to bottom. Nothing here
//! reads the file system, so a `ply` binary carries its own compiler rather than looking beside
//! itself for one.

/// The token stream: the reader `spine` drives.
pub const LEXER: &str = include_str!("../ply/lexer.ply");

/// The parser's spine -- a module, its items, and the reader the rest hangs off.
pub const SPINE: &str = include_str!("../ply/spine.ply");

/// Expressions, by precedence.
pub const EXPRS: &str = include_str!("../ply/exprs.ply");

/// Items: definitions, types, effects, laws, tests.
pub const ITEMS: &str = include_str!("../ply/items.ply");

/// Patterns, in `match` and in a binding position.
pub const PATTERNS: &str = include_str!("../ply/patterns.ply");

/// Type syntax.
pub const TYPES: &str = include_str!("../ply/types.ply");

/// The type representation the checker works over.
pub const TYCORE: &str = include_str!("../ply/tycore.ply");

/// Name resolution.
pub const RESOLVE: &str = include_str!("../ply/resolve.ply");

/// The three surface rewrites: defaults, record update, and the `?` operator.
pub const REWRITE: &str = include_str!("../ply/rewrite.ply");

/// Type inference, and the effect rows it settles.
pub const INFER: &str = include_str!("../ply/infer.ply");

/// The derivers.
pub const DERIVE: &str = include_str!("../ply/derive.ply");

/// Every diagnostic with its message, framed for the CLI.
pub const DIAG: &str = include_str!("../ply/diag.ply");

/// Content addressing: a definition's hash over its normalized form.
pub const HASH: &str = include_str!("../ply/hash.ply");

/// The lowered form the emitter walks.
pub const CODE: &str = include_str!("../ply/code.ply");

/// The emitter: the lowered form as C.
pub const EMIT: &str = include_str!("../ply/emit.ply");

/// The trusted list, read top to bottom, as `ply-std`'s is.
///
/// **The order is the identity.** `ply_codegen::c::producer` digests these texts in this order to
/// key everything the emitter produces, and the bootstrap bundle carries that digest; the order
/// here is the alphabetical one the directory walk it replaced produced, so a bundle emitted
/// before this crate existed still serves.
pub const MODULES: &[(&str, &str)] = &[
    ("code", CODE),
    ("derive", DERIVE),
    ("diag", DIAG),
    ("emit", EMIT),
    ("exprs", EXPRS),
    ("hash", HASH),
    ("infer", INFER),
    ("items", ITEMS),
    ("lexer", LEXER),
    ("patterns", PATTERNS),
    ("resolve", RESOLVE),
    ("rewrite", REWRITE),
    ("spine", SPINE),
    ("tycore", TYCORE),
    ("types", TYPES),
];

pub fn sources() -> impl Iterator<Item = (&'static str, &'static str)> {
    MODULES.iter().copied()
}

/// The bootstrap bundle: the C this compiler last emitted for itself.
///
/// A compiler written in the language it compiles has to come from somewhere, and this is where:
/// the unit is loaded from these bytes rather than emitted by the Rust fragment, which could not
/// emit it anyway -- the fragment refuses `perform`. `crates/ply-codegen-tests`'s fixpoint test is
/// what says the bundle serves: it was emitted from these sources, and the emitter built from it
/// emits, for them, the C it was built from.
pub mod bootstrap {
    /// The unit's C, gzipped. It carries everything loading it needs -- the runtime helper table
    /// it was emitted against, the constructor table its tags are positions in, its functions and
    /// their arities, its tables -- so building the emitter from it parses none of these sources,
    /// and it serves any runtime whose helper table starts with its own.
    pub const UNIT: &[u8] = include_bytes!("../bootstrap/unit.c.gz");

    /// The digest of the sources it was emitted from. The fixpoint test refuses a bundle whose
    /// digest is not these sources', and CI hands back a refreshed one as an artifact.
    pub const SOURCES: &str = include_str!("../bootstrap/SOURCES.digest");

    /// Where the bundle lives in the source tree, for the refresh that rewrites it.
    pub const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/bootstrap");
}

//! The compiler written in Ply, embedded as source text so a `ply` binary carries its own compiler.

pub const LEXER: &str = include_str!("../ply/lexer.ply");

pub const SPINE: &str = include_str!("../ply/spine.ply");

pub const EXPRS: &str = include_str!("../ply/exprs.ply");

pub const ITEMS: &str = include_str!("../ply/items.ply");

pub const PATTERNS: &str = include_str!("../ply/patterns.ply");

pub const TYPES: &str = include_str!("../ply/types.ply");

pub const TYCORE: &str = include_str!("../ply/tycore.ply");

pub const RESOLVE: &str = include_str!("../ply/resolve.ply");

/// Effect sets, the `?` operator, and the record update a checked program writes out.
pub const REWRITE: &str = include_str!("../ply/rewrite.ply");

pub const INFER: &str = include_str!("../ply/infer.ply");

pub const DERIVE: &str = include_str!("../ply/derive.ply");

pub const DIAG: &str = include_str!("../ply/diag.ply");

/// The front end's whole answer, framed for the driver.
pub const FRONT: &str = include_str!("../ply/front.ply");

pub const HASH: &str = include_str!("../ply/hash.ply");

pub const CODE: &str = include_str!("../ply/code.ply");

pub const COSTS: &str = include_str!("../ply/costs.ply");

pub const EMIT: &str = include_str!("../ply/emit.ply");

/// `ply fmt`: the tree printed back as source.
pub const FMT: &str = include_str!("../ply/fmt.ply");

/// The order is part of the identity: the producer digests these texts in this order.
pub const MODULES: &[(&str, &str)] = &[
    ("code", CODE),
    ("costs", COSTS),
    ("derive", DERIVE),
    ("diag", DIAG),
    ("emit", EMIT),
    ("exprs", EXPRS),
    ("fmt", FMT),
    ("front", FRONT),
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
pub mod bootstrap {
    /// The unit's C, gzipped; self-describing, so loading it parses none of the sources.
    pub const UNIT: &[u8] = include_bytes!("../bootstrap/unit.c.gz");

    /// The digest of the sources it was emitted from.
    pub const SOURCES: &str = include_str!("../bootstrap/SOURCES.digest");

    pub const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/bootstrap");
}

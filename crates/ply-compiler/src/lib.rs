//! The compiler written in Ply, embedded as source text so a `ply` binary carries its own compiler.

pub const LEXER: &str = include_str!("../ply/lexer.ply");

pub const PARSING: &str = include_str!("../ply/parsing.ply");

pub const EXPRS: &str = include_str!("../ply/exprs.ply");

pub const ITEMS: &str = include_str!("../ply/items.ply");

pub const PATTERNS: &str = include_str!("../ply/patterns.ply");

pub const PKG: &str = include_str!("../ply/pkg.ply");

pub const TYPE_SYNTAX: &str = include_str!("../ply/type_syntax.ply");

pub const TYPES: &str = include_str!("../ply/types.ply");

/// Whether a recursive group's calls back into itself descend a measure.
pub const TERMINATION: &str = include_str!("../ply/termination.ply");

pub const RESOLVE: &str = include_str!("../ply/resolve.ply");

/// Effect sets, the `?` operator, and the record update a checked program writes out.
pub const REWRITE: &str = include_str!("../ply/rewrite.ply");

pub const INFER: &str = include_str!("../ply/infer.ply");

pub const INTERFACE: &str = include_str!("../ply/interface.ply");

pub const DERIVE: &str = include_str!("../ply/derive.ply");

pub const DIAGNOSTICS: &str = include_str!("../ply/diagnostics.ply");

/// The front end's whole answer, as the rows the driver reads.
pub const FRONT: &str = include_str!("../ply/front.ply");

pub const HASH: &str = include_str!("../ply/hash.ply");

pub const CODE: &str = include_str!("../ply/code.ply");

pub const COSTS: &str = include_str!("../ply/costs.ply");

/// The `.plyx` container: the header, the section table, and what a program digest covers.
pub const PLYX: &str = include_str!("../ply/plyx.ply");

/// The one module that embeds [`PRELUDE`], kept small: a load reads it whole to learn that.
pub const PRELUDE_SOURCE: &str = include_str!("../ply/prelude_source.ply");

pub const EMIT: &str = include_str!("../ply/emit.ply");

/// `embed` and `embed_dir`, written out as the literals the driver read for them.
pub const EMBED: &str = include_str!("../ply/embed.ply");

/// `ply fmt`: the tree printed back as source.
pub const FMT: &str = include_str!("../ply/fmt.ply");

/// The bodies the emitter answered, closed under calls and placed as one translation unit.
pub const UNIT: &str = include_str!("../ply/unit.ply");

/// A program read from a directory, checked and emitted, as a program that drives another hands it.
pub const LOAD: &str = include_str!("../ply/load.ply");

/// The builder: what the launcher enters to make a program it ships out of that program's sources.
pub const BUILD: &str = include_str!("../ply/build.ply");

/// Laid out as the builder's sources; the emitter's identity digests them in this order.
pub const MODULES: &[(&str, &str)] = &[
    ("build", BUILD),
    ("code", CODE),
    ("costs", COSTS),
    ("derive", DERIVE),
    ("diagnostics", DIAGNOSTICS),
    ("embed", EMBED),
    ("emit", EMIT),
    ("exprs", EXPRS),
    ("fmt", FMT),
    ("front", FRONT),
    ("hash", HASH),
    ("infer", INFER),
    ("interface", INTERFACE),
    ("items", ITEMS),
    ("lexer", LEXER),
    ("load", LOAD),
    ("parsing", PARSING),
    ("patterns", PATTERNS),
    ("pkg", PKG),
    ("plyx", PLYX),
    ("prelude_source", PRELUDE_SOURCE),
    ("resolve", RESOLVE),
    ("rewrite", REWRITE),
    ("termination", TERMINATION),
    ("type_syntax", TYPE_SYNTAX),
    ("types", TYPES),
    ("unit", UNIT),
];

pub fn sources() -> impl Iterator<Item = (&'static str, &'static str)> {
    MODULES.iter().copied()
}

/// The builtins, declared: `prelude_source` embeds it from beside the package, so it is no module.
pub const PRELUDE: &str = include_str!("../prelude.ply");

/// The name [`PRELUDE`] is shelved under, beside [`MODULES`]: what lies beside a shipped package
/// is held under the package's root.
pub const PRELUDE_NAME: &str = "prelude";

/// The builder `ply bootstrap` last wrote for this compiler: the runnable the launcher enters.
pub mod bootstrap {
    pub const BUILDER: &[u8] = include_bytes!("../bootstrap/build.run");

    /// The digest of the shelf and runtime the committed builder was built for.
    pub const BUILDER_DIGEST: &str = include_str!("../bootstrap/build.digest");
}

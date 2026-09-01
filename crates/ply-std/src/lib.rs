//! The modules that ship with the compiler.

use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::ModuleName;
use std::path::{Path, PathBuf};

/// The reserved first segment.
pub const ROOT: &str = "std";

/// The pseudo-path prefix an embedded module's cache entries are keyed under.
pub const PSEUDO_ROOT: &str = "<std>";

/// The configuration effect, the schema a run checks itself against before it binds, and the twin a
/// test supplies values through.
pub const CONFIG: &str = include_str!("../ply/config.ply");

/// The database effect and the values that cross it.
pub const DB: &str = include_str!("../ply/db.ply");

/// The filesystem effect, rooted on a resource label, and the in-memory twin a test handles it over.
pub const FS: &str = include_str!("../ply/fs.ply");

/// BLAKE3, written in Ply, so the hash content addressing is defined by is a definition you can read.
pub const HASH: &str = include_str!("../ply/hash.ply");

/// The JSON value, its parser and serializer, and the primitives a `derive json for T` composes out
/// of.
pub const JSON: &str = include_str!("../ply/json.ply");

/// HTTP/1.1 framing: the head parser, the body decoder, the response encoder and the serve loop.
pub const HTTP: &str = include_str!("../ply/http.ply");

/// The network effect.
pub const NET: &str = include_str!("../ply/net.ply");

/// Routing: the table as ordinary data, the pure function that matches over it, and the two checks
/// a service asserts about the table's own shape.
pub const ROUTER: &str = include_str!("../ply/router.ply");

/// The observability effect, its values, and the collecting twin a test substitutes for a sink.
pub const TRACE: &str = include_str!("../ply/trace.ply");

/// The stop signal, and the `Stop` twin a test handles it over.
pub const SIGNAL: &str = include_str!("../ply/signal.ply");

/// The trusted list, read top to bottom — the same property that makes `ply-host`'s registry a
/// reviewable trusted computing base.
pub const MODULES: &[(&str, &str)] = &[
    ("std.config", CONFIG),
    ("std.db", DB),
    ("std.fs", FS),
    ("std.hash", HASH),
    ("std.http", HTTP),
    ("std.json", JSON),
    ("std.net", NET),
    ("std.router", ROUTER),
    ("std.signal", SIGNAL),
    ("std.trace", TRACE),
];

/// The source that ships for a module, or `None` if none does.
pub fn source(module: &ModuleName) -> Option<&'static str> {
    MODULES
        .iter()
        .find(|(name, _)| *name == module.as_str())
        .map(|(_, source)| *source)
}

pub fn modules() -> impl Iterator<Item = ModuleName> {
    MODULES
        .iter()
        .map(|(name, _)| ModuleName::from_dotted(name))
}

/// Every embedded module as `(program-wide name, source)`.
pub fn sources() -> impl Iterator<Item = (&'static str, &'static str)> {
    MODULES.iter().copied()
}

/// Whether a module name is under the reserved root.
pub fn is_std(module: &ModuleName) -> bool {
    is_reserved(module.as_str())
}

/// The same question for a name that is not a [`ModuleName`] yet — the check a path-derived name
/// has to pass before it can become one.
pub fn is_reserved(name: &str) -> bool {
    name == ROOT || name.starts_with(&format!("{ROOT}."))
}

/// Where an embedded module's fingerprint is filed.
pub fn pseudo_path(module: &ModuleName) -> PathBuf {
    let rest: Vec<&str> = module.segments().skip(1).collect();
    PathBuf::from(format!("{PSEUDO_ROOT}/{}.ply", rest.join("/")))
}

/// Whether a path is one [`pseudo_path`] produced, which is what tells a cache entry for an
/// embedded module apart from one for a file on disk.
pub fn is_pseudo_path(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|p| p.starts_with(&format!("{PSEUDO_ROOT}/")))
}

/// BLAKE3 over the canonical list of `(module name, hash of source bytes)`.
pub fn digest() -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for (name, source) in MODULES {
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update(blake3::hash(source.as_bytes()).as_bytes());
    }
    *hasher.finalize().as_bytes()
}

/// The form a CI check pins: `b3:` plus twelve hex characters, exactly as `ply hosts --digest`
/// prints.
pub fn digest_short() -> String {
    let bytes = digest();
    let mut out = String::from("b3:");
    for byte in &bytes[..6] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// A project file whose path derives a name under the reserved root.
pub fn reserved_diagnostic(path: &Path, name: &str) -> Diagnostic {
    Diagnostic::error(
        codes::RESERVED_MODULE_NAME,
        format!("`{}` would be the module `{name}`, and `std` is reserved", path.display()),
    )
    .primary(Span::DUMMY, "this file would shadow the modules that ship with the compiler")
    .note("`std` and everything under it name the modules embedded in `ply`; run `ply std` to list them")
    .note("rename the file or the directory it sits in")
}

/// An `import std.x` the table does not hold.
pub fn unknown_module(name: &ModuleName, span: Span) -> Diagnostic {
    let listed: Vec<String> = MODULES.iter().map(|(n, _)| format!("`{n}`")).collect();
    Diagnostic::error(
        codes::UNKNOWN_MODULE,
        format!("no module named `{name}` ships with this compiler"),
    )
    .primary(span, "not found")
    .note(format!("the stdlib holds: {}", listed.join(", ")))
    .note("`ply std` lists them with the digest this binary was built from")
}

/// A shipped module importing something this build does not ship — a module outside `std`, or a
/// `std.x` the table does not hold.
pub fn foreign_import(importer: &ModuleName, imported: &Symbol, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "the shipped module `{importer}` imports `{imported}`, which this build does not ship"
        ),
    )
    .primary(
        span,
        "a stdlib module may import only modules that ship with it",
    )
    .note("this is a defect in the compiler's own sources, not in this program")
    .note("please report it with the version of `ply` that produced it")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sorted and unique, because both [`digest`] and `ply std` present the table in its own order
    /// and a duplicate would make [`source`] answer with whichever entry came first.
    #[test]
    fn the_table_is_canonical() {
        let names: Vec<&str> = MODULES.iter().map(|(name, _)| *name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "the module table is not sorted, or repeats");
    }

    /// Every shipped module is under the reserved root, has a source that is not empty, and names
    /// itself the way an `import` would have to write it.
    #[test]
    fn every_shipped_module_is_addressable() {
        for (name, source) in MODULES {
            let module = ModuleName::from_dotted(name);
            assert!(is_std(&module), "`{name}` is not under `{ROOT}`");
            assert!(!source.is_empty(), "`{name}` ships no source");
            assert_eq!(super::source(&module), Some(*source));
            assert!(module.segments().count() >= 2, "`{name}` names no module");
        }
    }

    #[test]
    fn a_module_that_does_not_ship_has_no_source() {
        assert_eq!(source(&ModuleName::from_dotted("std.sql")), None);
        // The unqualified name is a project's to use, and never resolves here.
        assert_eq!(source(&ModuleName::from_dotted("net")), None);
        assert_eq!(source(&ModuleName::from_dotted("json")), None);
    }

    #[test]
    fn the_pseudo_path_is_slash_separated_and_outside_the_identifier_space() {
        assert_eq!(
            pseudo_path(&ModuleName::from_dotted("std.net")),
            PathBuf::from("<std>/net.ply")
        );
        assert_eq!(
            pseudo_path(&ModuleName::from_dotted("std.http.server")),
            PathBuf::from("<std>/http/server.ply")
        );
        assert!(is_pseudo_path(&pseudo_path(&ModuleName::from_dotted(
            "std.net"
        ))));
        assert!(!is_pseudo_path(Path::new("src/net.ply")));
    }

    #[test]
    fn the_reserved_root_covers_itself_and_everything_under_it() {
        assert!(is_reserved("std"));
        assert!(is_reserved("std.net"));
        assert!(is_reserved("std.a.b"));
        assert!(!is_reserved("stdlib"));
        assert!(!is_reserved("mine.std"));
        assert!(!is_reserved(""));
    }

    /// The digest is what a CI check pins, so it may not move between two calls in one build, and
    /// it has to cover the sources rather than only the names.
    #[test]
    fn the_digest_is_stable_and_covers_the_source_bytes() {
        assert_eq!(digest(), digest());
        let short = digest_short();
        assert!(short.starts_with("b3:"), "{short}");
        assert_eq!(short.len(), 15, "{short}");

        let mut hasher = blake3::Hasher::new();
        for (name, source) in MODULES {
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
            hasher.update(blake3::hash(source.as_bytes()).as_bytes());
        }
        assert_eq!(digest(), *hasher.finalize().as_bytes());

        // A source that moved by one byte moves the digest.
        let mut moved = blake3::Hasher::new();
        for (name, source) in MODULES {
            moved.update(&(name.len() as u64).to_le_bytes());
            moved.update(name.as_bytes());
            moved.update(blake3::hash(format!("{source}\n").as_bytes()).as_bytes());
        }
        assert_ne!(digest(), *moved.finalize().as_bytes());
    }

    /// The shipped sources are a program, so they have to parse.
    #[test]
    fn every_shipped_module_parses() {
        for (i, (name, source)) in MODULES.iter().enumerate() {
            let module = ModuleName::from_dotted(name);
            ply_syntax::parse_module(ply_span::SourceId(i as u32), module, source)
                .unwrap_or_else(|d| panic!("`{name}` does not parse: {d:?}"));
        }
    }

    /// A shipped module may import only `std.*`, and the check is here as well as in the loader so
    /// that a bad edit fails this crate's own suite.
    #[test]
    fn no_shipped_module_imports_outside_std() {
        for (i, (name, source)) in MODULES.iter().enumerate() {
            let module = ModuleName::from_dotted(name);
            let parsed = ply_syntax::parse_module(ply_span::SourceId(i as u32), module, source)
                .expect("it parses");
            for import in &parsed.imports {
                let imported = import.module_name();
                assert!(
                    is_std(&imported),
                    "`{name}` imports `{imported}`, which is not under `{ROOT}`"
                );
            }
        }
    }
}

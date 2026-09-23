//! The modules that ship with the compiler.

use ply_span::{Diagnostic, Span, codes};
use ply_ty::ModuleName;
use std::path::{Path, PathBuf};

/// The reserved first segment.
pub const ROOT: &str = "std";

/// The pseudo-path prefix an embedded module's cache entries are keyed under.
pub const PSEUDO_ROOT: &str = "<std>";

pub const BYTES: &str = include_str!("../ply/bytes.ply");

pub const CONFIG: &str = include_str!("../ply/config.ply");

pub const DB: &str = include_str!("../ply/db.ply");

pub const FS: &str = include_str!("../ply/fs.ply");

pub const HASH: &str = include_str!("../ply/hash.ply");

pub const JSON: &str = include_str!("../ply/json.ply");

pub const HTTP: &str = include_str!("../ply/http.ply");

pub const NET: &str = include_str!("../ply/net.ply");

pub const PATH: &str = include_str!("../ply/path.ply");

pub const PKG: &str = include_str!("../ply/pkg.ply");

pub const PROCESS: &str = include_str!("../ply/process.ply");

pub const ROUTER: &str = include_str!("../ply/router.ply");

pub const TRACE: &str = include_str!("../ply/trace.ply");

pub const SIGNAL: &str = include_str!("../ply/signal.ply");

pub const TIME: &str = include_str!("../ply/time.ply");

/// The trusted list, kept sorted and unique.
pub const MODULES: &[(&str, &str)] = &[
    ("std.bytes", BYTES),
    ("std.config", CONFIG),
    ("std.db", DB),
    ("std.fs", FS),
    ("std.hash", HASH),
    ("std.http", HTTP),
    ("std.json", JSON),
    ("std.net", NET),
    ("std.path", PATH),
    ("std.pkg", PKG),
    ("std.process", PROCESS),
    ("std.router", ROUTER),
    ("std.signal", SIGNAL),
    ("std.time", TIME),
    ("std.trace", TRACE),
];

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

pub fn sources() -> impl Iterator<Item = (&'static str, &'static str)> {
    MODULES.iter().copied()
}

pub fn is_std(module: &ModuleName) -> bool {
    is_reserved(module.as_str())
}

/// [`is_std`] for a name that is not a [`ModuleName`] yet.
pub fn is_reserved(name: &str) -> bool {
    name == ROOT || name.starts_with(&format!("{ROOT}."))
}

pub fn pseudo_path(module: &ModuleName) -> PathBuf {
    let rest: Vec<&str> = module.segments().skip(1).collect();
    PathBuf::from(format!("{PSEUDO_ROOT}/{}.ply", rest.join("/")))
}

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

/// `b3:` plus twelve hex characters, as `ply hosts --digest` prints.
pub fn digest_short() -> String {
    let bytes = digest();
    let mut out = String::from("b3:");
    for byte in &bytes[..6] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn reserved_diagnostic(path: &Path, name: &str) -> Diagnostic {
    Diagnostic::error(
        codes::RESERVED_MODULE_NAME,
        format!("`{}` would be the module `{name}`, and `std` is reserved", path.display()),
    )
    .primary(Span::DUMMY, "this file would shadow the modules that ship with the compiler")
    .note("`std` and everything under it name the modules embedded in `ply`; run `ply std` to list them")
    .note("rename the file or the directory it sits in")
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A duplicate would make [`source`] answer with whichever entry came first.
    #[test]
    fn the_table_is_canonical() {
        let names: Vec<&str> = MODULES.iter().map(|(name, _)| *name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "the module table is not sorted, or repeats");
    }

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

        let mut moved = blake3::Hasher::new();
        for (name, source) in MODULES {
            moved.update(&(name.len() as u64).to_le_bytes());
            moved.update(name.as_bytes());
            moved.update(blake3::hash(format!("{source}\n").as_bytes()).as_bytes());
        }
        assert_ne!(digest(), *moved.finalize().as_bytes());
    }
}

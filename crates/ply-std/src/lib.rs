//! The modules that ship with the compiler, by dotted name.

use std::path::{Path, PathBuf};

/// The reserved first segment.
pub const ROOT: &str = "std";

/// The pseudo-path prefix an embedded module's cache entries are keyed under.
pub const PSEUDO_ROOT: &str = "<std>";

pub const BASE64: &str = include_str!("../ply/base64.ply");

pub const BIGINT: &str = include_str!("../ply/bigint.ply");

pub const BIN: &str = include_str!("../ply/bin.ply");

pub const BYTES: &str = include_str!("../ply/bytes.ply");

pub const CERTGEN: &str = include_str!("../ply/certgen.ply");

pub const CONFIG: &str = include_str!("../ply/config.ply");

pub const CSV: &str = include_str!("../ply/csv.ply");

pub const DB: &str = include_str!("../ply/db.ply");

pub const DECIMAL: &str = include_str!("../ply/decimal.ply");

pub const FLOAT: &str = include_str!("../ply/float.ply");

pub const FS: &str = include_str!("../ply/fs.ply");

pub const HASH: &str = include_str!("../ply/hash.ply");

pub const JSON: &str = include_str!("../ply/json.ply");

pub const LIST: &str = include_str!("../ply/list.ply");

pub const MAP: &str = include_str!("../ply/map.ply");

pub const MATH: &str = include_str!("../ply/math.ply");

pub const MSGPACK: &str = include_str!("../ply/msgpack.ply");

pub const HTTP: &str = include_str!("../ply/http.ply");

pub const NET: &str = include_str!("../ply/net.ply");

pub const OPTION: &str = include_str!("../ply/option.ply");

pub const PARALLEL: &str = include_str!("../ply/parallel.ply");

pub const PARSE: &str = include_str!("../ply/parse.ply");

pub const PATH: &str = include_str!("../ply/path.ply");

pub const PG: &str = include_str!("../ply/pg.ply");

pub const PKG: &str = include_str!("../ply/pkg.ply");

pub const PROCESS: &str = include_str!("../ply/process.ply");

pub const RANDOM: &str = include_str!("../ply/random.ply");

pub const RESULT: &str = include_str!("../ply/result.ply");

pub const ROUTER: &str = include_str!("../ply/router.ply");

pub const SET: &str = include_str!("../ply/set.ply");
pub const SHOW: &str = include_str!("../ply/show.ply");

pub const TRACE: &str = include_str!("../ply/trace.ply");

pub const SIGNAL: &str = include_str!("../ply/signal.ply");

pub const STRING: &str = include_str!("../ply/string.ply");

pub const TIME: &str = include_str!("../ply/time.ply");

pub const URL: &str = include_str!("../ply/url.ply");

pub const UUID: &str = include_str!("../ply/uuid.ply");

pub const VALUE: &str = include_str!("../ply/value.ply");

/// The trusted list, kept sorted and unique.
pub const MODULES: &[(&str, &str)] = &[
    ("std.base64", BASE64),
    ("std.bigint", BIGINT),
    ("std.bin", BIN),
    ("std.bytes", BYTES),
    ("std.certgen", CERTGEN),
    ("std.config", CONFIG),
    ("std.csv", CSV),
    ("std.db", DB),
    ("std.decimal", DECIMAL),
    ("std.float", FLOAT),
    ("std.fs", FS),
    ("std.hash", HASH),
    ("std.http", HTTP),
    ("std.json", JSON),
    ("std.list", LIST),
    ("std.map", MAP),
    ("std.math", MATH),
    ("std.msgpack", MSGPACK),
    ("std.net", NET),
    ("std.option", OPTION),
    ("std.parallel", PARALLEL),
    ("std.parse", PARSE),
    ("std.path", PATH),
    ("std.pg", PG),
    ("std.pkg", PKG),
    ("std.process", PROCESS),
    ("std.random", RANDOM),
    ("std.result", RESULT),
    ("std.router", ROUTER),
    ("std.set", SET),
    ("std.show", SHOW),
    ("std.signal", SIGNAL),
    ("std.string", STRING),
    ("std.time", TIME),
    ("std.trace", TRACE),
    ("std.url", URL),
    ("std.uuid", UUID),
    ("std.value", VALUE),
];

pub fn source(name: &str) -> Option<&'static str> {
    MODULES
        .iter()
        .find(|(module, _)| *module == name)
        .map(|(_, source)| *source)
}

pub fn modules() -> impl Iterator<Item = &'static str> {
    MODULES.iter().map(|(name, _)| *name)
}

pub fn sources() -> impl Iterator<Item = (&'static str, &'static str)> {
    MODULES.iter().copied()
}

/// The reserved root itself, or a module under it.
pub fn is_std(name: &str) -> bool {
    name == ROOT || name.starts_with(&format!("{ROOT}."))
}

pub fn pseudo_path(name: &str) -> PathBuf {
    let rest: Vec<&str> = name.split('.').skip(1).collect();
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

/// `b3:` plus a digest's first twelve hex characters: every digest Ply prints for a person to
/// compare is written this way.
pub fn short_digest(digest: &[u8; 32]) -> String {
    let mut out = String::from("b3:");
    for byte in &digest[..6] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn digest_short() -> String {
    short_digest(&digest())
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
            assert!(is_std(name), "`{name}` is not under `{ROOT}`");
            assert!(!source.is_empty(), "`{name}` ships no source");
            assert_eq!(super::source(name), Some(*source));
            assert!(name.split('.').count() >= 2, "`{name}` names no module");
        }
    }

    #[test]
    fn a_module_that_does_not_ship_has_no_source() {
        assert_eq!(source("std.sql"), None);
        // The unqualified name is a project's to use, and never resolves here.
        assert_eq!(source("net"), None);
        assert_eq!(source("json"), None);
    }

    #[test]
    fn the_pseudo_path_is_slash_separated_and_outside_the_identifier_space() {
        assert_eq!(pseudo_path("std.net"), PathBuf::from("<std>/net.ply"));
        assert_eq!(
            pseudo_path("std.http.server"),
            PathBuf::from("<std>/http/server.ply")
        );
        assert!(is_pseudo_path(&pseudo_path("std.net")));
        assert!(!is_pseudo_path(Path::new("src/net.ply")));
    }

    #[test]
    fn the_reserved_root_covers_itself_and_everything_under_it() {
        assert!(is_std("std"));
        assert!(is_std("std.net"));
        assert!(is_std("std.a.b"));
        assert!(!is_std("stdlib"));
        assert!(!is_std("mine.std"));
        assert!(!is_std(""));
    }

    #[test]
    fn a_short_digest_is_its_prefix_and_six_bytes_of_hex() {
        assert_eq!(short_digest(&[0xab; 32]), "b3:abababababab");
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

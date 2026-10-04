//! The shipped modules: the standard library's, and the compiler's own under `compiler.<name>`, as
//! the pack carries them.
//!
//! A shipped module is resolved by its full dotted name like any other, is kept out of a program's
//! listings and closures, and cannot be shadowed by a file in a project.

use ply_eval::ModuleName;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The reserved first segments the standard library's and the compiler's modules answer to.
pub const STD_ROOT: &str = "std";
pub const COMPILER_ROOT: &str = "compiler";

/// The pseudo-path prefixes a shipped module's cache entries are keyed under.
const STD_PSEUDO_ROOT: &str = "<std>";
const COMPILER_PSEUDO_ROOT: &str = "<compiler>";

/// What lies beside the compiler's package is held under its root: the builtins it embeds.
const PRELUDE_NAME: &str = "prelude";

pub fn is_compiler(name: &str) -> bool {
    name == COMPILER_ROOT || name.starts_with("compiler.")
}

pub fn is_shipped(module: &ModuleName) -> bool {
    is_shipped_name(module.as_str())
}

/// [`is_shipped`] for a name that is not a [`ModuleName`] yet.
pub fn is_shipped_name(name: &str) -> bool {
    ply_eval::host::is_std(name) || is_compiler(name)
}

/// Every shipped module as `(name, text)`: the standard library's, the compiler's, then what the
/// compiler embeds from beside its package. Read once: the front end and the emitter must be
/// handed the same bytes, or they resolve the same module two ways.
pub fn sources() -> &'static [(String, String)] {
    static SOURCES: OnceLock<Vec<(String, String)>> = OnceLock::new();
    SOURCES.get_or_init(|| {
        let pack = ply_pack::installed();
        let named = |root: &'static str, dir: &'static str| {
            pack.files_in(dir).map(move |path| {
                let stem = path[dir.len() + 1..].trim_end_matches(".ply");
                (format!("{root}.{stem}"), text(path))
            })
        };
        named(STD_ROOT, ply_pack::STD)
            .chain(named(COMPILER_ROOT, ply_pack::COMPILER))
            .chain([(
                format!("{COMPILER_ROOT}.{PRELUDE_NAME}"),
                text(ply_pack::PRELUDE),
            )])
            .collect()
    })
}

fn text(path: &str) -> String {
    ply_pack::installed()
        .text(path)
        .unwrap_or_else(|| panic!("the pack lists `{path}` and carries it"))
        .to_string()
}

pub fn source(module: &ModuleName) -> Option<&'static str> {
    sources()
        .iter()
        .find(|(name, _)| name == module.as_str())
        .map(|(_, text)| text.as_str())
}

pub fn pseudo_path(module: &ModuleName) -> PathBuf {
    let (root, rest) = match module.as_str().strip_prefix("compiler.") {
        Some(rest) => (COMPILER_PSEUDO_ROOT, rest.replace('.', "/")),
        None => (
            STD_PSEUDO_ROOT,
            module
                .as_str()
                .split('.')
                .skip(1)
                .collect::<Vec<_>>()
                .join("/"),
        ),
    };
    PathBuf::from(format!("{root}/{rest}.ply"))
}

pub fn is_pseudo_path(path: &Path) -> bool {
    path.to_str().is_some_and(|p| {
        p.starts_with(&format!("{STD_PSEUDO_ROOT}/"))
            || p.starts_with(&format!("{COMPILER_PSEUDO_ROOT}/"))
    })
}

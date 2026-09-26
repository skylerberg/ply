//! The shelf the port pulls from: the built-in packages' modules, the standard library's
//! and the compiler's own under `compiler.<name>`.
//!
//! A shelved module is resolved by its full dotted name like any other, is kept out of a
//! program's listings and closures, and cannot be shadowed by a file in a project.

use ply_ty::ModuleName;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The reserved first segment the compiler's own modules answer to.
pub const COMPILER_ROOT: &str = "compiler";

/// The pseudo-path prefix a shelved compiler module's cache entries are keyed under.
const COMPILER_PSEUDO_ROOT: &str = "<compiler>";

pub fn is_compiler(name: &str) -> bool {
    name == COMPILER_ROOT || name.starts_with("compiler.")
}

/// A module the runtime carries, whichever shelf it sits on.
pub fn is_shipped(module: &ModuleName) -> bool {
    is_shipped_name(module.as_str())
}

/// [`is_shipped`] for a name that is not a [`ModuleName`] yet.
pub fn is_shipped_name(name: &str) -> bool {
    ply_std::is_reserved(name) || is_compiler(name)
}

/// The whole shelf the port pulls from, in the order the two tables hold it. Built once: the
/// front end and the emitter must be handed the same bytes, or they resolve the same module two
/// ways.
pub fn sources() -> &'static [(String, String)] {
    static SHELF: OnceLock<Vec<(String, String)>> = OnceLock::new();
    SHELF.get_or_init(|| {
        ply_std::sources()
            .map(|(name, text)| (name.to_string(), text.to_string()))
            .chain(
                ply_compiler::sources()
                    .map(|(name, text)| (format!("{COMPILER_ROOT}.{name}"), text.to_string())),
            )
            .collect()
    })
}

pub fn source(module: &ModuleName) -> Option<&'static str> {
    sources()
        .iter()
        .find(|(name, _)| name == module.as_str())
        .map(|(_, text)| text.as_str())
}

pub fn pseudo_path(module: &ModuleName) -> PathBuf {
    match module.as_str().strip_prefix("compiler.") {
        Some(rest) => PathBuf::from(format!(
            "{COMPILER_PSEUDO_ROOT}/{}.ply",
            rest.replace('.', "/")
        )),
        None => ply_std::pseudo_path(module),
    }
}

pub fn is_pseudo_path(path: &Path) -> bool {
    ply_std::is_pseudo_path(path)
        || path
            .to_str()
            .is_some_and(|p| p.starts_with(&format!("{COMPILER_PSEUDO_ROOT}/")))
}

/// The three store versions a decode refuses a mismatch of, as the runtime's store keeps them.
pub fn store_versions() -> (&'static str, &'static str, u32) {
    (
        ply_store::FRONTEND_VERSION,
        ply_store::RUNTIME_VERSION,
        ply_store::BODY_ENCODING,
    )
}

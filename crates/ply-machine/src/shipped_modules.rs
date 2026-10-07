//! The shipped modules: the standard library's, and the compiler's own under `compiler.<name>`, as
//! the pack carries them.
//!
//! A shipped module is resolved by its full dotted name like any other, is kept out of a program's
//! listings and closures, and cannot be shadowed by a file in a project.

use ply_eval::ModuleName;
use std::path::{Path, PathBuf};

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

/// Every shipped module's name: the standard library's, each file at or below its tree named by its
/// place there (`hash/legacy.ply` is `std.hash.legacy`), the compiler's, then what the compiler
/// embeds from beside its package. No text is read.
pub fn names() -> Vec<String> {
    let pack = ply_pack::installed();
    let named = |root: &'static str, dir: &'static str, path: &str| {
        let stem = path[dir.len() + 1..].strip_suffix(".ply")?;
        Some(format!("{root}.{}", stem.replace('/', ".")))
    };
    pack.modules_below(ply_pack::STD)
        .filter_map(|path| named(STD_ROOT, ply_pack::STD, path))
        .chain(
            pack.files_in(ply_pack::COMPILER)
                .filter_map(|path| named(COMPILER_ROOT, ply_pack::COMPILER, path)),
        )
        .chain([format!("{COMPILER_ROOT}.{PRELUDE_NAME}")])
        .collect()
}

/// Whether `module` is one this binary ships, read off the listing.
pub fn ships(module: &ModuleName) -> bool {
    names().iter().any(|name| name == module.as_str())
}

/// Every shipped module as `(name, text)`, in [`names`]'s order. The pack reads each file once, so
/// the front end and the emitter are handed the same bytes.
pub fn sources() -> Vec<(String, &'static str)> {
    names()
        .into_iter()
        .filter_map(|name| {
            let text = source(&ModuleName::from_dotted(&name))?;
            Some((name, text))
        })
        .collect()
}

/// Where the pack carries a shipped module.
fn path_of(module: &str) -> Option<String> {
    if module == format!("{COMPILER_ROOT}.{PRELUDE_NAME}") {
        return Some(ply_pack::PRELUDE.to_string());
    }
    let (dir, stem) = match module.split_once('.')? {
        (STD_ROOT, stem) => (ply_pack::STD, stem),
        (COMPILER_ROOT, stem) => (ply_pack::COMPILER, stem),
        _ => return None,
    };
    Some(format!("{dir}/{}.ply", stem.replace('.', "/")))
}

pub fn source(module: &ModuleName) -> Option<&'static str> {
    let path = path_of(module.as_str())?;
    let text = ply_pack::installed().bytes(&path)?;
    Some(
        std::str::from_utf8(text)
            .unwrap_or_else(|e| panic!("`{path}` in the pack is not UTF-8: {e}")),
    )
}

/// The name a data file a standard-library module embeds is asked for under: its place below the
/// library's directory, after the library's root.
fn data_name(path: &str) -> String {
    format!("{STD_ROOT}{}", &path[ply_pack::STD.len()..])
}

/// Where the pack would carry the data file `name`: no module's path is one.
fn data_path_of(name: &str) -> Option<String> {
    let below = name.strip_prefix(STD_ROOT)?.strip_prefix('/')?;
    (!below.ends_with(".ply")).then(|| format!("{}/{below}", ply_pack::STD))
}

/// Every data file the shipped modules embed, by name, ascending. No bytes are read.
pub fn data_names() -> Vec<String> {
    ply_pack::installed()
        .data_below(ply_pack::STD)
        .map(data_name)
        .collect()
}

pub fn data(name: &str) -> Option<&'static [u8]> {
    ply_pack::installed().bytes(&data_path_of(name)?)
}

/// What `name` is of the shipped modules, as a trace holds it: a module's text or a data file's
/// bytes, digested.
pub fn digest_of(name: &str) -> Option<String> {
    let path = data_path_of(name).or_else(|| path_of(name))?;
    let digest = ply_pack::installed().digest_of(&path)?;
    Some(blake3::Hash::from(digest).to_hex().to_string())
}

/// What everything shipped is: every module's text, then each data file by its name and the digest
/// the pack holds of its bytes, so no data file is read for it.
pub fn digest() -> String {
    let pack = ply_pack::installed();
    let mut hasher = blake3::Hasher::new();
    hasher.update(crate::builds::digest_of(&sources()).as_bytes());
    for path in pack.data_below(ply_pack::STD) {
        hasher.update(&[0]);
        hasher.update(data_name(path).as_bytes());
        hasher.update(&[0]);
        hasher.update(&pack.digest_of(path).expect("a listed path is carried"));
    }
    hasher.finalize().to_hex()[..16].to_string()
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

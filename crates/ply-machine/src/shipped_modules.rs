//! The shipped modules: the standard library's, and the compiler's own under `compiler.<name>`, as
//! the pack carries them.
//!
//! A shipped module is resolved by its full dotted name like any other, is kept out of a program's
//! listings and closures, and cannot be shadowed by a file in a project.

use ply_eval::ModuleName;
use std::path::PathBuf;

/// The reserved first segments the standard library's and the compiler's modules answer to.
pub const STD_ROOT: &str = "std";
pub const COMPILER_ROOT: &str = "compiler";

/// The pseudo-path prefixes a shipped module's cache entries are keyed under.
const STD_PSEUDO_ROOT: &str = "<std>";
const COMPILER_PSEUDO_ROOT: &str = "<compiler>";

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

/// Every shipped module's name: the standard library's, then the compiler's, each file at or below
/// its tree named by its place there (`hash/legacy.ply` is `std.hash.legacy`, `emit/roots.ply`
/// `compiler.emit.roots`). No text is read.
pub fn names() -> Vec<String> {
    let pack = ply_pack::installed();
    let named = |root: &'static str, dir: &'static str, path: &str| {
        let stem = path[dir.len() + 1..].strip_suffix(".ply")?;
        Some(format!("{root}.{}", stem.replace('/', ".")))
    };
    pack.modules_below(ply_pack::STD)
        .filter_map(|path| named(STD_ROOT, ply_pack::STD, path))
        .chain(
            pack.modules_below(ply_pack::COMPILER)
                .filter_map(|path| named(COMPILER_ROOT, ply_pack::COMPILER, path)),
        )
        .collect()
}

/// Whether `module` is one this binary ships, read off the listing.
pub fn ships(module: &ModuleName) -> bool {
    names().iter().any(|name| name == module.as_str())
}

/// [`names`] as a set, for a caller asking of many modules.
pub fn name_set() -> std::collections::HashSet<String> {
    names().into_iter().collect()
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

/// Each shipped file by the name the launcher lays it out under, `<module>.ply` or a data file's
/// name, a space, and its bytes' BLAKE3 digest in hex: a line each, ascending by name. What the
/// shipped modules' answers were made over, and what a run holds them to. No bytes are read.
pub fn digests() -> String {
    let mut lines: Vec<String> = names()
        .into_iter()
        .filter_map(|name| Some(format!("{name}.ply {}\n", digest_of(&name)?)))
        .chain(
            data_names()
                .into_iter()
                .filter_map(|name| Some(format!("{name} {}\n", digest_of(&name)?))),
        )
        .collect();
    lines.sort();
    lines.concat()
}

/// Every shipped module and data file, digested: it moves with any of them. No bytes are read.
pub fn digest() -> ply_pack::Digest {
    ply_pack::installed().digest_below(&[ply_pack::STD, ply_pack::COMPILER])
}

/// What `name` is of the shipped modules, as a trace holds it: a module's text or a data file's
/// bytes, digested.
pub fn digest_of(name: &str) -> Option<String> {
    let path = data_path_of(name).or_else(|| path_of(name))?;
    let digest = ply_pack::installed().digest_of(&path)?;
    Some(blake3::Hash::from(digest).to_hex().to_string())
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

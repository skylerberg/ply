//! The shelf the port pulls from: the standard library, and the compiler's own modules shelved
//! under `compiler.<name>`.
//!
//! A shelved module is resolved by its full dotted name like any other, is kept out of a
//! program's listings and closures, and cannot be shadowed by a file in a project.

use ply_span::{Diagnostic, Span, codes};
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
                    .map(|(name, text)| (format!("{COMPILER_ROOT}.{name}"), shelved(text))),
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

/// The compiler names its siblings bare; on the shelf they answer to `compiler.<name>`, so each
/// import of one is rewritten here. It has to be the text: the front end parses it to resolve the
/// import and the emitter parses it again to name the call, and a rename either one makes on its
/// own is a rename the other never sees.
fn shelved(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 512);
    for line in text.split_inclusive('\n') {
        match line.strip_prefix("import ") {
            Some(rest) if is_compiler_module(head_segment(rest)) => {
                out.push_str("import ");
                out.push_str(COMPILER_ROOT);
                out.push('.');
                out.push_str(rest);
            }
            _ => out.push_str(line),
        }
    }
    out
}

/// The first dotted segment of the module path an import line opens with.
fn head_segment(rest: &str) -> &str {
    let path = rest
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .next()
        .unwrap_or("");
    path.split('.').next().unwrap_or("")
}

fn is_compiler_module(name: &str) -> bool {
    ply_compiler::MODULES
        .iter()
        .any(|(module, _)| *module == name)
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

/// A project file whose path would name a shelved module, which nothing may shadow.
pub fn reserved_diagnostic(file: &Path, name: &str) -> Diagnostic {
    if ply_std::is_reserved(name) {
        return ply_std::reserved_diagnostic(file, name);
    }
    Diagnostic::error(
        codes::RESERVED_MODULE_NAME,
        format!(
            "`{}` would be the module `{name}`, and `{COMPILER_ROOT}` is reserved",
            file.display()
        ),
    )
    .primary(
        Span::DUMMY,
        "this file would shadow the compiler's own modules",
    )
    .note("`compiler` and everything under it name the compiler modules embedded in `ply`; `compiler.fmt` is the formatter `ply fmt` runs")
    .note("rename the file or the directory it sits in")
}

/// The three store versions a decode refuses a mismatch of, as the runtime's store keeps them.
pub fn store_versions() -> (&'static str, &'static str, u32) {
    (
        ply_store::FRONTEND_VERSION,
        ply_store::RUNTIME_VERSION,
        ply_store::BODY_ENCODING,
    )
}

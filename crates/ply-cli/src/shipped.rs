//! What this binary ships: the standard library, the compiler's own modules shelved under
//! `compiler.<name>`, and the `ply` program built from `crates/ply-cli/ply`.
//!
//! A shelved module is resolved by its full dotted name like any other, is kept out of a
//! program's listings and closures, and cannot be shadowed by a file in a project.

use ply_codegen::c::{bundle, producer};
use ply_span::{Diagnostic, Span, codes};
use ply_ty::ModuleName;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The reserved first segment the compiler's own modules answer to.
pub const COMPILER_ROOT: &str = "compiler";

/// The pseudo-path prefix a shelved compiler module's cache entries are keyed under.
const COMPILER_PSEUDO_ROOT: &str = "<compiler>";

/// The program `ply` runs, one module per file of `crates/ply-cli/ply`, each named by its stem.
/// The entry point is `ply.main`, which dispatches on the command word.
pub const PROGRAM_SOURCES: &[(&str, &str)] = &[
    ("args", include_str!("../ply/args.ply")),
    ("defs", include_str!("../ply/defs.ply")),
    ("diagnostic", include_str!("../ply/diagnostic.ply")),
    ("doc", include_str!("../ply/doc.ply")),
    ("explain", include_str!("../ply/explain.ply")),
    ("fmt", include_str!("../ply/fmt.ply")),
    ("hashes", include_str!("../ply/hashes.ply")),
    ("hosts", include_str!("../ply/hosts.ply")),
    ("ply", include_str!("../ply/ply.ply")),
    ("program", include_str!("../ply/program.ply")),
    ("report", include_str!("../ply/report.ply")),
    ("sources", include_str!("../ply/sources.ply")),
    ("style", include_str!("../ply/style.ply")),
    ("walk", include_str!("../ply/walk.ply")),
];

/// Where the built program and the digest of the sources it was built from are committed.
pub const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/bootstrap");

pub const ARTIFACT: &str = "ply.plyx";

pub const DIGEST: &str = "ply.digest";

/// The marker that says a shelf directory is whole; landed last, so a reader never sees half of
/// one. Not a `.ply` file, so the program's own listing passes over it.
const SHELF_MARKER: &str = "SHELF.ok";

pub fn is_compiler(name: &str) -> bool {
    name == COMPILER_ROOT || name.starts_with("compiler.")
}

/// A module this binary carries, whichever shelf it sits on.
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
        format!("`{}` would be the module `{name}`, and `{COMPILER_ROOT}` is reserved", file.display()),
    )
    .primary(
        Span::DUMMY,
        "this file would shadow the compiler's own modules",
    )
    .note("`compiler` and everything under it name the compiler modules embedded in `ply`; `compiler.fmt` is the formatter `ply fmt` runs")
    .note("rename the file or the directory it sits in")
}

// --- The `ply` program -------------------------------------------------------

/// The program's modules as the port takes them: `(name, text)`, which `digest_of` sorts.
pub fn program_sources() -> Vec<(String, String)> {
    PROGRAM_SOURCES
        .iter()
        .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
        .collect()
}

/// What the built program is a function of: its sources, the shelf it is closed over as the shelf
/// hands it out, the emitter that compiled it, and the three store versions a decode refuses a
/// mismatch of.
pub fn identity() -> String {
    let program = producer::digest_of(&program_sources());
    let mut hasher = blake3::Hasher::new();
    for part in [
        program.as_str(),
        producer::digest_of(sources()).as_str(),
        producer::identity().as_str(),
        ply_store::FRONTEND_VERSION,
        ply_store::RUNTIME_VERSION,
    ] {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    hasher.update(&ply_store::BODY_ENCODING.to_le_bytes());
    hasher.finalize().to_hex()[..16].to_string()
}

/// Where a program built for `identity` is kept between runs, beside the emitter's own stages. The
/// `Front` an opened artifact answers with is kept here too, since it is a function of the same
/// sources.
pub fn stage() -> PathBuf {
    bundle::stage_dir(&format!("cli-{}", identity()))
}

/// Where the modules this binary ships are laid out for the program to read, one flat
/// `<dotted name>.ply` each. A program cannot read the binary it runs in, and the front end it
/// runs pulls in the shipped modules a project imports, so they have to be somewhere it can reach.
pub fn shelf_dir() -> PathBuf {
    stage().join("shelf")
}

/// The shelf directory, laid out once per identity. Each file lands by a rename and the marker
/// lands last, so a run that finds the marker finds every module whole.
pub fn shelf() -> Result<PathBuf, Diagnostic> {
    let dir = shelf_dir();
    if dir.join(SHELF_MARKER).exists() {
        return Ok(dir);
    }
    match lay_out(&dir) {
        Ok(()) => Ok(dir),
        Err(e) => Err(unbuilt(format!(
            "the shipped modules could not be placed in `{}`: {e}",
            dir.display()
        ))),
    }
}

fn lay_out(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, text) in sources() {
        land_in(dir, &format!("{name}.ply"), text.as_bytes())?;
    }
    land_in(dir, SHELF_MARKER, identity().as_bytes())
}

fn land_in(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dir.join(name))
}

/// The digest the committed program was built from, when one is committed at all.
pub fn committed_digest() -> Option<String> {
    std::fs::read_to_string(Path::new(DIR).join(DIGEST))
        .ok()
        .map(|text| text.trim().to_string())
}

pub fn committed() -> PathBuf {
    Path::new(DIR).join(ARTIFACT)
}

/// The `ply` program: the committed artifact when it was built from these very sources, else the
/// stage an earlier run kept for them, else one built now and kept there. A binary whose committed
/// artifact is behind its sources therefore runs the sources, never the artifact.
pub fn program() -> Result<Vec<u8>, Diagnostic> {
    let identity = identity();
    if committed_digest().as_deref() == Some(identity.as_str())
        && let Ok(bytes) = std::fs::read(committed())
    {
        return Ok(bytes);
    }
    let staged = stage().join(ARTIFACT);
    if let Ok(bytes) = std::fs::read(&staged) {
        return Ok(bytes);
    }
    let bytes = build()?;
    land(&staged, &bytes);
    Ok(bytes)
}

/// The program built from its sources here and now, as `ply build` would build it: written into a
/// directory of its own, loaded whole, and closed over its one `main`.
pub fn build() -> Result<Vec<u8>, Diagnostic> {
    let dir = stage().join(format!("src.{}", std::process::id()));
    if let Err(e) = write_sources(&dir) {
        return Err(unbuilt(format!(
            "its sources could not be placed in `{}`: {e}",
            dir.display()
        )));
    }
    let built = build_in(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    built
}

fn write_sources(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, text) in PROGRAM_SOURCES {
        std::fs::write(dir.join(format!("{name}.ply")), text)?;
    }
    Ok(())
}

fn build_in(dir: &Path) -> Result<Vec<u8>, Diagnostic> {
    let loaded = crate::load::load(dir).map_err(|err| {
        // Rendered where it happened: this program is only ever built from sources in the tree,
        // so a refusal is a defect someone has to find, not a user's mistake to summarise.
        unbuilt(match err.diagnostics.first() {
            Some(d) => format!(
                "it does not check:\n{}",
                ply_span::render::to_terminal(d, &err.sources, false)
            ),
            None => "it does not check, and nothing said why".to_string(),
        })
    })?;
    let entry = crate::commands::run::entry_point(&loaded)
        .map_err(|d| unbuilt(format!("{} [{}]", d.message, d.code)))?;
    let built = crate::artifact::build(&loaded, entry, &[]).map_err(|diagnostics| {
        unbuilt(match diagnostics.first() {
            Some(d) => format!("{} [{}]", d.message, d.code),
            None => "nothing said why".to_string(),
        })
    })?;
    // `ply` enters the artifact's own unit and nothing else, so an artifact whose unit holds no
    // body for `main` cannot run. It is not landed: the failure belongs to the build, where the
    // emitter's reasons are still in hand, not to the next run, which would have none.
    if !built.entry_compiled {
        return Err(refused_entry(&built));
    }
    Ok(built.artifact.encode())
}

#[cold]
fn refused_entry(built: &crate::artifact::Built) -> Diagnostic {
    let why = if built.artifact.has_unit() {
        format!(
            "its compiled unit holds no body for `{}`, so nothing could be entered",
            built.entry_name
        )
    } else {
        "no compiled unit could be produced for it at all".to_string()
    };
    // The production's own account, which is the only thing that says why there is no unit; a
    // reader left without it can do nothing but guess at which half of the build gave way.
    let diagnostic = built
        .warnings
        .iter()
        .fold(unbuilt(why), |d, w| d.note(w.message.clone()));
    if built.refused.is_empty() {
        return diagnostic
            .note("the emitter refused nothing, so the entry was never offered to it");
    }
    diagnostic.note(crate::artifact::refusal_list(&built.refused))
}

/// By a rename, so a reader never sees half of one.
fn land(at: &Path, bytes: &[u8]) {
    let Some(parent) = at.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let tmp = parent.join(format!("{ARTIFACT}.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, at).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cold]
fn unbuilt(why: String) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the `ply` program could not be built: {why}"),
    )
    .primary(Span::DUMMY, "this is Ply's fault, not the program's")
    .note("the program is `crates/ply-cli/ply`, built from the compiler this binary ships")
}

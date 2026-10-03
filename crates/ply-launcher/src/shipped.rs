//! What this binary ships: the `ply` program built from `crates/ply-cli/ply`.

use ply_codegen::c::{bundle, producer};
use ply_eval::{Diagnostic, Span, codes};
use ply_machine::load::Loaded;
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/program_sources.rs"));

/// Where the built program and the digest of the sources it was built from are committed.
pub const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ply-cli/bootstrap");

pub const ARTIFACT: &str = "ply.plyx";

pub const DIGEST: &str = "ply.digest";

// --- The `ply` program -------------------------------------------------------

/// Where the CLI package sits inside a stage that carries its closure: the repository's own path,
/// because the keys below are the repository's own paths.
pub const ROOT: &str = "crates/ply-cli/ply";

/// The whole closure as the port takes it: `(path, text)`, which `digest_of` sorts. A package's own
/// modules and its manifest are keyed by the path they have in the repository, so nothing about the
/// program is a function of where this binary happens to be.
pub fn program_sources() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = PROGRAM_PACKAGES
        .iter()
        .map(|(dir, name, text)| (format!("{dir}/{name}"), (*text).to_string()))
        .collect();
    out.extend(
        PROGRAM_PACKAGE_MANIFESTS
            .iter()
            .map(|(dir, text)| (format!("{dir}/ply.pkg"), (*text).to_string())),
    );
    out
}

/// Lays the shipped closure out under `stage`, keyed by the repository's paths, so a package's
/// `Path(..)` dependency resolves the way it does in a checkout.
pub fn lay_out(stage: &Path) -> std::io::Result<()> {
    for (dir, name, text) in PROGRAM_PACKAGES {
        let package = stage.join(dir);
        std::fs::create_dir_all(&package)?;
        std::fs::write(package.join(format!("{name}.ply")), text)?;
    }
    for (dir, text) in PROGRAM_PACKAGE_MANIFESTS {
        std::fs::write(stage.join(dir).join("ply.pkg"), text)?;
    }
    Ok(())
}

/// What the built program is a function of: its sources, the shelf it is closed over as the shelf
/// hands it out, and the compiler and runtime a decode refuses a mismatch of. The artifact the
/// program is built into carries the same digest as its stamp.
pub fn identity() -> String {
    static IDENTITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            ply_machine::artifact::toolchain_stamp(&producer::digest_of(&program_sources()))
        })
        .clone()
}

/// Where what `identity` names is kept between runs, beside the emitter's own stages: the front an
/// opened artifact answers with, and the load of the sources when no committed artifact serves.
pub fn stage() -> PathBuf {
    bundle::stage_dir(&format!("cli-{}", identity()))
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

/// What the `ply` program is entered from.
pub enum Image {
    /// The committed artifact, which these very sources built.
    Committed(Vec<u8>),
    /// The sources' own load: this process's, or the one an earlier process kept for them.
    Loaded(Box<Loaded>),
}

/// The `ply` program: the committed artifact when it was built from these very sources, else the
/// load of them an earlier run kept, else one made now and kept. A binary whose committed artifact
/// is behind its sources therefore runs the sources, never the artifact.
pub fn program() -> Result<Image, Diagnostic> {
    if committed_digest().as_deref() == Some(identity().as_str())
        && let Ok(bytes) = std::fs::read(committed())
    {
        return Ok(Image::Committed(bytes));
    }
    if let Ok(bytes) = std::fs::read(kept_front())
        && let Some(handed) = ply_machine::driver::kept_front(&bytes)
        && let Ok(loaded) = ply_machine::driver::load_over_front_taken(PathBuf::from(ROOT), handed)
    {
        ply_codegen::c::sweep::used(&stage());
        return Ok(Image::Loaded(Box::new(loaded)));
    }
    Ok(Image::Loaded(Box::new(load_in(&laid_out()?.join(ROOT))?)))
}

/// The sources laid out once under the stage, so the places a kept load names stay on disk with
/// it. Laid out aside and renamed in whole: a directory that is there is complete.
fn laid_out() -> Result<PathBuf, Diagnostic> {
    let at = stage().join("src");
    if at.is_dir() {
        return Ok(at);
    }
    let aside = stage().join(format!("src.{}", std::process::id()));
    lay_out(&aside).map_err(|e| {
        unbuilt(format!(
            "its sources could not be placed in `{}`: {e}",
            aside.display()
        ))
    })?;
    // Another process that laid them out first wins, and its copy is the same.
    if std::fs::rename(&aside, &at).is_err() {
        let _ = std::fs::remove_dir_all(&aside);
    }
    Ok(at)
}

/// Where the load of these sources is kept: under their identity and the runtime that answered it,
/// which the identity names only by its helper table.
fn kept_front() -> PathBuf {
    stage().join(format!("front-{}", &ply_codegen::c::runtime_digest()[..16]))
}

/// What the launcher's rows are kept under, beside the stages.
const ROWS: &str = "cli";

/// The sources at `dir` loaded, seeded with the rows the last load kept, and what the next process
/// takes in its place kept: the rows this load published and its front.
fn load_in(dir: &Path) -> Result<Loaded, Diagnostic> {
    let seeded = ply_machine::load::load_seeded(dir, producer::kept_rows(ROWS)).map_err(|err| {
        // With where it happened: this program is only ever built from sources in the tree, so
        // a refusal is a defect someone has to find, not a user's mistake to summarise.
        unbuilt(match err.diagnostics.first() {
            Some(d) => format!("it does not check:\n{}", d.clone().placed(&err.sources)),
            None => "it does not check, and nothing said why".to_string(),
        })
    })?;
    seeded
        .loaded
        .sole_entry_point()
        .map_err(|d| unbuilt(format!("{} [{}]", d.message, d.code)))?;
    producer::keep_rows(ROWS, &seeded.rows);
    if let Some(front) = &seeded.front {
        land(&kept_front(), front);
    }
    Ok(seeded.loaded)
}

fn land(at: &Path, bytes: &[u8]) {
    let Some(parent) = at.parent() else { return };
    if std::fs::create_dir_all(parent).is_ok() {
        let _ = ply_eval::files::write_atomically(at, bytes);
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

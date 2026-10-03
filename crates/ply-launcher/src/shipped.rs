//! What this binary ships: the `ply` program built from `crates/ply-cli/ply`.

use ply_codegen::c::stage;
use ply_eval::{Diagnostic, Span, codes};
use ply_machine::runnable::{self, Runnable};
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/program_sources.rs"));

/// Where the built program and the digest of the sources it was built from are committed.
pub const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ply-cli/bootstrap");

pub const RUNNABLE: &str = "ply.run";

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
/// hands it out, the compiler that builds it among them, and the runtime its unit is compiled
/// against. `ply bootstrap` writes it beside the runnable it builds.
pub fn identity() -> String {
    static IDENTITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"ply program 1\0");
            for part in [
                ply_machine::builds::digest_of(&program_sources()),
                ply_machine::builds::digest_of(ply_machine::shelf::sources()),
                ply_codegen::c::runtime_digest().to_string(),
            ] {
                hasher.update(part.as_bytes());
                hasher.update(&[0]);
            }
            hasher.finalize().to_hex()[..16].to_string()
        })
        .clone()
}

/// Where what `identity` names is kept between runs, beside the emitter's own stages: the sources
/// laid out, and the runnable the builder made of them when no committed one serves.
pub fn stage() -> PathBuf {
    stage::stage_dir(&format!("cli-{}", identity()))
}

/// The digest the committed program was built from, when one is committed at all.
pub fn committed_digest() -> Option<String> {
    std::fs::read_to_string(Path::new(DIR).join(DIGEST))
        .ok()
        .map(|text| text.trim().to_string())
}

pub fn committed() -> PathBuf {
    Path::new(DIR).join(RUNNABLE)
}

/// The `ply` program: the committed runnable when it was built from these very sources, else the
/// one the builder made of them for an earlier process, else one it makes now. A binary whose
/// committed runnable is behind its sources therefore runs the sources, never the runnable.
pub fn program() -> Result<Runnable, Diagnostic> {
    if committed_digest().as_deref() == Some(identity().as_str())
        && let Ok(bytes) = std::fs::read(committed())
        && let Ok(program) = runnable::decode(&bytes)
    {
        return Ok(program);
    }
    let staged = stage().join(RUNNABLE);
    if let Ok(bytes) = std::fs::read(&staged)
        && let Ok(program) = runnable::decode(&bytes)
    {
        ply_codegen::c::sweep::used(&stage());
        return Ok(program);
    }
    ply_machine::builds::build(&laid_out()?, ROOT, "ply.main", &staged, "cli")?;
    let started = std::time::Instant::now();
    let bytes = std::fs::read(&staged)
        .map_err(|e| unbuilt(format!("what the builder made could not be read: {e}")))?;
    let program = runnable::decode(&bytes)
        .map_err(|why| unbuilt(format!("what the builder made does not read: {why}")))?;
    if std::env::var_os("PLY_C_PHASES").is_some() {
        eprintln!(
            "phases: ply.run decoded {}ms",
            started.elapsed().as_millis()
        );
    }
    Ok(program)
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

#[cold]
fn unbuilt(why: String) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the `ply` program could not be built: {why}"),
    )
    .primary(Span::DUMMY, "this is Ply's fault, not the program's")
    .note("the program is `crates/ply-cli/ply`, built from the compiler this binary ships")
}

//! What this binary ships: the `ply` program built from `crates/ply-cli/ply`.

use ply_codegen::c::{bundle, producer};
use ply_eval::{Diagnostic, Span, codes};
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
/// hands it out, the emitter that compiled it, and the three store versions a decode refuses a
/// mismatch of. The artifact the program is built into carries the same digest as its stamp.
pub fn identity() -> String {
    ply_machine::artifact::toolchain_stamp(&producer::digest_of(&program_sources()))
}

/// Where a program built for `identity` is kept between runs, beside the emitter's own stages. The
/// `Front` an opened artifact answers with is kept here too, since it is a function of the same
/// sources.
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

/// The `ply` program: the committed artifact when it was built from these very sources, else the
/// stage an earlier run kept for them, else one built now and kept there. A binary whose committed
/// artifact is behind its sources therefore runs the sources, never the artifact.
pub fn program() -> Result<Vec<u8>, Diagnostic> {
    if committed_digest().as_deref() == Some(identity().as_str())
        && let Ok(bytes) = std::fs::read(committed())
    {
        return Ok(bytes);
    }
    let staged = stage().join(ARTIFACT);
    if let Ok(bytes) = std::fs::read(&staged) {
        ply_codegen::c::sweep::used(&stage());
        return Ok(bytes);
    }
    let bytes = build()?;
    land(&staged, &bytes);
    Ok(bytes)
}

/// The program built from its sources here and now, as `ply build` would build it: written into a
/// directory of its own, loaded whole, and closed over its one `main`.
pub fn build() -> Result<Vec<u8>, Diagnostic> {
    let stage = stage().join(format!("src.{}", std::process::id()));
    if let Err(e) = lay_out(&stage) {
        return Err(unbuilt(format!(
            "its sources could not be placed in `{}`: {e}",
            stage.display()
        )));
    }
    let built = build_in(&stage.join(ROOT));
    let _ = std::fs::remove_dir_all(&stage);
    built
}

fn build_in(dir: &Path) -> Result<Vec<u8>, Diagnostic> {
    let loaded = ply_machine::load::load(dir).map_err(|err| {
        // With where it happened: this program is only ever built from sources in the tree, so a
        // refusal is a defect someone has to find, not a user's mistake to summarise.
        unbuilt(match err.diagnostics.first() {
            Some(d) => format!("it does not check:\n{}", d.clone().placed(&err.sources)),
            None => "it does not check, and nothing said why".to_string(),
        })
    })?;
    let entry = loaded
        .sole_entry_point()
        .map_err(|d| unbuilt(format!("{} [{}]", d.message, d.code)))?;
    // With its notes: a refusal here states the symptom and carries the reason in a note, so
    // dropping them leaves a reader the one thing that cannot be acted on.
    let built =
        crate::artifact::build(&loaded, entry, &[]).map_err(|diagnostics| {
            match diagnostics.first() {
                Some(d) => d.notes.iter().fold(
                    unbuilt(format!("{} [{}]", d.message, d.code)),
                    |out, note| out.note(note.clone()),
                ),
                None => unbuilt("nothing said why".to_string()),
            }
        })?;
    // `ply` enters the artifact's own unit and nothing else, so an artifact whose unit holds no
    // body for `main` cannot run. It is not landed: the failure belongs to the build, where the
    // emitter's reasons are still in hand, not to the next run, which would have none.
    if !built.entry_compiled {
        return Err(refused_entry(&built));
    }
    built.artifact.encode()
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

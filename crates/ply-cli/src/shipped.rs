//! What this binary ships: the `ply` program built from `crates/ply-cli/ply`, and the shelf the
//! runtime's `ply_machine::shelf` holds for it laid out where the program can read it.

use ply_codegen::c::{bundle, producer};
use ply_span::{Diagnostic, Span, codes};
use std::path::{Path, PathBuf};

/// The program `ply` runs, one module per file of `crates/ply-cli/ply`, each named by its stem.
/// The entry point is `ply.main`, which dispatches on the command word.
pub const PROGRAM_SOURCES: &[(&str, &str)] = &[
    ("appends", include_str!("../ply/appends.ply")),
    ("args", include_str!("../ply/args.ply")),
    ("bootstrap", include_str!("../ply/bootstrap.ply")),
    ("build", include_str!("../ply/build.ply")),
    ("cache", include_str!("../ply/cache.ply")),
    ("callers", include_str!("../ply/callers.ply")),
    ("check", include_str!("../ply/check.ply")),
    ("claims", include_str!("../ply/claims.ply")),
    ("cmdline", include_str!("../ply/cmdline.ply")),
    ("defs", include_str!("../ply/defs.ply")),
    ("diagnostic", include_str!("../ply/diagnostic.ply")),
    ("doc", include_str!("../ply/doc.ply")),
    ("entry", include_str!("../ply/entry.ply")),
    ("explain", include_str!("../ply/explain.ply")),
    ("fmt", include_str!("../ply/fmt.ply")),
    ("gzip", include_str!("../ply/gzip.ply")),
    ("hashes", include_str!("../ply/hashes.ply")),
    ("env", include_str!("../ply/env.ply")),
    ("machine", include_str!("../ply/machine.ply")),
    ("hosts", include_str!("../ply/hosts.ply")),
    ("paths", include_str!("../ply/paths.ply")),
    ("ply", include_str!("../ply/ply.ply")),
    ("program", include_str!("../ply/program.ply")),
    ("prove", include_str!("../ply/prove.ply")),
    ("replace", include_str!("../ply/replace.ply")),
    ("report", include_str!("../ply/report.ply")),
    ("review", include_str!("../ply/review.ply")),
    ("run", include_str!("../ply/run.ply")),
    ("show", include_str!("../ply/show.ply")),
    ("signature", include_str!("../ply/signature.ply")),
    ("sources", include_str!("../ply/sources.ply")),
    ("stdlib", include_str!("../ply/stdlib.ply")),
    ("surface", include_str!("../ply/surface.ply")),
    ("style", include_str!("../ply/style.ply")),
    ("tests", include_str!("../ply/tests.ply")),
    ("walk", include_str!("../ply/walk.ply")),
];

/// Where the built program and the digest of the sources it was built from are committed.
pub const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/bootstrap");

pub const ARTIFACT: &str = "ply.plyx";

pub const DIGEST: &str = "ply.digest";

/// The marker that says a shelf directory is whole; landed last, so a reader never sees half of
/// one. Not a `.ply` file, so the program's own listing passes over it.
const SHELF_MARKER: &str = "SHELF.ok";

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
    let (frontend_version, runtime_version, body_encoding) = ply_machine::shelf::store_versions();
    for part in [
        program.as_str(),
        producer::digest_of(ply_machine::shelf::sources()).as_str(),
        producer::identity().as_str(),
        frontend_version,
        runtime_version,
    ] {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    hasher.update(&body_encoding.to_le_bytes());
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
    for (name, text) in ply_machine::shelf::sources() {
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

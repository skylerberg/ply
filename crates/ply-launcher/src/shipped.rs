//! What this binary ships: the `ply` program built from `crates/ply-cli/ply`.

use ply_codegen::c::stage;
use ply_eval::{Diagnostic, Span, codes};
use ply_machine::runnable::{self, Runnable};
use std::path::{Path, PathBuf};

pub const RUNNABLE: &str = "ply.run";

/// The committed program, the digest of the sources it was built from, and what a builder keeps it
/// under, for a text that enters the definition it does: where `ply bootstrap` writes them.
const COMMITTED: &str = "crates/ply-cli/bootstrap/ply.run";
const COMMITTED_DIGEST: &str = "crates/ply-cli/bootstrap/ply.digest";
const COMMITTED_KEY: &str = "crates/ply-cli/bootstrap/ply.key";

// --- The `ply` program -------------------------------------------------------

/// Where the CLI package sits inside a stage that carries its closure: the repository's own path,
/// because the keys below are the repository's own paths.
pub const ROOT: &str = ply_pack::PROGRAM;

const ENTRY: &str = "ply.main";

/// The name the rows of the program's builds are kept under.
const ROWS: &str = "cli";

/// Each file of the program's packages as the pack carries it: the path it has in the repository.
fn program_files() -> Vec<&'static str> {
    let pack = ply_pack::installed();
    pack.program_packages()
        .iter()
        .flat_map(|package| pack.files_in(package))
        .collect()
}

/// The whole closure as the port takes it: `(path, text)`, which `digest_of` sorts. A module is
/// keyed by its package's path and its stem and a manifest by its own path, as the repository has
/// them, so nothing about the program is a function of where this binary happens to be.
pub fn program_sources() -> Vec<(String, String)> {
    let pack = ply_pack::installed();
    program_files()
        .into_iter()
        .map(|path| {
            let key = path.strip_suffix(".ply").unwrap_or(path).to_string();
            let text = pack
                .text(path)
                .expect("a listed path is carried")
                .to_string();
            (key, text)
        })
        .collect()
}

/// Lays the shipped closure out under `stage`, keyed by the repository's paths, so a package's
/// `Path(..)` dependency resolves the way it does in a checkout.
pub fn lay_out(stage: &Path) -> std::io::Result<()> {
    let pack = ply_pack::installed();
    for path in program_files() {
        let at = stage.join(path);
        if let Some(dir) = at.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(at, pack.bytes(path).expect("a listed path is carried"))?;
    }
    Ok(())
}

/// What the built program is a function of: its sources, the shipped modules it is closed over,
/// the compiler that builds it among them, and the runtime its unit is compiled against.
/// `ply bootstrap` writes it beside the runnable it builds.
pub fn identity() -> String {
    static IDENTITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"ply program 1\0");
            for part in [
                ply_machine::builds::digest_of(&program_sources()),
                ply_machine::shipped_modules::digest(),
                ply_codegen::c::runtime_digest().to_string(),
            ] {
                hasher.update(part.as_bytes());
                hasher.update(&[0]);
            }
            hasher.finalize().to_hex()[..16].to_string()
        })
        .clone()
}

/// The stage of the program the committed builder makes of these sources: a function of both.
pub fn stage_name() -> String {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let mut hasher = blake3::Hasher::new();
        hasher.update(identity().as_bytes());
        hasher.update(&[0]);
        hasher.update(ply_machine::builds::committed_digest().as_bytes());
        format!("cli-{}", &hasher.finalize().to_hex()[..16])
    })
    .clone()
}

/// Where that stage is kept between runs, beside the emitter's own: the sources laid out, and the
/// runnable the committed builder made of them when no committed one serves.
pub fn stage() -> PathBuf {
    stage::stage_dir(&stage_name())
}

/// The stage of the program this tree's own builder makes of these sources.
pub fn own_stage_name() -> String {
    format!("self-{}", identity())
}

/// The digest the committed program was built from, when one is committed at all.
pub fn committed_digest() -> Option<&'static str> {
    ply_pack::installed().text(COMMITTED_DIGEST).map(str::trim)
}

fn committed() -> Option<&'static [u8]> {
    ply_pack::installed().bytes(COMMITTED)
}

/// The `ply` program: the committed runnable when it was built from these very sources, else one
/// a builder made of them for an earlier process, else one it makes now. The committed builder
/// makes it, since every build since main's last refresh shares its rows and bodies; where these
/// sources need a rule that builder lacks and it refuses them, this tree's own does. A builder
/// takes a program it already built, the committed one among them, where these sources enter the
/// definition that one does: a binary whose committed runnable is behind its sources runs what the
/// sources mean, never a runnable that means something else.
pub fn program() -> Result<Runnable, Diagnostic> {
    if committed_digest() == Some(identity().as_str())
        && let Some(bytes) = committed()
        && let Ok(program) = runnable::decode(bytes)
    {
        return Ok(program);
    }
    let stage = stage();
    let staged = stage.join(RUNNABLE);
    let found = || ply_machine::builds::staged_at(&stage, &staged).or_else(staged_by_own_builder);
    if let Some(program) = found() {
        return Ok(program);
    }
    ply_machine::builds::alone(&stage, || {
        if let Some(program) = found() {
            return Ok(program);
        }
        ply_machine::builds::kept_as_built(ply_pack::installed().text(COMMITTED_KEY), || {
            committed().map(<[u8]>::to_vec)
        });
        match ply_machine::builds::build(&laid_out()?, ROOT, ENTRY, &staged, ROWS) {
            Ok(()) => read_back(&staged),
            Err(_) => {
                eprintln!(
                    "ply: the committed builder does not build this tree's `ply`, so this \
                     tree's own builder builds it"
                );
                program_by_own_builder()
            }
        }
    })
}

fn own_stage() -> PathBuf {
    stage::stage_dir(&own_stage_name())
}

fn staged_by_own_builder() -> Option<Runnable> {
    let stage = own_stage();
    ply_machine::builds::staged_at(&stage, &stage.join(RUNNABLE))
}

/// The `ply` program as this tree's own builder makes it: what shows the compiler these sources
/// hold builds the program they hold.
pub fn program_by_own_builder() -> Result<Runnable, Diagnostic> {
    if let Some(program) = staged_by_own_builder() {
        return Ok(program);
    }
    let stage = own_stage();
    ply_machine::builds::alone(&stage, || {
        if let Some(program) = staged_by_own_builder() {
            return Ok(program);
        }
        let staged = stage.join(RUNNABLE);
        ply_machine::builds::build_by_own(&laid_out()?, ROOT, ENTRY, &staged, ROWS)?;
        read_back(&staged)
    })
}

fn read_back(staged: &Path) -> Result<Runnable, Diagnostic> {
    let started = std::time::Instant::now();
    let bytes = std::fs::read(staged)
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
    let aside = stage().join(format!("src.{}", ply_machine::builds::aside()));
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

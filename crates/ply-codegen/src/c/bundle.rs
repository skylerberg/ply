//! A bootstrap bundle: the C a unit was emitted as, compressed, with the digest of the sources it
//! was emitted from. The emitter written in Ply is built from one of these, and the fixpoint test
//! in `crates/ply-codegen-tests` is what says the bundle serves: the emitter built from it emits,
//! for its own sources, the C it was built from.
//!
//! **The unit describes itself.** Its runtime helper table, its constructor table, its functions
//! and their arities, which of them are constants, its module count and its tables travel inside
//! the C (`exports.rs`), so a bundle is one file and one digest, and building the emitter from it
//! reads no source at all. The tables being the unit's own is what lets an older bundle still
//! run: a sum type's tags are positions in the constructor table baked into the C as numbers, and
//! the helpers are bound by position, so a bundle serves any runtime whose table starts with the
//! one it was emitted against, and refuses, naming the helper, any other.

use super::{Native, Refused};
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::Path;

const UNIT: &str = "unit.c.gz";
const SOURCES: &str = "SOURCES.digest";

/// Writes the bundle, replacing what was there; each file lands by a rename, so a reader never
/// sees half of one.
pub fn write(dir: &Path, text: &str, sources_digest: &str) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    let land = |name: &str, bytes: Vec<u8>| -> Result<()> {
        let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, dir.join(name))?;
        Ok(())
    };
    land(UNIT, pack(text)?)?;
    land(SOURCES, format!("{sources_digest}\n").into_bytes())
}

/// A bundle: its C and the digest of the sources it came from. Whether it serves this runtime is
/// the unit's own answer, given when it is built.
///
/// The embedded one is `ply-compiler`'s, which the binary carries and the fixpoint test refreshes
/// in place; a stage is what the committed emitter emitted for other sources, kept by their digest.
pub struct Bundle {
    unit: std::borrow::Cow<'static, [u8]>,
    sources: Option<String>,
}

pub fn embedded() -> Bundle {
    Bundle {
        unit: std::borrow::Cow::Borrowed(ply_compiler::bootstrap::UNIT),
        sources: Some(ply_compiler::bootstrap::SOURCES.trim().to_string()),
    }
}

/// The bundle a directory holds, when it holds one.
pub fn from_dir(dir: &Path) -> Option<Bundle> {
    Some(Bundle {
        unit: std::borrow::Cow::Owned(std::fs::read(dir.join(UNIT)).ok()?),
        sources: sources_digest(dir),
    })
}

/// Where the stage emitted for the sources of `identity` is kept between runs, under the unit cache.
/// A stage is a product of the emitter sources alone, so it lives beside the unit cache rather
/// than under it: a run with a cache of its own still finds the stage an earlier one wrote.
/// `PLY_C_STAGE` names another root.
pub fn stage_dir(identity: &str) -> std::path::PathBuf {
    std::env::var("PLY_C_STAGE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("ply-c-stage"))
        .join(identity)
}

impl Bundle {
    /// The digest of the emitter sources this was emitted from.
    pub fn sources_digest(&self) -> Option<&str> {
        self.sources.as_deref()
    }

    /// The unit's C, compressed, as the bundle stores it.
    pub fn unit(&self) -> &[u8] {
        &self.unit
    }
}

/// A unit's C compressed, as the bundle stores it and as an artifact embeds it.
pub fn pack(text: &str) -> Result<Vec<u8>> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(text.as_bytes())?;
    Ok(gz.finish()?)
}

pub fn unpack(bytes: &[u8]) -> Result<String> {
    let mut out = String::new();
    flate2::read::GzDecoder::new(bytes).read_to_string(&mut out)?;
    Ok(out)
}

/// The unit's C as a bundle holds it.
pub fn text_of(bundle: &Bundle) -> Result<String> {
    unpack(&bundle.unit)
}

/// The digest of the emitter sources the bundle was emitted from.
pub fn sources_digest(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(SOURCES))
        .ok()
        .map(|s| s.trim().to_string())
}

pub fn exists(dir: &Path) -> bool {
    dir.join(UNIT).is_file()
}

/// Builds the emitter's own program from the bundle: the bundle's C is compiled and loaded
/// against the tables it carries. Nothing is emitted and nothing is parsed, so a failure inside it
/// names no place. A bundle emitted against a helper table this runtime's does not start with
/// fails here with [`super::exports::Unserved`].
///
/// A bundle loads through [`super::upgrade`], except under nextest, where a background compile
/// would contend with the suite and change which object a later test loads.
pub fn build(bundle: &Bundle) -> Result<(Native, Vec<Refused>)> {
    let text = text_of(bundle)?;
    let lib = if std::env::var_os("NEXTEST").is_none() {
        super::upgrade::load(&text, "bootstrap")?
    } else {
        super::load::compile_and_load(&text, "bootstrap")?
    };
    super::build::finish_unit(lib, None)
}

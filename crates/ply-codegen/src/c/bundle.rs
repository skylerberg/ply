//! A bootstrap bundle: the C a unit was emitted as, compressed, with the digest of the sources it
//! was emitted from and of the runtime it calls into. The emitter written in Ply is built from one
//! of these rather than by a reference emitter, and the fixpoint test in `crates/ply-codegen-tests`
//! is what says the bundle serves: the emitter built from it emits, for its own sources, the C it
//! was built from.
//!
//! **The unit describes itself.** Its constructor table, its functions and their arities, which
//! of them are constants, its module count and its tables travel inside the C (`exports.rs`), so
//! a bundle is one file and two digests, and building the emitter from it reads no source at all.
//! The table being the unit's own is what lets an older bundle still run: a sum type's tags are
//! positions in the table baked into the C as numbers, and a bundle bound to the table of *later*
//! sources, one variant removed, would run old code against new numbers.

use super::{Native, Refused};
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::Path;

const UNIT: &str = "unit.c.gz";
const SOURCES: &str = "SOURCES.digest";
const RUNTIME: &str = "RUNTIME.digest";

/// The runtime the bundle's C calls into: every helper's name and shape. A helper that moves
/// leaves the bundle calling the old shape, which no digest of the emitter's sources sees.
pub fn runtime_digest() -> String {
    let mut h = blake3::Hasher::new();
    for helper in super::prelude::HELPERS {
        h.update(format!("{} {} {}\n", helper.name, helper.args, helper.answers).as_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// Whether the bundle was emitted against another runtime than this build's, in which case it
/// does not serve: [`exists`] answers `false` and the reference builds the producer.
pub fn stale_runtime(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(RUNTIME))
        .ok()
        .is_none_or(|s| s.trim() != runtime_digest())
}

/// Writes the bundle, replacing what was there.
pub fn write(dir: &Path, text: &str, sources_digest: &str) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    std::fs::write(dir.join(UNIT), pack(text)?)?;
    std::fs::write(dir.join(SOURCES), format!("{sources_digest}\n"))?;
    std::fs::write(dir.join(RUNTIME), format!("{}\n", runtime_digest()))?;
    Ok(())
}

/// A bundle that serves: its C and the digest of the sources it came from.
///
/// The embedded one is `ply-compiler`'s, which the binary carries; a directory is a working copy's
/// own `bootstrap/`, refreshed in place by the fixpoint test.
pub struct Bundle {
    unit: std::borrow::Cow<'static, [u8]>,
    sources: Option<String>,
}

/// The bundle for `src`, or `None` when there is none that serves this runtime.
pub fn of(src: &super::producer::Sources) -> Option<Bundle> {
    match src {
        super::producer::Sources::Embedded => {
            if ply_compiler::bootstrap::RUNTIME.trim() != runtime_digest() {
                return None;
            }
            Some(Bundle {
                unit: std::borrow::Cow::Borrowed(ply_compiler::bootstrap::UNIT),
                sources: Some(ply_compiler::bootstrap::SOURCES.trim().to_string()),
            })
        }
        super::producer::Sources::Directory(dir) => from_dir(&dir.join("bootstrap")),
    }
}

/// The bundle a directory holds, when it holds one that serves.
pub fn from_dir(dir: &Path) -> Option<Bundle> {
    if stale_runtime(dir) {
        return None;
    }
    Some(Bundle {
        unit: std::borrow::Cow::Owned(std::fs::read(dir.join(UNIT)).ok()?),
        sources: sources_digest(dir),
    })
}

impl Bundle {
    /// The digest of the emitter sources this was emitted from.
    pub fn sources_digest(&self) -> Option<&str> {
        self.sources.as_deref()
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
    dir.join(UNIT).is_file() && !stale_runtime(dir)
}

/// Builds the emitter's own program from the bundle: the bundle's C is compiled and loaded
/// against the table it carries. Nothing is emitted and nothing is parsed; the modules are
/// numbered from zero, as the bootstrap assigns them.
pub fn build(bundle: &Bundle) -> Result<(Native, Vec<Refused>)> {
    let text = text_of(bundle)?;
    super::build::load_unit(&text, None, "bootstrap")
}

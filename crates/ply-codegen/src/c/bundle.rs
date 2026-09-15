//! A bootstrap bundle: the C a unit was emitted as, compressed, with the record the cache keeps
//! beside a built unit, the constructor table the C was emitted against, and the digest of the
//! sources it was emitted from. The emitter written in Ply is built from one of these rather than
//! by a reference emitter, and the fixpoint test in `crates/ply-codegen-tests` is what says the
//! bundle serves: the emitter built from it emits, for its own sources, the C it was built from.
//!
//! **The constructor table is the bundle's own.** A sum type's tags are its variants' positions in
//! the program's table, baked into the C as numbers, so a bundle bound to the table of *later*
//! sources -- one variant removed, every tag after it moved -- runs old code against new numbers
//! and reads the wrong shape. That is what a stale bundle did before the table travelled with it.

use super::cache::{UnitCache, decode_unit, encode_unit};
use super::{Native, Refused};
use crate::Source;
use anyhow::{Context, Result, anyhow};
use ply_span::Symbol;
use std::io::{Read, Write};
use std::path::Path;

const UNIT: &str = "unit.c.gz";
const RECORD: &str = "unit.record";
const CTORS: &str = "unit.ctors";
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
pub fn write(
    dir: &Path,
    text: &str,
    record: &UnitCache,
    ctors: &[(Symbol, usize)],
    sources_digest: &str,
) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    std::fs::write(dir.join(UNIT), pack(text)?)?;
    std::fs::write(dir.join(RECORD), encode_unit(record))?;
    std::fs::write(dir.join(CTORS), encode_ctors(ctors))?;
    std::fs::write(dir.join(SOURCES), format!("{sources_digest}\n"))?;
    std::fs::write(dir.join(RUNTIME), format!("{}\n", runtime_digest()))?;
    Ok(())
}

/// One `name arity` per line, in tag order.
pub fn encode_ctors(ctors: &[(Symbol, usize)]) -> String {
    let mut out = String::new();
    for (name, arity) in ctors {
        out.push_str(&format!("{name} {arity}\n"));
    }
    out
}

/// `None` for a line that is not `name arity`, and for an empty table, which no unit was ever
/// emitted against.
pub fn decode_ctors(s: &str) -> Option<Vec<(Symbol, usize)>> {
    let table: Vec<(Symbol, usize)> = s
        .lines()
        .map(|line| {
            let (name, arity) = line.rsplit_once(' ')?;
            Some((Symbol::new(name), arity.parse().ok()?))
        })
        .collect::<Option<_>>()?;
    (!table.is_empty()).then_some(table)
}

fn ctors_in(dir: &Path) -> Option<Vec<(Symbol, usize)>> {
    decode_ctors(&std::fs::read_to_string(dir.join(CTORS)).ok()?)
}

/// A bundle that serves: its C, its record, its constructor table, and the digest of the sources
/// it came from.
///
/// The embedded one is `ply-compiler`'s, which the binary carries; a directory is a working copy's
/// own `bootstrap/`, refreshed in place by the fixpoint test.
pub struct Bundle {
    unit: std::borrow::Cow<'static, [u8]>,
    record: String,
    ctors: Vec<(Symbol, usize)>,
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
                record: ply_compiler::bootstrap::RECORD.to_string(),
                ctors: decode_ctors(ply_compiler::bootstrap::CTORS)?,
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
        record: std::fs::read_to_string(dir.join(RECORD)).ok()?,
        ctors: ctors_in(dir)?,
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

pub fn record(dir: &Path) -> Result<UnitCache> {
    let s = std::fs::read_to_string(dir.join(RECORD))
        .with_context(|| dir.join(RECORD).display().to_string())?;
    decode_unit(&s).ok_or_else(|| anyhow!("{} does not decode", dir.join(RECORD).display()))
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
        && dir.join(RECORD).is_file()
        && ctors_in(dir).is_some()
        && !stale_runtime(dir)
}

/// Builds `loaded`, the emitter's own program, from the bundle: the bundle's C is compiled and
/// loaded against the table it was emitted with, and its record stands in for what emitting would
/// have recorded. Nothing is emitted.
pub fn build(loaded: &'static Source, bundle: &Bundle) -> Result<(Native, Vec<Refused>)> {
    let text = text_of(bundle)?;
    let record = decode_unit(&bundle.record)
        .ok_or_else(|| anyhow!("the bootstrap bundle's record does not decode"))?;
    super::build::load_unit(loaded, &text, record, bundle.ctors.clone(), "bootstrap")
}

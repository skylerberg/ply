//! A bootstrap bundle: the C a unit was emitted as, compressed, with the record the cache keeps
//! beside a built unit and the digest of the sources it was emitted from. The emitter written in
//! Ply is built from one of these rather than by a reference emitter, and the fixpoint test in
//! `crates/ply-codegen-tests` is what says the bundle still serves: the emitter built from it
//! emits, for its own sources, the C it was built from.

use super::cache::{UnitCache, decode_unit, encode_unit};
use super::{Native, Refused};
use crate::Source;
use anyhow::{Context, Result, anyhow};
use std::io::{Read, Write};
use std::path::Path;

const UNIT: &str = "unit.c.gz";
const RECORD: &str = "unit.record";
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
pub fn write(dir: &Path, text: &str, record: &UnitCache, sources_digest: &str) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(text.as_bytes())?;
    std::fs::write(dir.join(UNIT), gz.finish()?)?;
    std::fs::write(dir.join(RECORD), encode_unit(record))?;
    std::fs::write(dir.join(SOURCES), format!("{sources_digest}\n"))?;
    std::fs::write(dir.join(RUNTIME), format!("{}\n", runtime_digest()))?;
    Ok(())
}

/// The unit's C as the bundle holds it.
pub fn text(dir: &Path) -> Result<String> {
    let bytes =
        std::fs::read(dir.join(UNIT)).with_context(|| dir.join(UNIT).display().to_string())?;
    let mut out = String::new();
    flate2::read::GzDecoder::new(&bytes[..]).read_to_string(&mut out)?;
    Ok(out)
}

pub fn record(dir: &Path) -> Result<UnitCache> {
    let s = std::fs::read_to_string(dir.join(RECORD))
        .with_context(|| dir.join(RECORD).display().to_string())?;
    decode_unit(&s).ok_or_else(|| anyhow!("{} does not decode", dir.join(RECORD).display()))
}

/// The digest of the emitter sources the bundle was emitted from.
pub fn sources_digest(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(SOURCES))
        .ok()
        .map(|s| s.trim().to_string())
}

pub fn exists(dir: &Path) -> bool {
    dir.join(UNIT).is_file() && dir.join(RECORD).is_file() && !stale_runtime(dir)
}

/// Builds `loaded`, the emitter's own program, from the bundle: the bundle's C is compiled and
/// loaded, and its record stands in for what emitting would have recorded. Nothing is emitted.
pub fn build(loaded: &'static Source, dir: &Path) -> Result<(Native, Vec<Refused>)> {
    let text = text(dir)?;
    let record = record(dir)?;
    let refused = record
        .refusals
        .iter()
        .map(|(function, construct)| Refused {
            function: function.clone(),
            construct: construct.clone(),
        })
        .collect();
    let lib = super::load::compile_and_load(&text, "bootstrap")?;
    let native = super::build::finish(loaded, lib, record, loaded.ctors())?;
    Ok((native, refused))
}

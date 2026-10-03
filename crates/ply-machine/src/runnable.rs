//! A program as the launcher enters it: the definition it runs, its front end's answer over every
//! source that answer's indices run over, and the C of its unit, in the value codec this runtime
//! reads, compressed. A program writes one through `shipped.runnable`; reading one runs no compiler.

use crate::driver::{LoadedAnalysis, LoadedFile};
use crate::payload::record;
use ply_eval::decode::At;
use ply_eval::{SourceId, Value};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// What a runnable is, as its `format` field says.
const FORMAT: &str = "ply runnable 1";

/// A program read back from its runnable.
pub struct Runnable {
    pub entry: String,
    pub front: LoadedAnalysis,
    /// The unit's C.
    pub unit: String,
}

/// The runnable of the program `entry` enters: `files` and `dump` as its front end answered them,
/// and `unit`, its C. Two builds of one program write the same bytes.
pub fn encode(entry: &str, files: &Value, dump: &Value, unit: &[u8]) -> Result<Vec<u8>, String> {
    let encoded = ply_eval::codec::encode(&record(vec![
        ("format", Value::str(FORMAT)),
        ("entry", Value::str(entry)),
        ("files", files.clone()),
        ("dump", canonical(dump)),
        ("unit", Value::bytes(unit)),
    ]))?;
    let mut packed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    packed.write_all(&encoded).map_err(|e| e.to_string())?;
    packed.finish().map_err(|e| e.to_string())
}

/// The front end's answer as a runnable holds it. What a load walked and what it warned of are
/// functions of the caches it read as much as of the program, and a program that checked runs
/// without either, so a runnable keeps neither.
fn canonical(dump: &Value) -> Value {
    let Value::Record(fields) = dump else {
        return dump.clone();
    };
    record(
        fields
            .iter()
            .map(|(name, value)| {
                let kept = match name.as_str() {
                    "walked" => Value::bytes(b""),
                    "diags" => Value::list(Vec::new()),
                    _ => value.clone(),
                };
                (name.as_str(), kept)
            })
            .collect(),
    )
}

/// The program `bytes` hold, or why they hold none.
pub fn decode(bytes: &[u8]) -> Result<Runnable, String> {
    let started = Instant::now();
    let mut encoded = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut encoded)
        .map_err(|e| format!("it is not compressed as a runnable is: {e}"))?;
    let value = ply_eval::codec::decode(&encoded)?;
    let at = At::new("a runnable", &value);
    let read = || -> Result<Runnable, ply_eval::decode::Error> {
        if at.field("format")?.str()? != FORMAT {
            return Err(at.error("a runnable of another format"));
        }
        let files = at.field("files")?.items(|file| {
            Ok(LoadedFile {
                path: file.field("path")?.str()?.to_string(),
                name: file.field("name")?.str()?.to_string(),
                text: String::from_utf8_lossy(file.field("text")?.bytes()?).into_owned(),
            })
        })?;
        let ids: Vec<SourceId> = (0..files.len()).map(|i| SourceId(i as u32)).collect();
        let answer = ply_codegen::c::dump::read(at.field("dump")?.value(), &ids)
            .map_err(|e| at.error(format!("its front end's answer does not read: {e}")))?;
        let unit = String::from_utf8(at.field("unit")?.bytes()?.to_vec())
            .map_err(|_| at.error("its unit's C is not UTF-8"))?;
        Ok(Runnable {
            entry: at.field("entry")?.str()?.to_string(),
            front: LoadedAnalysis {
                answer,
                files,
                read: Duration::ZERO,
                front: started.elapsed(),
                write_back: Duration::ZERO,
                cached: false,
            },
            unit,
        })
    };
    read().map_err(|e| e.to_string())
}

//! A program's front end as an artifact carries it for a machine to load: the definition it runs and
//! its front end's answer over every source that answer's indices run over, in the value codec this
//! runtime reads, compressed. A program writes one through `shipped.runnable`; reading one runs no
//! compiler.

use crate::driver::{LoadedAnalysis, LoadedFile};
use crate::payload::record;
use ply_core::decode::AnswerValue;
use ply_core::{SourceId, Value};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// What a runnable is, as its `format` field says.
const FORMAT: &str = "ply runnable 2";

/// A program read back from its runnable.
pub struct Runnable {
    pub entry: String,
    pub front: LoadedAnalysis,
}

/// The runnable of the program `entry` enters: `files` and `dump` as its front end answered them.
/// The same arguments write the same bytes.
pub fn encode(entry: &str, files: &Value, dump: &Value) -> Result<Vec<u8>, String> {
    let encoded = ply_core::codec::encode(&record(vec![
        ("format", Value::str(FORMAT)),
        ("entry", Value::str(entry)),
        ("files", files.clone()),
        ("dump", dump.clone()),
    ]))?;
    let mut packed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    packed.write_all(&encoded).map_err(|e| e.to_string())?;
    packed.finish().map_err(|e| e.to_string())
}

fn value_of(bytes: &[u8]) -> Result<Value, String> {
    let mut encoded = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut encoded)
        .map_err(|e| format!("it is not compressed as a runnable is: {e}"))?;
    ply_core::codec::decode(&encoded)
}

/// The program `bytes` hold, or why they hold none.
pub fn decode(bytes: &[u8]) -> Result<Runnable, String> {
    let started = Instant::now();
    let value = value_of(bytes)?;
    let at = AnswerValue::new("a runnable", &value);
    let read = || -> Result<Runnable, ply_core::decode::Error> {
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
        let answer = ply_compiled::c::dump::read(at.field("dump")?.value(), &ids)
            .map_err(|e| at.error(format!("its front end's answer does not read: {e}")))?;
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
        })
    };
    read().map_err(|e| e.to_string())
}

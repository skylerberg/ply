//! The compiler's answers, kept between runs. An entry's answer is a function of the emitter that
//! gave it, the runtime it ran on, the entry and its arguments, so the same question asked of the
//! same compiler is read back rather than worked out again.

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use ply_eval::Value;
use ply_eval::files::write_atomically;
use std::io::{Read, Write};
use std::path::PathBuf;

/// The sources of the runtime the emitter runs on, as `build.rs` digests them.
const RUNTIME: &str = env!("PLY_RUNTIME_DIGEST");

pub(super) fn dir() -> PathBuf {
    super::load::cache_dir().join("answers")
}

/// What `entry`'s answer over `args` is kept under, for the emitter `emitter` names; `None` for an
/// argument that has no encoding, whose answer is not kept.
pub fn key(emitter: &str, entry: &str, args: &[Value]) -> Option<String> {
    let mut h = blake3::Hasher::new();
    for part in ["ply-c-answer-1", RUNTIME, emitter, entry] {
        h.update(part.as_bytes());
        h.update(&[0]);
    }
    for arg in args {
        let bytes = ply_eval::codec::encode(arg).ok()?;
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    }
    Some(h.finalize().to_hex().to_string())
}

/// The answer kept under `key`, recorded as used so the sweep leaves what runs keep asking.
pub fn read(key: &str) -> Option<Value> {
    let path = dir().join(key);
    let packed = std::fs::read(&path).ok()?;
    let mut bytes = Vec::new();
    GzDecoder::new(packed.as_slice())
        .read_to_end(&mut bytes)
        .ok()?;
    let answer = ply_eval::codec::decode(&bytes).ok()?;
    super::sweep::used(&path);
    Some(answer)
}

/// Keep `answer` under `key`; a failed write is ignored.
pub fn write(key: &str, answer: &Value) {
    let Ok(bytes) = ply_eval::codec::encode(answer) else {
        return;
    };
    let mut packed = GzEncoder::new(Vec::new(), Compression::fast());
    if packed.write_all(&bytes).is_err() {
        return;
    }
    let Ok(packed) = packed.finish() else {
        return;
    };
    let d = dir();
    if std::fs::create_dir_all(&d).is_err() {
        return;
    }
    let _ = write_atomically(&d.join(key), &packed);
}

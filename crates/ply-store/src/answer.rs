//! The front end's whole answer over the last program asked about, filed under the key of
//! everything it is a function of. One file, replaced whole, so it never holds more than one.

use crate::{ContentHash, FRONTEND_FORMAT, FRONTEND_VERSION, disk};
use std::path::Path;

pub(crate) const ANSWER_FILE: &str = "frontend.answer";
pub(crate) const ANSWER_STEM: &str = "answer";

const MAGIC: &[u8; 8] = b"PLYFRONT";
const HEADER: usize = 8 + 32 + 32;

fn stamp(key: ContentHash) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&FRONTEND_FORMAT.to_le_bytes());
    h.update(FRONTEND_VERSION.as_bytes());
    h.update(&[0]);
    h.update(&key.0);
    *h.finalize().as_bytes()
}

/// `None` for a missing, foreign or damaged file as much as for another key: each is only a miss.
pub(crate) fn read(path: &Path, key: ContentHash) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < HEADER || &bytes[..8] != MAGIC || bytes[8..40] != stamp(key) {
        return None;
    }
    let dump = &bytes[HEADER..];
    if blake3::hash(dump).as_bytes()[..] != bytes[40..HEADER] {
        return None;
    }
    String::from_utf8(dump.to_vec()).ok()
}

pub(crate) fn write(dir: &Path, path: &Path, key: ContentHash, dump: &str) -> anyhow::Result<()> {
    let mut out = Vec::with_capacity(HEADER + dump.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&stamp(key));
    out.extend_from_slice(blake3::hash(dump.as_bytes()).as_bytes());
    out.extend_from_slice(dump.as_bytes());
    disk::write_atomic(dir, path, ANSWER_STEM, &out, "front-end answer")
}

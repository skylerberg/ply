//! The parts of the front end's last answer, in one file replaced whole.

use crate::{ContentHash, FRONTEND_FORMAT, FRONTEND_VERSION, disk};
use ply_span::frames::Cursor;
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) const ANSWER_FILE: &str = "frontend.answer";
pub(crate) const ANSWER_STEM: &str = "answer";

const MAGIC: &[u8; 8] = b"PLYPARTS";
const HEADER: usize = 8 + 32 + 32;

fn stamp() -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&FRONTEND_FORMAT.to_le_bytes());
    h.update(FRONTEND_VERSION.as_bytes());
    *h.finalize().as_bytes()
}

/// Empty for a missing, foreign or damaged file: each is only a miss.
pub(crate) fn read(path: &Path) -> BTreeMap<ContentHash, String> {
    parts(path).unwrap_or_default()
}

fn parts(path: &Path) -> Option<BTreeMap<ContentHash, String>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < HEADER || &bytes[..8] != MAGIC || bytes[8..40] != stamp() {
        return None;
    }
    let body = &bytes[HEADER..];
    if blake3::hash(body).as_bytes()[..] != bytes[40..HEADER] {
        return None;
    }
    let mut parts = BTreeMap::new();
    let mut cursor = Cursor::new(body, "part");
    while !cursor.done() {
        let (words, text) = cursor.unit().ok()?;
        let [key] = words[..] else {
            return None;
        };
        parts.insert(
            ContentHash::from_hex(key)?,
            String::from_utf8(text.to_vec()).ok()?,
        );
    }
    Some(parts)
}

pub(crate) fn write(
    dir: &Path,
    path: &Path,
    parts: &BTreeMap<ContentHash, String>,
) -> anyhow::Result<()> {
    let mut body = Vec::new();
    for (key, text) in parts {
        body.extend_from_slice(format!("{} {}\n", key.to_hex(), text.len()).as_bytes());
        body.extend_from_slice(text.as_bytes());
    }
    let mut out = Vec::with_capacity(HEADER + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&stamp());
    out.extend_from_slice(blake3::hash(&body).as_bytes());
    out.extend_from_slice(&body);
    disk::write_atomic(dir, path, ANSWER_STEM, &out, "front-end answer")
}

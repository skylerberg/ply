//! The parts of the port's per-module claims, in one file replaced whole.

use crate::{ContentHash, FRONTEND_FORMAT, FRONTEND_VERSION, disk};
use ply_span::frames::Cursor;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub(crate) const CLAIMS_FILE: &str = "claims.answer";
pub(crate) const CLAIMS_STEM: &str = "claims";

const MAGIC: &[u8; 8] = b"PLYPARTS";
const HEADER: usize = 8 + 32 + 32;

pub(crate) struct Answer {
    path: PathBuf,
    stem: &'static str,
    what: &'static str,
    stored: OnceLock<BTreeMap<ContentHash, String>>,
    pending: Option<BTreeMap<ContentHash, String>>,
}

impl Answer {
    pub(crate) fn new(path: PathBuf, stem: &'static str, what: &'static str) -> Answer {
        Answer {
            path,
            stem,
            what,
            stored: OnceLock::new(),
            pending: None,
        }
    }

    fn stored(&self) -> &BTreeMap<ContentHash, String> {
        self.stored.get_or_init(|| read(&self.path))
    }

    pub(crate) fn part(&self, key: ContentHash) -> Option<String> {
        self.pending
            .as_ref()
            .unwrap_or_else(|| self.stored())
            .get(&key)
            .cloned()
    }

    /// Replaces every part on disk at the next flush, unless these are the parts already there.
    pub(crate) fn put(&mut self, parts: BTreeMap<ContentHash, String>) {
        self.pending = if self.stored().keys().eq(parts.keys()) {
            None
        } else {
            Some(parts)
        };
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_none() && !self.path.exists()
    }

    pub(crate) fn flush(&mut self, dir: &Path) -> anyhow::Result<()> {
        if let Some(parts) = self.pending.take() {
            write(dir, &self.path, self.stem, &parts, self.what)?;
            self.stored = OnceLock::from(parts);
        }
        Ok(())
    }

    /// Under the cache lock.
    pub(crate) fn clear(&mut self) -> anyhow::Result<()> {
        self.pending = None;
        self.stored = OnceLock::from(BTreeMap::new());
        crate::remove(&self.path, self.what)
    }
}

fn stamp() -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&FRONTEND_FORMAT.to_le_bytes());
    h.update(FRONTEND_VERSION.as_bytes());
    *h.finalize().as_bytes()
}

/// Empty for a missing, foreign or damaged file: each is only a miss.
fn read(path: &Path) -> BTreeMap<ContentHash, String> {
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

fn write(
    dir: &Path,
    path: &Path,
    stem: &str,
    parts: &BTreeMap<ContentHash, String>,
    what: &str,
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
    disk::write_atomic(dir, path, stem, &out, what)
}

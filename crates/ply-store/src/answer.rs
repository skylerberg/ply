//! The parts of the port's per-module claims, in one file replaced whole. A part is bytes this
//! file does not read: the caller encodes it.

use crate::{ContentHash, FRONTEND_FORMAT, FRONTEND_VERSION, disk};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub(crate) const CLAIMS_FILE: &str = "claims.answer";
pub(crate) const CLAIMS_STEM: &str = "claims";

const MAGIC: &[u8; 8] = b"PLYPARTS";
const HEADER: usize = 8 + 32 + 32;

/// Bumped when what a part holds or how the parts are laid out changes; a file of another layout
/// is then only a miss.
const PARTS_FORMAT: u32 = 2;

pub(crate) struct Answer {
    path: PathBuf,
    stem: &'static str,
    what: &'static str,
    stored: OnceLock<BTreeMap<ContentHash, Vec<u8>>>,
    pending: Option<BTreeMap<ContentHash, Vec<u8>>>,
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

    fn stored(&self) -> &BTreeMap<ContentHash, Vec<u8>> {
        self.stored.get_or_init(|| read(&self.path))
    }

    pub(crate) fn part(&self, key: ContentHash) -> Option<Vec<u8>> {
        self.pending
            .as_ref()
            .unwrap_or_else(|| self.stored())
            .get(&key)
            .cloned()
    }

    /// Replaces every part on disk at the next flush, unless these are the parts already there.
    pub(crate) fn put(&mut self, parts: BTreeMap<ContentHash, Vec<u8>>) {
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
    h.update(&PARTS_FORMAT.to_le_bytes());
    h.update(&FRONTEND_FORMAT.to_le_bytes());
    h.update(FRONTEND_VERSION.as_bytes());
    *h.finalize().as_bytes()
}

/// Empty for a missing, foreign or damaged file: each is only a miss.
fn read(path: &Path) -> BTreeMap<ContentHash, Vec<u8>> {
    parts(path).unwrap_or_default()
}

/// Each part is its key, its length as eight little-endian bytes, and that many bytes.
fn parts(path: &Path) -> Option<BTreeMap<ContentHash, Vec<u8>>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < HEADER || &bytes[..8] != MAGIC || bytes[8..40] != stamp() {
        return None;
    }
    let body = &bytes[HEADER..];
    if blake3::hash(body).as_bytes()[..] != bytes[40..HEADER] {
        return None;
    }
    let mut parts = BTreeMap::new();
    let mut rest = body;
    while !rest.is_empty() {
        let (key, after) = rest.split_first_chunk::<32>()?;
        let (len, after) = after.split_first_chunk::<8>()?;
        let len = usize::try_from(u64::from_le_bytes(*len)).ok()?;
        let part = after.get(..len)?;
        parts.insert(ContentHash(*key), part.to_vec());
        rest = &after[len..];
    }
    Some(parts)
}

fn write(
    dir: &Path,
    path: &Path,
    stem: &str,
    parts: &BTreeMap<ContentHash, Vec<u8>>,
    what: &str,
) -> anyhow::Result<()> {
    let mut body = Vec::new();
    for (key, part) in parts {
        body.extend_from_slice(&key.0);
        body.extend_from_slice(&(part.len() as u64).to_le_bytes());
        body.extend_from_slice(part);
    }
    let mut out = Vec::with_capacity(HEADER + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&stamp());
    out.extend_from_slice(blake3::hash(&body).as_bytes());
    out.extend_from_slice(&body);
    disk::write_atomic(dir, path, stem, &out, what)
}

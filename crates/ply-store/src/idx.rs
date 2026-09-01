//! The two files behind the front-end cache: a small index, rewritten whole and atomically, over an
//! append-only data file that is mapped and whose entries are decoded one at a time.

use crate::{ContentHash, FRONTEND_FORMAT, FRONTEND_VERSION, disk};
use ply_hash::DefHash;
use ply_span::Diagnostic;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Seek, SeekFrom, Write};
use std::path::Path;

pub(crate) const INDEX_FILE: &str = "frontend.idx";
pub(crate) const DATA_FILE: &str = "frontend.dat";

const IDX_MAGIC: &[u8; 8] = b"PLYFEIDX";
const DAT_MAGIC: &[u8; 8] = b"PLYFEDAT";

pub(crate) const DATA_HEADER: u64 = 56;
const INDEX_HEADER: usize = 132;
pub(crate) const FRAME_HEADER: u64 = 13;

pub(crate) const KIND_DEF: u8 = 1;
pub(crate) const KIND_DECL: u8 = 2;
pub(crate) const KIND_BODY: u8 = 3;
pub(crate) const KIND_SOURCE: u8 = 4;

const SECTION_DEFS: u32 = 1;
const SECTION_DECLS: u32 = 2;
const SECTION_BODIES: u32 = 3;
const SECTION_SOURCES: u32 = 4;
const SECTION_PATHS: u32 = 5;

const HASH_RECORD: usize = 48;
const SOURCE_RECORD: usize = 24;
const DESCRIPTOR: usize = 24;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Located {
    pub(crate) offset: u64,
    pub(crate) len: u32,
}

impl Located {
    fn end(self) -> u64 {
        self.offset
            .saturating_add(FRAME_HEADER)
            .saturating_add(self.len as u64)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct HashSlot {
    pub(crate) hash: DefHash,
    pub(crate) at: Located,
}

/// Why a front-end cache was refused.
pub(crate) enum CacheError {
    Missing,
    Io(std::io::Error),
    Corrupt(String),
    Format(u32),
    Schema,
    Version,
    Unpaired,
}

impl CacheError {
    pub(crate) fn into_diagnostic(self, path: &Path) -> Diagnostic {
        let path = path.display();
        let d = match self {
            CacheError::Missing => Diagnostic::warning(
                crate::codes::CACHE_UNREADABLE,
                format!("no front-end cache at `{path}`"),
            ),
            CacheError::Io(e) => Diagnostic::warning(
                crate::codes::CACHE_UNREADABLE,
                format!("could not read the front-end cache `{path}`: {e}"),
            ),
            CacheError::Corrupt(what) => Diagnostic::warning(
                crate::codes::CACHE_CORRUPT,
                format!("the front-end cache `{path}` is corrupt: {what}"),
            ),
            CacheError::Format(found) => Diagnostic::warning(
                crate::codes::CACHE_CORRUPT,
                format!(
                    "the front-end cache `{path}` is format {found}, this build reads \
                     {FRONTEND_FORMAT}"
                ),
            ),
            CacheError::Schema => Diagnostic::warning(
                crate::codes::CACHE_VERSION_CHANGED,
                format!(
                    "the front-end cache `{path}` stores a different set of shapes than this \
                     build does"
                ),
            ),
            CacheError::Version => Diagnostic::warning(
                crate::codes::CACHE_VERSION_CHANGED,
                format!(
                    "the front-end cache `{path}` was written by another front end, \
                     this build is `{FRONTEND_VERSION}`"
                ),
            ),
            CacheError::Unpaired => Diagnostic::warning(
                crate::codes::CACHE_CORRUPT,
                format!("the front-end cache `{path}` and its data file were not written together"),
            ),
        };
        d.note(
            "continuing without it; every file is parsed and every definition rechecked, \
             and the cache is rewritten",
        )
    }
}

fn corrupt<T>(what: impl Into<String>) -> Result<T, CacheError> {
    Err(CacheError::Corrupt(what.into()))
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut out = [0u8; 8];
    out.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(out)
}

pub(crate) fn version_hash() -> [u8; 32] {
    *blake3::hash(FRONTEND_VERSION.as_bytes()).as_bytes()
}

/// A nonce pairs an index with the data file it was written against, so that restoring one of the
/// two from a backup, or deleting one, is detected rather than read as though the offsets still
/// meant something.
pub(crate) fn fresh_nonce() -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(
        &std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    let local = 0u8;
    hasher.update(&(std::ptr::addr_of!(local) as usize).to_le_bytes());
    let mut out = [0u8; 8];
    out.copy_from_slice(&hasher.finalize().as_bytes()[..8]);
    u64::from_le_bytes(out)
}

#[derive(Clone, Copy, Default)]
struct Section {
    start: usize,
    count: usize,
}

/// The index, held as the bytes that were read.
pub(crate) struct Index {
    bytes: Vec<u8>,
    nonce: u64,
    data_len: u64,
    defs: Section,
    decls: Section,
    bodies: Section,
    sources: Section,
    paths: Section,
}

impl Index {
    pub(crate) fn empty() -> Index {
        Index {
            bytes: Vec::new(),
            nonce: 0,
            data_len: DATA_HEADER,
            defs: Section::default(),
            decls: Section::default(),
            bodies: Section::default(),
            sources: Section::default(),
            paths: Section::default(),
        }
    }

    pub(crate) fn nonce(&self) -> u64 {
        self.nonce
    }

    pub(crate) fn data_len(&self) -> u64 {
        self.data_len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.defs.count == 0
            && self.decls.count == 0
            && self.bodies.count == 0
            && self.sources.count == 0
    }

    fn section(&self, kind: u8) -> Section {
        match kind {
            KIND_DEF => self.defs,
            KIND_DECL => self.decls,
            _ => self.bodies,
        }
    }

    fn slot(&self, section: Section, i: usize) -> HashSlot {
        let at = section.start + i * HASH_RECORD;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&self.bytes[at..at + 32]);
        HashSlot {
            hash: DefHash(hash),
            at: Located {
                offset: u64_at(&self.bytes, at + 32),
                len: u32_at(&self.bytes, at + 40),
            },
        }
    }

    /// Every slot filed under a hash, in index order.
    pub(crate) fn slots(&self, kind: u8, hash: DefHash) -> Vec<HashSlot> {
        let section = self.section(kind);
        if section.count == 0 {
            return Vec::new();
        }
        let (mut lo, mut hi) = (0usize, section.count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.slot(section, mid).hash < hash {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut out = Vec::new();
        while lo < section.count {
            let slot = self.slot(section, lo);
            if slot.hash != hash {
                break;
            }
            out.push(slot);
            lo += 1;
        }
        out
    }

    pub(crate) fn all_slots(&self, kind: u8) -> impl Iterator<Item = HashSlot> + '_ {
        let section = self.section(kind);
        (0..section.count).map(move |i| self.slot(section, i))
    }

    fn source(&self, i: usize) -> (&str, Located) {
        let at = self.sources.start + i * SOURCE_RECORD;
        let path_off = u32_at(&self.bytes, at) as usize;
        let path_len = u32_at(&self.bytes, at + 4) as usize;
        let start = self.paths.start + path_off;
        let path = std::str::from_utf8(&self.bytes[start..start + path_len]).unwrap_or("");
        (
            path,
            Located {
                offset: u64_at(&self.bytes, at + 8),
                len: u32_at(&self.bytes, at + 16),
            },
        )
    }

    pub(crate) fn sources(&self) -> impl Iterator<Item = (&str, Located)> + '_ {
        (0..self.sources.count).map(move |i| self.source(i))
    }

    pub(crate) fn find_source(&self, key: &str) -> Option<Located> {
        let (mut lo, mut hi) = (0usize, self.sources.count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.source(mid).0.cmp(key) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Equal => return Some(self.source(mid).1),
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }

    /// Every byte of the data file some index record names.
    pub(crate) fn live_bytes(&self) -> u64 {
        let frames = self.defs.count + self.decls.count + self.bodies.count + self.sources.count;
        let mut total = FRAME_HEADER * frames as u64;
        for kind in [KIND_DEF, KIND_DECL, KIND_BODY] {
            total += self.all_slots(kind).map(|s| s.at.len as u64).sum::<u64>();
        }
        total += self.sources().map(|(_, at)| at.len as u64).sum::<u64>();
        total
    }
}

/// Reads and fully validates the index.
pub(crate) fn read_index(path: &Path, schema: ContentHash) -> Result<Index, CacheError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(CacheError::Missing),
        Err(e) => return Err(CacheError::Io(e)),
    };
    if bytes.len() < INDEX_HEADER {
        return corrupt("the index is shorter than its header");
    }
    if &bytes[0..8] != IDX_MAGIC {
        return corrupt("the index does not begin with its magic number");
    }
    let format = u32_at(&bytes, 8);
    if format != FRONTEND_FORMAT {
        return Err(CacheError::Format(format));
    }
    if bytes[16..48] != schema.0 {
        return Err(CacheError::Schema);
    }
    if bytes[64..96] != version_hash() {
        return Err(CacheError::Version);
    }
    if *blake3::hash(&bytes[INDEX_HEADER..]).as_bytes() != bytes[100..132] {
        return corrupt("the index checksum does not match its contents");
    }

    let nonce = u64_at(&bytes, 48);
    let data_len = u64_at(&bytes, 56);
    if data_len < DATA_HEADER {
        return corrupt("the index claims a data file shorter than one header");
    }

    let count = u32_at(&bytes, 96) as usize;
    let table_end = INDEX_HEADER
        .checked_add(count.saturating_mul(DESCRIPTOR))
        .filter(|end| *end <= bytes.len());
    let Some(table_end) = table_end else {
        return corrupt("the section table runs past the end of the index");
    };

    let mut index = Index::empty();
    index.nonce = nonce;
    index.data_len = data_len;
    for i in 0..count {
        let at = INDEX_HEADER + i * DESCRIPTOR;
        let kind = u32_at(&bytes, at);
        let records = u32_at(&bytes, at + 4) as usize;
        let offset = u64_at(&bytes, at + 8) as usize;
        let length = u64_at(&bytes, at + 16) as usize;
        if offset < table_end || offset.saturating_add(length) > bytes.len() {
            return corrupt("a section lies outside the index");
        }
        let width = match kind {
            SECTION_DEFS | SECTION_DECLS | SECTION_BODIES => HASH_RECORD,
            SECTION_SOURCES => SOURCE_RECORD,
            SECTION_PATHS => 1,
            _ => return corrupt("the index carries a section this build does not know"),
        };
        if records.saturating_mul(width) != length {
            return corrupt("a section's record count does not match its length");
        }
        let section = Section {
            start: offset,
            count: records,
        };
        match kind {
            SECTION_DEFS => index.defs = section,
            SECTION_DECLS => index.decls = section,
            SECTION_BODIES => index.bodies = section,
            SECTION_SOURCES => index.sources = section,
            _ => index.paths = section,
        }
    }
    index.bytes = bytes;

    for kind in [KIND_DEF, KIND_DECL, KIND_BODY] {
        if index
            .all_slots(kind)
            .any(|slot| slot.at.end() > data_len || slot.at.offset < DATA_HEADER)
        {
            return corrupt("an index entry points past the end of the data file");
        }
    }
    for i in 0..index.sources.count {
        let at = index.sources.start + i * SOURCE_RECORD;
        let path_off = u32_at(&index.bytes, at) as usize;
        let path_len = u32_at(&index.bytes, at + 4) as usize;
        if path_off.saturating_add(path_len) > index.paths.count {
            return corrupt("a source record names a path outside the path blob");
        }
        let start = index.paths.start + path_off;
        if std::str::from_utf8(&index.bytes[start..start + path_len]).is_err() {
            return corrupt("a source record's path is not valid UTF-8");
        }
        let located = Located {
            offset: u64_at(&index.bytes, at + 8),
            len: u32_at(&index.bytes, at + 16),
        };
        if located.end() > data_len || located.offset < DATA_HEADER {
            return corrupt("an index entry points past the end of the data file");
        }
    }

    Ok(index)
}

/// The mapped data file, cut to exactly the length the index vouches for.
pub(crate) struct Data {
    map: Option<memmap2::Mmap>,
    len: u64,
}

impl Data {
    pub(crate) fn empty() -> Data {
        Data {
            map: None,
            len: DATA_HEADER,
        }
    }

    pub(crate) fn open(
        path: &Path,
        nonce: u64,
        data_len: u64,
        schema: ContentHash,
    ) -> Result<Data, CacheError> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(CacheError::Missing),
            Err(e) => return Err(CacheError::Io(e)),
        };
        let on_disk = file.metadata().map_err(CacheError::Io)?.len();
        if on_disk < data_len {
            return Err(CacheError::Unpaired);
        }
        let map = unsafe {
            memmap2::MmapOptions::new()
                .len(data_len as usize)
                .map(&file)
                .map_err(CacheError::Io)?
        };
        if &map[0..8] != DAT_MAGIC {
            return corrupt("the data file does not begin with its magic number");
        }
        let format = u32_at(&map, 8);
        if format != FRONTEND_FORMAT {
            return Err(CacheError::Format(format));
        }
        if map[16..48] != schema.0 {
            return Err(CacheError::Schema);
        }
        if u64_at(&map, 48) != nonce {
            return Err(CacheError::Unpaired);
        }
        Ok(Data {
            map: Some(map),
            len: data_len,
        })
    }

    /// The payload of the frame an index record claims, or the reason it cannot be believed.
    pub(crate) fn frame(&self, at: Located, kind: u8) -> Result<&[u8], &'static str> {
        let Some(map) = self.map.as_ref() else {
            return Err("the front-end cache has no data file");
        };
        if at.end() > self.len || at.offset < DATA_HEADER {
            return Err("an index entry points past the end of the data file");
        }
        let start = at.offset as usize;
        let header = &map[start..start + FRAME_HEADER as usize];
        if header[0] != kind {
            return Err("a cache entry is filed under a different kind than it was written as");
        }
        if u32_at(header, 1) != at.len {
            return Err("a cache entry's length does not match the index");
        }
        let payload = &map[start + FRAME_HEADER as usize..at.end() as usize];
        if u64_at(header, 5) != checksum(kind, at.len, payload) {
            return Err("a cache entry's checksum does not match its contents");
        }
        Ok(payload)
    }
}

fn checksum(kind: u8, len: u32, payload: &[u8]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[kind]);
    hasher.update(&len.to_le_bytes());
    hasher.update(payload);
    let mut out = [0u8; 8];
    out.copy_from_slice(&hasher.finalize().as_bytes()[..8]);
    u64::from_le_bytes(out)
}

fn data_header(nonce: u64, schema: ContentHash) -> [u8; DATA_HEADER as usize] {
    let mut header = [0u8; DATA_HEADER as usize];
    header[0..8].copy_from_slice(DAT_MAGIC);
    header[8..12].copy_from_slice(&FRONTEND_FORMAT.to_le_bytes());
    header[16..48].copy_from_slice(&schema.0);
    header[48..56].copy_from_slice(&nonce.to_le_bytes());
    header
}

/// The append half of a flush.
pub(crate) struct Appender {
    file: File,
    at: u64,
}

impl Appender {
    /// Truncates to the length the index vouches for, which is how a torn tail left by a killed
    /// writer is recovered: no indexed entry ever lies above it, so nothing indexed is lost.
    pub(crate) fn open(path: &Path, data_len: u64) -> std::io::Result<Appender> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.set_len(data_len)?;
        let mut file = file;
        file.seek(SeekFrom::Start(data_len))?;
        Ok(Appender { file, at: data_len })
    }

    pub(crate) fn create(
        path: &Path,
        nonce: u64,
        schema: ContentHash,
    ) -> std::io::Result<Appender> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.write_all(&data_header(nonce, schema))?;
        Ok(Appender {
            file,
            at: DATA_HEADER,
        })
    }

    pub(crate) fn append(&mut self, kind: u8, payload: &[u8]) -> std::io::Result<Located> {
        let len = u32::try_from(payload.len()).map_err(|_| {
            std::io::Error::new(
                ErrorKind::InvalidInput,
                "a cache entry is larger than 4 GiB",
            )
        })?;
        let mut header = [0u8; FRAME_HEADER as usize];
        header[0] = kind;
        header[1..5].copy_from_slice(&len.to_le_bytes());
        header[5..13].copy_from_slice(&checksum(kind, len, payload).to_le_bytes());
        self.file.write_all(&header)?;
        self.file.write_all(payload)?;
        let at = Located {
            offset: self.at,
            len,
        };
        self.at = at.end();
        Ok(at)
    }

    pub(crate) fn len(&self) -> u64 {
        self.at
    }

    pub(crate) fn sync(&self) -> std::io::Result<()> {
        self.file.sync_all()
    }
}

/// Everything the index names, ready to be written.
#[derive(Default)]
pub(crate) struct Directory {
    pub(crate) defs: Vec<HashSlot>,
    pub(crate) decls: Vec<HashSlot>,
    pub(crate) bodies: Vec<HashSlot>,
    pub(crate) sources: Vec<(String, Located)>,
}

pub(crate) fn write_index(
    dir: &Path,
    path: &Path,
    nonce: u64,
    data_len: u64,
    directory: &mut Directory,
    schema: ContentHash,
) -> anyhow::Result<()> {
    directory
        .defs
        .sort_by(|a, b| a.hash.cmp(&b.hash).then(a.at.offset.cmp(&b.at.offset)));
    directory
        .decls
        .sort_by(|a, b| a.hash.cmp(&b.hash).then(a.at.offset.cmp(&b.at.offset)));
    directory.bodies.sort_by(|a, b| a.hash.cmp(&b.hash));
    directory.sources.sort_by(|a, b| a.0.cmp(&b.0));

    let mut paths = Vec::new();
    let mut source_records = Vec::with_capacity(directory.sources.len() * SOURCE_RECORD);
    for (path, at) in &directory.sources {
        let offset = paths.len() as u32;
        paths.extend_from_slice(path.as_bytes());
        source_records.extend_from_slice(&offset.to_le_bytes());
        source_records.extend_from_slice(&(path.len() as u32).to_le_bytes());
        source_records.extend_from_slice(&at.offset.to_le_bytes());
        source_records.extend_from_slice(&at.len.to_le_bytes());
        source_records.extend_from_slice(&0u32.to_le_bytes());
    }

    let hash_section = |slots: &[HashSlot]| {
        let mut out = Vec::with_capacity(slots.len() * HASH_RECORD);
        for slot in slots {
            out.extend_from_slice(&slot.hash.0);
            out.extend_from_slice(&slot.at.offset.to_le_bytes());
            out.extend_from_slice(&slot.at.len.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out
    };

    let sections: Vec<(u32, usize, Vec<u8>)> = vec![
        (
            SECTION_DEFS,
            directory.defs.len(),
            hash_section(&directory.defs),
        ),
        (
            SECTION_DECLS,
            directory.decls.len(),
            hash_section(&directory.decls),
        ),
        (
            SECTION_BODIES,
            directory.bodies.len(),
            hash_section(&directory.bodies),
        ),
        (SECTION_SOURCES, directory.sources.len(), source_records),
        (SECTION_PATHS, paths.len(), paths),
    ];

    let table = INDEX_HEADER + sections.len() * DESCRIPTOR;
    let mut descriptors = Vec::with_capacity(sections.len() * DESCRIPTOR);
    let mut payloads = Vec::new();
    for (kind, count, bytes) in &sections {
        descriptors.extend_from_slice(&kind.to_le_bytes());
        descriptors.extend_from_slice(&(*count as u32).to_le_bytes());
        descriptors.extend_from_slice(&((table + payloads.len()) as u64).to_le_bytes());
        descriptors.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        payloads.extend_from_slice(bytes);
    }

    let mut out = Vec::with_capacity(table + payloads.len());
    out.extend_from_slice(IDX_MAGIC);
    out.extend_from_slice(&FRONTEND_FORMAT.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&schema.0);
    out.extend_from_slice(&nonce.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&version_hash());
    out.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 32]);
    out.extend_from_slice(&descriptors);
    out.extend_from_slice(&payloads);

    let checksum = *blake3::hash(&out[INDEX_HEADER..]).as_bytes();
    out[100..132].copy_from_slice(&checksum);

    disk::write_atomic(
        dir,
        path,
        crate::frontend::FRONTEND_STEM,
        &out,
        "front-end cache",
    )
}

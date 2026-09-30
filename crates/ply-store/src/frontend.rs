//! The front-end cache: `path -> SourceFingerprint`, and `(DefHash, name) -> Slot` for the values
//! the front end files and reads back, which this crate keeps without interpreting.

use crate::codec;
use crate::idx::{
    self, Appender, CacheError, DATA_HEADER, Data, Directory, HashSlot, Index, KIND_BODY,
    KIND_DECL, KIND_DEF, KIND_SOURCE, Located,
};
use crate::{ContentHash, DefBody, Pruned, disk};
use ply_span::{Diagnostic, SourceId, Span, Symbol};
use ply_ty::DefHash;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) const FRONTEND_FILE: &str = idx::INDEX_FILE;
pub(crate) const FRONTEND_DATA_FILE: &str = idx::DATA_FILE;

/// Temp-file prefix, so an abandoned flush's file is swept and nothing else is.
pub(crate) const FRONTEND_STEM: &str = "frontend";

/// A byte range within one source file: what a span degrades to outside the process.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileSpan {
    pub start: u32,
    pub end: u32,
}

impl FileSpan {
    /// A dummy span degrades to the empty range, which rebases onto a real offset 0.
    pub fn of(span: Span) -> FileSpan {
        if span.is_dummy() {
            FileSpan { start: 0, end: 0 }
        } else {
            FileSpan {
                start: span.start,
                end: span.end,
            }
        }
    }

    pub fn rebase(self, source: SourceId) -> Span {
        Span::new(source, self.start, self.end)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DefKind {
    Fn,
    Type,
    Effect,
}

/// A variant of a `type`, or an operation of an `effect`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Member {
    pub name: Symbol,
    pub span: FileSpan,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DefEntry {
    pub name: Symbol,
    pub hash: DefHash,
    pub span: FileSpan,
    pub kind: DefKind,
    /// Empty for a `fn`.
    pub members: Vec<Member>,
}

/// One `test` a file declared. `row` is what the front end filed for it, which it reads back and
/// this crate does not.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TestEntry {
    pub name: String,
    pub hash: DefHash,
    pub nondet: bool,
    pub span: FileSpan,
    pub row: Vec<u8>,
}

/// What one file declared when it was last checked, and the bytes it was checked as.
///
/// `module` is the program-wide name the file was checked under, which is not derivable from the
/// file: a dependency's module is named under the prefix its manifest grants, and what grants it
/// is the importing project's manifest closure.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SourceFingerprint {
    pub content_hash: ContentHash,
    pub module: String,
    pub defs: Vec<DefEntry>,
    pub tests: Vec<TestEntry>,
}

impl SourceFingerprint {
    pub fn new(content_hash: ContentHash) -> SourceFingerprint {
        SourceFingerprint {
            content_hash,
            module: String::new(),
            defs: Vec::new(),
            tests: Vec::new(),
        }
    }

    pub fn matches_bytes(&self, bytes: &[u8]) -> bool {
        self.content_hash == ContentHash::of(bytes)
    }

    pub fn referenced_hashes(&self) -> impl Iterator<Item = DefHash> + '_ {
        self.defs.iter().map(|d| d.hash)
    }
}

/// What the front end filed under a hash for one name. Two definitions may share a hash, so a
/// slot is found by both.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Slot {
    pub name: Symbol,
    pub value: Vec<u8>,
}

struct Staged<T> {
    bytes: Vec<u8>,
    value: Arc<T>,
    /// Replaces an index record for the same slot, so counting must not count both.
    supersedes: bool,
}

type SlotKey = (DefHash, Symbol);

#[derive(Default)]
struct Pending {
    defs: BTreeMap<SlotKey, Staged<Slot>>,
    decls: BTreeMap<SlotKey, Staged<Slot>>,
    bodies: BTreeMap<DefHash, Staged<DefBody>>,
    sources: BTreeMap<String, Staged<SourceFingerprint>>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.defs.is_empty()
            && self.decls.is_empty()
            && self.bodies.is_empty()
            && self.sources.is_empty()
    }

    fn slots(&self, kind: u8) -> &BTreeMap<SlotKey, Staged<Slot>> {
        if kind == KIND_DEF {
            &self.defs
        } else {
            &self.decls
        }
    }

    fn slots_mut(&mut self, kind: u8) -> &mut BTreeMap<SlotKey, Staged<Slot>> {
        if kind == KIND_DEF {
            &mut self.defs
        } else {
            &mut self.decls
        }
    }
}

/// Entries decoded during this run, keyed by where their frame lies.
#[derive(Default)]
struct Memo {
    slots: BTreeMap<u64, Arc<Slot>>,
    bodies: BTreeMap<u64, Arc<DefBody>>,
    sources: BTreeMap<u64, Arc<SourceFingerprint>>,
    names: BTreeMap<u64, Symbol>,
}

/// What a `prune` decided, held until a flush can act on it.
#[derive(Clone, Default)]
struct Retained {
    sources: Option<BTreeSet<String>>,
    hashes: Option<BTreeSet<DefHash>>,
}

impl Retained {
    fn is_empty(&self) -> bool {
        self.sources.is_none() && self.hashes.is_none()
    }

    fn source(&self, key: &str) -> bool {
        self.sources.as_ref().is_none_or(|keep| keep.contains(key))
    }

    fn hash(&self, hash: DefHash) -> bool {
        self.hashes.as_ref().is_none_or(|keep| keep.contains(&hash))
    }
}

trait Cached: Sized {
    fn decode(bytes: &[u8]) -> crate::binary::Decoded<Self>;
    fn memo(memo: &mut Memo) -> &mut BTreeMap<u64, Arc<Self>>;
}

impl Cached for Slot {
    fn decode(bytes: &[u8]) -> crate::binary::Decoded<Self> {
        codec::decode_slot(bytes)
    }
    fn memo(memo: &mut Memo) -> &mut BTreeMap<u64, Arc<Self>> {
        &mut memo.slots
    }
}

impl Cached for DefBody {
    fn decode(bytes: &[u8]) -> crate::binary::Decoded<Self> {
        codec::decode_body(bytes)
    }
    fn memo(memo: &mut Memo) -> &mut BTreeMap<u64, Arc<Self>> {
        &mut memo.bodies
    }
}

impl Cached for SourceFingerprint {
    fn decode(bytes: &[u8]) -> crate::binary::Decoded<Self> {
        codec::decode_fingerprint(bytes)
    }
    fn memo(memo: &mut Memo) -> &mut BTreeMap<u64, Arc<Self>> {
        &mut memo.sources
    }
}

pub(crate) enum StoredBody {
    Added,
    Unchanged,
    Conflict,
}

pub(crate) struct Frontend {
    index: Index,
    data: Data,
    pending: Pending,
    retained: Retained,
    schema: ContentHash,
    memo: Mutex<Memo>,
    /// Behind a lock because a read that finds a bad frame fills it.
    warnings: Mutex<Vec<Diagnostic>>,
}

impl Default for Frontend {
    fn default() -> Frontend {
        Frontend {
            index: Index::empty(),
            data: Data::empty(),
            pending: Pending::default(),
            retained: Retained::default(),
            schema: crate::schema_fingerprint(),
            memo: Mutex::new(Memo::default()),
            warnings: Mutex::new(Vec::new()),
        }
    }
}

fn guard<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|e| e.into_inner())
}

fn frame_slot_name(data: &Data, at: Located, kind: u8) -> Option<Symbol> {
    codec::peek_slot_name(data.frame(at, kind).ok()?).ok()
}

impl Frontend {
    pub(crate) fn open(index_path: &Path, data_path: &Path) -> (Frontend, Vec<Diagnostic>) {
        let mut frontend = Frontend::default();
        let schema = frontend.schema;
        let mut warnings = Vec::new();
        match idx::read_index(index_path, schema) {
            Ok(index) => match Data::open(data_path, index.nonce(), index.data_len(), schema) {
                Ok(data) => {
                    frontend.index = index;
                    frontend.data = data;
                }
                Err(CacheError::Missing) if index.is_empty() => {}
                // Reported against the index, the file a reader knows the cache by.
                Err(CacheError::Missing) => {
                    warnings.push(CacheError::Unpaired.into_diagnostic(index_path))
                }
                Err(e) => warnings.push(e.into_diagnostic(index_path)),
            },
            Err(CacheError::Missing) => {}
            Err(e) => warnings.push(e.into_diagnostic(index_path)),
        }
        (frontend, warnings)
    }

    pub(crate) fn take_warnings(&self) -> Vec<Diagnostic> {
        std::mem::take(&mut guard(&self.warnings))
    }

    pub(crate) fn warnings(&self) -> Vec<Diagnostic> {
        guard(&self.warnings).clone()
    }

    /// One warning per distinct degradation, however often a run consults the entry.
    fn refuse(&self, what: &str) {
        let message = format!("the front-end cache is corrupt: {what}");
        let mut warnings = guard(&self.warnings);
        if warnings.iter().any(|w| w.message == message) {
            return;
        }
        warnings.push(
            Diagnostic::warning(crate::codes::CACHE_CORRUPT, message).note(
                "that entry is treated as absent; whatever needed it is recomputed, and \
                 `ply cache compact` rewrites the data file",
            ),
        );
    }

    fn decode_at<T: Cached>(&self, at: Located, kind: u8) -> Option<Arc<T>> {
        if let Some(found) = T::memo(&mut guard(&self.memo)).get(&at.offset) {
            return Some(found.clone());
        }
        let payload = match self.data.frame(at, kind) {
            Ok(payload) => payload,
            Err(why) => {
                self.refuse(why);
                return None;
            }
        };
        let value = match T::decode(payload) {
            Ok(value) => Arc::new(value),
            Err(e) => {
                self.refuse(&e.to_string());
                return None;
            }
        };
        T::memo(&mut guard(&self.memo)).insert(at.offset, value.clone());
        Some(value)
    }

    fn slot_name(&self, at: Located, kind: u8) -> Option<Symbol> {
        if let Some(found) = guard(&self.memo).names.get(&at.offset) {
            return Some(found.clone());
        }
        let name = frame_slot_name(&self.data, at, kind)?;
        guard(&self.memo).names.insert(at.offset, name.clone());
        Some(name)
    }

    /// The slot filed under `hash` for `name`, a pending one first. Every slot under the hash is
    /// decoded, so one that no longer reads is reported whichever name it was filed for.
    fn slot(&self, kind: u8, hash: DefHash, name: &Symbol) -> Option<Arc<Slot>> {
        if !self.retained.hash(hash) {
            return None;
        }
        if let Some(staged) = self.pending.slots(kind).get(&(hash, name.clone())) {
            return Some(staged.value.clone());
        }
        self.index
            .slots(kind, hash)
            .into_iter()
            .filter_map(|slot| self.decode_at::<Slot>(slot.at, kind))
            .find(|slot| slot.name == *name)
    }

    fn put_slot(&mut self, kind: u8, hash: DefHash, slot: Slot) -> bool {
        let bytes = codec::encode_slot(&slot);
        let key = (hash, slot.name.clone());

        if let Some(staged) = self.pending.slots(kind).get(&key) {
            if staged.bytes == bytes {
                return false;
            }
            let supersedes = staged.supersedes;
            self.pending.slots_mut(kind).insert(
                key,
                Staged {
                    bytes,
                    value: Arc::new(slot),
                    supersedes,
                },
            );
            return true;
        }

        let mut supersedes = false;
        for stored in self.index.slots(kind, hash) {
            if self.slot_name(stored.at, kind).as_ref() != Some(&key.1) {
                continue;
            }
            supersedes = true;
            if self.data.frame(stored.at, kind).is_ok_and(|p| p == bytes) {
                return false;
            }
            break;
        }
        if let Some(keep) = self.retained.hashes.as_mut() {
            keep.insert(hash);
        }
        self.pending.slots_mut(kind).insert(
            key,
            Staged {
                bytes,
                value: Arc::new(slot),
                supersedes,
            },
        );
        true
    }

    fn slots_len(&self, kind: u8) -> usize {
        let stored = self
            .index
            .all_slots(kind)
            .filter(|slot| self.retained.hash(slot.hash))
            .count();
        let staged = self
            .pending
            .slots(kind)
            .iter()
            .filter(|((hash, _), entry)| self.retained.hash(*hash) && !entry.supersedes)
            .count();
        stored + staged
    }

    pub(crate) fn def_of(&self, hash: DefHash, name: &Symbol) -> Option<Arc<Slot>> {
        self.slot(KIND_DEF, hash, name)
    }

    pub(crate) fn put_def(&mut self, hash: DefHash, slot: Slot) -> bool {
        self.put_slot(KIND_DEF, hash, slot)
    }

    pub(crate) fn defs_len(&self) -> usize {
        self.slots_len(KIND_DEF)
    }

    pub(crate) fn decl_of(&self, hash: DefHash, name: &Symbol) -> Option<Arc<Slot>> {
        self.slot(KIND_DECL, hash, name)
    }

    pub(crate) fn put_decl(&mut self, hash: DefHash, slot: Slot) -> bool {
        self.put_slot(KIND_DECL, hash, slot)
    }

    pub(crate) fn decls_len(&self) -> usize {
        self.slots_len(KIND_DECL)
    }

    pub(crate) fn body(&self, hash: DefHash) -> Option<Arc<DefBody>> {
        if !self.retained.hash(hash) {
            return None;
        }
        if let Some(staged) = self.pending.bodies.get(&hash) {
            return Some(staged.value.clone());
        }
        let slot = self.index.slots(KIND_BODY, hash).into_iter().next()?;
        self.decode_at::<DefBody>(slot.at, KIND_BODY)
    }

    pub(crate) fn put_body(&mut self, hash: DefHash, body: DefBody) -> StoredBody {
        let bytes = codec::encode_body(&body);
        if let Some(staged) = self.pending.bodies.get(&hash) {
            return if staged.bytes == bytes {
                StoredBody::Unchanged
            } else {
                StoredBody::Conflict
            };
        }
        if let Some(slot) = self.index.slots(KIND_BODY, hash).into_iter().next() {
            return match self.data.frame(slot.at, KIND_BODY) {
                Ok(payload) if payload == bytes => StoredBody::Unchanged,
                Ok(_) => StoredBody::Conflict,
                Err(why) => {
                    self.refuse(why);
                    StoredBody::Conflict
                }
            };
        }
        if let Some(keep) = self.retained.hashes.as_mut() {
            keep.insert(hash);
        }
        self.pending.bodies.insert(
            hash,
            Staged {
                bytes,
                value: Arc::new(body),
                supersedes: false,
            },
        );
        StoredBody::Added
    }

    pub(crate) fn bodies_len(&self) -> usize {
        let stored = self
            .index
            .all_slots(KIND_BODY)
            .filter(|slot| self.retained.hash(slot.hash))
            .count();
        let staged = self
            .pending
            .bodies
            .keys()
            .filter(|hash| self.retained.hash(**hash))
            .count();
        stored + staged
    }

    pub(crate) fn fingerprint(&self, key: &str) -> Option<Arc<SourceFingerprint>> {
        if !self.retained.source(key) {
            return None;
        }
        if let Some(staged) = self.pending.sources.get(key) {
            return Some(staged.value.clone());
        }
        self.decode_at::<SourceFingerprint>(self.index.find_source(key)?, KIND_SOURCE)
    }

    pub(crate) fn put_source(&mut self, key: String, fingerprint: SourceFingerprint) -> bool {
        let bytes = codec::encode_fingerprint(&fingerprint);
        match self.pending.sources.get(&key) {
            Some(staged) if staged.bytes == bytes => return false,
            Some(_) => {}
            None => {
                let stored = self
                    .index
                    .find_source(&key)
                    .and_then(|at| self.data.frame(at, KIND_SOURCE).ok());
                if stored == Some(bytes.as_slice()) && self.retained.source(&key) {
                    return false;
                }
            }
        }
        if let Some(keep) = self.retained.sources.as_mut() {
            keep.insert(key.clone());
        }
        self.pending.sources.insert(
            key,
            Staged {
                bytes,
                value: Arc::new(fingerprint),
                supersedes: false,
            },
        );
        true
    }

    pub(crate) fn forget_source(&mut self, key: &str) -> bool {
        let keys = self.source_keys();
        if !keys.iter().any(|k| k == key) {
            return false;
        }
        self.pending.sources.remove(key);
        self.retained.sources = Some(keys.into_iter().filter(|k| k != key).collect());
        true
    }

    pub(crate) fn source_keys(&self) -> Vec<String> {
        let mut keys: BTreeSet<String> = self
            .index
            .sources()
            .filter(|(key, _)| self.retained.source(key))
            .map(|(key, _)| key.to_string())
            .collect();
        keys.extend(
            self.pending
                .sources
                .keys()
                .filter(|key| self.retained.source(key))
                .cloned(),
        );
        keys.into_iter().collect()
    }

    pub(crate) fn sources_len(&self) -> usize {
        self.source_keys().len()
    }

    pub(crate) fn sources(&self) -> Vec<(String, Arc<SourceFingerprint>)> {
        self.source_keys()
            .into_iter()
            .filter_map(|key| {
                let fingerprint = self.fingerprint(&key)?;
                Some((key, fingerprint))
            })
            .collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sources_len() == 0
            && self.defs_len() == 0
            && self.decls_len() == 0
            && self.bodies_len() == 0
    }

    pub(crate) fn is_dirty(&self) -> bool {
        !self.pending.is_empty() || !self.retained.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.index = Index::empty();
        self.data = Data::empty();
        self.pending = Pending::default();
        self.retained = Retained::default();
        *guard(&self.memo) = Memo::default();
        guard(&self.warnings).clear();
    }

    /// Data-file bytes no index record names: superseded records and whatever a prune left.
    pub(crate) fn garbage_bytes(&self) -> u64 {
        self.index
            .data_len()
            .saturating_sub(DATA_HEADER + self.index.live_bytes())
    }

    pub(crate) fn prune_would_change(&self, keep: &BTreeSet<String>) -> bool {
        let surviving = self
            .source_keys()
            .into_iter()
            .filter(|key| keep.contains(key))
            .count();
        surviving != self.sources_len()
            || !self.pending.sources.is_empty()
            || self.retained.hashes.is_some()
    }

    /// Drops fingerprints outside `keep`, and entries neither a survivor nor `roots` references.
    pub(crate) fn prune(&mut self, keep: &BTreeSet<String>, roots: &BTreeSet<DefHash>) -> Pruned {
        let before = self.counts();
        let surviving: BTreeSet<String> = self
            .source_keys()
            .into_iter()
            .filter(|key| keep.contains(key))
            .collect();

        if surviving.len() == before.sources
            && self.pending.sources.is_empty()
            && self.retained.hashes.is_none()
        {
            return Pruned::default();
        }

        let mut live: BTreeSet<DefHash> = roots.clone();
        for key in &surviving {
            if let Some(fingerprint) = self.fingerprint(key) {
                live.extend(fingerprint.referenced_hashes());
            }
        }

        let was = self.retained.clone();
        self.retained = Retained {
            sources: Some(surviving),
            hashes: Some(live),
        };
        let after = self.counts();
        let pruned = Pruned {
            sources: before.sources - after.sources,
            defs: before.defs - after.defs,
            decls: before.decls - after.decls,
            bodies: before.bodies - after.bodies,
        };
        // A no-op prune must leave the cache clean, or every unchanged run rewrites the index.
        if pruned == Pruned::default() {
            self.retained = was;
        }
        pruned
    }

    fn counts(&self) -> Pruned {
        Pruned {
            sources: self.sources_len(),
            defs: self.defs_len(),
            decls: self.decls_len(),
            bodies: self.bodies_len(),
        }
    }

    /// The index records of `kind` a flush keeps: the retained ones no pending slot replaces.
    fn kept_slots(&self, index: &Index, data: &Data, kind: u8) -> Vec<HashSlot> {
        let pending = self.pending.slots(kind);
        let pending_hashes: BTreeSet<DefHash> = pending.keys().map(|(h, _)| *h).collect();
        index
            .all_slots(kind)
            .filter(|slot| self.retained.hash(slot.hash))
            .filter(|slot| {
                !pending_hashes.contains(&slot.hash)
                    || frame_slot_name(data, slot.at, kind)
                        .is_none_or(|name| !pending.contains_key(&(slot.hash, name)))
            })
            .collect()
    }

    pub(crate) fn flush(
        &mut self,
        dir: &Path,
        index_path: &Path,
        data_path: &Path,
    ) -> anyhow::Result<()> {
        let schema = self.schema;
        // The on-disk index may be newer than the one mapped at open; its `data_len` and nonce win.
        let disk = idx::read_index(index_path, schema).ok().and_then(|index| {
            let (nonce, data_len) = (index.nonce(), index.data_len());
            Data::open(data_path, nonce, data_len, schema)
                .ok()
                .map(|data| (index, data, nonce, data_len))
        });

        let (index, data, nonce, mut appender) = match disk {
            Some((index, data, nonce, data_len)) => {
                let appender = Appender::open(data_path, data_len)?;
                (index, data, nonce, appender)
            }
            None => {
                let nonce = idx::fresh_nonce();
                let appender = Appender::create(data_path, nonce, schema)?;
                (Index::empty(), Data::empty(), nonce, appender)
            }
        };

        let mut directory = Directory {
            defs: self.kept_slots(&index, &data, KIND_DEF),
            decls: self.kept_slots(&index, &data, KIND_DECL),
            ..Directory::default()
        };
        let mut stored_bodies: BTreeSet<DefHash> = BTreeSet::new();
        for slot in index.all_slots(KIND_BODY) {
            if !self.retained.hash(slot.hash) {
                continue;
            }
            stored_bodies.insert(slot.hash);
            directory.bodies.push(slot);
        }
        for (key, at) in index.sources() {
            if !self.retained.source(key) || self.pending.sources.contains_key(key) {
                continue;
            }
            directory.sources.push((key.to_string(), at));
        }

        for ((hash, _), staged) in &self.pending.defs {
            if !self.retained.hash(*hash) {
                continue;
            }
            let at = appender.append(KIND_DEF, &staged.bytes)?;
            directory.defs.push(HashSlot { hash: *hash, at });
        }
        for ((hash, _), staged) in &self.pending.decls {
            if !self.retained.hash(*hash) {
                continue;
            }
            let at = appender.append(KIND_DECL, &staged.bytes)?;
            directory.decls.push(HashSlot { hash: *hash, at });
        }
        for (hash, staged) in &self.pending.bodies {
            if !self.retained.hash(*hash) || stored_bodies.contains(hash) {
                continue;
            }
            let at = appender.append(KIND_BODY, &staged.bytes)?;
            directory.bodies.push(HashSlot { hash: *hash, at });
        }
        for (key, staged) in &self.pending.sources {
            if !self.retained.source(key) {
                continue;
            }
            let at = appender.append(KIND_SOURCE, &staged.bytes)?;
            directory.sources.push((key.clone(), at));
        }

        appender.sync()?;
        let data_len = appender.len();
        drop(appender);
        idx::write_index(dir, index_path, nonce, data_len, &mut directory, schema)?;
        self.reload(index_path, data_path);
        Ok(())
    }

    /// Copies what the index names into a fresh data file; nothing else shrinks it.
    pub(crate) fn compact(
        &mut self,
        dir: &Path,
        index_path: &Path,
        data_path: &Path,
    ) -> anyhow::Result<()> {
        self.flush(dir, index_path, data_path)?;

        let nonce = idx::fresh_nonce();
        let temp = disk::temp_path(dir, FRONTEND_STEM);
        let outcome = self.rewrite(&temp, nonce, dir, index_path, data_path);
        if outcome.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        outcome?;
        self.reload(index_path, data_path);
        Ok(())
    }

    fn rewrite(
        &self,
        temp: &Path,
        nonce: u64,
        dir: &Path,
        index_path: &Path,
        data_path: &Path,
    ) -> anyhow::Result<()> {
        let mut fresh = Appender::create(temp, nonce, self.schema)?;
        let mut directory = Directory::default();
        for kind in [KIND_DEF, KIND_DECL, KIND_BODY] {
            for slot in self.index.all_slots(kind) {
                let Ok(payload) = self.data.frame(slot.at, kind) else {
                    self.refuse("an entry was dropped by compaction because it did not verify");
                    continue;
                };
                let at = fresh.append(kind, payload)?;
                let moved = HashSlot {
                    hash: slot.hash,
                    at,
                };
                match kind {
                    KIND_DEF => directory.defs.push(moved),
                    KIND_DECL => directory.decls.push(moved),
                    _ => directory.bodies.push(moved),
                }
            }
        }
        for (key, at) in self.index.sources() {
            let Ok(payload) = self.data.frame(at, KIND_SOURCE) else {
                self.refuse("an entry was dropped by compaction because it did not verify");
                continue;
            };
            let at = fresh.append(KIND_SOURCE, payload)?;
            directory.sources.push((key.to_string(), at));
        }
        fresh.sync()?;
        let data_len = fresh.len();
        drop(fresh);

        std::fs::rename(temp, data_path)?;
        if let Ok(handle) = std::fs::File::open(dir) {
            let _ = handle.sync_all();
        }
        idx::write_index(
            dir,
            index_path,
            nonce,
            data_len,
            &mut directory,
            self.schema,
        )
    }

    fn reload(&mut self, index_path: &Path, data_path: &Path) {
        self.pending = Pending::default();
        self.retained = Retained::default();
        *guard(&self.memo) = Memo::default();
        let (frontend, warnings) = Frontend::open(index_path, data_path);
        self.index = frontend.index;
        self.data = frontend.data;
        guard(&self.warnings).extend(warnings);
    }
}

/// The cache key for a source file: its path relative to the store root, with `/` separators.
pub(crate) fn source_key(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut parts: Vec<&str> = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(s) => parts.push(s.to_str()?),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// A key back under its root, spelled as the run that recorded it spelled the path. `source_key`
/// drops `.` on the way in, so the way out drops it too: `./m.ply` and `m.ply` are one path, but
/// only one of them is the string a report prints or a prefix test matches.
pub(crate) fn source_path(root: &Path, key: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for component in root.components() {
        if component != Component::CurDir {
            path.push(component);
        }
    }
    path.push(key);
    path
}

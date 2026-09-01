//! The front-end cache: `path -> SourceFingerprint` and `DefHash -> interface`.

use crate::canonical::{canonicalize_decl_body, canonicalize_scheme};
use crate::codec;
use crate::idx::{
    self, Appender, CacheError, DATA_HEADER, Data, Directory, HashSlot, Index, KIND_BODY,
    KIND_DECL, KIND_DEF, KIND_SOURCE, Located,
};
use crate::{ContentHash, DefBody, Pruned, disk};
use ply_core::{Footprint, Scheme, Type};
use ply_hash::DefHash;
use ply_span::{Diagnostic, SourceId, Span, Symbol};
use ply_syntax::ast::Mode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub(crate) const FRONTEND_FILE: &str = idx::INDEX_FILE;
pub(crate) const FRONTEND_DATA_FILE: &str = idx::DATA_FILE;

/// The prefix a flush's temp files carry, so that an abandoned one is swept without touching
/// anything else in the cache directory.
pub(crate) const FRONTEND_STEM: &str = "frontend";

/// A byte range within one source file, which is what a span degrades to once it leaves the
/// process.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FileSpan {
    pub start: u32,
    pub end: u32,
}

impl FileSpan {
    /// A dummy span has no file to be relative to, so it degrades to the empty range and rebases
    /// onto a real offset 0 rather than back to [`Span::DUMMY`].
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

/// A name paired with what it denoted; see [`witness_holds`].
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct NameRef {
    pub name: Symbol,
    pub hash: DefHash,
}

impl NameRef {
    pub fn new(name: impl Into<Symbol>, hash: DefHash) -> NameRef {
        NameRef {
            name: name.into(),
            hash,
        }
    }
}

/// A cached interface is written in terms of *names* — `Scheme` holds `Type::Con(Symbol, ..)` and a
/// `Footprint` holds effect labels — while a `DefHash` erases them, which is exactly what makes
/// renaming free.
pub fn witness_holds(
    names: &[NameRef],
    mut resolve: impl FnMut(&Symbol) -> Option<DefHash>,
) -> bool {
    names.iter().all(|n| resolve(&n.name) == Some(n.hash))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefKind {
    Fn,
    Type,
    Effect,
}

/// A variant of a `type`, or an operation of an `effect`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Member {
    pub name: Symbol,
    pub span: FileSpan,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DefEntry {
    pub name: Symbol,
    pub hash: DefHash,
    /// This definition's own form, with its references left as the names they
    /// were written as instead of normalized to what they denote. Editing a
    /// callee moves `hash` for every transitive caller and moves `own` for
    /// nobody but the callee, which is what lets a gate cut a recheck off.
    ///
    /// **Not an identity.** Two definitions calling differently-named functions
    /// of the same shape share an `own`, so nothing may key a cache on it.
    pub own: DefHash,
    /// Everything a caller can observe of this definition: its published
    /// scheme, its footprint, its published constraints. Signatures are
    /// written rather than inferred, so a callee's body edit that leaves this
    /// standing cannot change how a caller checks — but effect rows are still
    /// inferred, so a body that gains a `perform` moves it and does propagate.
    pub iface: DefHash,
    pub span: FileSpan,
    pub kind: DefKind,
    /// Empty for a `fn`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Member>,
    /// The names this definition mentions directly, in normalization order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<Symbol>,
    /// A `reuse fn`: gate 1 has to know without a parse, because the promise is checked
    /// whole-program and a module the gate skips can still hold one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reuse: bool,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CachedTest {
    pub name: String,
    pub hash: DefHash,
    pub nondet: bool,
    pub footprint: Footprint,
    pub span: FileSpan,
    pub name_span: FileSpan,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<Symbol>,
}

/// A module this file imports, with a digest of the exports it was compiled against.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ImportEdge {
    pub module: Symbol,
    pub exports: ContentHash,
}

/// `content_hash` is over the file's **raw bytes**, not over anything derived from parsing it: gate
/// 1 has to decide whether to parse before it has anything a parse would produce.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SourceFingerprint {
    pub content_hash: ContentHash,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<ImportEdge>,
    /// Every top-level name this file mentions but does not declare, and what it resolved to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<NameRef>,
    pub defs: Vec<DefEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<CachedTest>,
}

impl SourceFingerprint {
    pub fn new(content_hash: ContentHash) -> SourceFingerprint {
        SourceFingerprint {
            content_hash,
            imports: Vec::new(),
            deps: Vec::new(),
            defs: Vec::new(),
            tests: Vec::new(),
        }
    }

    /// Gate 1's first condition.
    pub fn matches_bytes(&self, bytes: &[u8]) -> bool {
        self.content_hash == ContentHash::of(bytes)
    }

    /// The `(name, hash)` pairs this file publishes, sorted — the input [`exports_digest`] is
    /// defined over.
    pub fn exports(&self) -> Vec<NameRef> {
        let mut out: Vec<NameRef> = self
            .defs
            .iter()
            .map(|d| NameRef {
                name: d.name.clone(),
                hash: d.hash,
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.hash.cmp(&b.hash)));
        out
    }

    /// Every hash this fingerprint refers to, so a garbage collector can tell a live interface from
    /// an abandoned one.
    pub fn referenced_hashes(&self) -> impl Iterator<Item = DefHash> + '_ {
        self.defs.iter().map(|d| d.hash)
    }
}

/// A stable digest over a module's exported `(name, hash)` pairs.
pub fn exports_digest(exports: &[NameRef]) -> ContentHash {
    let mut sorted: Vec<&NameRef> = exports.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name).then(a.hash.cmp(&b.hash)));
    sorted.dedup_by(|a, b| a.name == b.name && a.hash == b.hash);

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(sorted.len() as u32).to_le_bytes());
    for entry in sorted {
        let name = entry.name.as_str().as_bytes();
        bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&entry.hash.0);
    }
    ContentHash::of(&bytes)
}

/// The published interface of one `fn`, keyed by its [`DefHash`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CachedDef {
    pub scheme: Scheme,
    pub footprint: Footprint,
    /// What row inference computed for the body, which a declared row may be wider than.
    #[serde(default)]
    pub performed: Footprint,
    /// The `effect set` names the row was written with, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub row_aliases: Vec<Symbol>,
    /// See [`witness_holds`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<NameRef>,
}

impl CachedDef {
    /// `performed` starts equal to `footprint`, which is what it is for a definition with no
    /// annotation and the honest reading of "nothing narrower is known".
    pub fn new(scheme: Scheme, footprint: Footprint) -> CachedDef {
        CachedDef {
            scheme,
            performed: footprint.clone(),
            footprint,
            row_aliases: Vec::new(),
            names: Vec::new(),
        }
    }

    pub fn performing(mut self, performed: Footprint) -> CachedDef {
        self.performed = performed;
        self
    }

    pub fn written_as(mut self, row_aliases: Vec<Symbol>) -> CachedDef {
        self.row_aliases = row_aliases;
        self
    }

    pub fn witnessed_by(mut self, names: Vec<NameRef>) -> CachedDef {
        self.names = names;
        self
    }

    pub fn witness_holds(&self, resolve: impl FnMut(&Symbol) -> Option<DefHash>) -> bool {
        witness_holds(&self.names, resolve)
    }

    /// What [`crate::Store::put_def`] stores.
    pub fn canonicalized(self) -> CachedDef {
        CachedDef {
            scheme: canonicalize_scheme(&self.scheme),
            footprint: self.footprint,
            performed: self.performed,
            row_aliases: self.row_aliases,
            names: canonical_names(self.names),
        }
    }
}

/// A witness is a set, so two callers recording the same one in different orders must not produce
/// different bytes on disk.
fn canonical_names(mut names: Vec<NameRef>) -> Vec<NameRef> {
    names.sort_by(|a, b| a.name.cmp(&b.name).then(a.hash.cmp(&b.hash)));
    names.dedup();
    names
}

/// The published interface of one `type` or `effect`, keyed by its [`DefHash`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CachedDecl {
    pub body: DeclBody,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<NameRef>,
}

impl CachedDecl {
    pub fn new(body: DeclBody) -> CachedDecl {
        CachedDecl {
            body,
            names: Vec::new(),
        }
    }

    pub fn witnessed_by(mut self, names: Vec<NameRef>) -> CachedDecl {
        self.names = names;
        self
    }

    pub fn witness_holds(&self, resolve: impl FnMut(&Symbol) -> Option<DefHash>) -> bool {
        witness_holds(&self.names, resolve)
    }

    /// What [`crate::Store::put_decl`] stores.
    pub fn canonicalized(self) -> CachedDecl {
        CachedDecl {
            body: canonicalize_decl_body(&self.body),
            names: canonical_names(self.names),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "decl", rename_all = "snake_case")]
pub enum DeclBody {
    Type {
        /// Type parameters, by count: their names are binders and never escape.
        arity: usize,
        ctors: Vec<CachedCtor>,
    },
    Effect {
        nondet: bool,
        ops: Vec<CachedOp>,
    },
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CachedCtor {
    pub fields: Vec<Type>,
    pub scheme: Scheme,
}

/// The operation's name is stored beside its signature because normalization sorts an effect's
/// operations away — reordering them in source moves no `DefHash`, so a restore that paired them by
/// position would hand every operation its neighbour's mode and signature.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CachedOp {
    pub name: Symbol,
    pub mode: Mode,
    pub resource_param: bool,
    pub params: Vec<Type>,
    pub ret: Type,
}

/// The name an interface was written for: the one entry of its witness that names something at the
/// interface's own hash.
pub fn self_name(names: &[NameRef], hash: DefHash) -> Option<&Symbol> {
    names.iter().find(|n| n.hash == hash).map(|n| &n.name)
}

pub fn declares(names: &[NameRef], name: &Symbol, hash: DefHash) -> bool {
    match self_name(names, hash) {
        Some(found) => found == name,
        None => names.is_empty(),
    }
}

/// One entry as this run holds it: the bytes a flush would append, beside the value they decode to.
struct Staged<T> {
    bytes: Vec<u8>,
    value: Arc<T>,
    /// Whether an index record for the same slot is being replaced, so that counting entries does
    /// not count the old and the new one both.
    supersedes: bool,
}

/// A hash and the name the interface filed under it was written for.
type Slot = (DefHash, Option<Symbol>);

#[derive(Default)]
struct Pending {
    defs: BTreeMap<Slot, Staged<CachedDef>>,
    decls: BTreeMap<Slot, Staged<CachedDecl>>,
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
}

/// Entries decoded during this run, keyed by where their frame lies.
#[derive(Default)]
struct Memo {
    defs: BTreeMap<u64, Arc<CachedDef>>,
    decls: BTreeMap<u64, Arc<CachedDecl>>,
    bodies: BTreeMap<u64, Arc<DefBody>>,
    sources: BTreeMap<u64, Arc<SourceFingerprint>>,
    names: BTreeMap<u64, Arc<Vec<NameRef>>>,
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
    const KIND: u8;
    fn decode(bytes: &[u8]) -> crate::binary::Decoded<Self>;
    fn memo(memo: &mut Memo) -> &mut BTreeMap<u64, Arc<Self>>;
}

trait Interface: Cached + PartialEq {
    const TAG: u8;
    fn encode(&self) -> Vec<u8>;
    fn names(&self) -> &[NameRef];
    fn pending(pending: &Pending) -> &BTreeMap<Slot, Staged<Self>>;
    fn pending_mut(pending: &mut Pending) -> &mut BTreeMap<Slot, Staged<Self>>;
}

impl Cached for CachedDef {
    const KIND: u8 = KIND_DEF;
    fn decode(bytes: &[u8]) -> crate::binary::Decoded<Self> {
        codec::decode_def(bytes)
    }
    fn memo(memo: &mut Memo) -> &mut BTreeMap<u64, Arc<Self>> {
        &mut memo.defs
    }
}

impl Interface for CachedDef {
    const TAG: u8 = codec::DEF_TAG;
    fn encode(&self) -> Vec<u8> {
        codec::encode_def(self)
    }
    fn names(&self) -> &[NameRef] {
        &self.names
    }
    fn pending(pending: &Pending) -> &BTreeMap<Slot, Staged<Self>> {
        &pending.defs
    }
    fn pending_mut(pending: &mut Pending) -> &mut BTreeMap<Slot, Staged<Self>> {
        &mut pending.defs
    }
}

impl Cached for CachedDecl {
    const KIND: u8 = KIND_DECL;
    fn decode(bytes: &[u8]) -> crate::binary::Decoded<Self> {
        codec::decode_decl(bytes)
    }
    fn memo(memo: &mut Memo) -> &mut BTreeMap<u64, Arc<Self>> {
        &mut memo.decls
    }
}

impl Interface for CachedDecl {
    const TAG: u8 = codec::DECL_TAG;
    fn encode(&self) -> Vec<u8> {
        codec::encode_decl(self)
    }
    fn names(&self) -> &[NameRef] {
        &self.names
    }
    fn pending(pending: &Pending) -> &BTreeMap<Slot, Staged<Self>> {
        &pending.decls
    }
    fn pending_mut(pending: &mut Pending) -> &mut BTreeMap<Slot, Staged<Self>> {
        &mut pending.decls
    }
}

impl Cached for DefBody {
    const KIND: u8 = KIND_BODY;
    fn decode(bytes: &[u8]) -> crate::binary::Decoded<Self> {
        codec::decode_body(bytes)
    }
    fn memo(memo: &mut Memo) -> &mut BTreeMap<u64, Arc<Self>> {
        &mut memo.bodies
    }
}

impl Cached for SourceFingerprint {
    const KIND: u8 = KIND_SOURCE;
    fn decode(bytes: &[u8]) -> crate::binary::Decoded<Self> {
        codec::decode_fingerprint(bytes)
    }
    fn memo(memo: &mut Memo) -> &mut BTreeMap<u64, Arc<Self>> {
        &mut memo.sources
    }
}

/// What [`Frontend::put_body`] did, so that the conflict — which means a body's encoding depends on
/// something its key does not cover — is reported by the caller that holds the warning list.
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
    /// Filled by a *read* that found a frame it could not believe, which is why it is behind a lock
    /// rather than owned by the caller.
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

/// The name an index record's interface was written for, read from its witness without decoding the
/// scheme behind it.
fn frame_self_name(data: &Data, at: Located, kind: u8, tag: u8, hash: DefHash) -> Option<Symbol> {
    let payload = data.frame(at, kind).ok()?;
    let names = codec::peek_names(tag, payload).ok()?;
    self_name(&names, hash).cloned()
}

impl Frontend {
    /// Opens both files, or degrades to an empty cache and says why.
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
                // Named against the index either way: the two files are one cache, and the index is
                // the one a reader was told about.
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

    /// One warning per distinct degradation: a corrupt entry that a run consults a hundred times is
    /// one fact about the cache, not a hundred.
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

    fn decode_at<T: Cached>(&self, at: Located) -> Option<Arc<T>> {
        if let Some(found) = T::memo(&mut guard(&self.memo)).get(&at.offset) {
            return Some(found.clone());
        }
        let payload = match self.data.frame(at, T::KIND) {
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

    fn slot_name<T: Interface>(&self, at: Located, hash: DefHash) -> Option<Symbol> {
        if let Some(found) = guard(&self.memo).names.get(&at.offset) {
            return self_name(found, hash).cloned();
        }
        let payload = self.data.frame(at, T::KIND).ok()?;
        let names = codec::peek_names(T::TAG, payload).ok()?;
        let found = self_name(&names, hash).cloned();
        guard(&self.memo).names.insert(at.offset, Arc::new(names));
        found
    }

    /// Every interface stored under this hash, index order first.
    fn interfaces<T: Interface>(&self, hash: DefHash) -> Vec<Arc<T>> {
        let mut out = Vec::new();
        if !self.retained.hash(hash) {
            return out;
        }
        let staged = T::pending(&self.pending);
        for slot in self.index.slots(T::KIND, hash) {
            let name = self.slot_name::<T>(slot.at, hash);
            if staged.contains_key(&(hash, name)) {
                continue;
            }
            if let Some(entry) = self.decode_at::<T>(slot.at) {
                out.push(entry);
            }
        }
        for ((filed, _), entry) in staged.range((hash, None)..) {
            if *filed != hash {
                break;
            }
            out.push(entry.value.clone());
        }
        out
    }

    fn interface(&self, hash: DefHash) -> Option<Arc<CachedDef>> {
        self.interfaces::<CachedDef>(hash).into_iter().next()
    }

    fn put_interface<T: Interface>(&mut self, hash: DefHash, value: T) -> bool {
        let bytes = value.encode();
        let key = (hash, self_name(value.names(), hash).cloned());

        if let Some(staged) = T::pending(&self.pending).get(&key) {
            if staged.bytes == bytes {
                return false;
            }
            let supersedes = staged.supersedes;
            T::pending_mut(&mut self.pending).insert(
                key,
                Staged {
                    bytes,
                    value: Arc::new(value),
                    supersedes,
                },
            );
            return true;
        }

        let mut supersedes = false;
        for slot in self.index.slots(T::KIND, hash) {
            if self.slot_name::<T>(slot.at, hash) != key.1 {
                continue;
            }
            supersedes = true;
            if self.data.frame(slot.at, T::KIND).is_ok_and(|p| p == bytes) {
                return false;
            }
            break;
        }
        if let Some(keep) = self.retained.hashes.as_mut() {
            keep.insert(hash);
        }
        T::pending_mut(&mut self.pending).insert(
            key,
            Staged {
                bytes,
                value: Arc::new(value),
                supersedes,
            },
        );
        true
    }

    fn interfaces_len<T: Interface>(&self) -> usize {
        let stored = self
            .index
            .all_slots(T::KIND)
            .filter(|slot| self.retained.hash(slot.hash))
            .count();
        let staged = T::pending(&self.pending)
            .iter()
            .filter(|((hash, _), entry)| self.retained.hash(*hash) && !entry.supersedes)
            .count();
        stored + staged
    }

    pub(crate) fn def(&self, hash: DefHash) -> Option<Arc<CachedDef>> {
        self.interface(hash)
    }

    pub(crate) fn def_of(&self, hash: DefHash, name: &Symbol) -> Option<Arc<CachedDef>> {
        self.interfaces::<CachedDef>(hash)
            .into_iter()
            .find(|d| declares(&d.names, name, hash))
    }

    pub(crate) fn put_def(&mut self, hash: DefHash, def: CachedDef) -> bool {
        self.put_interface(hash, def.canonicalized())
    }

    pub(crate) fn defs_len(&self) -> usize {
        self.interfaces_len::<CachedDef>()
    }

    pub(crate) fn decl(&self, hash: DefHash) -> Option<Arc<CachedDecl>> {
        self.interfaces::<CachedDecl>(hash).into_iter().next()
    }

    pub(crate) fn decl_of(&self, hash: DefHash, name: &Symbol) -> Option<Arc<CachedDecl>> {
        self.interfaces::<CachedDecl>(hash)
            .into_iter()
            .find(|d| declares(&d.names, name, hash))
    }

    pub(crate) fn put_decl(&mut self, hash: DefHash, decl: CachedDecl) -> bool {
        self.put_interface(hash, decl.canonicalized())
    }

    pub(crate) fn decls_len(&self) -> usize {
        self.interfaces_len::<CachedDecl>()
    }

    pub(crate) fn body(&self, hash: DefHash) -> Option<Arc<DefBody>> {
        if !self.retained.hash(hash) {
            return None;
        }
        if let Some(staged) = self.pending.bodies.get(&hash) {
            return Some(staged.value.clone());
        }
        let slot = self.index.slots(KIND_BODY, hash).into_iter().next()?;
        self.decode_at::<DefBody>(slot.at)
    }

    /// A body is name-free, so it is a function of its hash and one hash has one body.
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
        self.decode_at::<SourceFingerprint>(self.index.find_source(key)?)
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

    /// What the data file holds that nothing in the index names: superseded records, and anything a
    /// prune left behind.
    pub(crate) fn garbage_bytes(&self) -> u64 {
        self.index
            .data_len()
            .saturating_sub(DATA_HEADER + self.index.live_bytes())
    }

    /// Drops every fingerprint outside `keep`, and every interface and body neither a surviving
    /// fingerprint nor `roots` declares.
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
        // A prune that drops nothing must leave the cache clean, or every run over an unchanged
        // project rewrites the index.
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

    /// Appends this run's entries and rewrites the index over them.
    pub(crate) fn flush(
        &mut self,
        dir: &Path,
        index_path: &Path,
        data_path: &Path,
    ) -> anyhow::Result<()> {
        let schema = self.schema;
        // The index on disk may be newer than the one mapped at open, and its `data_len` and nonce
        // are the authoritative ones.
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

        let mut directory = Directory::default();
        let def_hashes: BTreeSet<DefHash> = self.pending.defs.keys().map(|(h, _)| *h).collect();
        let decl_hashes: BTreeSet<DefHash> = self.pending.decls.keys().map(|(h, _)| *h).collect();

        for slot in index.all_slots(KIND_DEF) {
            if !self.retained.hash(slot.hash) {
                continue;
            }
            if def_hashes.contains(&slot.hash) {
                let name = frame_self_name(&data, slot.at, KIND_DEF, codec::DEF_TAG, slot.hash);
                if self.pending.defs.contains_key(&(slot.hash, name)) {
                    continue;
                }
            }
            directory.defs.push(slot);
        }
        for slot in index.all_slots(KIND_DECL) {
            if !self.retained.hash(slot.hash) {
                continue;
            }
            if decl_hashes.contains(&slot.hash) {
                let name = frame_self_name(&data, slot.at, KIND_DECL, codec::DECL_TAG, slot.hash);
                if self.pending.decls.contains_key(&(slot.hash, name)) {
                    continue;
                }
            }
            directory.decls.push(slot);
        }
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

    /// Copies what the index names into a fresh data file, which is the only thing that ever
    /// shrinks an append-only file.
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
    use std::path::Component;
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

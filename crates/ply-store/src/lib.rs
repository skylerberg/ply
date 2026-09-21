//! The `.ply-cache` directory: results keyed by `(RUNTIME_VERSION, DefHash)`, and the front end
//! keyed by `(FRONTEND_VERSION, path | DefHash)`, beside the parts of its last answer and of the
//! last claims it lowered.

mod answer;
mod binary;
pub mod body;
mod canonical;
pub mod codec;
pub mod diag;
pub mod disk;
pub mod frontend;
mod idx;
pub mod obligations;
pub mod reviews;
pub mod schema;
pub mod upstream;

use anyhow::Context;
use ply_span::{Diagnostic, Symbol};
use ply_ty::DefHash;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub use canonical::{canonicalize_decl_body, canonicalize_scheme};
pub use frontend::{
    CachedCtor, CachedDecl, CachedDef, CachedOp, CachedTest, DeclBody, DefEntry, DefKind, FileSpan,
    Member, NameRef, SourceFingerprint,
};
pub use obligations::{
    CachedCases, CachedCertificate, CachedEvidence, CachedObligation, CachedRule,
};
pub use reviews::ReviewRecord;
pub use schema::fingerprint as schema_fingerprint;
pub use upstream::Upstream;

/// Bumping this discards every cached result; a file from another runtime is never merged.
pub const RUNTIME_VERSION: &str = "0.15.0";

/// Bumping this discards every cached type, footprint, source fingerprint and front-end answer.
pub const FRONTEND_VERSION: &str = "0.25.0";

/// Bumping this re-attempts every obligation and re-runs no test.
pub const PROVER_VERSION: &str = "0.7.0";

pub const FRONTEND_FORMAT: u32 = 8;

pub const BODY_ENCODING: u32 = 7;

pub const CACHE_DIR_NAME: &str = ".ply-cache";

/// BLAKE3 over raw bytes: a source file's contents, or a digest of a module's exports.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub fn of(bytes: &[u8]) -> ContentHash {
        ContentHash(*blake3::hash(bytes).as_bytes())
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
            s.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'));
        }
        s
    }

    pub fn short(&self) -> String {
        self.to_hex()[..12].to_string()
    }

    pub fn from_hex(s: &str) -> Option<ContentHash> {
        if s.len() != 64 {
            return None;
        }
        let bytes = s.as_bytes();
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            let hi = (bytes[2 * i] as char).to_digit(16)?;
            let lo = (bytes[2 * i + 1] as char).to_digit(16)?;
            *byte = ((hi << 4) | lo) as u8;
        }
        Some(ContentHash(out))
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.short())
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        ContentHash::from_hex(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("malformed content hash `{s}`")))
    }
}

pub use ply_span::codes;

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DefBody {
    encoding: u32,
    #[serde(with = "hex_bytes")]
    bytes: Vec<u8>,
}

impl DefBody {
    pub fn new(encoding: u32, bytes: Vec<u8>) -> DefBody {
        DefBody { encoding, bytes }
    }

    pub fn encoding(&self) -> u32 {
        self.encoding
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
            out.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'));
        }
        s.serialize_str(&out)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        let raw = text.as_bytes();
        if raw.len() % 2 != 0 {
            return Err(serde::de::Error::custom("odd-length body encoding"));
        }
        let mut out = Vec::with_capacity(raw.len() / 2);
        for pair in raw.chunks_exact(2) {
            let hi = (pair[0] as char)
                .to_digit(16)
                .ok_or_else(|| serde::de::Error::custom("malformed body encoding"))?;
            let lo = (pair[1] as char)
                .to_digit(16)
                .ok_or_else(|| serde::de::Error::custom("malformed body encoding"))?;
            out.push(((hi << 4) | lo) as u8);
        }
        Ok(out)
    }
}

/// The definition set a test last passed against, keyed by `<module>.<label>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassRecord {
    pub test_hash: DefHash,
    /// The *functions* in the closure, by program-wide name.
    pub closure: std::collections::BTreeMap<Symbol, DefHash>,
    /// Kept apart from `closure` because a `fn` and a `type` may share a name.
    pub decls: std::collections::BTreeMap<Symbol, DefHash>,
}

impl PassRecord {
    pub fn hashes(&self) -> impl Iterator<Item = DefHash> {
        self.closure.values().chain(self.decls.values()).copied()
    }
}

#[derive(Serialize, Deserialize)]
struct PassRecordRepr {
    test_hash: DefHash,
    closure: std::collections::BTreeMap<String, DefHash>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    decls: std::collections::BTreeMap<String, DefHash>,
}

impl Serialize for PassRecord {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        PassRecordRepr {
            test_hash: self.test_hash,
            closure: self
                .closure
                .iter()
                .map(|(name, hash)| (name.to_string(), *hash))
                .collect(),
            decls: self
                .decls
                .iter()
                .map(|(name, hash)| (name.to_string(), *hash))
                .collect(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for PassRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let repr = PassRecordRepr::deserialize(d)?;
        Ok(PassRecord {
            test_hash: repr.test_hash,
            closure: repr
                .closure
                .into_iter()
                .map(|(name, hash)| (Symbol::new(name), hash))
                .collect(),
            decls: repr
                .decls
                .into_iter()
                .map(|(name, hash)| (Symbol::new(name), hash))
                .collect(),
        })
    }
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Pass,
    Fail {
        message: String,
        diagnostic: Option<Diagnostic>,
    },
}

impl Outcome {
    pub fn is_pass(&self) -> bool {
        matches!(self, Outcome::Pass)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum OutcomeRepr {
    Pass,
    Fail {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diagnostic: Option<diag::DiagnosticRepr>,
    },
}

/// Hand-written because `Diagnostic` deserializes only from `&'static` input.
impl Serialize for Outcome {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let repr = match self {
            Outcome::Pass => OutcomeRepr::Pass,
            Outcome::Fail {
                message,
                diagnostic,
            } => OutcomeRepr::Fail {
                message: message.clone(),
                diagnostic: diagnostic.as_ref().map(Into::into),
            },
        };
        repr.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Outcome {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match OutcomeRepr::deserialize(d)? {
            OutcomeRepr::Pass => Outcome::Pass,
            OutcomeRepr::Fail {
                message,
                diagnostic,
            } => Outcome::Fail {
                message,
                diagnostic: diagnostic.map(Into::into),
            },
        })
    }
}

pub struct Store {
    root: PathBuf,
    dir: PathBuf,
    path: PathBuf,
    entries: disk::Entries,
    definitions: disk::Definitions,
    passes: Passes,
    obligations: Lazy<DefHash, CachedObligation>,
    reviews: Lazy<Symbol, ReviewRecord>,
    dirty: bool,
    frontend_path: PathBuf,
    frontend_data_path: PathBuf,
    frontend: frontend::Frontend,
    answer: answer::Answer,
    claims: answer::Answer,
    warnings: Vec<Diagnostic>,
    stdlib: Stdlib,
    upstream: Option<Upstream>,
    /// Passes the upstream already holds, so a flush publishes only what is new to it.
    shared_passes: std::collections::BTreeSet<DefHash>,
}

#[derive(Default)]
struct Stdlib {
    path: PathBuf,
    stored: OnceLock<Option<String>>,
    /// Set only when it differs from what is on disk.
    pending: Option<String>,
}

/// Pass records, read on the first question rather than at [`Store::open`].
#[derive(Default)]
struct Passes {
    path: PathBuf,
    stored: OnceLock<disk::Passes>,
    added: disk::Passes,
    dirty: bool,
    /// Records a format 1 result cache held inline.
    inline: disk::Passes,
    warnings: Mutex<Vec<Diagnostic>>,
}

impl Passes {
    fn stored(&self) -> &disk::Passes {
        self.stored
            .get_or_init(|| match disk::load_passes(&self.path) {
                Ok(passes) => passes,
                Err(disk::LoadError::Missing) => self.inline.clone(),
                Err(e) => {
                    self.warnings
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(e.into_passes_diagnostic(&self.path));
                    self.inline.clone()
                }
            })
    }

    fn get(&self, key: &Symbol) -> Option<&PassRecord> {
        match self.added.get(key) {
            Some(record) => Some(record),
            None => self.stored().get(key),
        }
    }

    /// Forces the read, but otherwise an unchanged record rewrites the file every run.
    fn put(&mut self, key: Symbol, record: PassRecord) {
        if self.get(&key) == Some(&record) {
            return;
        }
        self.added.insert(key, record);
        self.dirty = true;
    }

    fn all(&self) -> impl Iterator<Item = (&Symbol, &PassRecord)> {
        self.stored()
            .iter()
            .filter(|(key, _)| !self.added.contains_key(*key))
            .chain(self.added.iter())
    }

    fn clear(&mut self) {
        self.added.clear();
        self.inline.clear();
        self.stored = OnceLock::from(disk::Passes::new());
        self.dirty = false;
    }

    fn take_warnings(&self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.warnings.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn warnings(&self) -> Vec<Diagnostic> {
        self.warnings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

struct Lazy<K: Ord, V> {
    path: PathBuf,
    stored: OnceLock<std::collections::BTreeMap<K, V>>,
    added: std::collections::BTreeMap<K, V>,
    dirty: bool,
    warnings: Mutex<Vec<Diagnostic>>,
    load: fn(&Path) -> Result<std::collections::BTreeMap<K, V>, disk::LoadError>,
    diagnose: fn(disk::LoadError, &Path) -> Diagnostic,
}

impl<K: Ord + Clone, V: Clone + PartialEq> Lazy<K, V> {
    fn new(
        path: PathBuf,
        load: fn(&Path) -> Result<std::collections::BTreeMap<K, V>, disk::LoadError>,
        diagnose: fn(disk::LoadError, &Path) -> Diagnostic,
    ) -> Lazy<K, V> {
        Lazy {
            path,
            stored: OnceLock::new(),
            added: std::collections::BTreeMap::new(),
            dirty: false,
            warnings: Mutex::new(Vec::new()),
            load,
            diagnose,
        }
    }

    /// An unreadable file is an empty map, never a partial one.
    fn stored(&self) -> &std::collections::BTreeMap<K, V> {
        self.stored.get_or_init(|| match (self.load)(&self.path) {
            Ok(entries) => entries,
            Err(disk::LoadError::Missing) => std::collections::BTreeMap::new(),
            Err(e) => {
                self.warnings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((self.diagnose)(e, &self.path));
                std::collections::BTreeMap::new()
            }
        })
    }

    fn get(&self, key: &K) -> Option<&V> {
        match self.added.get(key) {
            Some(value) => Some(value),
            None => self.stored().get(key),
        }
    }

    fn put(&mut self, key: K, value: V) {
        if self.get(&key) == Some(&value) {
            return;
        }
        self.added.insert(key, value);
        self.dirty = true;
    }

    fn all(&self) -> impl Iterator<Item = (&K, &V)> {
        self.stored()
            .iter()
            .filter(|(key, _)| !self.added.contains_key(*key))
            .chain(self.added.iter())
    }

    fn len(&self) -> usize {
        self.all().count()
    }

    /// Merges into what is on disk, under the caller's lock, so concurrent runs keep each other's.
    fn write(
        &mut self,
        dir: &Path,
        save: fn(&Path, &Path, &std::collections::BTreeMap<K, V>) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let mut merged = match (self.load)(&self.path) {
            Ok(entries) => entries,
            Err(disk::LoadError::Missing) => std::collections::BTreeMap::new(),
            Err(e) => {
                self.warnings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((self.diagnose)(e, &self.path));
                std::collections::BTreeMap::new()
            }
        };
        merged.extend(
            self.added
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        save(dir, &self.path, &merged)?;
        self.added.clear();
        self.stored = OnceLock::from(merged);
        self.dirty = false;
        Ok(())
    }

    fn clear(&mut self) {
        self.added.clear();
        self.stored = OnceLock::from(std::collections::BTreeMap::new());
        self.dirty = false;
    }

    fn take_warnings(&self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.warnings.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn warnings(&self) -> Vec<Diagnostic> {
        self.warnings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Pruned {
    pub sources: usize,
    pub defs: usize,
    pub decls: usize,
    pub bodies: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Compaction {
    pub dropped: Pruned,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CacheStats {
    pub results: usize,
    pub obligations: usize,
    pub reviews: usize,
    pub definitions_seen: usize,
    pub sources: usize,
    pub defs: usize,
    pub decls: usize,
    pub bodies: usize,
    pub results_bytes: u64,
    pub index_bytes: u64,
    pub data_bytes: u64,
    /// What [`Store::compact`] would reclaim: data-file bytes no index record names.
    pub garbage_bytes: Option<u64>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Found {
    Def(FoundDef),
    Test(FoundTest),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FoundDef {
    pub hash: DefHash,
    pub name: Symbol,
    pub kind: DefKind,
    pub path: PathBuf,
    pub span: FileSpan,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FoundTest {
    pub hash: DefHash,
    pub name: String,
    pub nondet: bool,
    pub footprint: ply_ty::Footprint,
    pub path: PathBuf,
    pub span: FileSpan,
}

impl Found {
    pub fn hash(&self) -> DefHash {
        match self {
            Found::Def(d) => d.hash,
            Found::Test(t) => t.hash,
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            Found::Def(d) => &d.path,
            Found::Test(t) => &t.path,
        }
    }
}

/// A missing file is zero bytes, not an error: callers only report sizes.
fn file_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn hash_prefix(query: &str) -> Option<Vec<u8>> {
    let query = query.to_ascii_lowercase();
    if query.len() < 4 || query.len() > 64 || !query.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(query.into_bytes())
}

fn starts_with(hash: DefHash, prefix: Option<&[u8]>) -> bool {
    let Some(prefix) = prefix else { return false };
    hash.to_hex().as_bytes().starts_with(prefix)
}

/// The last segment matches on its own: a diagnostic shows `place`, so a person types `place`.
fn names_match(name: &Symbol, query: &str) -> bool {
    name.as_str() == query || name.as_str().rsplit('.').next() == Some(query)
}

/// An obsolete front-end cache file, removed whenever the front end is written.
const LEGACY_FRONTEND_FILE: &str = "frontend.json";

impl Store {
    pub fn open(root: &Path) -> anyhow::Result<Store> {
        let dir = root.join(CACHE_DIR_NAME);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("could not create the cache directory `{}`", dir.display()))?;

        let path = dir.join(disk::RESULTS_FILE);
        let passes_path = dir.join(disk::PASSES_FILE);
        let obligations_path = dir.join(disk::OBLIGATIONS_FILE);
        let reviews_path = dir.join(disk::REVIEWS_FILE);
        let frontend_path = dir.join(frontend::FRONTEND_FILE);
        let frontend_data_path = dir.join(frontend::FRONTEND_DATA_FILE);
        let stdlib_path = dir.join(disk::STDLIB_FILE);
        let answer = answer::Answer::new(
            dir.join(answer::ANSWER_FILE),
            answer::ANSWER_STEM,
            "front-end answer",
        );
        let claims = answer::Answer::new(
            dir.join(answer::CLAIMS_FILE),
            answer::CLAIMS_STEM,
            "lowered claims",
        );
        let (frontend, frontend_warnings) =
            frontend::Frontend::open(&frontend_path, &frontend_data_path);
        let mut store = Store {
            root: root.to_path_buf(),
            dir,
            path,
            entries: disk::Entries::new(),
            definitions: disk::Definitions::new(),
            passes: Passes {
                path: passes_path,
                ..Passes::default()
            },
            obligations: Lazy::new(
                obligations_path,
                disk::load_obligations,
                disk::LoadError::into_obligations_diagnostic,
            ),
            reviews: Lazy::new(
                reviews_path,
                disk::load_reviews,
                disk::LoadError::into_reviews_diagnostic,
            ),
            dirty: false,
            frontend_path,
            frontend_data_path,
            frontend,
            answer,
            claims,
            warnings: frontend_warnings,
            upstream: None,
            shared_passes: std::collections::BTreeSet::new(),
            stdlib: Stdlib {
                path: stdlib_path,
                ..Stdlib::default()
            },
        };
        disk::sweep_stale_temps(&store.dir);

        match disk::load(&store.path) {
            Ok(cache) => {
                store.entries = cache.results;
                store.definitions = cache.definitions;
                // Rewritten even when empty, so no later run scans a format 1 file again.
                if cache.migrating {
                    store.passes.inline = cache.inline_passes;
                    store.passes.dirty = !store.passes.inline.is_empty();
                    store.dirty = true;
                }
            }
            Err(disk::LoadError::Missing) => {}
            Err(e) => {
                store.warnings.push(e.into_diagnostic(&store.path));
                // Nothing is pending; this only gets the unusable file replaced.
                store.dirty = true;
            }
        }
        Ok(store)
    }

    /// Reads the upstream's passes in, so this store answers for them; what it learns is written
    /// locally at the next flush, and what it records is published there.
    pub fn with_upstream(mut self, upstream: Option<Upstream>) -> Store {
        if let Some(upstream) = upstream {
            let passes = upstream.passes();
            for hash in &passes {
                if !self.entries.contains_key(hash) {
                    self.entries.insert(*hash, Outcome::Pass);
                    self.dirty = true;
                }
            }
            self.shared_passes = passes;
            self.upstream = Some(upstream);
        }
        self
    }

    pub fn upstream(&self) -> Option<&Upstream> {
        self.upstream.as_ref()
    }

    pub fn get(&self, hash: DefHash) -> Option<Outcome> {
        self.entries.get(&hash).cloned()
    }

    pub fn put(&mut self, hash: DefHash, outcome: Outcome) {
        self.entries.insert(hash, outcome);
        self.dirty = true;
    }

    pub fn pass_record(&self, key: &Symbol) -> Option<&PassRecord> {
        self.passes.get(key)
    }

    /// Never for a failing or `nondet` test, as with [`Outcome::Pass`].
    pub fn put_pass_record(&mut self, key: Symbol, record: PassRecord) {
        self.passes.put(key, record);
    }

    pub fn pass_records_len(&self) -> usize {
        self.passes.all().count()
    }

    /// The local entry, or else the upstream's.
    pub fn obligation(&self, key: DefHash) -> Option<CachedObligation> {
        if let Some(entry) = self.obligations.get(&key) {
            return Some(entry.clone());
        }
        self.upstream.as_ref()?.obligation(key)
    }

    /// Only a `Held` discharge may be written, and only under its tier's key.
    pub fn put_obligation(&mut self, key: DefHash, entry: CachedObligation) {
        self.obligations.put(key, entry);
    }

    pub fn obligations_len(&self) -> usize {
        self.obligations.len()
    }

    pub fn review_record(&self, name: &Symbol) -> Option<&ReviewRecord> {
        self.reviews.get(name)
    }

    pub fn put_review_record(&mut self, name: Symbol, record: ReviewRecord) {
        self.reviews.put(name, record);
    }

    pub fn review_records(&self) -> impl Iterator<Item = (&Symbol, &ReviewRecord)> {
        self.reviews.all()
    }

    pub fn review_records_len(&self) -> usize {
        self.reviews.len()
    }

    /// Merges with what other processes wrote since [`Store::open`], so no run discards another's.
    pub fn flush(&mut self) -> anyhow::Result<()> {
        if !self.dirty
            && !self.passes.dirty
            && !self.obligations.dirty
            && !self.reviews.dirty
            && !self.frontend.is_dirty()
            && self.stdlib.pending.is_none()
            && !self.answer.is_pending()
            && !self.claims.is_pending()
        {
            return Ok(());
        }
        let lock = disk::Lock::acquire(&self.dir);
        if !lock.held {
            self.warnings.push(
                Diagnostic::warning(
                    codes::CACHE_UNREADABLE,
                    format!(
                        "another `ply` run is holding the cache lock in `{}`",
                        self.dir.display()
                    ),
                )
                .note("nothing was written; this run is unaffected and the next one recomputes"),
            );
            return Ok(());
        }

        // Passes before results: writing results drops a format 1 file's inline pass records.
        self.write_passes()?;
        self.write_results()?;
        let dir = self.dir.clone();
        let fresh_obligations: Vec<(DefHash, CachedObligation)> = self
            .obligations
            .added
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        self.obligations.write(&dir, disk::save_obligations)?;
        self.reviews.write(&dir, disk::save_reviews)?;
        self.publish(&fresh_obligations);

        if self.frontend.is_dirty() {
            self.frontend
                .flush(&self.dir, &self.frontend_path, &self.frontend_data_path)?;
            let _ = std::fs::remove_file(self.dir.join(LEGACY_FRONTEND_FILE));
        }
        if let Some(digest) = self.stdlib.pending.take() {
            disk::save_stdlib(&self.dir, &self.stdlib.path, &digest)?;
            self.stdlib.stored = OnceLock::from(Some(digest));
        }
        self.answer.flush(&self.dir)?;
        self.claims.flush(&self.dir)?;
        Ok(())
    }

    /// After the local write, so an unreachable upstream never costs a run its own record.
    fn publish(&mut self, obligations: &[(DefHash, CachedObligation)]) {
        let Some(upstream) = self.upstream.clone() else {
            return;
        };
        let passes: Vec<DefHash> = self
            .entries
            .iter()
            .filter(|(hash, outcome)| {
                matches!(outcome, Outcome::Pass) && !self.shared_passes.contains(*hash)
            })
            .map(|(hash, _)| *hash)
            .collect();
        let mut failed = 0usize;
        for hash in passes {
            match upstream.publish_pass(hash) {
                Ok(()) => {
                    self.shared_passes.insert(hash);
                }
                Err(_) => failed += 1,
            }
        }
        for (hash, entry) in obligations {
            if upstream.publish_obligation(*hash, entry).is_err() {
                failed += 1;
            }
        }
        if failed > 0 {
            self.warnings.push(
                Diagnostic::warning(
                    codes::CACHE_UNREADABLE,
                    format!(
                        "{failed} {} could not be published to the upstream cache `{}`",
                        if failed == 1 { "entry" } else { "entries" },
                        upstream.root().display()
                    ),
                )
                .note("this run is unaffected; the upstream is read and written again next time"),
            );
        }
    }

    fn write_results(&mut self) -> anyhow::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let mut merged = disk::load(&self.path).unwrap_or_default();
        merged
            .results
            .extend(self.entries.iter().map(|(h, o)| (*h, o.clone())));
        merged.definitions.extend(self.definitions.iter().copied());
        disk::save(&self.dir, &self.path, &merged)?;
        self.entries = merged.results;
        self.definitions = merged.definitions;
        self.dirty = false;
        Ok(())
    }

    fn write_passes(&mut self) -> anyhow::Result<()> {
        if !self.passes.dirty {
            return Ok(());
        }
        let mut merged = match disk::load_passes(&self.passes.path) {
            Ok(passes) => passes,
            Err(disk::LoadError::Missing) => std::mem::take(&mut self.passes.inline),
            Err(e) => {
                self.warnings
                    .push(e.into_passes_diagnostic(&self.passes.path));
                std::mem::take(&mut self.passes.inline)
            }
        };
        merged.extend(
            self.passes
                .added
                .iter()
                .map(|(key, record)| (key.clone(), record.clone())),
        );
        disk::save_passes(&self.dir, &self.passes.path, &merged)?;
        self.passes.added.clear();
        self.passes.inline.clear();
        self.passes.stored = OnceLock::from(merged);
        self.passes.dirty = false;
        Ok(())
    }

    pub fn clear(&mut self) -> anyhow::Result<()> {
        self.entries.clear();
        self.definitions.clear();
        self.passes.clear();
        self.obligations.clear();
        self.frontend.clear();
        self.warnings.clear();
        self.dirty = false;
        // After a clear there is nothing a moved stdlib could have invalidated.
        self.stdlib.pending = None;
        self.stdlib.stored = OnceLock::from(None);
        let _lock = disk::Lock::acquire(&self.dir);
        remove(&self.stdlib.path, "stdlib digest")?;
        remove(&self.path, "result cache")?;
        remove(&self.passes.path, "pass records")?;
        remove(&self.obligations.path, "obligation cache")?;
        remove(&self.frontend_path, "front-end cache")?;
        remove(&self.frontend_data_path, "front-end cache")?;
        self.answer.clear()?;
        self.claims.clear()?;
        remove(&self.dir.join(LEGACY_FRONTEND_FILE), "front-end cache")?;
        disk::sweep_temps(&self.dir, None);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, hash: DefHash) -> bool {
        self.entries.contains_key(&hash)
    }

    pub fn knows_definition(&self, hash: DefHash) -> bool {
        self.definitions.contains(&hash)
    }

    /// Records definition hashes as seen, returning how many were new.
    pub fn observe_definitions(&mut self, hashes: impl IntoIterator<Item = DefHash>) -> usize {
        let mut added = 0;
        for hash in hashes {
            if self.definitions.insert(hash) {
                added += 1;
            }
        }
        if added > 0 {
            self.dirty = true;
        }
        added
    }

    pub fn definitions_len(&self) -> usize {
        self.definitions.len()
    }

    /// Includes degradations found on read, since entries are decoded on demand.
    pub fn warnings(&self) -> Vec<Diagnostic> {
        let mut warnings = self.warnings.clone();
        warnings.extend(self.passes.warnings());
        warnings.extend(self.obligations.warnings());
        warnings.extend(self.reviews.warnings());
        warnings.extend(self.frontend.warnings());
        warnings
    }

    pub fn take_warnings(&mut self) -> Vec<Diagnostic> {
        let mut warnings = std::mem::take(&mut self.warnings);
        warnings.extend(self.passes.take_warnings());
        warnings.extend(self.obligations.take_warnings());
        warnings.extend(self.reviews.take_warnings());
        warnings.extend(self.frontend.take_warnings());
        warnings
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn stdlib_digest(&self) -> Option<String> {
        self.stdlib
            .pending
            .clone()
            .or_else(|| self.stdlib_stored().clone())
    }

    fn stdlib_stored(&self) -> &Option<String> {
        self.stdlib
            .stored
            .get_or_init(|| disk::load_stdlib(&self.stdlib.path))
    }

    pub fn set_stdlib_digest(&mut self, digest: String) {
        if self.stdlib_stored().as_deref() == Some(digest.as_str()) {
            return;
        }
        self.stdlib.pending = Some(digest);
    }

    pub fn front_part(&self, key: ContentHash) -> Option<String> {
        self.answer.part(key)
    }

    /// Replaces every part on disk at the next flush, unless these are the parts already there.
    pub fn put_front_parts(&mut self, parts: std::collections::BTreeMap<ContentHash, String>) {
        self.answer.put(parts);
    }

    pub fn claims_part(&self, key: ContentHash) -> Option<String> {
        self.claims.part(key)
    }

    /// As [`Store::put_front_parts`], for the claims `ply prove` lowers.
    pub fn put_claims_parts(&mut self, parts: std::collections::BTreeMap<ContentHash, String>) {
        self.claims.put(parts);
    }

    pub fn frontend_path(&self) -> &Path {
        &self.frontend_path
    }

    pub fn frontend_data_path(&self) -> &Path {
        &self.frontend_data_path
    }

    /// Trustworthy only once its `content_hash` matches the file's current bytes.
    pub fn fingerprint(&self, path: &Path) -> Option<Arc<SourceFingerprint>> {
        self.frontend.fingerprint(&self.key(path)?)
    }

    /// `false` for a path that cannot be keyed relative to the root; it never takes the fast path.
    pub fn put_source(&mut self, path: &Path, fingerprint: SourceFingerprint) -> bool {
        let Some(key) = self.key(path) else {
            return false;
        };
        self.frontend.put_source(key, fingerprint);
        true
    }

    pub fn forget_source(&mut self, path: &Path) -> bool {
        let Some(key) = self.key(path) else {
            return false;
        };
        self.frontend.forget_source(&key)
    }

    pub fn source_paths(&self) -> Vec<PathBuf> {
        self.frontend
            .source_keys()
            .into_iter()
            .map(|key| frontend::source_path(&self.root, &key))
            .collect()
    }

    /// The keys themselves, for a caller that recorded a path which is not under the root at all:
    /// a module this binary ships has no file, and its key is the whole of what names it.
    pub fn source_keys(&self) -> Vec<String> {
        self.frontend.source_keys()
    }

    pub fn sources_len(&self) -> usize {
        self.frontend.sources_len()
    }

    /// Some interface stored under this hash.
    pub fn def(&self, hash: DefHash) -> Option<Arc<CachedDef>> {
        self.frontend.def(hash)
    }

    pub fn def_of(&self, hash: DefHash, name: &Symbol) -> Option<Arc<CachedDef>> {
        self.frontend.def_of(hash, name)
    }

    pub fn decl(&self, hash: DefHash) -> Option<Arc<CachedDecl>> {
        self.frontend.decl(hash)
    }

    pub fn decl_of(&self, hash: DefHash, name: &Symbol) -> Option<Arc<CachedDecl>> {
        self.frontend.decl_of(hash, name)
    }

    /// Stores the canonical form of `def`, which is what comes back out.
    pub fn put_def(&mut self, hash: DefHash, def: CachedDef) {
        self.frontend.put_def(hash, def);
    }

    pub fn defs_len(&self) -> usize {
        self.frontend.defs_len()
    }

    /// Canonicalizes on the way in, as [`Store::put_def`] does.
    pub fn put_decl(&mut self, hash: DefHash, decl: CachedDecl) {
        self.frontend.put_decl(hash, decl);
    }

    pub fn decls_len(&self) -> usize {
        self.frontend.decls_len()
    }

    /// `None` also when this build does not speak the body's encoding.
    pub fn body(&self, hash: DefHash) -> Option<Arc<DefBody>> {
        let body = self.frontend.body(hash)?;
        (body.encoding() == BODY_ENCODING).then_some(body)
    }

    pub fn has_body(&self, hash: DefHash) -> bool {
        self.body(hash).is_some()
    }

    /// One slot per hash: a body is name-free, unlike an interface.
    pub fn put_body(&mut self, hash: DefHash, body: DefBody) {
        if let frontend::StoredBody::Conflict = self.frontend.put_body(hash, body) {
            self.warnings.push(
                Diagnostic::warning(
                    codes::CACHE_CORRUPT,
                    format!(
                        "two different bodies were stored for definition `{}`",
                        hash.short()
                    ),
                )
                .note(
                    "a body is keyed by a hash of itself, so this means the body encoding \
                     depends on something the hash does not cover",
                )
                .note("keeping the body already stored; the new one is discarded"),
            );
        }
    }

    pub fn bodies_len(&self) -> usize {
        self.frontend.bodies_len()
    }

    /// Call only after discovering every `.ply` file under the root, or other files' work is lost.
    pub fn prune(&mut self, keep: &[PathBuf]) -> Pruned {
        let keep: std::collections::BTreeSet<String> =
            keep.iter().filter_map(|p| self.key(p)).collect();
        // Checked first, so an unchanged project does not pay to read the pass records.
        if !self.frontend.prune_would_change(&keep) {
            return Pruned::default();
        }
        self.frontend.prune(&keep, &self.baseline_hashes())
    }

    fn baseline_hashes(&self) -> std::collections::BTreeSet<DefHash> {
        self.passes
            .all()
            .flat_map(|(_, record)| record.hashes())
            .collect()
    }

    /// Reclaims what [`Store::prune`] made unreachable; nothing else shrinks the data file.
    pub fn compact(&mut self, keep: &[PathBuf]) -> anyhow::Result<Compaction> {
        let bytes_before = self.frontend_bytes();
        let dropped = self.prune(keep);
        let lock = disk::Lock::acquire(&self.dir);
        if !lock.held {
            anyhow::bail!(
                "another `ply` run is holding the cache lock in `{}`",
                self.dir.display()
            );
        }
        self.write_results()?;
        self.frontend
            .compact(&self.dir, &self.frontend_path, &self.frontend_data_path)?;
        let _ = std::fs::remove_file(self.dir.join(LEGACY_FRONTEND_FILE));
        Ok(Compaction {
            dropped,
            bytes_before,
            bytes_after: self.frontend_bytes(),
        })
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            results: self.entries.len(),
            obligations: self.obligations_len(),
            reviews: self.review_records_len(),
            definitions_seen: self.definitions.len(),
            sources: self.sources_len(),
            defs: self.defs_len(),
            decls: self.decls_len(),
            bodies: self.bodies_len(),
            results_bytes: file_bytes(&self.path)
                + file_bytes(&self.passes.path)
                + file_bytes(&self.obligations.path)
                + file_bytes(&self.reviews.path),
            index_bytes: file_bytes(&self.frontend_path),
            data_bytes: file_bytes(&self.frontend_data_path),
            garbage_bytes: Some(self.frontend.garbage_bytes()),
        }
    }

    /// Matches a program-wide name, a name as its module wrote it, or a hash prefix.
    pub fn lookup(&self, query: &str) -> Vec<Found> {
        let prefix = hash_prefix(query);
        let mut found = Vec::new();
        for (key, fingerprint) in self.frontend.sources() {
            let path = frontend::source_path(&self.root, &key);
            for def in &fingerprint.defs {
                if names_match(&def.name, query) || starts_with(def.hash, prefix.as_deref()) {
                    found.push(Found::Def(FoundDef {
                        hash: def.hash,
                        name: def.name.clone(),
                        kind: def.kind,
                        path: path.clone(),
                        span: def.span,
                    }));
                }
            }
            for test in &fingerprint.tests {
                if test.name == query || starts_with(test.hash, prefix.as_deref()) {
                    found.push(Found::Test(FoundTest {
                        hash: test.hash,
                        name: test.name.clone(),
                        nondet: test.nondet,
                        footprint: test.footprint.clone(),
                        path: path.clone(),
                        span: test.span,
                    }));
                }
            }
        }
        found
    }

    fn frontend_bytes(&self) -> u64 {
        file_bytes(&self.frontend_path) + file_bytes(&self.frontend_data_path)
    }

    pub fn frontend_is_empty(&self) -> bool {
        self.frontend.is_empty() && self.answer.is_empty() && self.claims.is_empty()
    }

    pub fn frontend_is_dirty(&self) -> bool {
        self.frontend.is_dirty()
    }

    fn key(&self, path: &Path) -> Option<String> {
        frontend::source_key(&self.root, path)
    }
}

fn remove(path: &Path, what: &str) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("could not delete the {what} `{}`", path.display()))),
    }
}

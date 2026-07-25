//! Two caches over one `.ply-cache` directory: results,
//! `(RUNTIME_VERSION, DefHash) -> Outcome` beside the set of definitions a run
//! has already seen, and the front end,
//! `(FRONTEND_VERSION, path | DefHash) -> fingerprint | interface`.
//!
//! Selection in Ply is exact rather than heuristic, so the cache is load-bearing
//! — a wrong answer here is a test that never runs. Every path in this crate is
//! arranged so the only two outcomes are "no result", which is safe because the
//! test re-runs, and "the result that was recorded"; never anything between.
//!
//! The on-disk form is JSON keyed by hex hash, because reading and editing it
//! by hand is worth more than the microseconds. The result cache is
//! pretty-printed; the front-end cache is not, because it reaches tens of
//! megabytes on a large project and is rewritten whole whenever a definition
//! changes, at which point the whitespace costs more than it buys. `jq .`
//! restores it for the rare occasion someone reads it.

mod canonical;
mod diag;
mod disk;
pub mod frontend;

#[cfg(test)]
mod tests;

use anyhow::Context;
use ply_hash::DefHash;
use ply_span::Diagnostic;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub use canonical::{canonicalize_decl_body, canonicalize_scheme};
pub use frontend::{
    CachedCtor, CachedDecl, CachedDef, CachedOp, CachedTest, DeclBody, DefEntry, DefKind, FileSpan,
    ImportEdge, Member, NameRef, SourceFingerprint, exports_digest, witness_holds,
};

/// Bumping this invalidates every cached result in existence: a cache file
/// written by a different runtime version is discarded whole, never merged.
/// Bump it for any change to evaluation semantics, to a prelude builtin, to the
/// hashing scheme, or to the on-disk shape of an [`Outcome`] — none of those
/// necessarily change a test's `DefHash`, and all of them can change what a
/// cache hit means.
///
/// The shape half of that rule is enforced: a pin test in this crate fails when
/// the serialized form changes, and says to bump this. The semantic half is
/// not — no test can see that the evaluator started rounding differently — so a
/// change to `ply-eval` or to normalization must bump this by hand.
pub const RUNTIME_VERSION: &str = "0.2.0";

/// Bumping this discards every cached type, footprint and source fingerprint.
///
/// Deliberately separate from [`RUNTIME_VERSION`]: a change to the evaluator
/// invalidates results without invalidating types, and a change to inference
/// invalidates types without invalidating a result that was proved by running
/// the code. Bump this for any change to normalization, to inference, to the
/// representation of `Scheme` or `Footprint`, or to the prelude's signatures —
/// none of those necessarily changes a `DefHash`, and all of them change what
/// the front end would compute for one.
///
/// A change to any *stored* type — `Type`, `Scheme`, `Footprint`, a
/// fingerprint's fields, the canonicalization rule — is caught by a pin test in
/// this crate, which fails and says to bump this. A change to inference or
/// normalization that leaves those shapes alone is not caught by anything, and
/// is the case a contributor has to remember: the stale entry it leaves behind
/// is a wrong *type*, which corrupts every hash keyed on it.
pub const FRONTEND_VERSION: &str = "0.3.0";

/// Directory created under the root passed to [`Store::open`].
pub const CACHE_DIR_NAME: &str = ".ply-cache";

/// BLAKE3 over raw bytes: a source file's contents, or a digest of a module's
/// exports. Distinct from [`DefHash`], which is over a *normalized definition* —
/// keeping the two in separate types is what stops a caller from asking the
/// definition-keyed maps a question only raw content can answer.
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

/// Hand-written rather than derived: `Diagnostic` deserializes only from
/// `&'static` input, which no file read at runtime can offer.
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
    dirty: bool,
    frontend_path: PathBuf,
    frontend: frontend::Frontend,
    /// The bytes this store's copy of the front-end cache was read from, so a
    /// flush can tell "nobody else wrote" from "merge needed" without parsing
    /// the file a second time.
    frontend_digest: Option<ContentHash>,
    frontend_dirty: bool,
    /// Set by whoever removed something, which is only correct for a run that
    /// saw every file under the root. A merging flush would resurrect exactly
    /// what was removed, so such a run has to write the front-end cache whole.
    /// The cost is losing entries a concurrent run added mid-flight, which is a
    /// recheck rather than a wrong answer.
    authoritative: bool,
    warnings: Vec<Diagnostic>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Pruned {
    pub sources: usize,
    pub defs: usize,
    pub decls: usize,
}

impl Store {
    /// Opens/creates `<root>/.ply-cache`. Fails only if that directory cannot
    /// exist; an unusable cache *file* degrades to an empty cache instead.
    pub fn open(root: &Path) -> anyhow::Result<Store> {
        let dir = root.join(CACHE_DIR_NAME);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("could not create the cache directory `{}`", dir.display()))?;

        let path = dir.join(disk::RESULTS_FILE);
        let frontend_path = dir.join(frontend::FRONTEND_FILE);
        let mut store = Store {
            root: root.to_path_buf(),
            dir,
            path,
            entries: disk::Entries::new(),
            definitions: disk::Definitions::new(),
            dirty: false,
            frontend_path,
            frontend: frontend::Frontend::default(),
            frontend_digest: None,
            frontend_dirty: false,
            authoritative: false,
            warnings: Vec::new(),
        };
        disk::sweep_stale_temps(&store.dir);

        match disk::load(&store.path) {
            Ok(cache) => {
                store.entries = cache.results;
                store.definitions = cache.definitions;
            }
            Err(disk::LoadError::Missing) => {}
            Err(e) => {
                store.warnings.push(e.into_diagnostic(&store.path));
                // Nothing was loaded, so nothing is pending; the flag exists
                // here only to get the unusable file replaced.
                store.dirty = true;
            }
        }

        match frontend::load_with_digest(&store.frontend_path) {
            Ok((f, digest)) => {
                store.frontend = f;
                store.frontend_digest = Some(digest);
            }
            Err(frontend::LoadError::Missing) => {}
            Err(e) => {
                store.warnings.push(e.into_diagnostic(&store.frontend_path));
                store.frontend_dirty = true;
            }
        }
        Ok(store)
    }

    pub fn get(&self, hash: DefHash) -> Option<Outcome> {
        self.entries.get(&hash).cloned()
    }

    pub fn put(&mut self, hash: DefHash, outcome: Outcome) {
        self.entries.insert(hash, outcome);
        self.dirty = true;
    }

    /// Folds in whatever another process wrote since [`Store::open`], so two
    /// concurrent runs cannot silently discard each other's results.
    pub fn flush(&mut self) -> anyhow::Result<()> {
        if !self.dirty && !self.frontend_dirty {
            return Ok(());
        }
        let _lock = disk::Lock::acquire(&self.dir);

        if self.dirty {
            let mut merged = disk::load(&self.path).unwrap_or_default();
            merged
                .results
                .extend(self.entries.iter().map(|(h, o)| (*h, o.clone())));
            merged.definitions.extend(self.definitions.iter().copied());

            disk::save(&self.dir, &self.path, &merged)?;
            self.entries = merged.results;
            self.definitions = merged.definitions;
            self.dirty = false;
        }

        if self.frontend_dirty {
            // Re-reading is only ever for entries some *other* process added.
            // When the bytes on disk are the ones this store already parsed
            // there is nothing to merge, and this run's map is already the
            // answer — no second parse, and no copy of it either. At ten
            // thousand definitions both of those are tens of milliseconds on a
            // run that changed one definition.
            let untouched = frontend::digest_of(&self.frontend_path) == self.frontend_digest;
            if self.authoritative || untouched {
                let written = frontend::save(&self.dir, &self.frontend_path, &self.frontend)?;
                self.frontend_digest = Some(written);
            } else {
                // Both interface maps are content-keyed, so an entry another
                // process wrote is as good as one of ours and the union is
                // always sound. A fingerprint is not, but it is re-validated
                // against the file's bytes before it is ever believed, so
                // last-writer-wins costs at worst a parse.
                let mut merged = frontend::load(&self.frontend_path).unwrap_or_default();
                for (&hash, slots) in &self.frontend.defs {
                    let into = merged.defs.entry(hash).or_default();
                    for def in slots {
                        frontend::upsert(into, def.clone(), hash, |d: &CachedDef| &d.names);
                    }
                }
                for (&hash, slots) in &self.frontend.decls {
                    let into = merged.decls.entry(hash).or_default();
                    for decl in slots {
                        frontend::upsert(into, decl.clone(), hash, |d: &CachedDecl| &d.names);
                    }
                }
                merged.sources.extend(
                    self.frontend
                        .sources
                        .iter()
                        .map(|(p, f)| (p.clone(), f.clone())),
                );
                let written = frontend::save(&self.dir, &self.frontend_path, &merged)?;
                self.frontend = merged;
                self.frontend_digest = Some(written);
            }
            self.frontend_dirty = false;
            self.authoritative = false;
        }
        Ok(())
    }

    /// Discards both caches: results *and* the front end. `ply cache clear` has
    /// to mean "prove everything again", and leaving cached types behind would
    /// make it mean "run the tests again against types I am still assuming".
    pub fn clear(&mut self) -> anyhow::Result<()> {
        self.entries.clear();
        self.definitions.clear();
        self.frontend = frontend::Frontend::default();
        self.frontend_digest = None;
        self.warnings.clear();
        self.dirty = false;
        self.frontend_dirty = false;
        self.authoritative = false;
        let _lock = disk::Lock::acquire(&self.dir);
        remove(&self.path, "result cache")?;
        remove(&self.frontend_path, "front-end cache")?;
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

    /// Whether some earlier run already saw this definition. What is unknown
    /// here is what the last edit produced, and intersecting that with a failing
    /// test's closure is that failure's suspect set.
    ///
    /// Deliberately not answerable from [`Store::get`]: an outcome is a claim
    /// about a *test*, so reading one as "unchanged" lets any green test that
    /// shares a definition vouch for it.
    pub fn knows_definition(&self, hash: DefHash) -> bool {
        self.definitions.contains(&hash)
    }

    /// Records definition hashes as seen, returning how many were new.
    ///
    /// Recording a definition ends its life as a suspect, so a caller that has
    /// just watched a test fail must withhold everything that failure reached:
    /// nothing about it has been resolved, and the next run has to be able to
    /// name the same suspects.
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

    /// Every degradation this cache took on the way up. Empty in the normal
    /// case; a caller that never reports these turns a corrupt cache into
    /// silence.
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    pub fn take_warnings(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.warnings)
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

    pub fn frontend_path(&self) -> &Path {
        &self.frontend_path
    }

    /// What this file compiled to last time — trustworthy only after its
    /// `content_hash` has been compared against the bytes on disk now.
    pub fn source(&self, path: &Path) -> Option<&SourceFingerprint> {
        self.frontend.sources.get(&self.key(path)?)
    }

    /// Returns `false` for a path that cannot be keyed relative to the root, in
    /// which case the file is simply never eligible for the fast path.
    ///
    /// Re-storing what is already here is not a write. That is what lets a run
    /// where nothing changed flush nothing: the cache file is rewritten whole,
    /// and at ten thousand definitions that is the single largest cost in a run
    /// that had no work to do.
    pub fn put_source(&mut self, path: &Path, fingerprint: SourceFingerprint) -> bool {
        let Some(key) = self.key(path) else {
            return false;
        };
        if self.frontend.sources.get(&key) == Some(&fingerprint) {
            return true;
        }
        self.frontend.sources.insert(key, fingerprint);
        self.frontend_dirty = true;
        true
    }

    pub fn forget_source(&mut self, path: &Path) -> bool {
        let Some(key) = self.key(path) else {
            return false;
        };
        let removed = self.frontend.sources.remove(&key).is_some();
        if removed {
            self.frontend_dirty = true;
            self.authoritative = true;
        }
        removed
    }

    pub fn source_paths(&self) -> Vec<PathBuf> {
        self.frontend
            .sources
            .keys()
            .map(|key| self.root.join(key))
            .collect()
    }

    pub fn sources_len(&self) -> usize {
        self.frontend.sources.len()
    }

    /// Some interface stored under this hash. Correct only where any of them
    /// will do — to reuse one as a definition's published type, ask for it by
    /// name with [`Store::cached_def_of`], because several definitions can share
    /// a hash and their schemes are not interchangeable.
    pub fn cached_def(&self, hash: DefHash) -> Option<&CachedDef> {
        self.frontend.defs.get(&hash)?.first()
    }

    pub fn cached_def_of(&self, hash: DefHash, name: &ply_span::Symbol) -> Option<&CachedDef> {
        self.frontend
            .defs
            .get(&hash)?
            .iter()
            .find(|d| frontend::declares(&d.names, name, hash))
    }

    /// Stores the canonical form of `def`, which is what comes back out. A
    /// caller comparing a freshly-inferred scheme against a cached one must
    /// canonicalize its own side with [`canonicalize_scheme`] first, or the two
    /// differ by nothing but the numbers its counter reached.
    pub fn put_def(&mut self, hash: DefHash, def: CachedDef) {
        let slots = self.frontend.defs.entry(hash).or_default();
        if frontend::upsert(slots, def.canonicalized(), hash, |d: &CachedDef| &d.names) {
            self.frontend_dirty = true;
        }
    }

    pub fn defs_len(&self) -> usize {
        self.frontend.defs.values().map(Vec::len).sum()
    }

    pub fn cached_decl(&self, hash: DefHash) -> Option<&CachedDecl> {
        self.frontend.decls.get(&hash)?.first()
    }

    pub fn cached_decl_of(&self, hash: DefHash, name: &ply_span::Symbol) -> Option<&CachedDecl> {
        self.frontend
            .decls
            .get(&hash)?
            .iter()
            .find(|d| frontend::declares(&d.names, name, hash))
    }

    /// Canonicalizes on the way in, as [`Store::put_def`] does.
    pub fn put_decl(&mut self, hash: DefHash, decl: CachedDecl) {
        let slots = self.frontend.decls.entry(hash).or_default();
        if frontend::upsert(slots, decl.canonicalized(), hash, |d: &CachedDecl| &d.names) {
            self.frontend_dirty = true;
        }
    }

    pub fn decls_len(&self) -> usize {
        self.frontend.decls.values().map(Vec::len).sum()
    }

    /// Only call this after a run that discovered every `.ply` file under the
    /// root: `ply check one.ply` sees one file, and pruning to that would throw
    /// away the rest of the project's work. A caller that is unsure must not
    /// call it — the cost of skipping it is disk, the cost of getting it wrong
    /// is a full recompile.
    pub fn prune(&mut self, keep: &[PathBuf]) -> Pruned {
        let keep: std::collections::BTreeSet<String> =
            keep.iter().filter_map(|p| self.key(p)).collect();

        let before = self.frontend.sources.len();
        self.frontend.sources.retain(|key, _| keep.contains(key));
        let sources = before - self.frontend.sources.len();

        let live: std::collections::BTreeSet<DefHash> = self
            .frontend
            .sources
            .values()
            .flat_map(|f| f.referenced_hashes())
            .collect();

        let before = self.defs_len();
        self.frontend.defs.retain(|hash, _| live.contains(hash));
        let defs = before - self.defs_len();

        let before = self.decls_len();
        self.frontend.decls.retain(|hash, _| live.contains(hash));
        let decls = before - self.decls_len();

        let pruned = Pruned {
            sources,
            defs,
            decls,
        };
        if sources + defs + decls > 0 {
            self.frontend_dirty = true;
            self.authoritative = true;
        }
        pruned
    }

    pub fn frontend_is_empty(&self) -> bool {
        self.frontend.is_empty()
    }

    /// Whether [`Store::flush`] would rewrite the front-end cache. A run that
    /// found everything already cached must leave this `false`.
    pub fn frontend_is_dirty(&self) -> bool {
        self.frontend_dirty
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

//! The result cache: `(RUNTIME_VERSION, DefHash) -> Outcome`.
//!
//! Selection in Ply is exact rather than heuristic, so the cache is load-bearing
//! — a wrong answer here is a test that never runs. Every path in this crate is
//! arranged so the only two outcomes are "no result", which is safe because the
//! test re-runs, and "the result that was recorded"; never anything between.
//!
//! The on-disk form is pretty-printed JSON keyed by hex hash, because reading
//! and editing it by hand is worth more than the microseconds.

mod diag;
mod disk;

#[cfg(test)]
mod tests;

use anyhow::Context;
use ply_hash::DefHash;
use ply_span::Diagnostic;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Bumping this invalidates every cached result in existence: a cache file
/// written by a different runtime version is discarded whole, never merged.
/// Bump it for any change to evaluation semantics, to a prelude builtin, or to
/// the hashing scheme — none of those necessarily change a test's `DefHash`,
/// and all of them can change its outcome.
pub const RUNTIME_VERSION: &str = "0.1.0";

/// Directory created under the root passed to [`Store::open`].
pub const CACHE_DIR_NAME: &str = ".ply-cache";

/// Cache trouble is never a fault in the user's program, so these sit outside
/// the `ply_span::codes` numbering and are always warnings.
pub mod codes {
    pub const CACHE_UNREADABLE: &str = "W0601";
    pub const CACHE_CORRUPT: &str = "W0602";
    pub const CACHE_VERSION_CHANGED: &str = "W0603";
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
    dir: PathBuf,
    path: PathBuf,
    entries: disk::Entries,
    dirty: bool,
    warnings: Vec<Diagnostic>,
}

impl Store {
    /// Opens/creates `<root>/.ply-cache`. Fails only if that directory cannot
    /// exist; an unusable cache *file* degrades to an empty cache instead.
    pub fn open(root: &Path) -> anyhow::Result<Store> {
        let dir = root.join(CACHE_DIR_NAME);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("could not create the cache directory `{}`", dir.display()))?;

        let path = dir.join(disk::RESULTS_FILE);
        let mut store = Store {
            dir,
            path,
            entries: disk::Entries::new(),
            dirty: false,
            warnings: Vec::new(),
        };
        disk::sweep_stale_temps(&store.dir);

        match disk::load(&store.path) {
            Ok(entries) => store.entries = entries,
            Err(disk::LoadError::Missing) => {}
            Err(e) => {
                store.warnings.push(e.into_diagnostic(&store.path));
                // Nothing was loaded, so nothing is pending; the flag exists
                // here only to get the unusable file replaced.
                store.dirty = true;
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
        if !self.dirty {
            return Ok(());
        }
        let _lock = disk::Lock::acquire(&self.dir);
        let mut merged = disk::load(&self.path).unwrap_or_default();
        merged.extend(self.entries.iter().map(|(h, o)| (*h, o.clone())));

        disk::save(&self.dir, &self.path, &merged)?;
        self.entries = merged;
        self.dirty = false;
        Ok(())
    }

    pub fn clear(&mut self) -> anyhow::Result<()> {
        self.entries.clear();
        self.warnings.clear();
        self.dirty = false;
        let _lock = disk::Lock::acquire(&self.dir);
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!(
                    "could not delete the result cache `{}`",
                    self.path.display()
                )));
            }
        }
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
}

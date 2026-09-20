//! A cache shared between checkouts and machines: `PLY_CACHE_UPSTREAM` names a directory of
//! passes and discharged obligations keyed by content and by version. There is no lock: a key
//! names its bytes, so a racing write is idempotent, and a reader sees a whole file or none.

use crate::obligations::CachedObligation;
use ply_ty::DefHash;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const ENV: &str = "PLY_CACHE_UPSTREAM";
pub const READONLY_ENV: &str = "PLY_CACHE_UPSTREAM_READONLY";

#[derive(Clone, Debug)]
pub struct Upstream {
    root: PathBuf,
    publish: bool,
}

impl Upstream {
    pub fn at(root: impl Into<PathBuf>, publish: bool) -> Upstream {
        Upstream {
            root: root.into(),
            publish,
        }
    }

    /// The upstream the environment names, if any.
    pub fn from_env() -> Option<Upstream> {
        let root = std::env::var_os(ENV).filter(|v| !v.is_empty())?;
        Some(Upstream::at(
            PathBuf::from(root),
            std::env::var_os(READONLY_ENV).is_none(),
        ))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Each layer lives under the version its local loader would check, so a build on another
    /// version misses rather than misreads.
    fn results(&self) -> PathBuf {
        self.root.join("v1").join("r").join(crate::RUNTIME_VERSION)
    }

    fn obligations(&self) -> PathBuf {
        self.root.join("v1").join("o").join(crate::PROVER_VERSION)
    }

    fn place(dir: &Path, hash: DefHash) -> PathBuf {
        let hex = hash.to_hex();
        dir.join(&hex[..2]).join(hex)
    }

    /// Every pass published for this runtime version; an unreadable directory holds none.
    pub fn passes(&self) -> BTreeSet<DefHash> {
        let mut out = BTreeSet::new();
        let Ok(shards) = std::fs::read_dir(self.results()) else {
            return out;
        };
        for shard in shards.flatten() {
            let Ok(files) = std::fs::read_dir(shard.path()) else {
                continue;
            };
            for file in files.flatten() {
                if let Some(hash) = file.file_name().to_str().and_then(DefHash::from_hex) {
                    out.insert(hash);
                }
            }
        }
        out
    }

    /// A pass is its key's presence: a zero-byte file, so nothing run-local can leak into it.
    pub fn publish_pass(&self, hash: DefHash) -> std::io::Result<()> {
        self.publish(&Self::place(&self.results(), hash), &[])
    }

    pub fn obligation(&self, hash: DefHash) -> Option<CachedObligation> {
        let bytes = std::fs::read(Self::place(&self.obligations(), hash)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn publish_obligation(
        &self,
        hash: DefHash,
        entry: &CachedObligation,
    ) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(entry).map_err(std::io::Error::other)?;
        self.publish(&Self::place(&self.obligations(), hash), &bytes)
    }

    /// Lands by a rename from a temp beside it; an entry already there is the same bytes.
    fn publish(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        if !self.publish || path.exists() {
            return Ok(());
        }
        let dir = path.parent().expect("a shard directory");
        std::fs::create_dir_all(dir)?;
        let temp = dir.join(format!(
            "{}.{}.tmp",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("entry"),
            std::process::id()
        ));
        if let Err(e) = crate::disk::write_new(&temp, bytes) {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
        Ok(())
    }
}

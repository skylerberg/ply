//! What survives one iteration of a warm process, so the next need not re-establish it.

use crate::load::Loaded;
use ply_hash::{DefHash, HashOutput};
use ply_store::ContentHash;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Modification time and length: hashing every file is the cost this exists to avoid.
type Stamp = (Option<SystemTime>, u64);

#[derive(Default)]
pub struct Warm {
    pub held: Option<Loaded>,
    pub stamps: BTreeMap<PathBuf, Stamp>,
    /// Lets a save with no edit cost a read rather than a front end.
    pub content: BTreeMap<PathBuf, ContentHash>,
    pub unit: Option<HeldUnit>,
}

/// A compiled unit and the definitions it was compiled from.
pub struct HeldUnit {
    provider: &'static dyn ply_eval::Provider,
    /// Two specs are two different units under one name.
    spec: ply_eval::BackendSpec,
    /// [`HashOutput::digest`], which a machine checks the unit against before entering it.
    key: DefHash,
}

/// Why an iteration did or did not reuse what the last one built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reuse {
    Cold,
    Whole,
    Reloaded { changed: usize },
}

impl Warm {
    /// The state to run against, or `None` to load it; a failed iteration leaves nothing held.
    pub fn take(&mut self, root: &Path) -> (Option<Loaded>, Reuse) {
        let held = self.held.take();
        let Some(held) = held else {
            return (None, Reuse::Cold);
        };
        // A file appearing or disappearing changes the module set, which no stamp shows.
        if discovered_more(root, &self.stamps) {
            return (None, Reuse::Reloaded { changed: 0 });
        }
        let now = stamps(&held.files);
        let moved: Vec<&PathBuf> = self
            .stamps
            .iter()
            .filter(|(path, was)| now.get(*path) != Some(was))
            .map(|(path, _)| path)
            .collect();
        if moved.is_empty() {
            return (Some(held), Reuse::Whole);
        }
        // A moved stamp is only a reason to look: same bytes, same front end.
        let mut changed = 0;
        for path in moved {
            let same = std::fs::read(path)
                .ok()
                .map(|bytes| ContentHash::of(&bytes))
                .is_some_and(|hash| self.content.get(path) == Some(&hash));
            if !same {
                changed += 1;
            }
        }
        if changed == 0 {
            // Re-stamp, or every later iteration rereads these files.
            self.stamps = now;
            return (Some(held), Reuse::Whole);
        }
        (None, Reuse::Reloaded { changed })
    }

    pub fn keep(&mut self, loaded: Loaded) {
        self.stamps = stamps(&loaded.files);
        self.content = loaded
            .files
            .iter()
            .filter_map(|path| {
                let bytes = std::fs::read(path).ok()?;
                Some((path.clone(), ContentHash::of(&bytes)))
            })
            .collect();
        self.held = Some(loaded);
    }

    /// The unit the last iteration compiled, if this iteration would compile the same one.
    pub fn unit_for(
        &self,
        spec: &ply_eval::BackendSpec,
        hashes: &HashOutput,
    ) -> Option<&'static dyn ply_eval::Provider> {
        let held = self.unit.as_ref()?;
        (held.spec == *spec && held.key == hashes.digest()).then_some(held.provider)
    }

    pub fn keep_unit(
        &mut self,
        spec: &ply_eval::BackendSpec,
        hashes: &HashOutput,
        provider: &'static dyn ply_eval::Provider,
    ) {
        self.unit = Some(HeldUnit {
            provider,
            spec: spec.clone(),
            key: hashes.digest(),
        });
    }
}

impl Warm {
    /// Whether anything under `root` differs from the held state, by a stat per file.
    pub fn tree_moved(&self, root: &Path) -> bool {
        if self.held.is_none() {
            return true;
        }
        for (path, was) in &self.stamps {
            let now = std::fs::metadata(path)
                .map(|m| (m.modified().ok(), m.len()))
                .unwrap_or((None, u64::MAX));
            if &now != was {
                return true;
            }
        }
        discovered_more(root, &self.stamps)
    }
}

pub fn stamps(files: &[PathBuf]) -> BTreeMap<PathBuf, Stamp> {
    files
        .iter()
        .map(|path| {
            let stamp = std::fs::metadata(path)
                .map(|m| (m.modified().ok(), m.len()))
                .unwrap_or((None, u64::MAX));
            (path.clone(), stamp)
        })
        .collect()
}

/// Whether the tree's `.ply` count differs from the held state's.
fn discovered_more(root: &Path, known: &BTreeMap<PathBuf, Stamp>) -> bool {
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            // A single-file root, or a tree that moved; the load path reports either.
            return dir == root && root.is_file() && !known.contains_key(root);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_none_or(|n| n != ".ply-cache") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "ply") {
                seen += 1;
            }
        }
    }
    seen != known.len()
}

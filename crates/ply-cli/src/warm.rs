//! What survives one iteration of a warm process, so the next need not re-establish it.

use crate::load::{Found, Loaded, Stamp, stamp_of};
use ply_store::ContentHash;
use ply_ty::{DefHash, HashOutput};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
        let Some(mut held) = held else {
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
            // Re-stamp, or every later iteration rereads these files. Onto the held state too,
            // since it is what the next `keep` writes the baseline back from.
            for file in &mut held.files {
                if let Some(stamp) = now.get(&file.path) {
                    file.stamp = *stamp;
                }
            }
            self.stamps = now;
            return (Some(held), Reuse::Whole);
        }
        (None, Reuse::Reloaded { changed })
    }

    /// The next iteration's baseline: what this load read, never a later look at the disk — the
    /// iteration ends long after the read, and a save in between is the change it exists to catch.
    pub fn keep(&mut self, loaded: Loaded) {
        self.stamps.clear();
        self.content.clear();
        for file in &loaded.files {
            self.stamps.insert(file.path.clone(), file.stamp);
            self.content.insert(file.path.clone(), file.content);
        }
        self.held = Some(loaded);
    }

    /// The unit the last iteration compiled, placed at `front`'s layout, if this iteration would
    /// compile the same one.
    pub fn unit_for(
        &self,
        spec: &ply_eval::BackendSpec,
        front: &ply_ty::Front,
        sources: &ply_span::SourceMap,
    ) -> Option<&'static dyn ply_eval::Provider> {
        let held = self.unit.as_ref()?;
        (held.spec == *spec
            && held.key == front.hashes.digest()
            && held.provider.relocate(front, sources))
        .then_some(held.provider)
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
            if &stamp_of(path) != was {
                return true;
            }
        }
        discovered_more(root, &self.stamps)
    }
}

/// What these files stamp now, which the held baseline is compared against.
pub fn stamps(files: &[Found]) -> BTreeMap<PathBuf, Stamp> {
    files
        .iter()
        .map(|f| (f.path.clone(), stamp_of(&f.path)))
        .collect()
}

/// Whether the tree's `.ply` count differs from the held state's.
fn discovered_more(root: &Path, known: &BTreeMap<PathBuf, Stamp>) -> bool {
    match sources_under(root) {
        // A single-file root, or a tree that moved; the load path reports either.
        None => root.is_file() && !known.contains_key(root),
        Some(found) => found.len() != known.len(),
    }
}

/// Every `.ply` file under `root`, or `None` when a directory could not be read.
fn sources_under(root: &Path) -> Option<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_none_or(|n| n != ".ply-cache") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "ply") {
                found.push(path);
            }
        }
    }
    Some(found)
}

/// How the tree stamps now, for a caller that watches it without holding a front end.
pub fn tree_stamps(root: &Path) -> BTreeMap<PathBuf, Stamp> {
    sources_under(root)
        .unwrap_or_else(|| vec![root.to_path_buf()])
        .into_iter()
        .map(|path| {
            let stamp = stamp_of(&path);
            (path, stamp)
        })
        .collect()
}

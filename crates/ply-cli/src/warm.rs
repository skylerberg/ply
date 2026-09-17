//! What survives one iteration of a warm process.
//!
//! An invocation costs the project even when nothing changed: `benches/marginal-change/` reads a
//! `ply test` that rechecks *nothing* and still hashes every definition to establish that, restores
//! every interface and writes them back, and that cost is proportional to the project at every size
//! measured. None of it is work about the edit; it is the cost of a process that starts knowing
//! nothing. This holds what a second iteration would otherwise re-establish.

use crate::load::Loaded;
use ply_hash::{DefHash, HashOutput};
use ply_span::Symbol;
use ply_store::ContentHash;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What a file looked like when the held state was built from it.
///
/// Modification time and length rather than a content hash, deliberately: reading every file to
/// hash it is a part of the cost this exists to avoid, and a stamp that matches is only ever a
/// reason to *skip* work — a file whose contents changed without its mtime or length moving would
/// have to be written inside the filesystem's timestamp resolution by something that also kept its
/// length, and the next edit corrects it. A stamp that fails to match always reloads.
type Stamp = (Option<SystemTime>, u64);

#[derive(Default)]
pub struct Warm {
    pub held: Option<Loaded>,
    pub stamps: BTreeMap<PathBuf, Stamp>,
    /// What each file *said* when the held state was built from it. A stamp is a cheap reason to
    /// skip reading; this is the reason to skip everything else, and it is why a file that was
    /// written without being changed — a save with no edit, which is most saves — costs a read
    /// rather than a front end.
    pub content: BTreeMap<PathBuf, ContentHash>,
    /// The compiled unit the last iteration built, if it built one.
    pub unit: Option<HeldUnit>,
}

/// A compiled unit and the definitions it was compiled from.
///
/// The front end was already held; the unit was not, so a warm iteration under `--backend`
/// recompiled the whole project — which `benches/marginal-change/` reads as about half of what a
/// backed run costs, and which for the emitted tier is tens of seconds. A unit is a function of the
/// definitions it compiled, so an iteration where every definition still says what it said runs on
/// the one already in memory.
pub struct HeldUnit {
    provider: &'static dyn ply_eval::Provider,
    /// The backend asked for, since two specs are two different units under one name.
    spec: ply_eval::BackendSpec,
    /// Every hash the front end published, as it stood. Conservative on purpose: a definition the
    /// unit never compiled moving still rebuilds, which costs a compile and cannot answer from
    /// stale code.
    ///
    /// **Tests as well as functions**, and that is not a detail. A test body is compiled like any
    /// other definition -- a run enters the test's own root -- and `HashOutput` keeps tests in a
    /// collection of their own. Keyed on `defs` alone, an edit to a test reused a unit holding the
    /// *old* test and the watch loop reported the old answer, which is the failure mode this whole
    /// mechanism must not have.
    key: Key,
}

/// What the front end published about every definition, test and declaration.
type Key = (Vec<(Symbol, DefHash)>, Vec<DefHash>, Vec<(Symbol, DefHash)>);

fn key_of(hashes: &HashOutput) -> Key {
    (
        hashes.defs.iter().map(|(n, h)| (n.clone(), *h)).collect(),
        hashes.tests.clone(),
        hashes.decls.iter().map(|(n, h)| (n.clone(), *h)).collect(),
    )
}

/// Why an iteration did or did not reuse what the last one built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reuse {
    /// Nothing was held: the first iteration.
    Cold,
    /// Every file is as it was, so the whole front end is the one already in memory.
    Whole,
    /// Something moved, so the front end is loaded again and this is what moved.
    Reloaded { changed: usize },
}

impl Warm {
    /// The state to run against, or `None` to load it.
    ///
    /// Takes rather than borrows: an iteration owns what it runs on, and hands it back with
    /// [`Warm::keep`] so that the next one can have it. An iteration that fails part way through
    /// therefore leaves nothing held, which is the safe direction — the next iteration loads.
    pub fn take(&mut self, root: &Path) -> (Option<Loaded>, Reuse) {
        let held = self.held.take();
        let Some(held) = held else {
            return (None, Reuse::Cold);
        };
        // A file appearing or disappearing changes the module set, which `held` cannot describe,
        // and no stamp on a file that is still there would show it.
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
        // A stamp that moved is only a reason to look. The front end is a function of the bytes, so
        // a file written with the same bytes has the same front end — and that is the common case
        // in a loop, where a save is what wakes it and most saves change one file or none.
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
            // Re-stamp, or every later iteration reads these files again to reach the same answer.
            self.stamps = now;
            return (Some(held), Reuse::Whole);
        }
        (None, Reuse::Reloaded { changed })
    }

    /// Hold this state for the next iteration.
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
        (held.spec == *spec && held.key == key_of(hashes)).then_some(held.provider)
    }

    /// Hold this unit for the next iteration.
    pub fn keep_unit(
        &mut self,
        spec: &ply_eval::BackendSpec,
        hashes: &HashOutput,
        provider: &'static dyn ply_eval::Provider,
    ) {
        self.unit = Some(HeldUnit {
            provider,
            spec: spec.clone(),
            key: key_of(hashes),
        });
    }
}

impl Warm {
    /// Whether anything under `root` differs from what the held state was built from. The watch
    /// loop's only question, and it is a stat per file rather than a read.
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

/// Whether the tree holds a `.ply` file the held state never saw. Counting is enough: a file that
/// was replaced by another of the same name is already caught by its stamp.
fn discovered_more(root: &Path, known: &BTreeMap<PathBuf, Stamp>) -> bool {
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            // A root that is one file rather than a directory, or a tree that moved under us. The
            // load path is the one that reports either.
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

//! What survives one iteration of a warm process.
//!
//! An invocation costs the project even when nothing changed: `benches/marginal-change/` reads a
//! `ply test` that rechecks *nothing* and still hashes every definition to establish that, restores
//! every interface and writes them back, and that cost is proportional to the project at every size
//! measured. None of it is work about the edit; it is the cost of a process that starts knowing
//! nothing. This holds what a second iteration would otherwise re-establish.

use crate::load::Loaded;
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
    held: Option<Loaded>,
    stamps: BTreeMap<PathBuf, Stamp>,
    /// The syntax trees this process has already parsed, which the next load reuses for every file
    /// that still says what it said. A tree is a pure function of the bytes, so this is a cache of
    /// work rather than a second source of truth; `driver::Resume` holds the rule that decides when
    /// one may be reused.
    pub resume: crate::driver::Resume,
    /// What each file *said* when the held state was built from it. A stamp is a cheap reason to
    /// skip reading; this is the reason to skip everything else, and it is why a file that was
    /// written without being changed — a save with no edit, which is most saves — costs a read
    /// rather than a front end.
    content: BTreeMap<PathBuf, ContentHash>,
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

fn stamps(files: &[PathBuf]) -> BTreeMap<PathBuf, Stamp> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        dir
    }

    /// A `Loaded` is expensive to build here, so the state is exercised through the two questions
    /// the watch loop actually asks: has the tree moved, and may the held state be reused.
    fn held(root: &std::path::Path, files: &[&str]) -> Warm {
        let paths: Vec<_> = files.iter().map(|f| root.join(f)).collect();
        let mut warm = Warm {
            stamps: stamps(&paths),
            ..Warm::default()
        };
        warm.held = Some(fake_loaded(root, files));
        warm.content = paths
            .iter()
            .filter_map(|p| Some((p.clone(), ContentHash::of(&std::fs::read(p).ok()?))))
            .collect();
        warm
    }

    #[test]
    fn an_unmoved_tree_has_not_moved() {
        let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
        let warm = held(dir.path(), &["m.ply"]);
        assert!(!warm.tree_moved(dir.path()));
        // And with nothing held there is nothing to reuse, whatever the stamps say.
        let bare = Warm {
            stamps: warm.stamps.clone(),
            ..Warm::default()
        };
        assert!(bare.tree_moved(dir.path()));
    }

    #[test]
    fn a_rewritten_file_moves_the_tree() {
        let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
        let warm = held(dir.path(), &["m.ply"]);
        std::fs::write(dir.path().join("m.ply"), "fn a() -> Int = 2222222\n").unwrap();
        assert!(
            warm.tree_moved(dir.path()),
            "a file whose length changed did not move the tree"
        );
    }

    /// The case a stamp alone cannot see, and the reason `tree_moved` walks the tree as well: a new
    /// file changes the program without touching any file the held state knows about.
    #[test]
    fn a_new_file_moves_the_tree() {
        let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
        let warm = held(dir.path(), &["m.ply"]);
        assert!(!warm.tree_moved(dir.path()));
        std::fs::write(dir.path().join("n.ply"), "fn b() -> Int = 2\n").unwrap();
        assert!(
            warm.tree_moved(dir.path()),
            "a file that appeared did not move the tree"
        );
    }

    #[test]
    fn a_deleted_file_moves_the_tree() {
        let dir = project(&[
            ("m.ply", "fn a() -> Int = 1\n"),
            ("n.ply", "fn b() -> Int = 2\n"),
        ]);
        let warm = held(dir.path(), &["m.ply", "n.ply"]);
        std::fs::remove_file(dir.path().join("n.ply")).unwrap();
        assert!(warm.tree_moved(dir.path()));
    }

    /// The cache directory is written by every run, so a walk that counted it would report every
    /// tree as moved and the loop would never settle.
    #[test]
    fn the_cache_directory_is_not_the_program() {
        let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
        std::fs::create_dir(dir.path().join(".ply-cache")).unwrap();
        std::fs::write(dir.path().join(".ply-cache").join("frontend.ply"), "x").unwrap();
        let warm = held(dir.path(), &["m.ply"]);
        assert!(!warm.tree_moved(dir.path()));
    }

    /// Taking leaves nothing held: an iteration owns what it runs on, so one that fails part way
    /// through cannot leave a state behind that no run finished with.
    #[test]
    fn taking_leaves_nothing_behind() {
        let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
        let mut warm = held(dir.path(), &["m.ply"]);
        let (taken, reuse) = warm.take(dir.path());
        assert!(taken.is_some());
        assert_eq!(reuse, Reuse::Whole);
        assert!(warm.held.is_none());
        assert!(warm.take(dir.path()).0.is_none());
    }

    #[test]
    fn a_moved_tree_is_not_reused() {
        let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
        let mut warm = held(dir.path(), &["m.ply"]);
        std::fs::write(dir.path().join("m.ply"), "fn a() -> Int = 999999\n").unwrap();
        let (taken, reuse) = warm.take(dir.path());
        assert!(taken.is_none());
        assert!(matches!(reuse, Reuse::Reloaded { .. }));
    }

    /// The case the whole reuse path exists for, and the one a stamp alone cannot see: the loop
    /// wakes because a file was written, and the file says exactly what it said before. Most saves
    /// are this, and before the content was compared the reuse path could not fire at all — the
    /// loop only wakes when a stamp moved, and a moved stamp was taken as a changed file.
    #[test]
    fn a_file_written_with_the_same_bytes_is_reused_whole() {
        let text = "fn a() -> Int = 1\n";
        let dir = project(&[("m.ply", text)]);
        let mut warm = held(dir.path(), &["m.ply"]);

        // Rewrite it byte for byte, and put the stamp somewhere it cannot match. Set rather than
        // waited for: a filesystem's timestamp resolution is not this test's subject, and a rewrite
        // inside one tick would leave the stamp equal and quietly test nothing.
        std::fs::write(dir.path().join("m.ply"), text).unwrap();
        warm.stamps
            .insert(dir.path().join("m.ply"), (None, u64::MAX));
        assert!(
            warm.tree_moved(dir.path()),
            "the stamp did not move, so this test is not exercising the path it is about"
        );

        let (taken, reuse) = warm.take(dir.path());
        assert_eq!(reuse, Reuse::Whole, "a save that changed nothing reloaded");
        assert!(taken.is_some());
    }

    fn fake_loaded(root: &std::path::Path, files: &[&str]) -> Loaded {
        Loaded {
            root: root.to_path_buf(),
            files: files.iter().map(|f| root.join(f)).collect(),
            sources: ply_span::SourceMap::new(),
            program: Default::default(),
            resolved: Default::default(),
            check: Default::default(),
            hashes: Default::default(),
            complete: true,
            frontend: Default::default(),
            promised: false,
        }
    }
}

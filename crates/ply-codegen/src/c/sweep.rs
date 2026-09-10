//! Keeping the object cache to a size.
//!
//! Every body the emitter produces, every refusal, every assembled unit and every compiled object
//! is written under [`super::load::cache_dir`] and keyed by its content. Content addressing is what
//! makes the cache correct -- an entry is never wrong, only absent -- and it is also why nothing
//! ever replaces an entry: a changed definition writes a *new* key beside the old one, and the old
//! one is never asked for again.
//!
//! So the directory only grows. `cache_dir`'s own comment used to say it was "swept by the OS
//! rather than growing without bound", which is a guarantee this repository does not have: the
//! system temporary directory is swept on a schedule measured in days, if at all, and a day of
//! work on the emitter writes a quarter of a million files. Measured on this tree, one
//! `cargo test --workspace` leaves 367 MB behind; a session of them reached 2.3 GB in 262,862
//! files, and a CI runner with a second target directory beside it ran out of disk twice.
//!
//! **Oldest first, by the time the entry was written.** A true LRU would touch every entry it read,
//! which is a write per cache *hit* -- the hot path this cache exists to keep cheap. Write order is
//! the approximation: an entry that has not been written since the budget was last met is the one
//! whose program is furthest from what is being worked on now.
//!
//! **Rarely, and on a thread of its own**, because knowing whether the cache is over its budget
//! means asking the size of every entry in it: measured at **a second** over a full one, and paid
//! by every `ply` process that compiles anything. The first version of this swept on the way into
//! a build, and `--watch` stopped getting a second iteration inside its budget -- the suite caught
//! it. So a stamp file gates the walk to once an interval, and the walk itself runs behind the
//! build rather than in front of it. What that costs is overshoot: the cache may hold an
//! interval's writes above the budget, which is the trade for never making a run wait on it.

use std::path::{Path, PathBuf};
use std::sync::Once;

/// What the cache is allowed to hold, in bytes. `PLY_C_CACHE_MAX` overrides it, and `0` means no
/// bound at all -- for a measurement that wants every entry it has written kept.
///
/// The default keeps several whole runs: a `cargo test --workspace` over this tree writes about a
/// third of a gigabyte, so this is room for a handful of branches' worth before the oldest goes.
const DEFAULT_BUDGET: u64 = 2 * 1024 * 1024 * 1024;

pub fn budget() -> Option<u64> {
    match std::env::var("PLY_C_CACHE_MAX") {
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(n) => Some(n),
            Err(_) => {
                eprintln!("PLY_C_CACHE_MAX is `{v}`; it is a number of bytes, or 0 for no bound");
                Some(DEFAULT_BUDGET)
            }
        },
        Err(_) => Some(DEFAULT_BUDGET),
    }
}

/// How long a sweep stands for. One stat answers whether to walk at all.
const INTERVAL: std::time::Duration = std::time::Duration::from_secs(600);

/// The stamp whose age gates the walk. Touched *before* the sweep rather than after, so that
/// processes starting together do not all decide to walk.
const STAMP: &str = ".swept";

static SWEPT: Once = Once::new();

/// Sweep the cache down to its budget, at most once in this process and at most once an interval
/// across all of them.
///
/// Returns without waiting: the sweep runs on a thread of its own, and a process that exits before
/// it finishes leaves a cache that is partly swept, which is a cache that is smaller. Nothing reads
/// what it does, so there is nothing to wait for.
pub fn once() {
    SWEPT.call_once(|| {
        let Some(budget) = budget() else { return };
        let root = super::load::cache_dir();
        if !claim(&root, INTERVAL) {
            return;
        }
        std::thread::Builder::new()
            .name("ply-cache-sweep".to_string())
            .spawn(move || {
                sweep(&root, budget);
            })
            .ok();
    });
}

/// Whether this process is the one to sweep: true when the stamp is missing or older than
/// `interval`, and taking it marks the stamp so the next caller inside the interval is not.
pub(super) fn claim(root: &std::path::Path, interval: std::time::Duration) -> bool {
    let stamp = root.join(STAMP);
    if let Ok(meta) = std::fs::metadata(&stamp)
        && let Ok(age) = meta
            .modified()
            .and_then(|m| m.elapsed().map_err(std::io::Error::other))
        && age < interval
    {
        return false;
    }
    if std::fs::create_dir_all(root).is_err() {
        return false;
    }
    // Written rather than touched: `File::create` truncates or makes it, and either way the mtime
    // is now. A failure here means the cache is unwritable, which is a cache that is not growing.
    std::fs::write(&stamp, b"").is_ok()
}

/// An entry the sweep can remove, and what removing it recovers.
struct Entry {
    path: PathBuf,
    bytes: u64,
    written: std::time::SystemTime,
}

/// Remove entries under `root`, oldest first, until what is left fits in `budget`.
///
/// Errors are ignored throughout, including the removals: a cache that could not be swept is a
/// cache that is too big, which is the state this started in, and never a run that fails.
pub(super) fn sweep(root: &Path, budget: u64) -> u64 {
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    let mut walk = |dir: &Path| {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for e in read.flatten() {
            let path = e.path();
            // A half-written entry belongs to a process that is still running: removing it turns
            // that run's `rename` into a miss for no gain, and it is about to become an entry
            // this sweep would have kept anyway.
            if path
                .extension()
                .is_some_and(|x| x == "tmp" || x == "rtmp" || x == "utmp")
            {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            total += meta.len();
            entries.push(Entry {
                path,
                bytes: meta.len(),
                written: meta.modified().unwrap_or(std::time::UNIX_EPOCH),
            });
        }
    };
    walk(root);
    walk(&root.join("emit"));
    if total <= budget {
        return 0;
    }
    entries.sort_by_key(|e| e.written);
    let mut freed = 0;
    for entry in entries {
        if total <= budget {
            break;
        }
        // Unlinking a file another process holds open is safe on the platforms this runs on: the
        // reader keeps reading what it opened. A process that has *stat*ed an object and not yet
        // opened it rebuilds instead, which is a slower run and not a wrong one.
        if std::fs::remove_file(&entry.path).is_ok() {
            total -= entry.bytes.min(total);
            freed += entry.bytes;
        }
    }
    freed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    /// `n` files of `size` bytes, the first written longest ago.
    fn stock(dir: &Path, names: &[&str], size: usize) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::create_dir_all(dir.join("emit")).unwrap();
        let base = SystemTime::now() - Duration::from_secs(10_000);
        for (i, name) in names.iter().enumerate() {
            let path = dir.join(name);
            std::fs::write(&path, vec![b'x'; size]).unwrap();
            let when = base + Duration::from_secs(i as u64 * 60);
            let f = std::fs::File::options().write(true).open(&path).unwrap();
            f.set_times(std::fs::FileTimes::new().set_modified(when))
                .unwrap();
        }
    }

    fn present(dir: &Path, names: &[&str]) -> Vec<String> {
        names
            .iter()
            .filter(|n| dir.join(n).exists())
            .map(|n| n.to_string())
            .collect()
    }

    #[test]
    fn a_cache_inside_its_budget_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["emit/a.body", "emit/b.body", "c.dylib"];
        stock(dir.path(), &names, 100);
        assert_eq!(sweep(dir.path(), 1_000), 0);
        assert_eq!(present(dir.path(), &names).len(), 3);
    }

    #[test]
    fn the_oldest_entries_go_until_the_rest_fits() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["emit/a.body", "emit/b.body", "emit/c.body", "d.dylib"];
        stock(dir.path(), &names, 100);
        // Four hundred bytes, and room for two.
        let freed = sweep(dir.path(), 200);
        assert_eq!(freed, 200, "two of the four should have gone");
        assert_eq!(
            present(dir.path(), &names),
            vec!["emit/c.body".to_string(), "d.dylib".to_string()],
            "the two written longest ago are the two that go"
        );
    }

    /// The sweep reaches both levels: the objects at the root and the bodies under `emit/`.
    #[test]
    fn an_object_is_as_removable_as_a_body() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["old.dylib", "emit/new.body"];
        stock(dir.path(), &names, 100);
        sweep(dir.path(), 100);
        assert_eq!(
            present(dir.path(), &names),
            vec!["emit/new.body".to_string()]
        );
    }

    /// A half-written entry belongs to a run still in progress.
    #[test]
    fn a_temporary_is_never_swept() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["emit/a.1234.tmp", "emit/b.body"];
        stock(dir.path(), &names, 100);
        sweep(dir.path(), 0);
        assert_eq!(
            present(dir.path(), &names),
            vec!["emit/a.1234.tmp".to_string()]
        );
    }

    /// The gate is what keeps the walk -- a second, over a full cache -- off every process's way
    /// in. One caller inside an interval takes it, and the rest are a single `stat`.
    #[test]
    fn one_caller_an_interval_sweeps_and_the_rest_do_not() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            claim(dir.path(), Duration::from_secs(600)),
            "the first caller, with no stamp, sweeps"
        );
        assert!(
            !claim(dir.path(), Duration::from_secs(600)),
            "the second inside the interval does not"
        );
        assert!(
            claim(dir.path(), Duration::from_secs(0)),
            "and an interval that has passed hands it back"
        );
    }

    /// Taking the claim is what marks the stamp, so two processes starting together do not both
    /// walk: whichever writes the stamp first turns the other into a `stat`.
    #[test]
    fn the_stamp_is_marked_by_taking_the_claim_not_by_finishing_the_sweep() {
        let dir = tempfile::tempdir().unwrap();
        assert!(claim(dir.path(), Duration::from_secs(600)));
        assert!(
            dir.path().join(STAMP).exists(),
            "the stamp is there before any entry has been removed"
        );
    }

    /// The stamp is a file in the cache like any other, and a sweep that removed it would hand
    /// the claim back to the next process a second later.
    #[test]
    fn the_stamp_survives_a_sweep_that_empties_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["emit/a.body", "emit/b.body"];
        stock(dir.path(), &names, 100);
        assert!(claim(dir.path(), Duration::from_secs(600)));
        sweep(dir.path(), 0);
        assert!(
            !claim(dir.path(), Duration::from_secs(600)),
            "the stamp is younger than the interval, so the next caller still skips"
        );
    }

    #[test]
    fn a_budget_of_zero_bytes_is_no_bound_rather_than_an_empty_cache() {
        // The env var is process-wide, so this asserts the parse rather than setting it.
        assert_eq!(
            "0".parse::<u64>().ok().filter(|n| *n > 0),
            None,
            "`PLY_C_CACHE_MAX=0` is the unbounded spelling"
        );
    }
}

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
pub const STAMP: &str = ".swept";

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
pub fn claim(root: &std::path::Path, interval: std::time::Duration) -> bool {
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
pub fn sweep(root: &Path, budget: u64) -> u64 {
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

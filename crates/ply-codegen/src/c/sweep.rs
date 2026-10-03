//! Keeping the content-addressed object cache and the stage directory to a size, gated by a stamp
//! and run on a background thread so no build waits on it. Both go least recently used first: a
//! read of an entry is recorded by [`used`], since an entry every run reads is written only once.

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{Duration, SystemTime};

/// What each of the two roots may hold, in bytes; `PLY_C_CACHE_MAX` overrides it, `0` meaning no
/// bound.
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

/// How long a sweep stands for.
const INTERVAL: std::time::Duration = std::time::Duration::from_secs(600);

/// The stamp whose age gates the walk; touched before sweeping so concurrent processes skip it.
pub const STAMP: &str = ".swept";

static SWEPT: Once = Once::new();

/// Sweep both roots down to their budget in the background, at most once per process and interval.
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
                sweep_stages(&super::bundle::stage_root(), budget, SystemTime::now());
            })
            .ok();
    });
}

/// Whether this process should sweep: the stamp is missing or stale, and it is now refreshed.
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
    std::fs::write(&stamp, b"").is_ok()
}

struct Entry {
    path: PathBuf,
    bytes: u64,
    last_used: std::time::SystemTime,
}

/// Remove entries under `root`, least recently used first, until the rest fits in `budget`. Errors
/// are ignored.
pub fn sweep(root: &Path, budget: u64) -> u64 {
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    let mut walk = |dir: &Path| {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for e in read.flatten() {
            let path = e.path();
            // A half-written entry belongs to a running process about to rename it.
            if path.extension().is_some_and(|x| x == "tmp") {
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
                last_used: meta.modified().unwrap_or(std::time::UNIX_EPOCH),
            });
        }
    };
    walk(root);
    walk(&root.join("emit"));
    walk(&root.join("obj"));
    walk(&root.join("answers"));
    if total <= budget {
        return 0;
    }
    entries.sort_by_key(|e| e.last_used);
    let mut freed = 0;
    for entry in entries {
        if total <= budget {
            break;
        }
        // Unlinking an open file is safe here; a reader that has not yet opened it just rebuilds.
        if std::fs::remove_file(&entry.path).is_ok() {
            total -= entry.bytes.min(total);
            freed += entry.bytes;
        }
    }
    freed
}

/// Where a stage records its last use, since a directory's own time moves only when it is written.
pub const USED: &str = ".used";

/// The stage-directory entry holding one file per opened artifact, each swept on its own.
pub const FRONTS: &str = "artifact-fronts";

/// The stage-directory entry holding one file per closure `ply run` loaded, each swept on its own.
pub const RUNS: &str = "run-fronts";

/// The stage-directory entries that hold files swept one by one, where any other is a stage swept
/// whole.
const BY_FILE: [&str; 2] = [FRONTS, RUNS];

/// An entry used within this long is never swept: a run may still be reading it.
const RECENT: Duration = Duration::from_secs(3600);

/// Records that a stage directory or a cached file was just used. A file is opened for reading
/// only: an object a loader has mapped may refuse a writer.
pub fn used(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::write(path.join(USED), b"");
    } else if let Ok(f) = std::fs::File::open(path) {
        let _ = f.set_times(std::fs::FileTimes::new().set_modified(SystemTime::now()));
    }
}

/// Remove stage-directory entries, least recently used first, until the rest fits in `budget`.
/// Each stage directory goes whole; each file under one of [`BY_FILE`] goes on its own. Errors are
/// ignored.
pub fn sweep_stages(root: &Path, budget: u64, now: SystemTime) -> u64 {
    let Ok(read) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    for e in read.flatten() {
        let path = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_dir() {
            continue;
        }
        if BY_FILE.iter().any(|name| e.file_name() == *name) {
            let Ok(fronts) = std::fs::read_dir(&path) else {
                continue;
            };
            for f in fronts.flatten() {
                let path = f.path();
                if path.extension().is_some_and(|x| x == "tmp") {
                    continue;
                }
                let Ok(meta) = f.metadata() else { continue };
                if !meta.is_file() {
                    continue;
                }
                total += meta.len();
                entries.push(Entry {
                    path,
                    bytes: meta.len(),
                    last_used: meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                });
            }
            continue;
        }
        let bytes = size_of(&path);
        let last = std::fs::metadata(path.join(USED))
            .and_then(|m| m.modified())
            .into_iter()
            .chain(meta.modified())
            .max()
            .unwrap_or(std::time::UNIX_EPOCH);
        total += bytes;
        entries.push(Entry {
            path,
            bytes,
            last_used: last,
        });
    }
    if total <= budget {
        return 0;
    }
    entries.retain(|e| {
        now.duration_since(e.last_used)
            .is_ok_and(|age| age >= RECENT)
    });
    entries.sort_by_key(|e| e.last_used);
    let mut freed = 0;
    for entry in entries {
        if total <= budget {
            break;
        }
        let gone = if entry.path.is_dir() {
            std::fs::remove_dir_all(&entry.path)
        } else {
            std::fs::remove_file(&entry.path)
        };
        if gone.is_ok() {
            total -= entry.bytes.min(total);
            freed += entry.bytes;
        }
    }
    freed
}

fn size_of(path: &Path) -> u64 {
    let Ok(read) = std::fs::read_dir(path) else {
        return 0;
    };
    read.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => size_of(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

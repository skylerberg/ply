//! Keeping the content-addressed C cache and the stage directory to a size, gated by a stamp a
//! sweep leaves when it finishes and run on a background thread so no build waits on it. Both go least recently used first: a
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

/// How long a finished sweep stands for.
const INTERVAL: Duration = Duration::from_secs(600);

/// How long a sweep that began is given before another process may begin one: the process that
/// began it may have exited mid-walk, which finishes nothing.
const LEASE: Duration = Duration::from_secs(120);

/// Written when a sweep finishes; its age gates the next.
pub const STAMP: &str = ".swept";

/// Touched when a sweep begins, so processes starting together do not all walk.
pub const BEGUN: &str = ".sweeping";

static SWEPT: Once = Once::new();

/// Sweep both roots down to their budget in the background, at most once per process, when the
/// last sweep to finish is an interval old and none began within the lease.
pub fn once() {
    SWEPT.call_once(|| {
        let Some(budget) = budget() else { return };
        let root = super::load::cache_dir();
        if !due(&root, INTERVAL) || !claim(&root, LEASE) {
            return;
        }
        std::thread::Builder::new()
            .name("ply-cache-sweep".to_string())
            .spawn(move || {
                sweep(&root, budget);
                sweep_stages(&super::stage::stage_root(), budget, SystemTime::now());
                finished(&root);
            })
            .ok();
    });
}

fn younger(mark: &Path, than: Duration) -> bool {
    std::fs::metadata(mark)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < than)
}

/// Whether a sweep is owed: none has finished, or the last to finish is `interval` old.
pub fn due(root: &Path, interval: Duration) -> bool {
    !younger(&root.join(STAMP), interval)
}

/// Whether this process may begin a sweep: none began within `lease`, and this one now has.
pub fn claim(root: &Path, lease: Duration) -> bool {
    let begun = root.join(BEGUN);
    if younger(&begun, lease) || std::fs::create_dir_all(root).is_err() {
        return false;
    }
    std::fs::write(&begun, b"").is_ok()
}

/// Records that a sweep ran to its end.
pub fn finished(root: &Path) {
    let _ = std::fs::write(root.join(STAMP), b"");
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
            // A half-written entry belongs to a running process about to rename it, and the
            // sweep's own marks are not entries.
            if path.extension().is_some_and(|x| x == "tmp")
                || e.file_name() == STAMP
                || e.file_name() == BEGUN
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
                last_used: meta.modified().unwrap_or(std::time::UNIX_EPOCH),
            });
        }
    };
    walk(root);
    walk(&root.join("obj"));
    walk(&root.join("bodies"));
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

/// The stage-directory entry holding one runnable per program the builder answered for in memory,
/// each swept on its own.
pub const ANSWERED: &str = "answered";

/// The stage-directory entry holding one file per closure `ply run` loaded, each swept on its own.
pub const RUNS: &str = "run-fronts";

/// The stage-directory entry holding the rows each build of a program published, a file per
/// program and front end that published them, each swept on its own.
pub const ROWS: &str = "rows";

/// The stage-directory entries that hold files swept one by one, where any other is a stage swept
/// whole.
const BY_FILE: [&str; 3] = [ANSWERED, RUNS, ROWS];

/// An entry used within this long is never swept: a run may still be reading it.
const RECENT: Duration = Duration::from_secs(3600);

/// Records that a stage directory or a cached file was just used. A file is opened for reading
/// only: an object a loader has mapped may refuse a writer. A mark younger than [`MARKED`] stands,
/// since the sweep orders entries by hours.
pub fn used(path: &Path) {
    let marked = if path.is_dir() {
        path.join(USED)
    } else {
        path.to_path_buf()
    };
    let fresh = std::fs::metadata(&marked)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < MARKED);
    if fresh {
        return;
    }
    if path.is_dir() {
        let _ = std::fs::write(marked, b"");
    } else if let Ok(f) = std::fs::File::open(path) {
        let _ = f.set_times(std::fs::FileTimes::new().set_modified(SystemTime::now()));
    }
}

/// How old a mark may be and still stand for a use now.
const MARKED: Duration = Duration::from_secs(600);

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

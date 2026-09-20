//! Keeping the content-addressed object cache to a size: oldest-written first (true LRU would
//! write on every hit), gated by a stamp and run on a background thread so no build waits on it.

use std::path::{Path, PathBuf};
use std::sync::Once;

/// What the cache may hold, in bytes; `PLY_C_CACHE_MAX` overrides it, `0` meaning no bound.
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

/// Sweep the cache down to its budget in the background, at most once per process and interval.
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
    written: std::time::SystemTime,
}

/// Remove entries under `root`, oldest first, until the rest fits in `budget`. Errors are ignored.
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
            if path
                .extension()
                .is_some_and(|x| x == "tmp" || x == "rtmp" || x == "utmp" || x == "otmp")
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
    walk(&root.join("obj"));
    if total <= budget {
        return 0;
    }
    entries.sort_by_key(|e| e.written);
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

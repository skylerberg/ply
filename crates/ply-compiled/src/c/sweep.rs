use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{Duration, SystemTime};

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

const INTERVAL: Duration = Duration::from_secs(600);

const LEASE: Duration = Duration::from_secs(120);

pub const STAMP: &str = ".swept";

pub const BEGUN: &str = ".sweeping";

static SWEPT: Once = Once::new();

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

pub fn due(root: &Path, interval: Duration) -> bool {
    !younger(&root.join(STAMP), interval)
}

pub fn claim(root: &Path, lease: Duration) -> bool {
    let begun = root.join(BEGUN);
    if younger(&begun, lease) || std::fs::create_dir_all(root).is_err() {
        return false;
    }
    std::fs::write(&begun, b"").is_ok()
}

pub fn finished(root: &Path) {
    let _ = std::fs::write(root.join(STAMP), b"");
}

struct Entry {
    path: PathBuf,
    bytes: u64,
    last_used: std::time::SystemTime,
}

pub fn sweep(root: &Path, budget: u64) -> u64 {
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    let stores = root.join(STORES);
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in read.flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "tmp")
                || e.file_name() == STAMP
                || e.file_name() == BEGUN
            {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() && dir == stores {
                let bytes = size_of(&path);
                total += bytes;
                entries.push(Entry {
                    last_used: last_use(&path, &meta),
                    path,
                    bytes,
                });
                continue;
            }
            if meta.is_dir() {
                dirs.push(path);
                continue;
            }
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
    }
    if total <= budget {
        return 0;
    }
    let now = SystemTime::now();
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
        if removed(&entry.path) {
            total -= entry.bytes.min(total);
            freed += entry.bytes;
        }
    }
    freed
}

pub const STORES: &str = "bodies";

pub const USED: &str = ".used";

fn last_use(path: &Path, meta: &std::fs::Metadata) -> SystemTime {
    std::fs::metadata(path.join(USED))
        .and_then(|m| m.modified())
        .into_iter()
        .chain(meta.modified())
        .max()
        .unwrap_or(std::time::UNIX_EPOCH)
}

fn removed(path: &Path) -> bool {
    if path.is_dir() {
        std::fs::remove_dir_all(path).is_ok()
    } else {
        std::fs::remove_file(path).is_ok()
    }
}

pub const RETURNED: &str = "answered";

pub const REUSED: &str = "reused";

const BY_FILE: [&str; 2] = [RETURNED, REUSED];

const RECENT: Duration = Duration::from_secs(3600);

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
        .is_some_and(|age| age < MARK_REFRESH_AGE);
    if fresh {
        return;
    }
    if path.is_dir() {
        let _ = std::fs::write(marked, b"");
    } else if let Ok(f) = std::fs::File::open(path) {
        let _ = f.set_times(std::fs::FileTimes::new().set_modified(SystemTime::now()));
    }
}

const MARK_REFRESH_AGE: Duration = Duration::from_secs(600);

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
            let Ok(files) = std::fs::read_dir(&path) else {
                continue;
            };
            for f in files.flatten() {
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
        total += bytes;
        entries.push(Entry {
            last_used: last_use(&path, &meta),
            path,
            bytes,
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
        if removed(&entry.path) {
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

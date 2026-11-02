use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const RECORD: &str = "binary";

static DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn claim(identity: &str) -> &'static Path {
    DIR.get_or_init(|| {
        let exe = std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .unwrap_or_default();
        claimed(
            &super::cache_dir_for("binaries", "PLY_C_CACHE"),
            identity,
            &exe,
        )
    })
}

pub fn binary_dir() -> &'static Path {
    DIR.get()
        .expect("the launcher claims the binary's directory before anything reads it")
}

pub fn described(binary: &Path) -> Option<String> {
    let meta = std::fs::metadata(binary).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!(
        "{}\n{}\n{modified}\n",
        binary.display(),
        meta.len()
    ))
}

pub fn claimed(root: &Path, identity: &str, binary: &Path) -> PathBuf {
    let dir = root.join(identity);
    released_except(root, &dir);
    let _ = std::fs::create_dir_all(&dir);
    if let Some(description) = described(binary) {
        let _ = ply_core::files::write_atomically(&dir.join(RECORD), description.as_bytes());
    }
    dir
}

pub fn released_except(root: &Path, own: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for dir in entries.flatten().map(|entry| entry.path()) {
        if dir == own {
            continue;
        }
        let Ok(recorded) = std::fs::read_to_string(dir.join(RECORD)) else {
            continue;
        };
        let binary = recorded.lines().next().unwrap_or_default();
        if described(Path::new(binary)).as_deref() != Some(recorded.as_str()) {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

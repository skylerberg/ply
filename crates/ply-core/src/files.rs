//! Files a reader may open while they are being written.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A name beside `path` that no other write uses: the pid alone would let two threads of one
/// process write through the same temporary.
pub fn temp_beside(path: &Path) -> PathBuf {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(
        "{name}.{}.{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ))
}

/// `bytes` at `path` by a rename, so a reader sees the old file or the new one, never half of one.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = temp_beside(path);
    let landed = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if landed.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    landed
}

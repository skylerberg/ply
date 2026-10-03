//! The committed builder, embedded when one is committed: the runnable `ply bootstrap` wrote for the
//! compiler's own `build.main`, and the digest of what it was built from. A checkout without one
//! embeds nothing, and the launcher builds its first builder another way.

use std::path::{Path, PathBuf};

/// What `ply bootstrap` writes into `bootstrap/`; a file of another name there is one nothing embeds.
const WRITTEN: [&str; 4] = ["build.run", "build.digest", "unit.c.gz", "SOURCES.digest"];

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("bootstrap");
    println!("cargo:rerun-if-changed={}", dir.display());
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            WRITTEN.contains(&name.as_str()),
            "`bootstrap/{name}` is something `ply bootstrap` does not write and nothing embeds; it writes {WRITTEN:?}"
        );
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    for name in ["build.run", "build.digest"] {
        let bytes = std::fs::read(dir.join(name)).unwrap_or_default();
        std::fs::write(out.join(name), bytes).expect("the build script writes OUT_DIR");
    }
}

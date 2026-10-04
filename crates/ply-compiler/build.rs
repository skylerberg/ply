//! `bootstrap/` holds what `ply bootstrap` writes and nothing else.

use std::path::Path;

/// What `ply bootstrap` writes into `bootstrap/`; a file of another name there is one nothing embeds.
const WRITTEN: [&str; 3] = ["build.run", "build.digest", "build.key"];

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
}

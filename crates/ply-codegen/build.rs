//! The digest of the runtime an emitted body is a function of: every file under `src/` of this crate
//! and of the workspace crates it links whose code runs while the emitter emits. `ply-std` and
//! `ply-compiler` are left out: what they hold is Ply sources and the bundle, which a body's key
//! already names through its definition's key and the emitter's identity.

use std::path::{Path, PathBuf};

const CRATES: &[&str] = &["ply-codegen", "ply-eval", "ply-ty", "ply-span"];

fn main() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for name in CRATES {
        let src = crates.join(name).join("src");
        println!("cargo:rerun-if-changed={}", src.display());
        walk(&src, &mut files);
    }
    files.sort();
    let mut h = blake3::Hasher::new();
    for path in &files {
        let name = path
            .strip_prefix(&crates)
            .expect("every file walked is under crates/")
            .to_string_lossy()
            .into_owned();
        let bytes = std::fs::read(path)
            .unwrap_or_else(|e| panic!("the source file {} reads: {e}", path.display()));
        h.update(&(name.len() as u64).to_le_bytes());
        h.update(name.as_bytes());
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    }
    println!(
        "cargo:rustc-env=PLY_RUNTIME_DIGEST={}",
        h.finalize().to_hex()
    );
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("the source directory {} reads: {e}", dir.display()))
    {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[path = "src/code.rs"]
mod code;

use std::path::{Path, PathBuf};

const RUNTIME: &[&str] = &[
    "ply-core",
    "ply-compiled",
    "ply-privileged",
    "ply-host",
    "ply-launcher",
];

fn main() {
    let repo = code::normalize(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let lock = std::fs::read_to_string(repo.join("Cargo.lock")).expect("Cargo.lock reads");
    println!(
        "cargo:rerun-if-changed={}",
        repo.join("Cargo.lock").display()
    );
    println!(
        "cargo:rustc-env=PLY_RUNTIME_SOURCES={}",
        digest(
            &repo,
            &crate_files(&repo, RUNTIME),
            &[code::reachable_lock_entries(&lock, RUNTIME).as_bytes()]
        )
    );
}

fn crate_files(repo: &Path, crates: &[&str]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for name in crates {
        let dir = repo.join("crates").join(name);
        println!("cargo:rerun-if-changed={}", dir.join("src").display());
        walk(&dir.join("src"), &mut paths);
        paths.push(dir.join("Cargo.toml"));
    }
    paths
}

fn digest(repo: &Path, files: &[PathBuf], extra: &[&[u8]]) -> String {
    let mut paths = files.to_vec();
    paths.sort();
    paths.dedup();
    let mut h = blake3::Hasher::new();
    for path in &paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path
            .strip_prefix(repo)
            .expect("every file digested is in the repository")
            .to_string_lossy()
            .into_owned();
        let bytes = std::fs::read(path)
            .unwrap_or_else(|e| panic!("the source file {} reads: {e}", path.display()));
        h.update(&(name.len() as u64).to_le_bytes());
        h.update(name.as_bytes());
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    }
    for bytes in extra {
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    h.finalize().to_hex().to_string()
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

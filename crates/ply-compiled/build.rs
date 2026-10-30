//! A digest of the runtime. `PLY_SEMANTICS_DIGEST` is every file under `src/` of this crate and of
//! the workspace crates it links, less the files that only cache, sweep and load C: what a result
//! the program's own emitter gives, running inside a loaded unit, is a function of. The Ply
//! packages are left out: what they hold is Ply sources and the builder, which a body's key already
//! names through its definition's key and the emitter's identity.

use std::path::{Path, PathBuf};

const CRATES: &[&str] = &["ply-compiled", "ply-core"];

/// Under `crates/`: how C is cached, swept and loaded. None of it runs as a loaded unit computes.
const OUTSIDE_SEMANTICS: &[&str] = &[
    "ply-compiled/src/c/linked.rs",
    "ply-compiled/src/c/load.rs",
    "ply-compiled/src/c/stage.rs",
    "ply-compiled/src/c/sweep.rs",
];

fn main() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for name in CRATES {
        let src = crates.join(name).join("src");
        println!("cargo:rerun-if-changed={}", src.display());
        walk(&src, &mut files);
    }
    files.sort();
    let named = |path: &Path| {
        path.strip_prefix(&crates)
            .expect("every file walked is under crates/")
            .to_string_lossy()
            .into_owned()
    };
    for outside in OUTSIDE_SEMANTICS {
        assert!(
            files.iter().any(|path| named(path) == *outside),
            "`{outside}` is left out of the semantics digest and no longer exists"
        );
    }
    // The manifests too: a dependency's version is part of what the runtime computes.
    let mut semantic: Vec<PathBuf> = files
        .iter()
        .filter(|path| !OUTSIDE_SEMANTICS.contains(&named(path).as_str()))
        .cloned()
        .collect();
    for name in CRATES {
        let manifest = crates.join(name).join("Cargo.toml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        semantic.push(manifest);
    }
    println!(
        "cargo:rustc-env=PLY_SEMANTICS_DIGEST={}",
        digest(&semantic, &named)
    );
}

fn digest(files: &[PathBuf], named: &dyn Fn(&Path) -> String) -> String {
    let mut h = blake3::Hasher::new();
    for path in files {
        let name = named(path);
        let bytes = std::fs::read(path)
            .unwrap_or_else(|e| panic!("the source file {} reads: {e}", path.display()));
        h.update(&(name.len() as u64).to_le_bytes());
        h.update(name.as_bytes());
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
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

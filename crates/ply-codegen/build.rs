//! Two digests of the runtime. `PLY_RUNTIME_DIGEST` is every file under `src/` of this crate and of
//! the workspace crates it links: what a body Rust emits and an answer Rust asks of the compiler are
//! a function of, since all of it can run while they are made. `PLY_SEMANTICS_DIGEST` leaves out
//! the files that only compile, keep, sweep and load C, and Rust's own asking of the compiler: what
//! an answer the program's own emitter gives, running inside a loaded unit, is a function of.
//! `ply-std` and `ply-compiler` are left out of both: what they hold is Ply sources and the bundle,
//! which a body's key already names through its definition's key and the emitter's identity.

use std::path::{Path, PathBuf};

const CRATES: &[&str] = &["ply-codegen", "ply-eval"];

/// Under `crates/`: how C is compiled, kept and swept, and how Rust asks the compiler for an answer.
/// None of it runs while a loaded unit computes.
const OUTSIDE_SEMANTICS: &[&str] = &[
    "ply-codegen/src/c/answers.rs",
    "ply-codegen/src/c/bundle.rs",
    "ply-codegen/src/c/cache.rs",
    "ply-codegen/src/c/dump.rs",
    "ply-codegen/src/c/load.rs",
    "ply-codegen/src/c/producer.rs",
    "ply-codegen/src/c/sweep.rs",
    "ply-codegen/src/c/toolchain.rs",
    "ply-codegen/src/c/upgrade.rs",
    "ply-codegen/src/source.rs",
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
        "cargo:rustc-env=PLY_RUNTIME_DIGEST={}",
        digest(&files, &named)
    );
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

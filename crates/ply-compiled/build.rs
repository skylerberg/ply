use std::path::{Path, PathBuf};

const CRATES: &[&str] = &["ply-compiled", "ply-core"];

const EXCLUDED_FROM_DIGEST: &[&str] = &[
    "ply-compiled/src/c/linked.rs",
    "ply-compiled/src/c/load.rs",
    "ply-compiled/src/c/stage.rs",
    "ply-compiled/src/c/sweep.rs",
];

fn main() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for name in CRATES {
        let source = crates.join(name).join("src");
        println!("cargo:rerun-if-changed={}", source.display());
        walk(&source, &mut files);
    }
    files.sort();
    let named = |path: &Path| {
        path.strip_prefix(&crates)
            .expect("every file walked is under crates/")
            .to_string_lossy()
            .into_owned()
    };
    for outside in EXCLUDED_FROM_DIGEST {
        assert!(
            files.iter().any(|path| named(path) == *outside),
            "`{outside}` is left out of the semantics digest and no longer exists"
        );
    }
    let mut semantic: Vec<PathBuf> = files
        .iter()
        .filter(|path| !EXCLUDED_FROM_DIGEST.contains(&named(path).as_str()))
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

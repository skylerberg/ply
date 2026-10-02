//! What a store's groups are a function of in the code `ply` is built from, worked out at build
//! time: the Ply modules a root module reaches by its imports, and the `Cargo.lock` entries the
//! runtime's crates reach. `build.rs` includes this file, so it reads the files themselves.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Where the shipped modules are: `std.` and `compiler.` imports resolve here.
pub struct Shelf<'a> {
    pub std: &'a Path,
    pub compiler: &'a Path,
}

/// Every module `roots` (stems of the package at `package`) reach by their imports, and the
/// manifest of every package among them, as files. An import's first segment names `std` or
/// `compiler`, a module of the importing package, or else one of that package's path dependencies;
/// which one is the manifest's meaning, so every path dependency counts, whole. An import reaches
/// at least every definition a reference can, so nothing outside these files is code the roots run.
pub fn closure(shelf: &Shelf, package: &Path, roots: &[&str]) -> Vec<PathBuf> {
    let mut files: BTreeSet<PathBuf> = BTreeSet::new();
    let mut whole: BTreeSet<PathBuf> = BTreeSet::new();
    let mut queue: Vec<PathBuf> = roots
        .iter()
        .map(|root| package.join(format!("{root}.ply")))
        .collect();
    files.insert(package.join("ply.pkg"));
    while let Some(file) = queue.pop() {
        if files.contains(&file) {
            continue;
        }
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("the module {} reads: {e}", file.display()));
        files.insert(file.clone());
        let dir = file.parent().expect("a module is in a directory");
        for import in imports_of(&text) {
            let (first, rest) = import.split_once('.').unwrap_or((import, ""));
            let local = dir.join(format!("{first}.ply"));
            if first == "std" || first == "compiler" {
                let from = if first == "std" {
                    shelf.std
                } else {
                    shelf.compiler
                };
                queue.push(from.join(format!("{rest}.ply")));
            } else if local.is_file() {
                queue.push(local);
            } else {
                let deps = path_dependencies(&dir.join("ply.pkg"));
                assert!(
                    !deps.is_empty(),
                    "`import {import}` in {} names no module of its package, and the package \
                     depends on none by path",
                    file.display()
                );
                for dep in deps {
                    if whole.insert(dep.clone()) {
                        files.insert(dep.join("ply.pkg"));
                        queue.extend(modules(&dep));
                    }
                }
            }
        }
    }
    files.into_iter().collect()
}

/// The module path each `import` line opens with.
fn imports_of(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|line| line.strip_prefix("import "))
        .map(|rest| {
            rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
                .next()
                .unwrap_or("")
        })
        .collect()
}

/// The directories every `Path("...")` of a manifest names, resolved against its own.
pub fn path_dependencies(manifest: &Path) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(manifest).unwrap_or_default();
    let dir = manifest.parent().expect("a manifest is in a directory");
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("Path(\"") {
        rest = &rest[at + "Path(\"".len()..];
        match rest.find('"') {
            Some(end) => {
                out.push(normalize(&dir.join(&rest[..end])));
                rest = &rest[end + 1..];
            }
            None => break,
        }
    }
    out
}

/// The `.ply` files of one package directory, ascending.
pub fn modules(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("the package directory {} reads: {e}", dir.display()))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|x| x == "ply"))
        .collect();
    out.sort();
    out
}

/// Lexical, because a dependency is written as `../../x` and `Path::join` keeps the `..`s: the keys
/// have to line up with the repository's own paths.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// The `[[package]]` entries of a `Cargo.lock` that `roots` reach through their `dependencies`,
/// in the lock's order: a change to any other crate's entry builds none of the code they run.
pub fn lock_closure(lock: &str, roots: &[&str]) -> String {
    let blocks: Vec<&str> = lock.split("\n[[package]]\n").skip(1).collect();
    let field = |block: &str, key: &str| -> Option<String> {
        block
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key} = \"")))
            .map(|rest| rest.trim_end_matches('"').to_string())
    };
    let named: Vec<(String, String)> = blocks
        .iter()
        .map(|block| {
            (
                field(block, "name").unwrap_or_default(),
                field(block, "version").unwrap_or_default(),
            )
        })
        .collect();
    let mut reached: BTreeSet<usize> = BTreeSet::new();
    let mut queue: Vec<(String, Option<String>)> =
        roots.iter().map(|root| (root.to_string(), None)).collect();
    while let Some((name, version)) = queue.pop() {
        let found = named
            .iter()
            .enumerate()
            .find(|(_, (n, v))| *n == name && version.as_ref().is_none_or(|wanted| wanted == v));
        let Some((at, _)) = found else {
            panic!("`{name}` is in no `[[package]]` of the lock");
        };
        if !reached.insert(at) {
            continue;
        }
        let block = blocks[at];
        let Some(list) = block.split("dependencies = [").nth(1) else {
            continue;
        };
        for entry in list.split(']').next().unwrap_or("").lines() {
            let entry = entry.trim().trim_end_matches(',').trim_matches('"');
            let mut parts = entry.split(' ');
            if let Some(dep) = parts.next().filter(|dep| !dep.is_empty()) {
                queue.push((dep.to_string(), parts.next().map(str::to_string)));
            }
        }
    }
    reached
        .into_iter()
        .map(|at| blocks[at])
        .collect::<Vec<_>>()
        .join("\n[[package]]\n")
}

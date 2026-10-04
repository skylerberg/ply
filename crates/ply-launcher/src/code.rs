//! What the build script reads of the tree `ply` is built from: the `Cargo.lock` entries the
//! runtime's crates reach. `build.rs` includes this file, so it reads the files themselves.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Lexical, because `Path::join` keeps the `..`s: the digest's keys are the repository's paths.
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

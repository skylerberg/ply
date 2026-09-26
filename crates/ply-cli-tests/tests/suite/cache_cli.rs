//! What the `ply` command line decides about a project's cache: where it goes, and that nothing
//! about how a run was scheduled reaches a hash.

use crate::harness::{ply, write};
use std::path::Path;

const CORE: &str = r#"
pub type Money = Int

pub type Item =
  | Book(String, Money)
  | Note(String)

pub effect db {
  read  get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Int
}

pub fn price(i: Item) -> Money =
  match i {
    Book(_, p) -> p,
    Note(_) -> 0,
  }

pub fn label(i: Item) -> String =
  match i {
    Book(t, _) -> t,
    Note(t) -> t,
  }

test "a note is free" {
  assert_eq(price(Note("n")), 0)
}
"#;

const SHOP: &str = r#"
import core

fn total(items: List<core::Item>) -> Int =
  fold(items, 0, |acc, i: core::Item| acc + core::price(i))

fn stored(k: Int) -> Int / {core::db.read[cart]} = core::db.get[cart](k)

test "a shelf adds up" {
  assert_eq(total([core::Book("b", 3), core::Note("n")]), 3)
}
"#;

const LEAF: &str = "pub fn one() -> Int = 1\npub fn two() -> Int = one() + one()\n";

fn corpus() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "core.ply", CORE);
    write(dir.path(), "shop.ply", SHOP);
    write(dir.path(), "leaf.ply", LEAF);
    dir
}

fn test_hashes(dir: &Path, extra: &[&str]) -> Vec<String> {
    let out = ply(dir)
        .arg("test")
        .arg("--json")
        .args(extra)
        .output()
        .unwrap();
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json emits one object");
    v["selection"]["tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| format!("{} {}", t["key"], t["hash"]))
        .collect()
}

/// The front end is serial, so the worker count must not reach a hash.
#[test]
fn the_worker_count_does_not_reach_a_test_hash() {
    let dir = corpus();
    let one = test_hashes(dir.path(), &["--jobs", "1"]);
    let many = test_hashes(dir.path(), &["--jobs", "10"]);
    assert_eq!(one, many);
    assert_eq!(one, test_hashes(dir.path(), &["--no-cache"]));
}

#[test]
fn a_relative_and_an_absolute_path_share_one_cache() {
    let dir = corpus();
    // The front-end cache is opened by the load, so nothing has to run for this to be about it.
    ply(dir.path())
        .args(["test", "--filter", "nonexistent"])
        .output()
        .unwrap();

    let out = ply(dir.path())
        .args(["test", "--filter", "nonexistent"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    // One cache under the project root either way: an absolute path must not open a second one.
    let caches: Vec<_> = walkdir(dir.path())
        .into_iter()
        .filter(|p| p.ends_with(".ply-cache"))
        .collect();
    assert_eq!(caches.len(), 1, "{caches:?}");
}

/// Every directory under `root`, so a second cache anywhere below it is visible.
fn walkdir(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.push(path.clone());
                stack.push(path);
            }
        }
    }
    out
}

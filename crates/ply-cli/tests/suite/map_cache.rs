//! A `Map` through the result cache, which is where a non-canonical iteration order would do its
//! quietest damage.

use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

/// The orders are data rather than source, so the two runs below are the *same* definitions — which
/// is what makes the second one a cache read rather than a second first run.
const SOURCE: &str = "\
fn build(ks: List<Int>) -> Map<Int, Int> =
  fold(ks, map_new(), |m, k| map_insert(m, k, k * 10))

fn ascending() -> List<Int> = [1, 2, 3, 4, 5]
fn descending() -> List<Int> = [5, 4, 3, 2, 1]
fn shuffled() -> List<Int> = [3, 1, 5, 2, 4]

test \"insertion order does not change the value\" {
  assert_eq(build(ascending()), build(descending()));
  assert_eq(build(ascending()), build(shuffled()));
  assert_eq(map_keys(build(shuffled())), [1, 2, 3, 4, 5]);
  assert_eq(map_fold(build(descending()), 0, |acc, k, v| acc * 10 + k), 12345)
}
";

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), source).unwrap();
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn text_of(out: &std::process::Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

#[test]
fn a_test_over_maps_built_in_different_orders_passes_and_then_caches() {
    let dir = project(SOURCE);

    let first = ply(dir.path()).arg("test").output().unwrap();
    let text = text_of(&first);
    assert_eq!(first.status.code(), Some(0), "{text}");
    assert!(text.contains("selected 1 of 1 (0 cached)"), "{text}");
    assert!(text.contains("0 failed, 1 passed, 0 cached"), "{text}");

    let second = ply(dir.path()).arg("test").output().unwrap();
    let text = text_of(&second);
    assert_eq!(second.status.code(), Some(0), "{text}");
    assert!(
        text.contains("selected 0 of 1 (1 cached)"),
        "a map-valued pass must be cacheable:\n{text}"
    );
}

/// Both engines, over the same program, with the cache bypassed.
#[test]
fn both_engines_agree_about_a_map() {
    let dir = project(SOURCE);
    let out = ply(dir.path())
        .args(["test", "--engine", "both"])
        .output()
        .unwrap();
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(!text.contains("E0503"), "{text}");
}

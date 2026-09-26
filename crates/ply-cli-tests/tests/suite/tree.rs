//! What holds this suite to its boundary: every test file is reachable from `main.rs`, and every
//! command goes through [`crate::harness`].
//!
//! The `unit` tree this package used to carry was left behind by a move: `ply-cli`'s Rust modules
//! went to `ply-machine`, the tests stayed, and nothing failed. The first check turns that into a
//! failure. The second keeps the environment hygiene in one place, since a test that builds its
//! own `ply` inherits the machine's environment and its own idea of the flags.

use std::collections::BTreeSet;
use std::path::Path;

fn suite() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/suite")
}

/// The `mod name;` declarations `main.rs` makes.
fn declared() -> BTreeSet<String> {
    let text = std::fs::read_to_string(suite().join("main.rs")).expect("the suite has a root");
    text.lines()
        .filter_map(|line| line.strip_prefix("mod "))
        .filter_map(|rest| rest.strip_suffix(';'))
        .map(str::to_string)
        .collect()
}

/// Every module the root can reach: each `<name>.rs` beside it, and each `<name>/main.rs` below it.
fn present() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(suite()).expect("the suite directory is readable") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            if path.join("main.rs").is_file() {
                out.insert(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            if name != "main" {
                out.insert(name);
            }
        }
    }
    out
}

#[test]
fn every_test_file_is_declared_and_every_declaration_has_a_file() {
    let declared = declared();
    let present = present();
    let orphaned: Vec<&String> = present.difference(&declared).collect();
    let missing: Vec<&String> = declared.difference(&present).collect();
    assert!(
        orphaned.is_empty(),
        "these files are not reachable from main.rs, so nothing runs them: {orphaned:?}"
    );
    assert!(
        missing.is_empty(),
        "main.rs declares these modules and there is no file: {missing:?}"
    );
}

#[test]
fn the_harness_is_the_only_place_the_ply_binary_is_named() {
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(suite()).expect("the suite directory is readable") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        if path.file_name().unwrap() == "harness.rs" || path.file_name().unwrap() == "tree.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("every file is utf-8");
        if text.contains("cargo_bin(") {
            offenders.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    assert!(
        offenders.is_empty(),
        "these files build their own `ply` instead of going through the harness: {offenders:?}"
    );
}

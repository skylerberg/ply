//! The real-code row, end to end: the checkout's compiler and CLI, each front-ended by the product
//! itself in a test of its own, with the digest it is pinned by beside its verdict.

use crate::support::{corpus, document, measured, outcome, repo, row};
use std::path::Path;

/// The pin, taken here the way the program says it takes it: BLAKE3 over every `.ply` file and
/// manifest under the trees, outside hidden directories, as its path under the repository, a zero
/// byte, its text and a zero byte, in path order.
fn pin_of(trees: &[&str]) -> String {
    fn walk(root: &Path, dir: &Path, into: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                walk(root, &entry.path(), into);
            } else if kind.is_file() && (name.ends_with(".ply") || name == "ply.pkg") {
                let relative = entry.path().strip_prefix(root).unwrap().to_owned();
                into.push((
                    relative.to_string_lossy().into_owned(),
                    std::fs::read(entry.path()).unwrap(),
                ));
            }
        }
    }
    let root = repo();
    let mut files = Vec::new();
    for tree in trees {
        walk(&root, &root.join(tree), &mut files);
    }
    files.sort();
    let mut h = blake3::Hasher::new();
    for (path, text) in &files {
        h.update(path.as_bytes());
        h.update(&[0]);
        h.update(text);
        h.update(&[0]);
    }
    format!("b3:{}", h.finalize().to_hex())
}

fn front_ended(member: &str) -> serde_json::Value {
    let out = corpus(
        &repo(),
        &["real", "--member", member, "--no-tests", "--json"],
    );
    assert!(
        out.status.success(),
        "the real-code row refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = document(&out);
    assert_eq!(report["ok"].as_bool(), Some(true), "{report:#}");
    let members = report["members"].as_array().expect("members is an array");
    assert_eq!(
        members.len(),
        1,
        "only the member asked for is pinned: {report:#}"
    );
    assert_eq!(members[0]["name"].as_str(), Some(member), "{report:#}");
    let rows = report["rows"].as_array().expect("rows is an array");
    assert_eq!(
        rows.len(),
        1,
        "one check row, the tests skipped: {report:#}"
    );
    let checked = row(&report, &format!("{member} check"));
    assert_eq!(outcome(checked), "pass", "{checked:#}");
    assert_eq!(checked["detail"], members[0]["digest"], "{checked:#}");
    report
}

#[test]
fn the_compiler_frontends_clean_at_its_pin() {
    let report = front_ended("compiler");
    let compiler = &report["members"][0];
    assert_eq!(
        compiler["digest"].as_str(),
        Some(pin_of(&["crates/ply-compiler/ply"]).as_str()),
        "{compiler:#}"
    );
    let checked = row(&report, "compiler check");
    assert!(
        measured(checked, "definitions") > 1000.0,
        "the compiler is the real one: {checked:#}"
    );
}

#[test]
fn the_cli_frontends_clean_at_its_pin() {
    let report = front_ended("cli");
    let cli = &report["members"][0];
    // The CLI reads the packages its manifest names by path, so its pin covers them too.
    let trees = [
        "crates/ply-cli/ply",
        "crates/ply-test/ply",
        "crates/ply-prove/ply",
    ];
    assert_eq!(cli["trees"], serde_json::json!(trees), "{cli:#}");
    assert_eq!(
        cli["digest"].as_str(),
        Some(pin_of(&trees).as_str()),
        "{cli:#}"
    );
}

/// The tests above take a member each, so they are the whole row only while their members are
/// every member `real` takes: a name it does not take is refused with the ones it does.
#[test]
fn the_members_tested_here_are_every_member_real_takes() {
    let out = corpus(&repo(), &["real", "--member", "none"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("[possible values: compiler, cli]"),
        "{stderr}"
    );
}

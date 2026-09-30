//! The real-code row, end to end: the checkout's compiler and CLI, front-ended by the product
//! itself, with the digest each member is pinned by beside the verdicts.

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

#[test]
fn the_toolchain_trees_frontend_clean_at_their_pins() {
    let out = corpus(&repo(), &["real", "--no-tests", "--json"]);
    assert!(
        out.status.success(),
        "the real-code row refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = document(&out);
    assert_eq!(report["ok"].as_bool(), Some(true), "{report:#}");
    let members = report["members"].as_array().expect("members is an array");
    assert_eq!(members.len(), 2);
    let rows = report["rows"].as_array().expect("rows is an array");
    assert_eq!(
        rows.len(),
        2,
        "one check row for each member, the tests skipped: {report:#}"
    );

    let compiler = &members[0];
    assert_eq!(compiler["name"].as_str(), Some("compiler"));
    assert_eq!(
        compiler["digest"].as_str(),
        Some(pin_of(&["crates/ply-compiler/ply"]).as_str()),
        "{compiler:#}"
    );
    let checked = row(&report, "compiler check");
    assert_eq!(outcome(checked), "pass", "{checked:#}");
    assert_eq!(checked["detail"], compiler["digest"], "{checked:#}");
    assert!(
        measured(checked, "definitions") > 1000.0,
        "the compiler is the real one: {checked:#}"
    );

    // The CLI reads the packages its manifest names by path, so its pin covers them too.
    let cli = &members[1];
    assert_eq!(cli["name"].as_str(), Some("cli"));
    assert_eq!(
        cli["trees"],
        serde_json::json!([
            "crates/ply-cli/ply",
            "crates/ply-test/ply",
            "crates/ply-prove/ply"
        ]),
        "{cli:#}"
    );
    assert_eq!(
        cli["digest"].as_str(),
        Some(
            pin_of(&[
                "crates/ply-cli/ply",
                "crates/ply-test/ply",
                "crates/ply-prove/ply"
            ])
            .as_str()
        ),
        "{cli:#}"
    );
    assert_eq!(outcome(row(&report, "cli check")), "pass", "{report:#}");
}

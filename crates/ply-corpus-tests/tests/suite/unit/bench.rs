//! The bench, end to end: a tiny generated corpus, the corpus program's `bench` driving the real
//! `ply`, and the scenarios judged in the report it answers.

use crate::support::{corpus, document, generate, measured, outcome, row};
use std::path::{Path, PathBuf};

fn corpus_at(root: &Path) {
    generate(
        root,
        &[
            "--seed",
            "4",
            "--modules",
            "5",
            "--defs-per-module",
            "6",
            "--tests",
            "10",
            "--depth",
            "2",
        ],
    );
}

/// `bench corpus`, run from the directory above the corpus, as the report it wrote.
fn bench(root: &Path) -> serde_json::Value {
    let out = corpus(
        root.parent().unwrap(),
        &["bench", "corpus", "--repeats", "1", "--json"],
    );
    assert!(
        out.status.success(),
        "the bench refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    document(&out)
}

/// Every `.ply` file under `dir` past hidden directories, in path order, with its text.
fn sources(dir: &Path) -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, into: &mut Vec<(PathBuf, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let hidden = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if path.is_dir() && !hidden {
                walk(&path, into);
            } else if path.extension().is_some_and(|e| e == "ply") {
                let text = std::fs::read_to_string(&path).unwrap();
                into.push((path, text));
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, &mut found);
    found.sort();
    found
}

#[test]
fn the_bench_verdicts_hold_on_a_generated_corpus() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let report = bench(&root);
    assert_eq!(report["ok"].as_bool(), Some(true), "{report:#}");
    assert_eq!(
        report["corpus"]["root"].as_str(),
        Some("corpus"),
        "{report:#}"
    );
    assert!(
        report["provenance"]["corpus"]
            .as_str()
            .is_some_and(|d| d.starts_with("b3:")),
        "the report is pinned to the corpus that measured it: {report:#}"
    );
    assert!(
        report["provenance"]["runtime"]
            .as_str()
            .is_some_and(|v| v.starts_with("ply ")),
        "{report:#}"
    );

    for name in [
        "cold",
        "warm",
        "rename",
        "edit-leaf",
        "edit-hub",
        "pipeline",
    ] {
        assert_eq!(outcome(row(&report, name)), "pass", "{report:#}");
    }

    // The pipeline row is the harness's own phases beside the toolchain's: it read the tree it walked.
    assert!(measured(row(&report, "pipeline"), "files") > 0.0);

    let warm = row(&report, "warm");
    assert_eq!(measured(warm, "selected"), 0.0, "{warm:#}");
    assert_eq!(
        measured(row(&report, "rename"), "selected"),
        0.0,
        "a rename must select nothing"
    );
    let leaf = measured(row(&report, "edit-leaf"), "selected");
    let hub = measured(row(&report, "edit-hub"), "selected");
    assert!(
        hub > leaf,
        "editing a hub selects more than editing a leaf: {leaf} against {hub}"
    );
}

#[test]
fn a_mutation_is_undone_when_the_scenario_ends() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let before = sources(&root);
    let report = bench(&root);
    assert_eq!(report["ok"].as_bool(), Some(true), "{report:#}");
    assert_eq!(
        before,
        sources(&root),
        "the scenarios left the corpus mutated"
    );
}

#[test]
fn a_stale_edit_site_is_an_error_rather_than_a_silent_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let manifest_path = root.join("corpus.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["leaf_edit"]["find"] =
        serde_json::Value::String("fn definitely_not_here() -> Int = 0\n".to_string());
    std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap()).unwrap();

    let out = corpus(dir.path(), &["bench", "corpus", "--repeats", "1"]);
    assert_eq!(out.status.code(), Some(1), "a stale site ran to the end");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ply-corpus: ") && stderr.contains("occurs 0 times"),
        "the stale site must say so, not slip by: {stderr}"
    );
}

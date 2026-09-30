//! The corpus package's own `test` blocks, and its fixtures'.

use serde_json::Value;

/// The programs under `fixtures/` the corpus runs: each checks, and its tests pass.
#[test]
fn every_fixture_the_corpus_runs_checks_and_passes_its_own_tests() {
    for fixture in [
        "load.ply",
        "layers.ply",
        "shape.ply",
        "scans.ply",
        "rungs.ply",
    ] {
        let path = format!("crates/ply-corpus/fixtures/{fixture}");
        let out = std::process::Command::new(crate::support::ply())
            .args(["test", &path, "--no-cache", "--json"])
            .current_dir(crate::support::repo())
            .output()
            .expect("the CLI runs");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{fixture}'s tests are red:\n{stderr}");
        let report: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{fixture}: stdout was not one JSON object: {e}"));
        assert!(
            report["summary"]["passed"].as_u64().is_some_and(|n| n > 0),
            "{fixture} tested nothing: {}",
            report["summary"]
        );
    }
}

/// The corpus package's `ply/*.ply` `test` blocks, run by the compiled tier.
///
/// Nothing in CI ran them: the shards run `ply-corpus-tests` (Rust tests of the harness) and the
/// generated corpora, and the corpus's own in-package tests were taken by hand or not at all.
///
/// `--no-cache` is what makes the `backend` counts worth asserting: on a fully cached run nothing
/// is offered or entered, so `offered: 0` there says the run was cached, not that the compiled tier
/// was skipped.
#[test]
fn the_corpus_packages_own_tests_run_green_on_the_compiled_tier() {
    let out = std::process::Command::new(crate::support::ply())
        .args(["test", "crates/ply-corpus/ply", "--no-cache", "--json"])
        .current_dir(crate::support::repo())
        .output()
        .expect("the CLI runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the corpus package's own tests are red:\n{stderr}"
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout was not one JSON object: {e}\n---\n{}\n---\n{stderr}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    assert_eq!(report["ok"], Value::Bool(true), "{report}");
    assert_eq!(report["summary"]["failed"].as_u64(), Some(0), "{report}");
    assert_eq!(
        report["summary"]["cached"].as_u64(),
        Some(0),
        "`--no-cache` ran nothing: {}",
        report["summary"]
    );
    assert!(
        report["summary"]["passed"].as_u64().is_some_and(|n| n > 0),
        "the run tested nothing at all: {}",
        report["summary"]
    );
    assert_eq!(report["backend"]["name"], "c", "{}", report["backend"]);
    for what in ["offered", "entered"] {
        assert!(
            report["backend"][what].as_u64().is_some_and(|n| n > 0),
            "the compiled tier {what} nothing: {}",
            report["backend"]
        );
    }
}

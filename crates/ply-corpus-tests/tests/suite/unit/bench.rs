//! The bench, end to end: a tiny generated corpus, the corpus program's `bench` driving the real
//! `ply`, and the scenarios judged from the report it answers.

use crate::support::{corpus, document, generate};
use std::path::Path;

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

fn scenario<'a>(report: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    report["scenarios"]
        .as_array()
        .expect("scenarios is an array")
        .iter()
        .find(|s| s["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("the report carries `{name}`"))
}

#[test]
fn the_bench_verdicts_hold_on_a_generated_corpus() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let report = bench(&root);
    assert_eq!(report["ok"].as_bool(), Some(true), "{report:#}");
    assert_eq!(report["root"].as_str(), Some("corpus"), "{report:#}");

    // The pipeline section is the harness's own phases beside the toolchain's: it must have run, and
    // it must have read the tree it walked.
    let pipeline = &report["pipeline"];
    assert_eq!(
        pipeline["ok"].as_bool(),
        Some(true),
        "the pipeline did not run: {pipeline:#}"
    );
    assert!(
        pipeline["stages"]["harness"]["files"].as_i64().unwrap_or(0) > 0,
        "the harness walked no files: {pipeline:#}"
    );

    let warm = scenario(&report, "warm");
    assert_eq!(warm["tests_selected"].as_i64(), Some(0), "{warm:#}");

    let rename = scenario(&report, "rename");
    assert_eq!(
        rename["tests_selected"].as_i64(),
        warm["tests_selected"].as_i64(),
        "a rename must select nothing: {rename:#}"
    );

    let leaf = scenario(&report, "edit-leaf");
    let hub = scenario(&report, "edit-hub");
    assert!(
        hub["tests_selected"].as_i64() > leaf["tests_selected"].as_i64(),
        "editing a hub selects more than editing a leaf: {leaf:#} {hub:#}"
    );
}

#[test]
fn a_mutation_is_undone_when_the_scenario_ends() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let before: Vec<String> = ply_corpus::pipeline::discover(&root)
        .unwrap()
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();

    let report = bench(&root);
    assert_eq!(report["ok"].as_bool(), Some(true), "{report:#}");

    let after: Vec<String> = ply_corpus::pipeline::discover(&root)
        .unwrap()
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    assert_eq!(before, after, "the scenarios left the corpus mutated");
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

/// The in-process half: what a continuation resumption costs, measured by driving the machine the
/// corpus program itself holds, over a fixture whose front end the program ran. The property is
/// that each resumption does work — the fixture is the same computation resumed a varying number of
/// times, so the steps have to climb — and the points are the counts 0, 1, 2 and 4.
#[test]
fn the_resumption_curve_climbs_with_the_number_of_resumptions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let report = bench(&root);
    let measure = &report["measure"];
    assert_eq!(
        measure["ok"].as_bool(),
        Some(true),
        "the measurement did not run: {measure:#}"
    );
    let points = measure["resumptions"]["points"]
        .as_array()
        .unwrap_or_else(|| panic!("the resumption curve is not a list: {measure:#}"));
    assert_eq!(
        points.iter().map(|p| p["n"].as_i64()).collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2), Some(4)],
        "{measure:#}"
    );
    let steps: Vec<i64> = points
        .iter()
        .map(|p| p["steps"].as_i64().unwrap_or(0))
        .collect();
    assert!(
        steps.windows(2).all(|w| w[0] < w[1]),
        "more resumptions must do more work: {steps:?}"
    );
    // The first point resumes nothing, so its marginal is the zero it was given.
    assert_eq!(points[0]["marginal_steps"].as_i64(), Some(0), "{measure:#}");
    // And every later point's marginal is positive: each resumption costs something.
    assert!(
        points[1..]
            .iter()
            .all(|p| p["marginal_steps"].as_i64().unwrap_or(0) > 0),
        "a resumption that costs nothing is a resumption that did not happen: {measure:#}"
    );
    // Throughput is one call to a fixed-work definition, so it reports a step count too.
    assert!(
        measure["throughput"]["points"][0]["steps"]
            .as_i64()
            .unwrap_or(0)
            > 0,
        "{measure:#}"
    );
    // Both fixtures were read back and front-ended by the program before the machine held them.
    for curve in ["resumptions", "throughput"] {
        let loaded = &measure[curve]["loaded"];
        assert!(
            loaded["read_us"].as_i64().is_some_and(|us| us >= 0)
                && loaded["front_us"].as_i64().is_some_and(|us| us > 0),
            "`{curve}` carries no timed front end: {measure:#}"
        );
    }
    // And each was taken back out, so the tree the scenarios compile is the one `gen` wrote.
    assert!(
        std::fs::read_dir(&root).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("measure-")),
        "a fixture was left in the corpus"
    );
}

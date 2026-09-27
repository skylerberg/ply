//! The bench, end to end: a tiny generated corpus, the corpus package's own `bench.run`
//! driving the real `ply`, and the scenarios judged from the report it answers.

use crate::support::{generate, ply};
use ply_corpus::cmd::run_ply_subcommand;
use ply_corpus::spec::CorpusSpec;
use std::path::Path;

fn corpus_at(root: &Path) {
    let spec = CorpusSpec {
        seed: 4,
        modules: 5,
        defs_per_module: 6,
        tests: 10,
        depth: 2,
        ..CorpusSpec::default()
    };
    generate(root, &spec);
}

fn bench(root: &Path) -> serde_json::Value {
    let root = &root.canonicalize().unwrap();
    let value = run_ply_subcommand(
        "bench.run",
        vec![
            ply_eval::Value::str(root.to_string_lossy()),
            ply_eval::Value::Int(1),
        ],
        root,
        &ply(),
    )
    .expect("the bench runs");
    let ply_eval::Value::Ctor { name, args } = &value else {
        panic!("`bench.run` answered {value}, not an `Ok` or an `Err`");
    };
    let ply_eval::Value::Str(text) = &args[0] else {
        panic!("`bench.run`'s answer is not text: {value}");
    };
    match name.as_str() {
        "Ok" => serde_json::from_str(text).expect("the bench's report is JSON"),
        "Err" => panic!("the bench refused: {text}"),
        other => panic!("`bench.run` answered `{other}`"),
    }
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

    let value = run_ply_subcommand(
        "bench.run",
        vec![
            ply_eval::Value::str(root.to_string_lossy()),
            ply_eval::Value::Int(1),
        ],
        &root.canonicalize().unwrap(),
        &ply(),
    )
    .expect("the bench runs");
    let ply_eval::Value::Ctor { name, args } = &value else {
        panic!("`bench.run` answered {value}, not an `Err`");
    };
    assert_eq!(name.as_str(), "Err", "{value}");
    let ply_eval::Value::Str(why) = &args[0] else {
        panic!("the refusal is not text: {value}");
    };
    assert!(
        why.contains("occurs 0 times"),
        "the stale site must say so, not slip by: {why}"
    );
}

/// The in-process half: what a continuation resumption costs, measured by driving the machine the
/// corpus program itself holds. The property is that each resumption does work — the fixture is the
/// same computation resumed a varying number of times, so the steps have to climb — and the points
/// are the counts 0, 1, 2 and 4.
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
}

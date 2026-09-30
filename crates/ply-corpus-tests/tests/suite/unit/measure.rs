//! `measure`, end to end: the in-process rows over fixtures the program front-ends and loads into
//! the machine it is lent, and the product's own rows over a generated corpus.

use crate::support::{corpus, document, generate, measured, outcome, row};
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

/// No fixture is left in `dir`: the tree a later run reads is the one it was handed.
fn no_fixture_left(dir: &Path) {
    let left: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("fixture-"))
        .collect();
    assert!(left.is_empty(), "fixtures left behind: {left:?}");
}

/// The in-process half. The resumption fixture is one computation resumed 0, 1, 2 and 4 times, so
/// the steps have to climb, and the criterion says so; each fixture was front-ended by the program
/// before the machine held it.
#[test]
fn a_resumption_costs_steps_and_a_call_does_its_work() {
    let dir = tempfile::tempdir().unwrap();
    let out = corpus(dir.path(), &["measure", "--repeats", "1", "--json"]);
    assert!(
        out.status.success(),
        "measure refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = document(&out);
    assert_eq!(report["corpus"], serde_json::Value::Null, "{report:#}");

    let resumptions = row(&report, "resumptions");
    assert_eq!(outcome(resumptions), "pass", "{resumptions:#}");
    let steps: Vec<f64> = ["r0_steps", "r1_steps", "r2_steps", "r4_steps"]
        .iter()
        .map(|name| measured(resumptions, name))
        .collect();
    assert!(
        steps.windows(2).all(|w| w[0] < w[1]),
        "more resumptions must do more work: {steps:?}"
    );
    // The curve's own points carry what each resumption added over the one before.
    let points = resumptions["detail"]["points"]
        .as_array()
        .unwrap_or_else(|| panic!("the curve is not a list: {resumptions:#}"));
    assert_eq!(
        points.iter().map(|p| p["n"].as_i64()).collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2), Some(4)]
    );
    assert!(
        points[1..]
            .iter()
            .all(|p| p["marginal_steps"].as_i64().unwrap_or(0) > 0),
        "a resumption that costs nothing did not happen: {resumptions:#}"
    );

    let call = row(&report, "call");
    assert_eq!(outcome(call), "pass", "{call:#}");
    for curve in [resumptions, call] {
        assert!(
            measured(curve, "front") > 0.0,
            "no front end was timed before the machine held the fixture: {curve:#}"
        );
    }
    no_fixture_left(dir.path());
}

/// The product's half, over a corpus: the store a run filled, every test on one worker taken first
/// and again, and the schedule a cold run needs, each judged by its own criterion.
#[test]
fn a_corpus_is_measured_by_the_products_own_reports() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let out = corpus(
        dir.path(),
        &["measure", "corpus", "--repeats", "2", "--json"],
    );
    assert!(
        out.status.success(),
        "measure refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = document(&out);

    let store = row(&report, "store");
    assert_eq!(outcome(store), "pass", "{store:#}");
    assert!(measured(store, "open") >= 0.0);

    let first = row(&report, "first-pass");
    assert_eq!(outcome(first), "pass", "{first:#}");
    assert_eq!(measured(first, "workers"), 1.0, "{first:#}");
    assert!(measured(first, "tests") >= 10.0, "{first:#}");
    assert!(measured(first, "execute") > 0.0, "{first:#}");

    // Timed against the first pass; whichever way it went, it was measured and judged.
    let steady = row(&report, "steady-pass");
    assert_ne!(outcome(steady), "inconclusive", "{steady:#}");
    assert_eq!(
        steady["criterion"]["of"].as_str(),
        Some("first-pass"),
        "{steady:#}"
    );

    let scheduling = row(&report, "scheduling");
    assert_eq!(outcome(scheduling), "pass", "{scheduling:#}");
    assert_eq!(
        measured(scheduling, "isolated") + measured(scheduling, "shared"),
        measured(scheduling, "tests")
    );
    assert_eq!(
        measured(scheduling, "groups"),
        measured(scheduling, "shared_groups").max(1.0)
    );
    no_fixture_left(&root);
}

#[test]
fn only_the_throughput_needs_a_corpus_to_time() {
    let dir = tempfile::tempdir().unwrap();
    let out = corpus(dir.path(), &["measure", "--only-throughput"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("name the corpus"), "{stderr}");
}

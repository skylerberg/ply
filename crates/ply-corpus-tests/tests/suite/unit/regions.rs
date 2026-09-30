//! `regions`, end to end: projects whose tests carry `cell` atoms, and `examples/`, each measured by
//! the product and coloured with the runner's own colouring under both models.

use crate::support::{corpus, document, measured, outcome, repo, row};
use std::path::Path;

/// `cells` tests each writing the cell of one of `labels` labels, then `pure` tests touching nothing.
fn cell_project(dir: &Path, cells: usize, labels: usize, pure: usize) {
    let mut src = String::new();
    for i in 0..cells {
        let label = i % labels;
        src.push_str(&format!(
            "fn touch{i}() -> Int / {{cell.read[r{label}], cell.write[r{label}]}} =\n  \
             with_cell[r{label}](0) {{ c -> {{ cell_set(c, {i}); cell_get(c) }} }}\n\n\
             test \"cell test {i}\" {{ assert_eq(touch{i}(), {i}) }}\n\n"
        ));
    }
    for i in 0..pure {
        src.push_str(&format!(
            "test \"pure test {i}\" {{ assert_eq({i} + 1, {}) }}\n",
            i + 1
        ));
    }
    std::fs::create_dir_all(dir).expect("the scratch project directory");
    std::fs::write(dir.join("main.ply"), src).expect("writing the scratch project");
}

fn regions(dir: &Path, args: &[&str]) -> serde_json::Value {
    let mut line = vec!["regions"];
    line.extend_from_slice(args);
    line.push("--json");
    let out = corpus(dir, &line);
    assert!(
        out.status.success(),
        "regions refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    document(&out)
}

fn effects(row: &serde_json::Value) -> Vec<&str> {
    row["detail"]["effects"]
        .as_array()
        .expect("the effects are a list")
        .iter()
        .filter_map(|e| e.as_str())
        .collect()
}

#[test]
fn a_cell_atom_reaches_a_footprint_and_only_forking_hides_it() {
    let dir = tempfile::tempdir().unwrap();
    cell_project(&dir.path().join("cells"), 24, 4, 8);
    let report = regions(dir.path(), &["cells", "--jobs", "4"]);

    let cost = row(&report, "cells");
    assert!(
        effects(cost).contains(&"cell"),
        "the project was written so that a `cell` atom reaches a footprint: {cost:#}"
    );
    assert_eq!(measured(cost, "tests"), 32.0);
    assert_eq!(
        measured(cost, "isolated"),
        32.0,
        "forking makes every one of them isolated, which is the property being priced"
    );
    assert_eq!(measured(cost, "groups_forked"), 1.0);
    assert_eq!(
        measured(cost, "groups_regions"),
        6.0,
        "six tests share each of four labels, so the clique is six colours wide"
    );
    assert_eq!(measured(cost, "newly_serialized"), 24.0);
    assert!(
        measured(cost, "critical_regions") > measured(cost, "critical_forked"),
        "six barriers where there was one has to cost something: {cost:#}"
    );
    // The criterion is that the region model serializes nothing, and here it serializes all of them.
    assert_eq!(outcome(cost), "fail", "{cost:#}");
    // And the model's colouring is the runner's own.
    let agreement = row(&report, "cells colouring");
    assert_eq!(outcome(agreement), "pass", "{agreement:#}");
}

#[test]
fn cell_tests_on_distinct_labels_are_free_to_lose_the_exemption() {
    let dir = tempfile::tempdir().unwrap();
    cell_project(&dir.path().join("cells"), 16, 16, 8);
    let report = regions(dir.path(), &["cells", "--jobs", "4"]);

    let cost = row(&report, "cells");
    assert_eq!(measured(cost, "cell"), 16.0);
    assert_eq!(measured(cost, "newly_serialized"), 0.0);
    assert_eq!(
        measured(cost, "groups_forked"),
        measured(cost, "groups_regions")
    );
    assert_eq!(outcome(cost), "pass", "{cost:#}");
    assert_eq!(report["ok"].as_bool(), Some(true), "{report:#}");
}

#[test]
fn the_examples_suite_loses_nothing_to_the_region_model() {
    let report = regions(&repo(), &["examples", "--jobs", "8"]);

    let cost = row(&report, "examples");
    assert!(
        !effects(cost).contains(&"cell"),
        "no test in `examples/` carries a `cell` atom; if one now does, the cost is no longer zero"
    );
    let tests = measured(cost, "tests");
    assert!(tests > 0.0, "{cost:#}");
    assert!(
        measured(cost, "pure") <= measured(cost, "isolated"),
        "{cost:#}"
    );
    assert!(measured(cost, "isolated") <= tests, "{cost:#}");
    assert!(measured(cost, "seeded") <= tests, "{cost:#}");
    assert_eq!(measured(cost, "cell"), 0.0);
    assert_eq!(measured(cost, "newly_serialized"), 0.0);
    assert_eq!(
        measured(cost, "groups_forked"),
        measured(cost, "groups_regions")
    );
    assert_eq!(
        measured(cost, "critical_forked"),
        measured(cost, "critical_regions")
    );
    assert_eq!(outcome(cost), "pass", "{cost:#}");
    let agreement = row(&report, "examples colouring");
    assert_eq!(outcome(agreement), "pass", "{agreement:#}");
}

#[test]
fn a_hypothetical_is_priced_without_a_tree() {
    let dir = tempfile::tempdir().unwrap();
    let report = regions(dir.path(), &["--hypothetical", "12:3"]);
    let rows = report["rows"].as_array().expect("rows is an array");
    assert_eq!(
        rows.len(),
        1,
        "one hypothetical asked for is one row: {report:#}"
    );
    let cost = row(&report, "hypothetical 12:3");
    assert_eq!(measured(cost, "tests"), 12.0 + 10.0 + 165.0);
    assert!(measured(cost, "newly_serialized") > 0.0, "{cost:#}");
}

#[test]
fn nothing_to_analyse_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = corpus(dir.path(), &["regions"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nothing to analyse"), "{stderr}");
}

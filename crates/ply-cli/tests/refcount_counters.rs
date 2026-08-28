//! `ply run --json`'s `counters` object, and `ply check --field-order`.
//!
//! ADR 0020 §9 recorded that *"No deterministic counter turned out to exist —
//! `ply run --json` reports no step, call or allocation count — so wall clock
//! was unavoidable"*. `ply_eval::rc::Stats` had counted reuse since ADR 0017 §4
//! and was read by three test files and nothing else. These tests are what make
//! the CLI surface a fact rather than an intention: without them a later change
//! that dropped the key would be a silent return to timing things.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

/// The push is in the last field, so every step rewrites the list.
const LINEAR: &str = "\
type S = { pos: Int, toks: List<Int> }
fn empty() -> S = {pos: 0, toks: []}
fn step(s: S, i: Int) -> S = { pos: i, toks: push(s.toks, i) }
fn main() -> Int = len(fold(range(0, 200), empty(), step).toks)
";

/// The same function with one field moved, which is the whole of `W0611`.
const QUADRATIC: &str = "\
type S = { pos: Int, toks: List<Int>, tail: Int }
fn empty() -> S = {pos: 0, toks: [], tail: 0}
fn step(s: S, i: Int) -> S = { pos: i, toks: push(s.toks, i), tail: s.tail }
fn main() -> Int = len(fold(range(0, 200), empty(), step).toks)
";

/// Fails at run time, so the counters have to survive the error path too.
const FAILING: &str = "\
fn main() -> Int = { let xs = push([], 1); assert_eq(len(xs), 2); 0 }
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

fn json_of(output: &std::process::Output) -> Value {
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout was not one JSON object: {e}\n---\n{text}\n---"))
}

fn counters(dir: &Path, extra: &[&str]) -> Value {
    let mut cmd = ply(dir);
    cmd.arg("run").arg("--json").arg("m.ply");
    for a in extra {
        cmd.arg(a);
    }
    let out = cmd.output().unwrap();
    let object = json_of(&out);
    assert!(
        object.get("counters").is_some(),
        "`ply run --json` has no `counters` key; ADR 0020 §9's premise is back.\n{object:#}"
    );
    object["counters"].clone()
}

#[test]
fn a_successful_run_reports_what_the_reference_counters_saw() {
    let dir = project(LINEAR);
    let c = counters(dir.path(), &[]);
    assert_eq!(c["engine"], "machine");
    assert_eq!(c["updates"], 200);
    assert_eq!(c["updates_in_place"], 199);
    // Every key the object promises, so that dropping one is a red test.
    for key in [
        "engine",
        "updates",
        "updates_in_place",
        "in_place",
        "takes_attempted",
        "takes_moved",
        "dup_sites",
        "dup_emitted",
        "drop_sites",
        "drop_emitted",
        "elided",
        "cycles",
    ] {
        assert!(c.get(key).is_some(), "`counters.{key}` is missing");
    }
}

/// The number the lint predicts, on the two programs that differ by one field's
/// position. This is the whole claim in one assertion.
#[test]
fn moving_one_field_moves_the_counter_from_all_to_nothing() {
    let linear = counters(project(LINEAR).path(), &[]);
    let quadratic = counters(project(QUADRATIC).path(), &[]);
    assert_eq!(quadratic["updates"], linear["updates"]);
    assert_eq!(quadratic["updates_in_place"], 0);
    assert_eq!(quadratic["in_place"], 0.0);
    assert!(linear["in_place"].as_f64().unwrap() >= 0.99);
}

/// A run that raised still reports them: the counters describe what happened,
/// and a failure is the case where a reader most wants to know.
#[test]
fn a_failed_run_reports_them_too() {
    let dir = project(FAILING);
    let out = ply(dir.path())
        .arg("run")
        .arg("--json")
        .arg("m.ply")
        .output()
        .unwrap();
    let object = json_of(&out);
    assert_eq!(object["ok"], false);
    assert!(
        object.get("counters").is_some(),
        "the error arm dropped the `counters` key"
    );
    assert_eq!(object["counters"]["updates"], 1);
}

/// **R1.** Three runs of one program must produce byte-identical counters. If
/// they do not, nothing measured with them means anything.
#[test]
fn three_runs_of_one_program_report_identical_counters() {
    let dir = project(QUADRATIC);
    let first = counters(dir.path(), &[]);
    for _ in 0..2 {
        assert_eq!(
            counters(dir.path(), &[]),
            first,
            "two runs of one program disagreed about what they counted"
        );
    }
}

/// `--engine both` evaluates the program twice. A pooled figure would be two
/// runs added together and would silently double every number a reader compares
/// across engines.
#[test]
fn engine_both_reports_one_engines_counters_and_not_the_sum() {
    let dir = project(QUADRATIC);
    let machine = counters(dir.path(), &["--engine", "machine"]);
    let both = counters(dir.path(), &["--engine", "both"]);
    assert_eq!(both["engine"], "machine");
    assert_eq!(
        both["updates"], machine["updates"],
        "`--engine both` doubled the counters instead of reporting one engine's"
    );
}

/// The tree-walker never lowers, so it bumps no `dup_*`/`drop_*`. Reported as
/// zero and `null` rather than as the machine's numbers, because a figure
/// attributed to the wrong evaluator is worse than an absent one.
#[test]
fn the_treewalker_reports_its_own_counters_and_does_not_borrow_the_machines() {
    let dir = project(QUADRATIC);
    let c = counters(dir.path(), &["--engine", "treewalk"]);
    assert_eq!(c["engine"], "treewalk");
    assert_eq!(c["dup_sites"], 0);
    assert_eq!(c["elided"], Value::Null);
    // `push` is reached by both engines, so this one still moves.
    assert_eq!(c["updates"], 200);
}

// --- the lint ---------------------------------------------------------------

#[test]
fn the_field_order_lint_is_off_unless_it_is_asked_for() {
    let dir = project(QUADRATIC);
    let quiet = ply(dir.path())
        .arg("check")
        .arg("--json")
        .arg("m.ply")
        .output()
        .unwrap();
    let object = json_of(&quiet);
    assert!(
        !object["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "W0611"),
        "W0611 fired without `--field-order`"
    );

    let asked = ply(dir.path())
        .arg("check")
        .arg("--json")
        .arg("--field-order")
        .arg("m.ply")
        .output()
        .unwrap();
    let object = json_of(&asked);
    let fired: Vec<&Value> = object["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "W0611")
        .collect();
    assert_eq!(fired.len(), 1, "expected one W0611, got {fired:?}");
    assert_eq!(fired[0]["severity"], "warning");
    // A warning: the program is correct and its answer is unchanged.
    assert_eq!(object["exit_code"], 0);
    assert_eq!(asked.status.code(), Some(0));
}

#[test]
fn the_lint_is_silent_on_the_program_written_the_fast_way() {
    let dir = project(LINEAR);
    let out = ply(dir.path())
        .arg("check")
        .arg("--json")
        .arg("--field-order")
        .arg("m.ply")
        .output()
        .unwrap();
    let object = json_of(&out);
    assert!(
        !object["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "W0611"),
        "W0611 fired on a definition whose `push` is already last"
    );
}

/// The lint's answer must be a function of the source, not of what the cache
/// held — `DefInfo::performed`'s doc requires a reviewing command to print the
/// same bytes warm as cold, and `--field-order` forces the complete parse for
/// exactly this reason.
#[test]
fn a_warm_run_and_a_cold_run_report_the_same_lint() {
    let dir = project(QUADRATIC);
    let once = ply(dir.path())
        .arg("check")
        .arg("--json")
        .arg("--field-order")
        .arg("m.ply")
        .output()
        .unwrap();
    let twice = ply(dir.path())
        .arg("check")
        .arg("--json")
        .arg("--field-order")
        .arg("m.ply")
        .output()
        .unwrap();
    let pick = |o: &std::process::Output| {
        json_of(o)["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["code"] == "W0611")
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(pick(&once), pick(&twice));
    assert_eq!(pick(&once).len(), 1);
}

/// The opt-in flag's cost is that a lint nobody turns on buys nothing, so it is
/// also on the command a project already runs to decide what a human should
/// look at. `ply review` has parsed every file by then, so it costs the walk.
#[test]
fn review_carries_the_lint_when_asked_and_an_empty_array_when_not() {
    let dir = project(QUADRATIC);
    let quiet = ply(dir.path())
        .arg("review")
        .arg("--json")
        .arg(".")
        .output()
        .unwrap();
    let object = json_of(&quiet);
    assert_eq!(
        object["field_order"].as_array().map(Vec::len),
        Some(0),
        "the key is present and empty rather than missing, so a consumer needs no branch"
    );

    let asked = ply(dir.path())
        .arg("review")
        .arg("--json")
        .arg("--field-order")
        .arg(".")
        .output()
        .unwrap();
    let object = json_of(&asked);
    let fired = object["field_order"].as_array().expect("an array");
    assert_eq!(fired.len(), 1, "expected one W0611, got {fired:?}");
    assert_eq!(fired[0]["code"], "W0611");
}

//! `ply run --json` reports what the reference-counting pass actually did.

use std::process::Command;

fn run(engine: &str, src: &str) -> serde_json::Value {
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join("m.ply");
    std::fs::write(&file, src).expect("write");
    let out = Command::new(env!("CARGO_BIN_EXE_ply"))
        .args(["run", "--json", "--engine", engine])
        .arg(&file)
        .output()
        .expect("`ply run` must start");
    serde_json::from_slice(&out.stdout).expect("`--json` must answer one object")
}

/// A loop that appends 200 times with the growing field last, so the machine can reuse at every
/// step and the counts are a round number a reader can check.
const APPENDS: &str = "\
fn build(n: Int) -> List<Int> =
  iterate({i: 0, out: []}, n + 1, |s: {i: Int, out: List<Int>}|
    if s.i >= n { Stop(s.out) } else { Continue({i: s.i + 1, out: push(s.out, s.i)}) })
fn main() -> Int = len(build(200))
";

#[test]
fn the_machine_reports_what_it_reused() {
    let v = run("machine", APPENDS);
    let c = &v["counters"];
    assert_eq!(c["engine"], "machine");
    assert_eq!(c["updates"], 200, "200 appends were made: {c}");
    assert_eq!(
        c["updates_in_place"], 200,
        "the growing field is last, so every append should reuse: {c}"
    );
    assert_eq!(c["in_place"], 1.0, "{c}");
}

/// The correction that makes this surface honest rather than merely present.
#[test]
fn the_tree_walker_reports_no_in_place_ratio_because_it_counts_nothing() {
    let v = run("treewalk", APPENDS);
    let c = &v["counters"];
    assert_eq!(c["engine"], "treewalk");
    assert_eq!(
        c["updates"], 200,
        "the appends still happen on this engine: {c}"
    );
    assert!(
        c["in_place"].is_null(),
        "`in_place` must be null on the tree-walker, which runs no reference \
         counting: a 0.0 here reads as a fact about the program and is a fact \
         about the engine. Got: {c}"
    );
}

/// `--engine both` runs two evaluators that do not count the same thing, and their sum is a figure
/// about neither.
#[test]
fn running_both_engines_reports_no_counters_rather_than_a_blend() {
    let v = run("both", APPENDS);
    assert!(
        v["counters"].is_null(),
        "counters under `--engine both` must be null rather than a blended \
         total. Got: {}",
        v["counters"]
    );
}

/// Non-vacuity: the counters must move with the program, or the three tests above would pass over a
/// surface that reports a constant.
#[test]
fn the_counts_follow_the_program_rather_than_being_a_constant() {
    let ten = run("machine", &APPENDS.replace("build(200)", "build(10)"));
    let many = run("machine", APPENDS);
    assert_eq!(ten["counters"]["updates"], 10, "{}", ten["counters"]);
    assert_eq!(many["counters"]["updates"], 200, "{}", many["counters"]);
}

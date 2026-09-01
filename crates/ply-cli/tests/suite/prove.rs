//! `ply prove` and `ply review` through the real binary.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

const UNSPECIFIED: &str = "\
fn double(x: Int) -> Int = x * 2

fn triple(x: Int) -> Int = x * 3

fn main() -> Int = double(21)
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

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn json_of(output: &std::process::Output) -> Value {
    let text = stdout_of(output);
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout was not one JSON object: {e}\n---\n{text}\n---"))
}

/// The line that is never behind a flag, on the command that has the least to report.
#[test]
fn prove_leads_with_the_review_surface() {
    let dir = project(UNSPECIFIED);
    let out = ply(dir.path()).arg("prove").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    let first = text.lines().next().unwrap_or_default();
    assert!(
        first.contains("3 definitions") && first.contains("0 carry an obligation"),
        "the coverage line has to be first: {text}"
    );
    assert!(
        text.contains("3 not covered by a claim that holds: m.double, m.main, m.triple"),
        "uncovered definitions are a list to work through, not only a number: {text}"
    );
    assert!(text.contains("0 obligations"), "{text}");
}

#[test]
fn prove_json_is_one_object_carrying_the_coverage_block() {
    let dir = project(UNSPECIFIED);
    let out = ply(dir.path()).args(["prove", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v = json_of(&out);
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["command"], "prove");
    assert_eq!(v["ok"], true);
    assert_eq!(v["coverage"]["definitions"], 3);
    assert_eq!(v["coverage"]["covered"], 0);
    assert_eq!(v["specified"], 0);
    assert_eq!(v["coverage"]["uncovered"].as_array().unwrap().len(), 3);
    assert!(v["plan"]["cases"].is_number());
    assert!(v["obligations"].as_array().unwrap().is_empty());
}

#[test]
fn prove_json_is_byte_identical_across_runs_and_job_counts() {
    let dir = project(UNSPECIFIED);
    let strip = |v: Value| {
        let mut v = v;
        // The clock is the one thing that legitimately differs.
        v["duration_ms"] = Value::Null;
        v
    };
    let one = strip(json_of(
        &ply(dir.path())
            .args(["prove", "--json", "--jobs", "1"])
            .output()
            .unwrap(),
    ));
    let many = strip(json_of(
        &ply(dir.path())
            .args(["prove", "--json", "--jobs", "16"])
            .output()
            .unwrap(),
    ));
    assert_eq!(one, many);
}

/// `ply prove` must not mark definitions as seen: a definition an obligation exercised has not been
/// vindicated as a *test* subject, and recording it would empty the next `ply test`'s suspect set.
#[test]
fn proving_does_not_touch_what_the_next_test_run_suspects() {
    let dir = project(UNSPECIFIED);
    let before = ply(dir.path())
        .args(["cache", "stats", "--json"])
        .output()
        .unwrap();
    let before = json_of(&before)["definitions_seen"].clone();

    assert_eq!(
        ply(dir.path()).arg("prove").output().unwrap().status.code(),
        Some(0)
    );

    let after = ply(dir.path())
        .args(["cache", "stats", "--json"])
        .output()
        .unwrap();
    assert_eq!(json_of(&after)["definitions_seen"], before);
}

/// One of each outcome, so the determinism and cache checks below run against a report that has
/// something to disagree about.
const SPECIFIED: &str = "\
fn one(x: Int) -> Int
  ensures result >= 0
= 1

fn size_of(xs: List<Int>) -> Int
  ensures result >= 0
= len(xs)

law \"length is never negative\"
  forall (xs: List<Int>) {
    size_of(xs) >= 0
  }

law \"a batch never holds more than three entries\"
  forall (xs: List<Int>) {
    len(xs) <= 3
  }
";

fn artifact(output: &std::process::Output) -> Value {
    let mut v = json_of(output);
    // The clock, and where each answer came from, are the two things that legitimately differ
    // between a warm run and a cold one.
    v["duration_ms"] = Value::Null;
    v["cached"] = Value::Null;
    v
}

/// The determinism check above runs over a program with no obligations, which cannot catch an order
/// that depends on the worker pool.
#[test]
fn prove_json_agrees_across_job_counts_over_real_obligations() {
    let dir = project(SPECIFIED);
    let one = artifact(
        &ply(dir.path())
            .args(["prove", "--json", "--jobs", "1", "--no-cache"])
            .output()
            .unwrap(),
    );
    let many = artifact(
        &ply(dir.path())
            .args(["prove", "--json", "--jobs", "16", "--no-cache"])
            .output()
            .unwrap(),
    );
    assert_eq!(one["summary"]["proved"], 1);
    assert_eq!(one["summary"]["refuted"], 1);
    assert_eq!(one, many);
}

/// The whole risk of an obligation cache is that a read says something the work would not have.
#[test]
fn a_cached_run_reports_exactly_what_a_fresh_one_does() {
    let dir = project(SPECIFIED);
    let fresh = artifact(
        &ply(dir.path())
            .args(["prove", "--json", "--no-cache"])
            .output()
            .unwrap(),
    );
    // Populate, then read back.
    ply(dir.path()).args(["prove", "--json"]).output().unwrap();
    let warm = &ply(dir.path()).args(["prove", "--json"]).output().unwrap();
    assert!(
        json_of(warm)["cached"].as_u64().unwrap_or(0) > 0,
        "the second run has to have read something"
    );
    assert_eq!(fresh, artifact(warm));
}

// --- review -----------------------------------------------------------------

#[test]
fn review_reports_everything_as_unreviewed_until_something_is_accepted() {
    let dir = project(UNSPECIFIED);
    let out = ply(dir.path())
        .args(["review", "--changed"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("3 definitions"), "{text}");
    assert!(text.contains("never reviewed"), "{text}");
}

#[test]
fn accepting_a_review_makes_the_next_one_quiet() {
    let dir = project(UNSPECIFIED);
    let accepted = ply(dir.path())
        .args(["review", "--accept"])
        .output()
        .unwrap();
    assert_eq!(accepted.status.code(), Some(0));
    assert!(stdout_of(&accepted).contains("3 definitions accepted"));

    let out = ply(dir.path())
        .args(["review", "--changed"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout_of(&out).contains("no definition changed since the last accepted review"),
        "{}",
        stdout_of(&out)
    );
}

/// The row where review still costs what it costs today — and the wording that must not overstate
/// it.
#[test]
fn an_unspecified_change_is_reported_as_invisible_rather_than_as_nothing() {
    let dir = project(UNSPECIFIED);
    ply(dir.path())
        .args(["review", "--accept"])
        .output()
        .unwrap();
    std::fs::write(
        dir.path().join("m.ply"),
        UNSPECIFIED.replace("x * 2", "x + x"),
    )
    .unwrap();

    let out = ply(dir.path())
        .args(["review", "--changed"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(
        text.contains("m.double · implementation changed · no spec"),
        "{text}"
    );
    assert!(
        text.contains("read the implementation, line by line"),
        "{text}"
    );
    assert!(
        text.contains(
            "carry no obligation that holds, so this run says nothing about whether their \
             behaviour changed"
        ),
        "the limit has to be visible at the point of use: {text}"
    );
    assert!(
        !text.contains("nothing changed"),
        "an unspecified change is still a change: {text}"
    );
}

#[test]
fn review_json_names_what_moved_and_what_the_claim_covers() {
    let dir = project(UNSPECIFIED);
    ply(dir.path())
        .args(["review", "--accept"])
        .output()
        .unwrap();
    std::fs::write(
        dir.path().join("m.ply"),
        UNSPECIFIED.replace("x * 3", "x + x + x"),
    )
    .unwrap();

    let out = ply(dir.path())
        .args(["review", "--changed", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["command"], "review");
    assert_eq!(v["definitions"], 3);
    assert_eq!(v["unspecified_changed"], 1);
    assert_eq!(v["specified_changed"], 0);
    let changed = v["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["name"], "m.triple");
    assert_eq!(changed[0]["implementation"], "changed");
    assert_eq!(changed[0]["spec"], "none");
    assert!(
        v["headline"]
            .as_str()
            .unwrap()
            .contains("carry no obligation"),
        "{v:#}"
    );
}

#[test]
fn renaming_a_definition_loses_its_baseline_rather_than_its_history() {
    let dir = project(UNSPECIFIED);
    ply(dir.path())
        .args(["review", "--accept"])
        .output()
        .unwrap();
    std::fs::write(
        dir.path().join("m.ply"),
        UNSPECIFIED.replace("triple", "treble"),
    )
    .unwrap();

    let out = ply(dir.path())
        .args(["review", "--changed", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    let changed = v["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["name"], "m.treble");
    assert_eq!(changed[0]["implementation"], "never reviewed");
}

/// The row `ply review` must never overstate: a definition whose only claim the machine could not
/// attempt.
#[test]
fn a_change_under_an_undischargeable_obligation_is_never_reported_as_checked() {
    const EFFECTFUL: &str = "\
effect db {
  read  get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Int
}

fn stored(k: Int) -> Int / {db.read[rows]}
  ensures result >= 0
= db.get[rows](k)
";
    let dir = project(EFFECTFUL);
    ply(dir.path())
        .args(["review", "--accept"])
        .output()
        .unwrap();
    std::fs::write(
        dir.path().join("m.ply"),
        EFFECTFUL.replace("= db.get[rows](k)", "= db.get[rows](k) + 1"),
    )
    .unwrap();

    let out = ply(dir.path())
        .args(["review", "--changed"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "a gap is not a failure");
    let text = stdout_of(&out);
    assert!(
        text.contains("m.stored · implementation changed · spec unchanged"),
        "{text}"
    );
    assert!(text.contains("unattempted"), "{text}");
    assert!(
        text.contains("not covered by a claim that holds: m.stored"),
        "the definition a reader still has to read has to be named: {text}"
    );
    assert!(
        !text.contains("no specified behaviour changed"),
        "nothing here established anything: {text}"
    );

    let v = json_of(
        &ply(dir.path())
            .args(["review", "--changed", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["broken"], 1);
    assert_eq!(v["undischarged"], 1);
    // The number the headline discloses the blind spot with, and the sentence an agent consuming
    // this artifact would act on.
    assert_eq!(v["specified_changed"], 0);
    assert_eq!(v["unspecified_changed"], 1);
    assert_eq!(v["changed"][0]["specified"], false);
    assert_eq!(
        v["changed"][0]["advice"],
        "no obligation on this definition holds: read the implementation, line by line"
    );
    assert_eq!(v["coverage"]["covered"], 0);
    assert_eq!(
        v["coverage"]["uncovered"].as_array().unwrap(),
        &vec![Value::from("m.stored")]
    );
    assert_eq!(v["changed"][0]["obligations"][0]["outcome"], "unattempted");
    assert_eq!(v["changed"][0]["obligations"][0]["tier"], Value::Null);
}

/// A guard the generator cannot hit is a gap in the **search**, not a defect in the spec, and the
/// two must not print the same thing.
#[test]
fn a_guard_the_search_missed_is_a_gap_and_not_a_vacuity() {
    const SOURCE: &str = "\
fn seen(xs: List<Int>, x: Int) -> Bool =
  match xs {
    [y, ..rest] -> if y == x { true } else { seen(rest, x) },
    _ -> false,
  }

law \"a narrow window\" forall (xs: List<Int>, x: Int)
  where x > 1000000 && x < 1000010 { seen(push(xs, x), x) }
";
    let dir = project(SOURCE);
    let out = ply(dir.path())
        .args(["prove", "--no-cache"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "a gap is not a failure");
    let text = stdout_of(&out);
    assert!(text.contains("unattempted"), "{text}");
    assert!(
        !text.contains("vacuous"),
        "the guard admits nine values: {text}"
    );
    assert!(
        text.contains("but it does admit a value"),
        "the report has to say which claim it is making: {text}"
    );

    let v = json_of(
        &ply(dir.path())
            .args(["prove", "--no-cache", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["ok"], true);
    assert_eq!(v["obligations"][0]["outcome"], "unattempted");
    assert_eq!(v["coverage"]["covered"], 0);
}

/// The boundary, end to end: a postcondition valid over ℤ and unevaluable at `i64::MAX` is not
/// proved, and the same claim over a domain its arithmetic fits in is.
#[test]
fn a_postcondition_that_raises_at_the_boundary_is_not_proved() {
    const SOURCE: &str = "\
fn inc(x: Int) -> Int
  ensures result > x
= x + 1

fn bounded(x: Int) -> Int
  requires x < 100
  ensures result > x
= x + 1
";
    let dir = project(SOURCE);
    let v = json_of(
        &ply(dir.path())
            .args(["prove", "--no-cache", "--json"])
            .output()
            .unwrap(),
    );
    let by_label = |needle: &str| -> Value {
        v["obligations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["label"].as_str().unwrap_or_default().contains(needle))
            .cloned()
            .unwrap_or_else(|| panic!("no obligation for `{needle}`: {v}"))
    };
    assert_eq!(by_label("m.inc")["outcome"], "unattempted");
    assert_eq!(by_label("m.inc")["tier"], Value::Null);
    assert_eq!(by_label("m.bounded")["tier"], "proved");
    assert_eq!(
        v["coverage"]["uncovered"].as_array().unwrap(),
        &vec![Value::from("m.inc")],
        "a definition whose only claim is a gap is one a reviewer still has to read"
    );
}

/// ADR 0014 §6.1: under a hermetic run — which is `ply prove`'s default — a `law/host` is reported
/// `W0604 unattempted` with the reason, never green.
#[test]
fn a_law_host_is_unattempted_under_a_hermetic_run_and_never_green() {
    const SOURCE: &str = "\
nondet effect db {
  read get[r](key: Int) -> Int
}

fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)

law/host \"the store answers\" forall (k: Int) { lookup(k) == lookup(k) }

law \"an ordinary claim\" forall (k: Int) { k == k }
";
    let dir = project(SOURCE);
    let out = ply(dir.path())
        .args(["prove", "--no-cache"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "a gap is not a failure");
    let text = stdout_of(&out);
    assert!(text.contains("unattempted"), "{text}");
    assert!(
        text.contains("reaches the host"),
        "the reader is not told why: {text}"
    );
    assert!(
        text.contains("ply prove --host"),
        "the reader is not told what to run: {text}"
    );
    assert!(
        text.contains("1 unattempted"),
        "the count has to carry it: {text}"
    );
    // And the law beside it is discharged as usual, so this is a claim about the one law rather
    // than about the run.
    assert!(text.contains("an ordinary claim"), "{text}");

    let v = json_of(
        &ply(dir.path())
            .args(["prove", "--no-cache", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["summary"]["unattempted"], 1);
    // A `law/host` can never be `proved`: the static tier and the finite enumeration are both
    // skipped, because either would be a claim about every value and the world is not a function of
    // the arguments.
    let hosted = v["obligations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| {
            o["owner"]
                .as_str()
                .is_some_and(|s| s.contains("the store answers"))
        })
        .expect("the host law is reported");
    assert_eq!(hosted["outcome"], "unattempted");
    assert!(hosted["tier"].is_null(), "{hosted}");
}

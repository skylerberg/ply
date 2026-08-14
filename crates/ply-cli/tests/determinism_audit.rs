//! The same audit as `ply-eval`'s `determinism_audit`, one level up: through the
//! real binary, across separate processes.
//!
//! A unit test shares an address space, an allocator and a thread, so it cannot
//! see the three things most likely to make a seeded run irreproducible in the
//! field — a different process, a different worker count, and a different set of
//! tests running beside it. Everything here is a claim about the *artifact*: the
//! `--json` an agent is handed, which is what a repro is made of.
//!
//! Timings are the only thing allowed to differ, and [`scrub`] is where that
//! exception is written down. Anything else that varies between two runs of one
//! seed is a defect: a seed that does not reproduce makes the artifact worthless.

use assert_cmd::Command;
use serde_json::{Map, Value};
use std::path::Path;
use tempfile::TempDir;

/// A racy test, a test that is order-insensitive, and one that never simulates —
/// so the artifact has failures, exhaustive searches and ordinary cached tests
/// in it at once, and a difference in any of them shows.
const CORPUS: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  clock.now();
  counter.put[n](seen + 1)
}

test "two increments race" {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| bump());
        let b = task.spawn(|| bump());
        task.join(a);
        task.join(b);
        assert_eq(counter.get[n](), 2)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}

test "two increments land in some order" {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| bump());
        let b = task.spawn(|| bump());
        task.join(a);
        task.join(b);
        assert(counter.get[n]() >= 1)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}

test "a sleeper costs no wall clock" {
  simulate {
    let t = task.spawn(|| { clock.sleep(30000000000); clock.now() });
    assert_eq(task.join(t), 30000000000)
  }
}

test "the arithmetic is not concurrent" { assert_eq(1 + 1, 2) }
"#;

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

/// Everything a run is allowed to differ in, and nothing else.
///
/// Wall-clock timings and the worker count are the whole of the exception:
/// timings because they measure the host, and `workers` because it is the flag
/// being varied. `front_end` is dropped whole — it is timings and nothing an
/// interleaving depends on. If a future field belongs here, it needs the same
/// one-line justification, because every name added is a place a divergence can
/// hide.
fn scrub(value: &Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .filter(|(k, _)| {
                    !(k.contains("duration")
                        || k.ends_with("_ms")
                        || k == &"front_end"
                        || k == &"workers"
                        || k == &"elapsed")
                })
                .map(|(k, v)| (k.clone(), scrub(v)))
                .collect::<Map<String, Value>>(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(scrub).collect()),
        other => other.clone(),
    }
}

fn artifact(dir: &Path, args: &[&str]) -> Value {
    let out = ply(dir)
        .args(["test", "--json", "--no-cache"])
        .args(args)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let json: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout was not one JSON object: {e}\n---\n{text}\n---"));
    scrub(&json)
}

/// The result for one test, by key, so that a whole-suite artifact and a
/// filtered one can be compared over the test they have in common.
fn result_for<'a>(artifact: &'a Value, key: &str) -> &'a Value {
    artifact["results"]
        .as_array()
        .expect("results is an array")
        .iter()
        .find(|r| r["key"].as_str().is_some_and(|k| k.ends_with(key)))
        .unwrap_or_else(|| panic!("no result for `{key}` in {artifact}"))
}

/// The claim in its plainest form: run the same command in eight separate
/// processes and get the same artifact.
///
/// Separate processes rather than a loop, because an allocator's layout, a
/// thread's identity and an environment's contents are the same within one
/// process and not across them. This is the test that fails if any of those
/// reaches a scheduling decision.
#[test]
fn one_seed_is_one_artifact_across_separate_processes() {
    let dir = project(CORPUS);
    let first = artifact(dir.path(), &["--seed", "0"]);
    for run in 1..8 {
        assert_eq!(
            artifact(dir.path(), &["--seed", "0"]),
            first,
            "process {run} produced a different artifact for one seed"
        );
    }
}

/// The same, for the search rather than for one interleaving. `dpor` is the
/// default and is where the reported counts, the exhaustiveness claim and the
/// failing seed all come from, so it is the mode whose reproducibility the
/// milestone actually sells.
#[test]
fn a_whole_search_is_one_artifact_across_separate_processes() {
    let dir = project(CORPUS);
    let first = artifact(dir.path(), &[]);
    for run in 1..6 {
        assert_eq!(
            artifact(dir.path(), &[]),
            first,
            "process {run} searched differently"
        );
    }
    // ...and the artifact is worth comparing: it has to carry a failure with a
    // seed in it, or this test would pass on an empty run.
    let failures = first["failures"].as_array().expect("failures is an array");
    assert!(
        failures.iter().any(|f| f["seed"].is_string()),
        "the corpus must produce a seeded failure, or these comparisons prove nothing: {first}"
    );
}

/// A scheduling decision that read anything varying with thread count would show
/// here and nowhere else. `rayon` schedules whole tests, so a worker count
/// changes which tests share a thread and in what order they reach it.
#[test]
fn the_worker_count_does_not_reach_a_scheduling_decision() {
    let dir = project(CORPUS);
    let one = artifact(dir.path(), &["--jobs", "1"]);
    for jobs in ["2", "3", "8", "17"] {
        assert_eq!(
            artifact(dir.path(), &["--jobs", jobs]),
            one,
            "`--jobs {jobs}` disagreed with `--jobs 1`"
        );
    }
}

/// A test run alone must report exactly what it reports run beside others. If it
/// does not, a `--filter` handed to an agent to reproduce a failure reproduces
/// something else — which is the one thing the replay path may not do.
#[test]
fn running_a_test_alone_reports_what_it_reports_in_company() {
    let dir = project(CORPUS);
    let whole = artifact(dir.path(), &[]);
    for name in [
        "two increments race",
        "two increments land in some order",
        "a sleeper costs no wall clock",
    ] {
        let alone = artifact(dir.path(), &["--filter", name]);
        assert_eq!(
            result_for(&alone, name),
            result_for(&whole, name),
            "`{name}` reported differently when run alone"
        );
    }
}

/// The front-end cache changes what is parsed, hashed and restored rather than
/// re-derived. None of that may reach an interleaving, and a warm cache is the
/// state every run after the first is in.
#[test]
fn a_warm_front_end_cache_does_not_change_an_interleaving() {
    let dir = project(CORPUS);
    let cold = artifact(dir.path(), &["--no-incremental"]);
    // Populate the front-end cache, then run against it.
    ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let warm = artifact(dir.path(), &[]);
    assert_eq!(
        warm, cold,
        "a warm front-end cache changed what the search did"
    );
}

/// The repro path, end to end: the seed a failure prints, handed back to the
/// binary, reproduces that failure and no other.
#[test]
fn the_seed_a_failure_prints_replays_that_failure() {
    let dir = project(CORPUS);
    let searched = artifact(dir.path(), &[]);
    let failure = searched["failures"]
        .as_array()
        .expect("failures is an array")
        .iter()
        .find(|f| f["seed"].is_string())
        .expect("the racy test must fail with a seed");
    let seed = failure["seed"].as_str().expect("a seed").to_string();
    let message = failure["diagnostic"]["message"].clone();

    let replayed = artifact(dir.path(), &["--seed", &seed]);
    let again = replayed["failures"]
        .as_array()
        .expect("failures is an array")
        .iter()
        .find(|f| f["seed"].as_str() == Some(seed.as_str()))
        .unwrap_or_else(|| panic!("seed {seed} did not reproduce the failure: {replayed}"));
    assert_eq!(
        again["diagnostic"]["message"], message,
        "seed {seed} reproduced a different failure"
    );

    // Twice more, in two more processes, because a repro that works once is not
    // a repro.
    for _ in 0..2 {
        assert_eq!(artifact(dir.path(), &["--seed", &seed]), replayed);
    }
}

/// The converse defect. If every seed named the same interleaving the search
/// would be theatre, and it would look exactly like a search that works.
#[test]
fn a_seed_actually_decides_which_interleaving_runs() {
    let dir = project(CORPUS);
    let mut verdicts = std::collections::BTreeSet::new();
    for root in 0..24u64 {
        let run = artifact(dir.path(), &["--seed", &root.to_string()]);
        verdicts.insert(
            result_for(&run, "two increments race")["status"]
                .as_str()
                .expect("a status")
                .to_string(),
        );
    }
    assert!(
        verdicts.len() > 1,
        "24 seeds gave one verdict on a racy test, so the seed decides nothing: {verdicts:?}"
    );
}

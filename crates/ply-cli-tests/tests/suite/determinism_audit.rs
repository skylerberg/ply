use crate::harness::{ply, project};
use serde_json::{Map, Value};
use std::path::Path;

/// Failures, exhaustive searches and ordinary cached tests at once, so a difference in any of them shows.
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

/// Everything a run is allowed to differ in, and nothing else.
fn scrub(value: &Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .filter(|(k, _)| {
                    // `*_nanos` are the tier's compile wall-clocks, which vary run to run like every duration.
                    !(k.contains("duration")
                        || k.ends_with("_ms")
                        || k.ends_with("_nanos")
                        || k == &"front_end"
                        || k == &"workers"
                        // The tier builds a unit per worker, so `units` scales with `--jobs`: a compile artifact, not a scheduling decision.
                        || k == &"units"
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

fn result_for<'a>(artifact: &'a Value, key: &str) -> &'a Value {
    artifact["results"]
        .as_array()
        .expect("results is an array")
        .iter()
        .find(|r| r["key"].as_str().is_some_and(|k| k.ends_with(key)))
        .unwrap_or_else(|| panic!("no result for `{key}` in {artifact}"))
}

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
    // The artifact must carry the race's own failure with a seed, or this test would pass on an
    // empty run, or on one the tier declined.
    let failures = first["failures"].as_array().expect("failures is an array");
    assert!(
        failures
            .iter()
            .any(|f| f["seed"].is_string() && f["diagnostic"]["code"] == "E0501"),
        "the corpus must produce a seeded failure, or these comparisons prove nothing: {first}"
    );
}

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

#[test]
fn a_warm_front_end_cache_does_not_change_an_interleaving() {
    let dir = project(CORPUS);
    let cold = artifact(dir.path(), &[]);
    // Populate the front-end cache, then run against it.
    ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let warm = artifact(dir.path(), &[]);
    assert_eq!(
        warm, cold,
        "a warm front-end cache changed what the search did"
    );
}

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
    assert_eq!(
        failure["diagnostic"]["code"], "E0501",
        "the race's own assertion is what the seed names: {failure}"
    );
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

    // Twice more, in two more processes, because a repro that works once is not a repro.
    for _ in 0..2 {
        assert_eq!(artifact(dir.path(), &["--seed", &seed]), replayed);
    }
}

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

/// The tasks touch the cell themselves, so a race is reported over the cell's name rather than over
/// an effect's atom.
const CELL_RACE: &str = r#"
test "a lost update on a cell" {
  with_cell[n](0) { c ->
    simulate {
      let a = task.spawn(|| { let seen = cell_get(c); clock.now(); cell_set(c, seen + 1) });
      let b = task.spawn(|| { let seen = cell_get(c); clock.now(); cell_set(c, seen + 1) });
      task.join(a);
      task.join(b);
      assert_eq(cell_get(c), 2)
    }
  }
}
"#;

/// Each interleaving is an entry on the tier the test runs on, as is every search before it on that
/// thread, so a race names the cell as a fresh tier does or the report depends on what ran first.
#[test]
fn every_search_on_one_thread_names_a_races_cell_alike() {
    let source: String = (0..3)
        .map(|i| {
            CELL_RACE.replace(
                "a lost update on a cell",
                &format!("a lost update on cell {i}"),
            )
        })
        .collect();
    let dir = project(&source);
    let run = artifact(dir.path(), &["--jobs", "1"]);
    let races: Vec<&Value> = run["failures"]
        .as_array()
        .expect("failures is an array")
        .iter()
        .map(|f| &f["race"])
        .collect();
    assert_eq!(races.len(), 3, "each copy reaches the lost update: {run}");
    for race in &races {
        for side in ["left", "right"] {
            let access = race[side]["access"].as_str().unwrap_or_default();
            assert!(
                access.starts_with("cell.") && access.ends_with("[@0.0]"),
                "the test's one cell, named as a fresh tier names it: {race}"
            );
        }
        // Each copy has spans and a name of its own; what it names of the cell is the same.
        let named = |r: &Value| {
            (
                r["at"].clone(),
                [&r["left"], &r["right"]].map(|s| (s["task"].clone(), s["access"].clone())),
            )
        };
        assert_eq!(
            named(race),
            named(races[0]),
            "a later search on the thread named the race differently"
        );
    }
}

const TWO_REGIONS: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  clock.now();
  counter.put[n](seen + 1)
}

test "a race in the first region and a quiet second one" {
  with_cell[n](0) { c ->
    handle {
      {
        simulate {
          let a = task.spawn(|| bump());
          let b = task.spawn(|| bump());
          task.join(a);
          task.join(b)
        };
        simulate {
          clock.now();
          ()
        };
        assert_eq(counter.get[n](), 2)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;

const REGION_IN_A_HELPER: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  clock.now();
  counter.put[n](seen + 1)
}

fn race() -> Unit / {counter.read[n], counter.write[n], sim.read} = simulate {
  let a = task.spawn(|| bump());
  let b = task.spawn(|| bump());
  task.join(a);
  task.join(b)
}

test "the same region twice through a call" {
  with_cell[n](0) { c ->
    handle {
      {
        race();
        race();
        assert_eq(counter.get[n](), 4)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;

/// The fixture's assertion seeing an update lost: a failed test whose diagnostic says so.
#[track_caller]
fn assert_lost_update(run: &Value, total: i64, what: &str) {
    let failure = run["failures"]
        .as_array()
        .and_then(|fs| fs.first())
        .unwrap_or_else(|| panic!("{what}: no interleaving failed: {run}"));
    assert_eq!(failure["diagnostic"]["code"], "E0501", "{what}: {failure}");
    let message = failure["diagnostic"]["message"]
        .as_str()
        .unwrap_or_default();
    let lost = format!("assertion failed: expected {total}, found ");
    assert!(
        message.starts_with(&lost) && message != format!("{lost}{total}"),
        "{what}: {message}"
    );
    assert!(
        failure["seed"].is_string(),
        "{what}: a lost update names its seed: {failure}"
    );
}

#[test]
fn a_second_simulate_region_does_not_hide_the_first_regions_race() {
    for (source, name, total) in [
        (TWO_REGIONS, "two regions written out", 2),
        (
            REGION_IN_A_HELPER,
            "one region reached twice through a call",
            4,
        ),
    ] {
        let dir = project(source);
        let sampled = artifact(dir.path(), &["--sim", "random", "--seeds", "64"]);
        assert_lost_update(
            &sampled,
            total,
            &format!(
                "`{name}`: the fixture must contain a reachable lost update, or it proves nothing"
            ),
        );
        let searched = artifact(dir.path(), &["--sim-budget", "1024"]);
        assert_lost_update(&searched, total, &format!("`{name}`: the search"));
    }
}

/// The second region's shape depends on what the first raced to, so replaying a branch rebuilds a
/// region whose enabled sets differ from the recording's only if the replay strayed.
const SHAPE_FOLLOWS_A_RACE: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
  write note[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.get[n], counter.put[n], clock.now} = {
  let seen = counter.get[n]();
  clock.now();
  counter.put[n](seen + 1)
}

fn noise() -> Unit / {counter.get[n], counter.note[m], clock.now} = {
  let seen = counter.get[n]();
  clock.now();
  counter.note[m](seen)
}

test "the second region's shape depends on what the first raced to" {
  with_cell[n](0) { c -> {
  with_cell[m](0) { d ->
    handle {
      {
        simulate {
          let a = task.spawn(|| bump());
          let b = task.spawn(|| bump());
          task.join(a);
          task.join(b)
        };
        simulate {
          if counter.get[n]() == 2 {
            let a = task.spawn(|| noise());
            let b = task.spawn(|| noise());
            task.join(a);
            task.join(b)
          } else {
            let a = task.spawn(|| noise());
            task.join(a)
          }
        };
        assert(cell_get(d) >= 0)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
      counter.note[m](v) -> cell_set(d, v),
    }
  }
  } }
}
"#;

#[test]
fn a_legal_program_is_never_reported_as_a_simulation_divergence() {
    let dir = project(SHAPE_FOLLOWS_A_RACE);
    let run = artifact(dir.path(), &[]);
    if let Some(failure) = run["failures"]
        .as_array()
        .expect("failures is an array")
        .first()
    {
        assert_ne!(
            failure["diagnostic"]["code"], "E0415",
            "a legal program was blamed on Ply's simulation: {failure}"
        );
        panic!("its assertion holds in every interleaving, and a run failed: {failure}");
    }
    // Replay is checked on a branch, so a search that took none compared nothing.
    let explored = run["results"][0]["simulation"]["explored"]
        .as_u64()
        .unwrap_or(0);
    assert!(explored > 1, "{run}");
}

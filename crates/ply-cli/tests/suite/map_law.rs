//! A law quantified over a `Map`, through the real binary.
//!
//! Leaving a new primitive ungeneratable would regress M8's guarantee on contact
//! with it — the law would report `E0418` and the coverage line would count a
//! definition as unclaimed for a reason nobody could act on. This is the same
//! argument, and the same required test, that `Bytes` got in W1.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

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

const HOLDS: &str = "\
fn size(m: Map<String, Int>) -> Int = map_len(m)

law \"a map's key count is its length\"
  forall (m: Map<String, Int>) {
    len(map_keys(m)) == size(m)
  }

law \"inserting a key you already have does not grow the map\"
  forall (m: Map<String, Int>, k: String, v: Int) where map_contains(m, k) {
    map_len(map_insert(m, k, v)) == map_len(m)
  }
";

/// Required test: a `forall (m: Map<String, Int>)` law is discharged rather than
/// `E0418`.
#[test]
fn a_law_over_a_map_is_discharged() {
    let dir = project(HOLDS);
    let out = ply(dir.path()).args(["prove", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", json_of(&out));
    let v = json_of(&out);
    let obligations = v["obligations"].as_array().unwrap();
    assert_eq!(obligations.len(), 2, "{v}");
    for o in obligations {
        // A tier at all is the claim: a binder the generator refuses reports a
        // gap and no tier, which is what `E0418` looks like from out here.
        assert!(
            o["tier"].is_string(),
            "a map law must earn a tier rather than a gap: {o}"
        );
        assert!(o["gap"].is_null(), "{o}");
    }
    // The unguarded one is sampled over the whole domain, so it earns the
    // stronger of the two labels a map can reach.
    assert_eq!(obligations[0]["tier"], "property", "{v}");
}

/// The other half: a false law over a map is refuted, and the counterexample
/// shrinks toward `map_new()` — entries come out before values do, so the
/// witness is the smallest map that still breaks it rather than whatever the
/// generator happened to draw.
#[test]
fn a_refuted_map_law_shrinks_toward_the_empty_map() {
    let dir = project(
        "law \"every map is empty\"\n  forall (m: Map<String, Int>) {\n    map_len(m) == 0\n  }\n",
    );
    let out = ply(dir.path()).args(["prove", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "a false law must fail the run");
    let v = json_of(&out);
    let o = &v["obligations"].as_array().unwrap()[0];
    assert_eq!(o["outcome"], "refuted", "{o}");
    let binding = &o["counterexample"]["bindings"][0];
    assert_eq!(binding["name"], "m", "{o}");
    // One entry, and the smallest key and value the generators reach: any larger
    // witness means the shrinker stopped early or never entered the map. The
    // draw it started from is reported beside it, and is bigger.
    assert_eq!(binding["value"], "{\"\": 0}", "{o}");
    assert_ne!(
        o["counterexample"]["original"][0]["value"], binding["value"],
        "the witness must have been shrunk, not merely drawn: {o}"
    );
}

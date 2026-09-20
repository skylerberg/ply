use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;

const SOURCE: &str = "\
fn add(a: Int, b: Int) -> Int = a + b
fn clamp(n: Int) -> Int = if n < 0 { 0 } else { n }
fn unused(n: Int) -> Int = n * 2 + 0
test \"add adds\" { assert_eq(add(2, 3), 5); assert_eq(add(0, 1), 1) }
test \"clamp keeps a positive\" { assert_eq(clamp(5), 5); assert_eq(clamp(-3), 0) }
";

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), SOURCE).unwrap();
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn json_of(out: &std::process::Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn coverage_names_the_definitions_no_test_reaches() {
    let dir = project();
    let out = ply(dir.path())
        .args(["test", "--coverage", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["exit_code"], 0, "{v}");
    assert_eq!(v["coverage"]["unreached"], serde_json::json!(["m.unused"]));
    let add = v["coverage"]["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "m.add")
        .unwrap();
    assert_eq!(add["reached_by"], serde_json::json!(["m.add adds"]));
}

#[test]
fn a_mutant_the_tests_let_through_survives_and_fails_the_run() {
    let dir = project();
    let out = ply(dir.path())
        .args(["test", "--mutate", "clamp", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    let m = &v["mutants"];
    assert_eq!(m["definitions"], 1, "{m}");
    assert!(m["killed"].as_u64().unwrap() >= 1, "{m}");
    assert!(m["survived"].as_u64().unwrap() >= 1, "{m}");
    let survivor = &m["survivors"][0];
    assert_eq!(survivor["definition"], "m.clamp");
    assert_eq!(
        survivor["tests"],
        serde_json::json!(["m.clamp keeps a positive"])
    );
    assert!(
        survivor["location"]["line"].as_u64().unwrap() >= 2,
        "{survivor}"
    );
    assert_eq!(v["exit_code"], 1, "a survivor fails the run: {v}");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn every_mutant_of_a_well_tested_definition_is_killed_and_the_cache_is_untouched() {
    let dir = project();
    ply(dir.path()).arg("test").assert().success();
    let before = std::fs::read(dir.path().join(".ply-cache/results.json")).unwrap();

    let out = ply(dir.path())
        .args(["test", "--mutate", "m.add", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    let m = &v["mutants"];
    assert_eq!(m["survived"], 0, "{m}");
    assert!(m["killed"].as_u64().unwrap() >= 1, "{m}");
    assert_eq!(v["exit_code"], 0, "{v}");

    let after = std::fs::read(dir.path().join(".ply-cache/results.json")).unwrap();
    assert_eq!(before, after, "a mutant left something in the cache");
}

#[test]
fn a_definition_no_test_reaches_is_reported_rather_than_mutated() {
    let dir = project();
    let out = ply(dir.path())
        .args(["test", "--mutate", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    let m = &v["mutants"];
    assert_eq!(m["unreached"], serde_json::json!(["m.unused"]), "{m}");
    assert_eq!(m["definitions"], 3);

    let out = ply(dir.path())
        .args(["test", "--mutate", "nothing"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

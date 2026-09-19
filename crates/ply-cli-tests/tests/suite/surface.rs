use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;

const SOURCE: &str = "\
fn one() -> Int = 1
fn two() -> Int = one() + 1
fn three() -> Int = two() + 1
fn alone() -> Int = 3
test \"two is two\" { assert_eq(two(), 2) }
test \"alone is three\" { assert_eq(alone(), 3) }
law \"three is three\" { three() == 3 }
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
fn defs_lists_every_definition_with_its_place_hash_signature_and_references() {
    let dir = project();
    let out = ply(dir.path()).args(["defs", "--json"]).output().unwrap();
    let v = json_of(&out);
    assert_eq!(v["exit_code"], 0);
    let defs = v["definitions"].as_array().unwrap();
    let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["m.one", "m.two", "m.three", "m.alone"]);
    let two = &defs[1];
    assert_eq!(two["type"], "() -> Int");
    assert_eq!(two["footprint"], "{}");
    assert_eq!(two["deps"], serde_json::json!(["m.one"]));
    assert_eq!(two["hash"].as_str().unwrap().len(), 64);
    assert!(
        two["location"].as_str().unwrap().ends_with("m.ply:2:1"),
        "{two}"
    );
    assert_eq!(two["start"], 20);

    let out = ply(dir.path())
        .args(["defs", "--filter", "thr"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("m.three") && !text.contains("m.two"),
        "{text}"
    );
}

#[test]
fn callers_names_what_mentions_a_definition_and_what_reaches_it() {
    let dir = project();
    let out = ply(dir.path())
        .args(["callers", "one", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["definition"]["name"], "m.one");
    let names = |group: &str, kind: &str| -> Vec<String> {
        v[group][kind]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names("direct", "definitions"), ["m.two"]);
    assert!(names("direct", "tests").is_empty());
    assert_eq!(names("transitive", "definitions"), ["m.three", "m.two"]);
    assert_eq!(names("transitive", "tests"), ["two is two"]);
    assert_eq!(names("transitive", "laws"), ["three is three"]);

    let out = ply(dir.path())
        .args(["callers", "m.alone"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("test \"alone is three\""), "{text}");

    let out = ply(dir.path())
        .args(["callers", "nothing"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

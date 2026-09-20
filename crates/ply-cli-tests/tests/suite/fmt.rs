use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

const UNFORMATTED: &str = "\
// A leading comment.
fn   add(a: Int,b: Int)->Int = a+b // trailing


fn main() -> Int = { let x = 0xFF; add(x, 1_000) }
";

const FORMATTED: &str = "\
// A leading comment.
fn add(a: Int, b: Int) -> Int = a + b  // trailing

fn main() -> Int = {
  let x = 0xFF;
  add(x, 1_000)
}
";

#[test]
fn a_file_is_formatted_in_place_and_a_second_run_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("m.ply");
    std::fs::write(&file, UNFORMATTED).unwrap();
    let out = ply(dir.path()).args(["fmt", "m.ply"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "formatted m.ply\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), FORMATTED);

    let out = ply(dir.path()).args(["fmt", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["command"], "fmt");
    assert_eq!(v["ok"], true);
    assert_eq!(v["files"][0]["path"], "m.ply");
    assert_eq!(v["files"][0]["changed"], false);
    assert_eq!(v["errors"].as_array().unwrap().len(), 0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), FORMATTED);
}

#[test]
fn check_names_the_file_that_would_change_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("m.ply");
    std::fs::write(&file, UNFORMATTED).unwrap();
    let out = ply(dir.path()).args(["fmt", "--check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "would format m.ply\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), UNFORMATTED);

    let out = ply(dir.path())
        .args(["fmt", "--check", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["exit_code"], 1);
    assert_eq!(v["files"][0]["changed"], true);
}

#[test]
fn a_file_that_does_not_parse_exits_two_and_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("m.ply");
    let broken = "fn f( = 1\n";
    std::fs::write(&file, broken).unwrap();
    let out = ply(dir.path()).args(["fmt"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("m.ply") && stderr.contains("E0001"),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), broken);

    let out = ply(dir.path()).args(["fmt", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["exit_code"], 2);
    assert_eq!(v["errors"][0]["path"], "m.ply");
}

/// The diagnostic codes `ply check` reports for `dir`, sorted; what formatting must not change.
fn check_codes(dir: &Path, target: &str) -> Vec<String> {
    let out = ply(dir).args(["check", target, "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    let mut codes: Vec<String> = v["diagnostics"]
        .as_array()
        .map(|ds| {
            ds.iter()
                .filter_map(|d| d["code"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    codes.sort();
    codes
}

/// Formats a copy of every `.ply` file under `relative` and requires the result to be a fixed
/// point that `ply check` reads the same way it read the original.
fn corpus_round_trip(relative: &str, per_file: bool) {
    let dir = tempfile::tempdir().unwrap();
    let mut names = Vec::new();
    for entry in std::fs::read_dir(repo().join(relative)).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|x| x == "ply") {
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            std::fs::copy(&path, dir.path().join(&name)).unwrap();
            names.push(name);
        }
    }
    names.sort();
    assert!(!names.is_empty(), "{relative} holds no .ply files");
    let targets: Vec<String> = if per_file {
        names.clone()
    } else {
        vec![".".to_string()]
    };
    for target in &targets {
        let before = check_codes(dir.path(), target);
        let out = ply(dir.path()).args(["fmt", target]).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        match out.status.code() {
            Some(0) => {}
            // A fixture written to exercise the parser's recovery does not parse, and stays as it was.
            Some(2) if per_file && target.contains("-err-") => {
                assert!(stderr.contains("E0"), "{target}: {stderr}");
                continue;
            }
            code => panic!("{relative}/{target}: ply fmt exited {code:?}\n{stderr}"),
        }
        let again = ply(dir.path())
            .args(["fmt", "--check", target])
            .output()
            .unwrap();
        assert_eq!(
            again.status.code(),
            Some(0),
            "{relative}/{target} is not a fixed point:\n{}{}",
            String::from_utf8_lossy(&again.stdout),
            String::from_utf8_lossy(&again.stderr)
        );
        assert_eq!(
            check_codes(dir.path(), target),
            before,
            "{relative}/{target} checks differently after formatting"
        );
    }
}

#[test]
fn the_examples_format_to_a_fixed_point_and_still_check() {
    corpus_round_trip("examples", true);
}

#[test]
fn the_standard_library_formats_to_a_fixed_point_and_still_checks() {
    corpus_round_trip("crates/ply-std/ply", true);
}

#[test]
fn the_parser_fixtures_format_to_a_fixed_point_and_still_check() {
    corpus_round_trip("crates/ply-codegen-tests/fixtures", true);
}

#[test]
fn the_compiler_formats_to_a_fixed_point_and_still_checks() {
    corpus_round_trip("crates/ply-compiler/ply", false);
}

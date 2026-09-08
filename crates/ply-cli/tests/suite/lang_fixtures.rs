//! The language claims that are *raises*, sorted out of `crates/ply-eval-tests` (ADR 0045
//! §"The suites"): a fixture under `tests/fixtures/lang/` states, in `// raises <test> :: <text>`
//! lines, which of its tests fail with `E0502` carrying `<text>`, and this runs it on the machine
//! and with the tier as the only engine.

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
        .canonicalize()
        .expect("the repository path exists")
}

fn expectations(source: &str) -> Vec<(String, String)> {
    source
        .lines()
        .filter_map(|l| l.strip_prefix("// raises "))
        .map(|l| {
            let (test, text) = l.split_once(" :: ").expect("`raises <test> :: <text>`");
            (test.trim().to_string(), text.trim().to_string())
        })
        .collect()
}

fn failures(dir: &Path, tier_only: bool) -> Vec<Value> {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.args(["--color", "never", "test", "--json", "--no-cache"])
        .current_dir(dir);
    if tier_only {
        cmd.args(["--backend", "c"]).env("PLY_TIER_ONLY", "1").env(
            "PLY_C_EMITTER",
            format!("ply-whole:{}", repo("spikes/ply-parser").display()),
        );
    }
    let out = cmd.output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let report: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    report["failures"]
        .as_array()
        .expect("a failures array")
        .clone()
}

fn check(fixture: &str, tier_only: bool) {
    let source = std::fs::read_to_string(repo(&format!("tests/fixtures/lang/{fixture}.ply")))
        .expect("the fixture is part of the repository");
    let expected = expectations(&source);
    assert!(!expected.is_empty(), "{fixture} states no raises");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), &source).unwrap();
    let failed = failures(dir.path(), tier_only);
    let engine = if tier_only { "the tier" } else { "the machine" };
    for (test, text) in &expected {
        let failure = failed
            .iter()
            .find(|f| f["name"].as_str() == Some(test))
            .unwrap_or_else(|| panic!("{fixture}: `{test}` did not fail on {engine}: {failed:?}"));
        let diagnostic = &failure["diagnostic"];
        assert_eq!(
            diagnostic["code"].as_str(),
            Some("E0502"),
            "{fixture}: `{test}` on {engine}: {diagnostic}"
        );
        // The machine labels a span; compiled code, which has none, says the same in a note.
        let texts = |key: &str| {
            diagnostic[key]
                .as_array()
                .map(|xs| {
                    xs.iter()
                        .map(|x| {
                            x.as_str()
                                .or_else(|| x["message"].as_str())
                                .unwrap_or_default()
                                .to_string()
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default()
        };
        let carried = format!(
            "{} {} {}",
            diagnostic["message"].as_str().unwrap_or_default(),
            texts("labels"),
            texts("notes")
        );
        assert!(
            carried.contains(text.as_str()),
            "{fixture}: `{test}` on {engine} raised without `{text}`: {diagnostic}"
        );
    }
    assert_eq!(
        failed.len(),
        expected.len(),
        "{fixture} on {engine}: every failure must be an expected raise: {failed:?}"
    );
}

fn footprints(source: &str) -> Vec<(String, String)> {
    source
        .lines()
        .filter_map(|l| l.strip_prefix("// footprint "))
        .map(|l| {
            let (def, row) = l
                .split_once(" :: ")
                .expect("`footprint <definition> :: <row>`");
            (def.trim().to_string(), row.trim().to_string())
        })
        .collect()
}

/// A claim about the checker's footprint inference: `ply check --json` reports the row.
#[test]
fn the_footprints_the_checker_infers_are_the_ones_stated() {
    let source = std::fs::read_to_string(repo("tests/fixtures/lang/footprints.ply"))
        .expect("the fixture is part of the repository");
    let expected = footprints(&source);
    assert!(!expected.is_empty());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), &source).unwrap();
    let out = Command::cargo_bin("ply")
        .unwrap()
        .args(["--color", "never", "check", "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let report: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    let defs = report["definitions"]
        .as_array()
        .unwrap_or_else(|| panic!("a definitions array: {text}"));
    for (def, row) in &expected {
        let name = format!("m.{def}");
        let found = defs
            .iter()
            .find(|d| d["name"].as_str() == Some(&name))
            .unwrap_or_else(|| panic!("`{name}` is not reported: {text}"));
        assert_eq!(found["footprint"].as_str(), Some(row.as_str()), "`{name}`");
    }
}

#[test]
fn the_numeric_raises_are_the_same_on_both_engines() {
    check("numbers_raise", false);
    check("numbers_raise", true);
}

#[test]
fn the_shift_and_overflow_raises_are_the_same_on_both_engines() {
    check("bits_raise", false);
    check("bits_raise", true);
}

#[test]
fn the_byte_builtins_raises_are_the_same_on_both_engines() {
    check("bytes_raise", false);
    check("bytes_raise", true);
}

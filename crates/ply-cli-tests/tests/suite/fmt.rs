use assert_cmd::Command;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
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

/// A `ply fmt` over `dir`, as JSON. `check` asks what would move instead of moving it.
fn fmt_json(dir: &Path, check: bool) -> Value {
    let mut cmd = ply(dir);
    cmd.arg("fmt");
    if check {
        cmd.arg("--check");
    }
    let out = cmd.arg("--json").output().unwrap();
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

/// Formats a copy of every `.ply` file under `relative` and requires the result to be a fixed
/// point that `ply check` reads the same way it read the original.
///
/// One `ply fmt` over the corpus, not one per file: the command walks the tree and reports every
/// file it took, so the per-file answer is already in its report. The `check` runs are kept for
/// the files the formatter touches, since an untouched file's diagnostics were read off the same
/// bytes they are read off after.
fn corpus_round_trip(relative: &str) {
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

    // What the formatter would move, and what it will not parse.
    let planned = fmt_json(dir.path(), true);
    let reported: BTreeSet<String> = planned["files"]
        .as_array()
        .expect("a files array")
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    let moving: BTreeSet<String> = planned["files"]
        .as_array()
        .expect("a files array")
        .iter()
        .filter(|f| f["changed"].as_bool() == Some(true))
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    let refused: BTreeMap<String, String> = planned["errors"]
        .as_array()
        .expect("an errors array")
        .iter()
        .map(|e| {
            (
                e["path"].as_str().unwrap().to_string(),
                e["error"].as_str().unwrap().to_string(),
            )
        })
        .collect();

    // Every file is one the formatter named or one it refused.
    for name in &names {
        assert!(
            reported.contains(name) || refused.contains_key(name),
            "{relative}/{name}: ply fmt --check named neither that it would move nor that it refused"
        );
    }

    // A fixture written to exercise the parser's recovery does not parse, and stays as it was.
    for (name, why) in &refused {
        assert!(why.contains("E0"), "{relative}/{name}: {why}");
        assert_eq!(
            std::fs::read(dir.path().join(name)).unwrap(),
            std::fs::read(repo().join(relative).join(name)).unwrap(),
            "{relative}/{name} was rewritten although it does not parse"
        );
    }

    // What `ply check` said about every file the formatter will touch.
    let before: BTreeMap<String, Vec<String>> = moving
        .iter()
        .chain(refused.keys())
        .map(|name| (name.clone(), check_codes(dir.path(), name)))
        .collect();
    for (name, why) in &refused {
        assert!(
            before[name].iter().any(|c| c.starts_with('E')),
            "{relative}/{name}: ply fmt refused it ({why}) but `ply check` raised no error"
        );
    }

    // One `ply fmt` over the corpus.
    let done = fmt_json(dir.path(), false);
    assert_eq!(
        done["ok"].as_bool(),
        Some(refused.is_empty()),
        "{relative}: ply fmt answered {done}"
    );

    // The answer is a fixed point: a second check finds nothing left to move.
    let again = fmt_json(dir.path(), true);
    let moved: Vec<String> = again["files"]
        .as_array()
        .expect("a files array")
        .iter()
        .filter(|f| f["changed"].as_bool() == Some(true))
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    if !moved.is_empty() {
        let mut report = String::new();
        for file in &moved {
            let once = std::fs::read_to_string(dir.path().join(file)).unwrap();
            ply(dir.path()).args(["fmt", file]).output().unwrap();
            let twice = std::fs::read_to_string(dir.path().join(file)).unwrap();
            report.push_str(&first_difference(file, &once, &twice));
        }
        panic!("{relative} is not a fixed point:\n{again}\n{report}");
    }

    // Formatting moved no diagnostic, for every file it touched.
    for name in &moving {
        assert_eq!(
            check_codes(dir.path(), name),
            before[name],
            "{relative}/{name} checks differently after formatting"
        );
    }
}

/// The first line where a second pass over `file` differs from the first, with five lines of
/// context from each.
fn first_difference(file: &str, once: &str, twice: &str) -> String {
    let a: Vec<&str> = once.lines().collect();
    let b: Vec<&str> = twice.lines().collect();
    let at = (0..a.len().max(b.len()))
        .find(|&i| a.get(i) != b.get(i))
        .unwrap_or(0);
    let window = |lines: &[&str]| -> String {
        let lo = at.saturating_sub(5);
        let hi = (at + 6).min(lines.len());
        lines[lo..hi]
            .iter()
            .enumerate()
            .map(|(i, l)| format!("{:>5} | {l}\n", lo + i + 1))
            .collect()
    };
    format!(
        "--- {file}: line {} differs\n--- first pass:\n{}--- second pass:\n{}",
        at + 1,
        window(&a),
        window(&b)
    )
}

/// Every `.ply` source the repository maintains, as against the fixtures written to be malformed
/// or to pin a golden. `examples` is not here yet: several harnesses rewrite those files by
/// matching their text, and they have to stop before the formatter may touch them.
const MAINTAINED: [&str; 6] = [
    "crates/ply-compiler/ply",
    "crates/ply-cli/ply",
    "crates/ply-std/ply",
    "crates/ply-corpus/ply",
    "benches",
    "tests/lang",
];

/// A layout only a scratch copy is ever held to is not canonical. A source committed unformatted
/// puts its reformatting into the next change that touches it, where it is indistinguishable from
/// that change.
#[test]
fn the_maintained_sources_are_committed_formatted() {
    let out = ply(&repo())
        .arg("fmt")
        .arg("--check")
        .args(MAINTAINED)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "`ply fmt` would rewrite sources that are committed as they are; run it and commit that:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn the_examples_format_to_a_fixed_point_and_still_check() {
    corpus_round_trip("examples");
}

#[test]
fn the_parser_fixtures_format_to_a_fixed_point_and_still_check() {
    corpus_round_trip("crates/ply-codegen-tests/fixtures");
}

//! `ply run` over sources files what its front end answered under a key of everything that answer
//! is a function of, and a later run whose walk hashes the same takes it and runs no front end.
//! `--explain` is how a test sees which it was; everything else a run says is the same either way.

use crate::harness::{json_of, ply, process, project, stderr_of, stdout_of, write};
use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

fn run(dir: &Path, args: &[&str]) -> Output {
    ply(dir)
        .arg("run")
        .args(args)
        .output()
        .expect("`ply run` starts")
}

/// Whether the run's load took an answer an earlier run filed, as `--explain` says.
#[track_caller]
fn reused(out: &Output) -> bool {
    let err = stderr_of(out);
    let said = err
        .lines()
        .find(|l| l.trim_start().starts_with("front end"))
        .unwrap_or_else(|| panic!("`--explain` said nothing about the load:\n{err}"));
    match (said.contains(" reused "), said.contains(" built ")) {
        (true, false) => true,
        (false, true) => false,
        _ => panic!("the load was neither reused nor built: {said}"),
    }
}

/// The value the entry answered with: the run's last line on stdout, after what the binding
/// disclosed.
fn value(out: &Output) -> String {
    stdout_of(out)
        .lines()
        .last()
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[track_caller]
fn ran(out: &Output) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout_of(out),
        stderr_of(out)
    );
}

#[test]
fn a_second_run_of_an_unchanged_package_takes_the_answer_the_first_filed() {
    let dir = project("fn main() -> Int = 42\n");
    let first = run(dir.path(), &["--explain"]);
    ran(&first);
    assert!(!reused(&first), "a first run has nothing to reuse");
    let second = run(dir.path(), &["--explain"]);
    ran(&second);
    assert!(reused(&second), "{}", stderr_of(&second));
    assert_eq!(value(&first), "42");
    assert_eq!(value(&second), "42");
}

/// Everything a run says without `--explain`, compared whole: the reused answer is placed over
/// this run's own files, so nothing it reports can tell the two apart.
#[test]
fn a_reused_load_reports_exactly_what_a_built_one_reported() {
    let dir = project("fn main() -> Int = 42\n");
    let built = run(dir.path(), &[]);
    let reused_run = run(dir.path(), &[]);
    assert!(reused(&run(dir.path(), &["--explain"])));
    assert_eq!(stdout_of(&built), stdout_of(&reused_run));
    assert_eq!(stderr_of(&built), stderr_of(&reused_run));
    assert_eq!(built.status.code(), reused_run.status.code());
    let document = json_of(&run(dir.path(), &["--json"]));
    assert_eq!(
        document["files"],
        serde_json::json!(["m.ply"]),
        "{document}"
    );
    assert_eq!(document["value"], "42", "{document}");
    assert!(document.get("front_end").is_none(), "{document}");
}

/// A raise is reported where it happened in the source a reader has open, and the reused answer's
/// spans are into this run's own files, so they read the same.
#[test]
fn a_raise_on_a_reused_load_is_placed_where_a_built_one_placed_it() {
    let dir = project("fn boom() -> Int = panic(\"boom\")\n\nfn main() -> Int = boom() + 1\n");
    let built = run(dir.path(), &[]);
    let again = run(dir.path(), &["--explain"]);
    assert!(reused(&again));
    let reused_run = run(dir.path(), &[]);
    assert_eq!(built.status.code(), Some(1), "{}", stderr_of(&built));
    assert_eq!(reused_run.status.code(), Some(1));
    let err = stderr_of(&built);
    assert!(err.contains("raised at m.ply:1:"), "{err}");
    assert_eq!(err, stderr_of(&reused_run));
    assert_eq!(stdout_of(&built), stdout_of(&reused_run));
}

/// app -> lib -> base, each a package of its own beside the others.
fn graph() -> TempDir {
    let manifest = |name: &str, deps: &str| {
        format!(
            "import std.pkg (Manifest)\nfn package() -> Manifest = {{name: \"{name}\", version: {{major: 0, minor: 0, patch: 1}}, prefix: None, runtime: {{major: 0, minor: 0, patch: 1}}, dependencies: [{deps}], entry: None}}\n"
        )
    };
    let dep = |name: &str, path: &str| {
        format!(
            "{{name: \"{name}\", prefix: None, min: {{major: 0, minor: 0, patch: 1}}, source: Path(\"{path}\")}}"
        )
    };
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    write(root, "app/ply.pkg", &manifest("app", &dep("lib", "../lib")));
    write(
        root,
        "app/main.ply",
        "import lib.answer\nfn main() -> Int = answer::answer()\n",
    );
    write(
        root,
        "lib/ply.pkg",
        &manifest("lib", &dep("base", "../base")),
    );
    write(
        root,
        "lib/answer.ply",
        "import base.deep\npub fn answer() -> Int = deep::deep() + 35\n",
    );
    write(root, "base/ply.pkg", &manifest("base", ""));
    write(root, "base/deep.ply", "pub fn deep() -> Int = 7\n");
    dir
}

fn appended(path: &Path, text: &str) {
    let mut body = std::fs::read_to_string(path).expect("the file reads");
    body.push_str(text);
    std::fs::write(path, body).expect("the file is written");
}

#[test]
fn an_edit_to_a_module_a_dependency_or_a_manifest_runs_the_front_end_again() {
    let dir = graph();
    let root = dir.path();
    let app = |expect_reused: bool, value_is: &str| {
        let out = run(root, &["--explain", "app"]);
        ran(&out);
        assert_eq!(reused(&out), expect_reused, "{}", stderr_of(&out));
        assert_eq!(value(&out), value_is);
    };
    app(false, "42");
    app(true, "42");
    write(root, "base/deep.ply", "pub fn deep() -> Int = 8\n");
    app(false, "43");
    app(true, "43");
    appended(
        &root.join("lib/answer.ply"),
        "\npub fn unused() -> Int = 0\n",
    );
    app(false, "43");
    appended(&root.join("lib/ply.pkg"), "// edited\n");
    app(false, "43");
    appended(&root.join("app/ply.pkg"), "// edited\n");
    app(false, "43");
    write(
        root,
        "app/main.ply",
        "import lib.answer\nfn main() -> Int = answer::answer() * 2\n",
    );
    app(false, "86");
    app(true, "86");
}

const SCHEMAS: &str = "\
import std.config
import std.config (config)

pub fn spec() -> config::ConfigSpec =
  config::spec([config::with_default(\"REGION\", config::SText, \"us\")])

pub fn other() -> config::ConfigSpec =
  config::spec([config::with_default(\"REGION\", config::SText, \"eu\")])

fn main() -> Option<String> / {config.get[server]} = config.get[server](\"REGION\")
";

/// `--config-schema` is resolved against the answer before the machine is handed it, so it is
/// part of the key: a schema is never taken as checked on another schema's answer.
#[test]
fn another_config_schema_takes_nothing_another_schema_filed() {
    let dir = project(SCHEMAS);
    let with = |schema: &str| {
        run(
            dir.path(),
            &["--explain", "--host", "--config-schema", schema],
        )
    };
    let first = with("m.spec");
    ran(&first);
    assert!(!reused(&first));
    let again = with("m.spec");
    ran(&again);
    assert!(reused(&again));
    assert!(value(&again).contains("us"), "{}", value(&again));
    let other = with("m.other");
    ran(&other);
    assert!(!reused(&other), "another schema's answer was reused");
    assert!(value(&other).contains("eu"), "{}", value(&other));
    let absent = with("m.absent");
    assert_eq!(absent.status.code(), Some(2), "{}", stderr_of(&absent));
    assert!(
        stderr_of(&absent).contains("E0440"),
        "{}",
        stderr_of(&absent)
    );
}

/// What the run lends and binds is this run's own, so a reused load is refused a grant exactly as
/// a load that built its answer is.
#[test]
fn a_reused_load_is_granted_and_bound_anew() {
    let dir = project("fn main() -> Int = 42\n");
    ran(&run(dir.path(), &[]));
    let hit = run(dir.path(), &["--explain", "--host", "--allow", "tester"]);
    assert!(reused(&hit));
    let fresh = project("fn main() -> Int = 42\n");
    let miss = run(fresh.path(), &["--host", "--allow", "tester"]);
    assert_eq!(miss.status.code(), Some(2), "{}", stderr_of(&miss));
    assert_eq!(hit.status.code(), miss.status.code());
    let refused = stderr_of(&miss);
    assert!(refused.contains("E0459"), "{refused}");
    assert!(
        stderr_of(&hit).ends_with(&refused),
        "the reused load was refused otherwise:\n{}\n---\n{refused}",
        stderr_of(&hit)
    );
    assert_eq!(stdout_of(&hit), stdout_of(&miss));
}

#[test]
fn a_single_file_is_keyed_and_reused_as_a_package_is() {
    let dir = project("fn main() -> Int = 42\n");
    let first = run(dir.path(), &["--explain", "m.ply"]);
    ran(&first);
    assert!(!reused(&first));
    let second = run(dir.path(), &["--explain", "m.ply"]);
    ran(&second);
    assert!(reused(&second));
    assert_eq!(value(&second), "42");
}

#[test]
fn two_runs_of_one_package_at_once_both_run_it() {
    let dir = project("fn main() -> Int = 42\n");
    let start = || {
        process(dir.path())
            .arg("run")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("`ply run` starts")
    };
    let (one, two) = (start(), start());
    for out in [one, two].map(|child| child.wait_with_output().expect("`ply run` ends")) {
        ran(&out);
        assert_eq!(value(&out), "42");
    }
    let after = run(dir.path(), &["--explain"]);
    ran(&after);
    assert!(reused(&after), "{}", stderr_of(&after));
}

/// Where the entries live, told to the run rather than assumed of it.
fn stage() -> PathBuf {
    ply_codegen::c::bundle::stage_root()
}

#[test]
fn an_entry_that_does_not_read_is_built_again_and_written_over() {
    let dir = project("fn main() -> Int = 42\n");
    let explained = || {
        json_of(
            &ply(dir.path())
                .env("PLY_C_STAGE", stage())
                .args(["run", "--json", "--explain"])
                .output()
                .expect("`ply run` starts"),
        )
    };
    let first = explained();
    assert_eq!(first["front_end"]["reused"], false, "{first}");
    let key = first["front_end"]["key"]
        .as_str()
        .expect("the key is a string");
    let entry = stage().join(ply_codegen::c::sweep::RUNS).join(key);
    let whole = std::fs::read(&entry).expect("the run filed its answer under its key");
    std::fs::write(&entry, &whole[..whole.len() / 2]).expect("the entry is torn");
    let torn = explained();
    assert_eq!(torn["front_end"]["reused"], false, "{torn}");
    assert_eq!(torn["value"], "42", "{torn}");
    let after = explained();
    assert_eq!(after["front_end"]["reused"], true, "{after}");
    assert_eq!(after["front_end"]["key"], key);
    std::fs::write(&entry, b"").expect("the entry is emptied");
    assert_eq!(explained()["front_end"]["reused"], false);
}

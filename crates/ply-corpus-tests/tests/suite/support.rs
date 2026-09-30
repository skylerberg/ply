//! Driving the corpus program the way `benches/corpus.sh` does: `ply run` over the package, with
//! the grants it runs under and the working directory as its `work` root.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn ply() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/ply")
}

/// The transitional executor, which runs the subcommands still written in Rust.
pub fn executor() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/ply-corpus")
}

/// The repository root, which is where the corpus package is reachable by path.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the crate lives two levels below the repository root")
}

/// What `ply` and the program read from the environment, and nothing else from the test's.
const INHERITED: &[&str] = &["PATH", "HOME", "TMPDIR", "TEMP", "TMP", "NEXTEST"];

/// `ply run` over the corpus package in `dir`, with the grants the program's own subcommands are
/// run with, and `args` after `--`.
pub fn corpus(dir: &Path, args: &[&str]) -> Output {
    run(dir, &[], args)
}

/// The product itself in `dir`, as the corpus drives it: `ply` with `args`, and the same few
/// variables from the environment.
pub fn product(dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(ply());
    cmd.env_clear();
    for key in INHERITED {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    cmd.args(args)
        .current_dir(dir)
        .output()
        .expect("`ply` starts")
}

/// The one JSON document a `--json` run of the product wrote, whatever it exited with.
pub fn product_document(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout was not one JSON document: {e}\n---\n{}\n---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// A report's row by name.
#[track_caller]
pub fn row<'a>(report: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    report["rows"]
        .as_array()
        .expect("rows is an array")
        .iter()
        .find(|r| r["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("the report carries no `{name}` row: {report:#}"))
}

/// A row's measurement by name, as a number.
#[track_caller]
pub fn measured(row: &serde_json::Value, name: &str) -> f64 {
    row["measurements"]
        .as_array()
        .expect("measurements is an array")
        .iter()
        .find(|m| m["name"].as_str() == Some(name))
        .and_then(|m| m["value"].as_f64())
        .unwrap_or_else(|| panic!("the row measured no `{name}`: {row:#}"))
}

/// A row's verdict: `pass`, `fail` or `inconclusive`.
pub fn outcome(row: &serde_json::Value) -> &str {
    row["verdict"]["outcome"].as_str().unwrap_or("")
}

/// The same, with the executor bound too, for a subcommand the program hands to it.
pub fn delegated(dir: &Path, args: &[&str]) -> Output {
    run(
        dir,
        &[format!("--exec=executor={}", executor().display())],
        args,
    )
}

fn run(dir: &Path, grants: &[String], args: &[&str]) -> Output {
    let mut cmd = Command::new(ply());
    cmd.env_clear();
    for key in INHERITED {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    cmd.arg("run")
        .arg(repo().join("crates/ply-corpus/ply"))
        .args(["--host", "--allow", "machine", "--allow", "claims"])
        .arg(format!("--exec=ply={}", ply().display()))
        .args(grants)
        .args(["--fs", "work=."])
        .arg(format!("--fs=repo={}", repo().display()))
        .arg("--")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("`ply run` starts")
}

/// The program's stdout after the lines `ply run` writes about the binding before the entry
/// runs, which are indented.
pub fn written(out: &Output) -> String {
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .skip_while(|line| line.starts_with("   "))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// The one JSON document a `--json` subcommand wrote.
pub fn document(out: &Output) -> serde_json::Value {
    let text = written(out);
    serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "stdout was not one JSON document: {e}\n---\n{text}\n---\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// A corpus generated at `root` and verified, as `gen` writes one; `shape` is its shape flags.
pub fn generate(root: &Path, shape: &[&str]) {
    let parent = root.parent().expect("a corpus root has a parent");
    let name = root
        .file_name()
        .expect("a corpus root has a name")
        .to_string_lossy()
        .into_owned();
    let mut args = vec!["gen", "--out", name.as_str()];
    args.extend_from_slice(shape);
    let out = corpus(parent, &args);
    assert!(
        out.status.success(),
        "`gen {}` failed:\n{}\n{}",
        shape.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

//! The third comparison: `rewrite.ply` — the effect-set, record-update and try-operator rewrites
//! `Parser::run` applies after the grammar — against `ply_syntax::parse_recovering`, tree and
//! diagnostics, over every input the parser differential reads.
//!
//! The port is entered in-process through `port`: the bundle the binary carries is the compiler
//! under test, and `PLY_C_EMITTER=ply:<dir>` enters a working copy `stage` has bootstrapped.

use crate::harness::{bundle, golden, port, records};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits at <root>/crates/ply-compiler-diff")
        .to_path_buf()
}

/// This crate's own directory, which is where the mined corpora live.
fn here() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The first record the two dumps disagree on, with context, or `None`.
fn first_difference(reference: &str, actual: &str) -> Option<String> {
    let want = records(reference);
    let got = records(actual);
    for i in 0..want.len().max(got.len()) {
        let a = want.get(i).copied();
        let b = got.get(i).copied();
        if a != b {
            let from = i.saturating_sub(6);
            let mut report = format!("record {i} of {} differs\n", want.len());
            report.push_str(&format!(
                "  rust: {a:?}\n  ply : {b:?}\n  context (rust):\n"
            ));
            for (j, r) in want.iter().enumerate().skip(from).take(14) {
                report.push_str(&format!("    {j:>7} {r}\n"));
            }
            report.push_str("  context (ply):\n");
            for (j, r) in got.iter().enumerate().skip(from).take(14) {
                report.push_str(&format!("    {j:>7} {r}\n"));
            }
            return Some(report);
        }
    }
    None
}

fn ply_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    out.sort();
    out
}

fn compare(label: &str, inputs: &[(String, Vec<u8>)]) {
    let mut failures: Vec<String> = Vec::new();
    let mut total = 0usize;
    for (name, text) in inputs {
        let actual = port::dump("rewrite.dump_expanded", text);
        total += records(&actual).len();
        if let Err(report) = golden::check("rewrite", name, &actual, first_difference) {
            failures.push(format!("{label}: {report}"));
        }
    }
    println!(
        "  {label}: {} input(s), {total} records agree",
        inputs.len() - failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {} inputs disagree\n\n{}",
        failures.len(),
        inputs.len(),
        failures.join("\n")
    );
}

fn files_in(dir: &Path) -> Vec<(String, Vec<u8>)> {
    ply_files(dir)
        .into_iter()
        .map(|p| {
            (
                p.display().to_string(),
                std::fs::read(&p).expect("readable"),
            )
        })
        .collect()
}

#[test]
fn the_rewrites_agree_with_ply_syntax_on_every_example() {
    compare("examples", &files_in(&repo_root().join("examples")));
}

#[test]
fn the_rewrites_agree_with_ply_syntax_on_the_shipped_standard_library() {
    compare("stdlib", &files_in(&repo_root().join("crates/ply-std/ply")));
}

#[test]
fn the_rewrites_agree_with_ply_syntax_on_the_hand_written_fixtures() {
    compare("fixtures", &files_in(&here().join("fixtures")));
}

#[test]
fn the_rewrites_agree_with_ply_syntax_on_the_reference_own_test_inputs() {
    let text = std::fs::read_to_string(here().join("fixtures/reference-tests.corpus"))
        .expect("the mined corpus");
    let inputs: Vec<(String, Vec<u8>)> = bundle(&text)
        .into_iter()
        .enumerate()
        .map(|(i, f)| (format!("reference-tests.corpus#{i}"), f.into_bytes()))
        .collect();
    assert!(!inputs.is_empty());
    compare("reference inputs", &inputs);
}

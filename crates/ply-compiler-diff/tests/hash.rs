//! The sixth differential: `hash.ply` against `crates/ply-hash`. Every hash the reference
//! publishes for a program — each definition's, each declaration's, each test's and law's, the
//! own-form and spec keys — and the reference graph beside them, compared record by record.
//!
//! The port is entered in-process through `port`: the bundle the binary carries is the compiler
//! under test, and `PLY_C_EMITTER=ply:<dir>` enters a working copy `stage` has bootstrapped.

use ply_compiler_diff::part;
use ply_compiler_diff::{port, programs, records, reference_hash_dump};
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

fn first_difference(reference: &str, actual: &str) -> Option<String> {
    let want = records(reference);
    let got = records(actual);
    for i in 0..want.len().max(got.len()) {
        let a = want.get(i).copied();
        let b = got.get(i).copied();
        if a != b {
            let from = i.saturating_sub(4);
            let mut report = format!(
                "record {i} of {} differs\n  rust: {a:?}\n  ply : {b:?}\n  context (rust):\n",
                want.len()
            );
            for (j, r) in want.iter().enumerate().skip(from).take(10) {
                report.push_str(&format!("    {j:>7} {r}\n"));
            }
            report.push_str("  context (ply):\n");
            for (j, r) in got.iter().enumerate().skip(from).take(10) {
                report.push_str(&format!("    {j:>7} {r}\n"));
            }
            return Some(report);
        }
    }
    None
}

fn compare(label: &str, inputs: &[(String, Vec<(String, String)>)]) {
    let mut failures: Vec<String> = Vec::new();
    let mut records_total = 0usize;
    for (name, program) in inputs {
        let actual = port::dump_program("hash.hash_dump", program);
        let reference = reference_hash_dump(program);
        records_total += records(&reference).len();
        if let Some(report) = first_difference(&reference, &actual) {
            failures.push(format!(
                "{label}: the two hashers disagree on {name}:\n{report}"
            ));
        }
    }
    println!(
        "  {label}: {} program(s), {records_total} records agree",
        inputs.len() - failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {} programs disagree\n\n{}",
        failures.len(),
        inputs.len(),
        failures.join("\n")
    );
}

fn ply_files(dir: &Path, prefix: &str) -> Vec<(String, String)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
            (
                format!("{prefix}{stem}"),
                std::fs::read_to_string(&p).expect("UTF-8"),
            )
        })
        .collect()
}

fn std_modules() -> Vec<(String, String)> {
    ply_files(&repo_root().join("crates/ply-std/ply"), "std.")
}

#[test]
fn the_ply_hasher_agrees_with_ply_hash_on_the_standard_library() {
    compare(
        "std",
        &[("the standard library".to_string(), std_modules())],
    );
}

fn the_ply_hasher_agrees_with_ply_hash_on_every_example_with_the_standard_library(
    index: usize,
    of: usize,
) {
    let std = std_modules();
    let inputs: Vec<(String, Vec<(String, String)>)> = ply_files(&repo_root().join("examples"), "")
        .into_iter()
        .map(|(name, text)| {
            let mut program = std.clone();
            program.push((name.clone(), text));
            (format!("std + examples/{name}.ply"), program)
        })
        .collect();
    compare("examples", &part(&inputs, index, of));
}

#[test]
fn the_ply_hasher_agrees_with_ply_hash_on_every_example_with_the_standard_library_part_1_of_2() {
    the_ply_hasher_agrees_with_ply_hash_on_every_example_with_the_standard_library(0, 2);
}

#[test]
fn the_ply_hasher_agrees_with_ply_hash_on_every_example_with_the_standard_library_part_2_of_2() {
    the_ply_hasher_agrees_with_ply_hash_on_every_example_with_the_standard_library(1, 2);
}

#[test]
fn the_ply_hasher_agrees_with_ply_hash_on_the_bundles() {
    let mut inputs: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for (file, label) in [
        ("fixtures/resolve-programs.corpus", "resolve-programs"),
        ("fixtures/check-programs.corpus", "check-programs"),
        ("fixtures/reference-programs.corpus", "reference-programs"),
    ] {
        let text = std::fs::read_to_string(here().join(file)).expect("a bundle");
        for (i, p) in programs(&text).into_iter().enumerate() {
            inputs.push((format!("{label}.corpus#{i}"), p));
        }
    }
    for (file, label, module) in [
        ("fixtures/reference-checks.corpus", "reference-checks", "m"),
        ("fixtures/reference-hashes.corpus", "reference-hashes", "m"),
    ] {
        let Ok(text) = std::fs::read_to_string(here().join(file)) else {
            continue;
        };
        for (i, f) in ply_compiler_diff::bundle(&text).into_iter().enumerate() {
            inputs.push((format!("{label}.corpus#{i}"), vec![(module.to_string(), f)]));
        }
    }
    assert!(!inputs.is_empty());
    compare("bundles", &inputs);
}

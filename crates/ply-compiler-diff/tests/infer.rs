//! The fourth comparison: `infer.ply`'s check of a whole program — every definition's scheme,
//! footprint and constraints, every test's and law's footprint, every effect and constructor, or
//! the diagnostics — against `ply_core::check_program`'s, over the standard library, the standard
//! library with each example, the programs the resolve comparison reads and the reference
//! checker's own inputs; and the same through the restored path, each program checked from what
//! its own first check published.
//!
//! The port is entered in-process through `port`: the bundle the binary carries is the compiler
//! under test, and `PLY_C_EMITTER=ply:<dir>` enters a working copy `stage` has bootstrapped.

use ply_compiler_diff::{
    port, programs, records, reference_check_dump, reference_check_dump_known,
};
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
            let from = i.saturating_sub(6);
            let mut report = format!(
                "record {i} of {} differs\n  rust: {a:?}\n  ply : {b:?}\n  context (rust):\n",
                want.len()
            );
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

fn std_modules() -> Vec<(String, String)> {
    let dir = repo_root().join("crates/ply-std/ply");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the standard library directory")
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
                format!("std.{stem}"),
                std::fs::read_to_string(&p).expect("UTF-8"),
            )
        })
        .collect()
}

fn examples() -> Vec<(String, String)> {
    let dir = repo_root().join("examples");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the examples directory")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
            (stem, std::fs::read_to_string(&p).expect("UTF-8"))
        })
        .collect()
}

/// Runs `inputs` through both sides and reports every disagreement.
fn compare(label: &str, inputs: &[(String, Vec<(String, String)>)]) {
    compare_through("infer.check_dump", reference_check_dump, label, inputs);
}

/// The same, through the restored path: each program checked from what its own first check
/// published.
fn compare_known(label: &str, inputs: &[(String, Vec<(String, String)>)]) {
    compare_through(
        "infer.check_dump_known",
        reference_check_dump_known,
        label,
        inputs,
    );
}

fn compare_through(
    entry: &str,
    reference_dump: fn(&[(String, String)]) -> String,
    label: &str,
    inputs: &[(String, Vec<(String, String)>)],
) {
    let mut failures: Vec<String> = Vec::new();
    let mut records_total = 0usize;
    for (name, program) in inputs {
        let actual = port::dump_program(entry, program);
        let reference = reference_dump(program);
        records_total += records(&reference).len();
        if let Some(report) = first_difference(&reference, &actual) {
            failures.push(format!(
                "{label}: the two resolvers disagree on {name}:\n{report}"
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

#[test]
fn the_ply_checker_agrees_with_ply_core_on_the_standard_library() {
    compare(
        "std",
        &[("the standard library".to_string(), std_modules())],
    );
}

#[test]
fn the_ply_checker_agrees_with_ply_core_on_every_example_with_the_standard_library() {
    let std = std_modules();
    let inputs: Vec<(String, Vec<(String, String)>)> = examples()
        .into_iter()
        .map(|(name, text)| {
            let mut program = std.clone();
            program.push((name.clone(), text));
            (format!("std + examples/{name}.ply"), program)
        })
        .collect();
    compare("examples", &inputs);
}

#[test]
fn the_ply_checker_agrees_with_ply_core_on_the_resolvers_reference_programs() {
    let text = std::fs::read_to_string(here().join("fixtures/reference-programs.corpus"))
        .expect("the mined programs; run mine-programs.py");
    let inputs: Vec<(String, Vec<(String, String)>)> = programs(&text)
        .into_iter()
        .enumerate()
        .map(|(i, p)| (format!("reference-programs.corpus#{i}"), p))
        .collect();
    assert!(!inputs.is_empty(), "the mined bundle holds no program");
    compare("reference programs", &inputs);
}

#[test]
fn the_ply_checker_agrees_with_ply_core_on_the_resolvers_hand_written_programs() {
    let text = std::fs::read_to_string(here().join("fixtures/resolve-programs.corpus"))
        .expect("the hand-written programs");
    let inputs: Vec<(String, Vec<(String, String)>)> = programs(&text)
        .into_iter()
        .enumerate()
        .map(|(i, p)| (format!("resolve-programs.corpus#{i}"), p))
        .collect();
    assert!(
        !inputs.is_empty(),
        "the hand-written bundle holds no program"
    );
    compare("hand-written programs", &inputs);
}

/// The restored path: the reference and the port each check a program, hand what it published
/// back in as `Known`, check again from those interfaces, and must publish the same thing.
#[test]
fn the_ply_checker_restored_from_its_own_interfaces_agrees_with_ply_core_on_the_standard_library() {
    compare_known(
        "std, restored",
        &[("the standard library".to_string(), std_modules())],
    );
}

#[test]
fn the_ply_checker_restored_from_its_own_interfaces_agrees_with_ply_core_on_the_bundles() {
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
    let text = std::fs::read_to_string(here().join("fixtures/reference-checks.corpus"))
        .expect("the mined checker inputs; run mine-checks.py");
    for (i, f) in ply_compiler_diff::bundle(&text).into_iter().enumerate() {
        inputs.push((
            format!("reference-checks.corpus#{i}"),
            vec![("m".to_string(), f)],
        ));
    }
    assert!(!inputs.is_empty());
    compare_known("bundles, restored", &inputs);
}

#[test]
fn the_ply_checker_agrees_with_ply_core_on_the_checkers_hand_written_programs() {
    let text = std::fs::read_to_string(here().join("fixtures/check-programs.corpus"))
        .expect("the hand-written checker programs");
    let inputs: Vec<(String, Vec<(String, String)>)> = programs(&text)
        .into_iter()
        .enumerate()
        .map(|(i, p)| (format!("check-programs.corpus#{i}"), p))
        .collect();
    assert!(
        !inputs.is_empty(),
        "the hand-written checker bundle holds no program"
    );
    compare("hand-written checker programs", &inputs);
}

/// The reference checker's own test inputs, each as a one-module program named `m`, the way
/// `crates/ply-core-tests/tests/suite/unit/infer.rs` checks them. Most are error paths: this is where the port's
/// diagnostics are compared, code by code and label by label.
#[test]
fn the_ply_checker_agrees_with_ply_core_on_the_references_own_inputs() {
    let text = std::fs::read_to_string(here().join("fixtures/reference-checks.corpus"))
        .expect("the mined checker inputs; run mine-checks.py");
    let inputs: Vec<(String, Vec<(String, String)>)> = ply_compiler_diff::bundle(&text)
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            (
                format!("reference-checks.corpus#{i}"),
                vec![("m".to_string(), f)],
            )
        })
        .collect();
    assert!(!inputs.is_empty(), "the mined bundle holds no input");
    compare("reference checks", &inputs);
}

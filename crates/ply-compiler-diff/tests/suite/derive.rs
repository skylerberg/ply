//! The fifth differential: `derive.ply` against `crates/ply-derive`. Every module is expanded on
//! its own, and the source each derivation generates is compared byte for byte, with the
//! diagnostics expansion raises.
//!
//! The port is entered in-process through `port`: the bundle the binary carries is the compiler
//! under test, and `PLY_C_EMITTER=ply:<dir>` enters a working copy `stage` has bootstrapped.

use ply_compiler_diff::{golden, port, reference_derive_dump};
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
    if reference == actual {
        return None;
    }
    let at = reference
        .bytes()
        .zip(actual.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or(reference.len().min(actual.len()));
    let from = at.saturating_sub(80);
    let window = |s: &str| {
        let end = (at + 120).min(s.len());
        s.get(from..end).unwrap_or("").to_string()
    };
    Some(format!(
        "differs at byte {at} of {} (rust) / {} (ply)\n  rust: ...{}...\n  ply : ...{}...",
        reference.len(),
        actual.len(),
        window(reference),
        window(actual)
    ))
}

fn compare(label: &str, inputs: &[(String, Vec<(String, String)>)]) {
    let mut failures: Vec<String> = Vec::new();
    let mut generated = 0usize;
    for (name, program) in inputs {
        let actual = port::dump_program("derive.derive_dump", program);
        let reference = reference_derive_dump(program);
        generated += reference.matches("S;").count();
        if let Err(report) = golden::check("derive", name, &reference, &actual, first_difference) {
            failures.push(format!("{label}: {report}"));
        }
    }
    println!(
        "  {label}: {} module(s), {generated} generated definition(s) agree",
        inputs.len() - failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {} modules disagree\n\n{}",
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

#[test]
fn the_ply_deriver_agrees_with_ply_derive_on_the_hand_written_modules() {
    let text = std::fs::read_to_string(here().join("fixtures/derive-programs.corpus"))
        .expect("the hand-written derive modules");
    let inputs: Vec<(String, Vec<(String, String)>)> = ply_compiler_diff::bundle(&text)
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            (
                format!("derive-programs.corpus#{i}"),
                vec![("t".to_string(), f)],
            )
        })
        .collect();
    assert!(
        !inputs.is_empty(),
        "the hand-written bundle holds no module"
    );
    compare("hand-written modules", &inputs);
}

#[test]
fn the_ply_deriver_agrees_with_ply_derive_on_every_example_and_the_standard_library() {
    let mut inputs: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for (name, text) in ply_files(&repo_root().join("examples"), "examples/") {
        inputs.push((name.clone(), vec![(name, text)]));
    }
    for (name, text) in ply_files(&repo_root().join("crates/ply-std/ply"), "std.") {
        inputs.push((name.clone(), vec![(name, text)]));
    }
    compare("examples and std", &inputs);
}

use crate::harness::{bundle, fixtures, golden, own, part, port, programs, records, repo_root};
use std::path::{Path, PathBuf};

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

fn compare(
    label: &str,
    inputs: &[(String, Vec<(String, String)>)],
    view: fn(String, &[(String, String)]) -> String,
) {
    let mut failures: Vec<String> = Vec::new();
    let mut records_total = 0usize;
    for (name, program) in inputs {
        let actual = view(port::dump_program("hash.hash_dump", program), program);
        records_total += records(&actual).len();
        if let Err(report) = golden::check("hash", name, &actual, first_difference) {
            failures.push(format!("{label}: {report}"));
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
fn the_ply_hasher_matches_its_golden_on_the_standard_library() {
    compare(
        "std",
        &[("the standard library".to_string(), std_modules())],
        |d, _| d,
    );
}

fn the_ply_hasher_matches_its_golden_on_every_example_with_the_standard_library(
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
    compare("examples", &part(&inputs, index, of), |d, p| {
        own::keyed(
            &d,
            p,
            &["H", "Y", "T", "L", "W", "O", "S", "X", "D", "C"],
            &["T", "L", "W"],
        )
    });
}

#[test]
fn the_ply_hasher_matches_its_golden_on_every_example_with_the_standard_library_part_1_of_2() {
    ply_codegen::c::producer::reset_census();
    the_ply_hasher_matches_its_golden_on_every_example_with_the_standard_library(0, 2);
    // The standard library is counted once per program, as it is hashed once per program.
    let lines: usize = std_modules().iter().map(|(_, t)| t.lines().count()).sum();
    if let Err(report) = crate::harness::census::hold("hasher-over-std-and-examples-part-1", lines)
    {
        panic!("{report}");
    }
}

#[test]
fn the_ply_hasher_matches_its_golden_on_every_example_with_the_standard_library_part_2_of_2() {
    the_ply_hasher_matches_its_golden_on_every_example_with_the_standard_library(1, 2);
}

#[test]
fn the_ply_hasher_matches_its_golden_on_the_bundles() {
    let mut inputs: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for (file, label) in [
        ("resolve-programs.corpus", "resolve-programs"),
        ("check-programs.corpus", "check-programs"),
        ("reference-programs.corpus", "reference-programs"),
    ] {
        let text = std::fs::read_to_string(fixtures().join(file)).expect("a bundle");
        for (i, p) in programs(&text).into_iter().enumerate() {
            inputs.push((format!("{label}.corpus#{i}"), p));
        }
    }
    for (file, label, module) in [
        ("reference-checks.corpus", "reference-checks", "m"),
        ("reference-hashes.corpus", "reference-hashes", "m"),
    ] {
        let Ok(text) = std::fs::read_to_string(fixtures().join(file)) else {
            continue;
        };
        for (i, f) in bundle(&text).into_iter().enumerate() {
            inputs.push((format!("{label}.corpus#{i}"), vec![(module.to_string(), f)]));
        }
    }
    assert!(!inputs.is_empty());
    compare("bundles", &inputs, |d, _| d);
}

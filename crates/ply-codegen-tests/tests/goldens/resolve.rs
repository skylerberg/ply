use crate::harness::{fixtures, golden, own, part, port, programs, records, repo_root};
use std::path::PathBuf;

fn first_difference(reference: &str, actual: &str) -> Option<String> {
    let want = records(reference);
    let got = records(actual);
    for i in 0..want.len().max(got.len()) {
        let a = want.get(i).copied();
        let b = got.get(i).copied();
        if a != b {
            let from = i.saturating_sub(6);
            let mut report = format!(
                "record {i} of {} differs\n  golden: {a:?}\n  port  : {b:?}\n  context (golden):\n",
                want.len()
            );
            for (j, r) in want.iter().enumerate().skip(from).take(14) {
                report.push_str(&format!("    {j:>7} {r}\n"));
            }
            report.push_str("  context (port):\n");
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

fn compare(label: &str, inputs: &[(String, Vec<(String, String)>)], view: fn(String) -> String) {
    let mut failures: Vec<String> = Vec::new();
    let mut records_total = 0usize;
    for (name, program) in inputs {
        let actual = view(port::dump_program("resolve.resolve_dump", program));
        records_total += records(&actual).len();
        if let Err(report) = golden::check("resolve", name, &actual, first_difference) {
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

#[test]
fn the_ply_resolver_matches_its_golden_on_the_standard_library() {
    compare(
        "std",
        &[("the standard library".to_string(), std_modules())],
        |d| d,
    );
}

fn the_ply_resolver_matches_its_golden_on_every_example_with_the_standard_library(
    index: usize,
    of: usize,
) {
    let std = std_modules();
    let inputs: Vec<(String, Vec<(String, String)>)> = examples()
        .into_iter()
        .map(|(name, text)| {
            let mut program = std.clone();
            program.push((name.clone(), text));
            (format!("std + examples/{name}.ply"), program)
        })
        .collect();
    compare("examples", &part(&inputs, index, of), |d| own::resolved(&d));
}

#[test]
fn the_ply_resolver_matches_its_golden_on_every_example_with_the_standard_library_part_1_of_2() {
    the_ply_resolver_matches_its_golden_on_every_example_with_the_standard_library(0, 2);
}

#[test]
fn the_ply_resolver_matches_its_golden_on_every_example_with_the_standard_library_part_2_of_2() {
    the_ply_resolver_matches_its_golden_on_every_example_with_the_standard_library(1, 2);
}

#[test]
fn the_ply_resolver_matches_its_golden_on_the_references_own_programs() {
    let text = std::fs::read_to_string(fixtures().join("reference-programs.corpus"))
        .expect("the mined programs");
    let inputs: Vec<(String, Vec<(String, String)>)> = programs(&text)
        .into_iter()
        .enumerate()
        .map(|(i, p)| (format!("reference-programs.corpus#{i}"), p))
        .collect();
    assert!(!inputs.is_empty(), "the mined bundle holds no program");
    compare("reference programs", &inputs, |d| d);
}

#[test]
fn the_ply_resolver_matches_its_golden_on_the_hand_written_programs() {
    let text = std::fs::read_to_string(fixtures().join("resolve-programs.corpus"))
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
    compare("hand-written programs", &inputs, |d| d);
}

use crate::harness::{bundle, fixtures, golden, port, records, repo_root};
use std::path::{Path, PathBuf};

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
                "  golden: {a:?}\n  port  : {b:?}\n  context (golden):\n"
            ));
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
                p.strip_prefix(repo_root())
                    .expect("an input under the repository")
                    .display()
                    .to_string(),
                std::fs::read(&p).expect("readable"),
            )
        })
        .collect()
}

#[test]
fn the_rewrites_match_their_golden_on_every_example() {
    compare("examples", &files_in(&repo_root().join("examples")));
}

#[test]
fn the_rewrites_match_their_golden_on_the_shipped_standard_library() {
    compare("stdlib", &files_in(&repo_root().join("crates/ply-std/ply")));
}

#[test]
fn the_rewrites_match_their_golden_on_the_hand_written_fixtures() {
    compare("fixtures", &files_in(&fixtures()));
}

#[test]
fn the_rewrites_match_their_golden_on_the_reference_own_test_inputs() {
    let text = std::fs::read_to_string(fixtures().join("reference-tests.corpus"))
        .expect("the mined corpus");
    let inputs: Vec<(String, Vec<u8>)> = bundle(&text)
        .into_iter()
        .enumerate()
        .map(|(i, f)| (format!("reference-tests.corpus#{i}"), f.into_bytes()))
        .collect();
    assert!(!inputs.is_empty());
    compare("reference inputs", &inputs);
}

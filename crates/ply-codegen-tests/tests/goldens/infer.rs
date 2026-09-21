use crate::harness::{bundle, fixtures, golden, own, part, port, programs, records, repo_root};
use std::path::{Path, PathBuf};

fn first_difference(reference: &str, actual: &str) -> Option<String> {
    difference("golden", reference, "port", actual)
}

/// The first record two dumps disagree on, with the run-up to it from each side.
fn difference(left: &str, left_text: &str, right: &str, right_text: &str) -> Option<String> {
    let want = records(left_text);
    let got = records(right_text);
    for i in 0..want.len().max(got.len()) {
        let a = want.get(i).copied();
        let b = got.get(i).copied();
        if a != b {
            let from = i.saturating_sub(6);
            let mut report = format!(
                "record {i} of {} differs\n  {left}: {a:?}\n  {right}: {b:?}\n  context ({left}):\n",
                want.len()
            );
            for (j, r) in want.iter().enumerate().skip(from).take(14) {
                report.push_str(&format!("    {j:>7} {r}\n"));
            }
            report.push_str(&format!("  context ({right}):\n"));
            for (j, r) in got.iter().enumerate().skip(from).take(14) {
                report.push_str(&format!("    {j:>7} {r}\n"));
            }
            return Some(report);
        }
    }
    None
}

/// Every `.ply` file in `dir`, in byte order, each named `<prefix><stem>`.
fn ply_modules(dir: &Path, prefix: &str) -> Vec<(String, String)> {
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
    ply_modules(&repo_root().join("crates/ply-std/ply"), "std.")
}

/// The compiler's own modules, named as they name each other.
fn compiler_modules() -> Vec<(String, String)> {
    ply_modules(&repo_root().join("crates/ply-compiler/ply"), "")
}

fn examples() -> Vec<(String, String)> {
    ply_modules(&repo_root().join("examples"), "")
}

fn compare(label: &str, inputs: &[(String, Vec<(String, String)>)]) {
    compare_through("infer.check_dump", label, inputs, |d, _| d);
}

/// Each program checked from what its own first check published.
fn compare_known(label: &str, inputs: &[(String, Vec<(String, String)>)]) {
    compare_through("infer.check_dump_known", label, inputs, |d, _| d);
}

fn compare_through(
    entry: &str,
    label: &str,
    inputs: &[(String, Vec<(String, String)>)],
    view: fn(String, &[(String, String)]) -> String,
) {
    let mut failures: Vec<String> = Vec::new();
    let mut records_total = 0usize;
    for (name, program) in inputs {
        let actual = view(port::dump_program(entry, program), program);
        records_total += records(&actual).len();
        if let Err(report) = golden::check(entry, name, &actual, first_difference) {
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
fn the_ply_checker_matches_its_golden_on_the_standard_library() {
    compare(
        "std",
        &[("the standard library".to_string(), std_modules())],
    );
}

fn the_ply_checker_matches_its_golden_on_every_example_with_the_standard_library(
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
    compare_through(
        "infer.check_dump",
        "examples",
        &part(&inputs, index, of),
        |d, p| own::keyed(&d, p, &["F", "T", "L", "E", "C"], &[]),
    );
}

#[test]
fn the_ply_checker_matches_its_golden_on_every_example_with_the_standard_library_part_1_of_3() {
    the_ply_checker_matches_its_golden_on_every_example_with_the_standard_library(0, 3);
}

#[test]
fn the_ply_checker_matches_its_golden_on_every_example_with_the_standard_library_part_2_of_3() {
    the_ply_checker_matches_its_golden_on_every_example_with_the_standard_library(1, 3);
}

#[test]
fn the_ply_checker_matches_its_golden_on_every_example_with_the_standard_library_part_3_of_3() {
    the_ply_checker_matches_its_golden_on_every_example_with_the_standard_library(2, 3);
}

#[test]
fn the_ply_checker_matches_its_golden_on_the_resolvers_reference_programs() {
    let text = std::fs::read_to_string(fixtures().join("reference-programs.corpus"))
        .expect("the mined programs");
    let inputs: Vec<(String, Vec<(String, String)>)> = programs(&text)
        .into_iter()
        .enumerate()
        .map(|(i, p)| (format!("reference-programs.corpus#{i}"), p))
        .collect();
    assert!(!inputs.is_empty(), "the mined bundle holds no program");
    compare("reference programs", &inputs);
}

#[test]
fn the_ply_checker_matches_its_golden_on_the_resolvers_hand_written_programs() {
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
    compare("hand-written programs", &inputs);
}

/// The seam a definition-granular cache is built on, on the largest program there is: every
/// definition is republished from the interface the first check stored for it, and the answer has
/// to be the one walking the body gave. Compared against the walked dump rather than a golden,
/// since a golden is blessed rather than failed and would record a drift instead of refusing it.
#[test]
fn the_ply_checker_seeded_from_its_own_interfaces_agrees_on_the_compiler_and_the_library() {
    let mut program = std_modules();
    program.extend(compiler_modules());
    let head = format!("K;{};", program.len());

    let walked = port::dump_program("infer.check_dump", &program);
    assert!(
        walked.starts_with(&head) && !walked[head.len()..].starts_with("X;"),
        "the standard library and the compiler do not check as one program:\n{}",
        walked.chars().take(4000).collect::<String>()
    );

    let seeded = port::dump_program("infer.check_dump_known", &program);
    if walked != seeded {
        let report = difference("walked", &walked, "seeded", &seeded).unwrap_or_else(|| {
            format!(
                "every record agrees, so the two differ only in trailing bytes: {} against {}",
                walked.len(),
                seeded.len()
            )
        });
        panic!(
            "over {} modules, a definition published from its stored interface does not answer as \
             the walked body did\n{report}",
            program.len()
        );
    }
}

#[test]
fn the_ply_checker_restored_from_its_own_interfaces_matches_its_golden_on_the_standard_library() {
    compare_known(
        "std, restored",
        &[("the standard library".to_string(), std_modules())],
    );
}

#[test]
fn the_ply_checker_restored_from_its_own_interfaces_matches_its_golden_on_the_bundles() {
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
    let text = std::fs::read_to_string(fixtures().join("reference-checks.corpus"))
        .expect("the reference checker inputs");
    for (i, f) in bundle(&text).into_iter().enumerate() {
        inputs.push((
            format!("reference-checks.corpus#{i}"),
            vec![("m".to_string(), f)],
        ));
    }
    assert!(!inputs.is_empty());
    compare_known("bundles, restored", &inputs);
}

#[test]
fn the_ply_checker_matches_its_golden_on_the_checkers_hand_written_programs() {
    let text = std::fs::read_to_string(fixtures().join("check-programs.corpus"))
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

#[test]
fn the_ply_checker_matches_its_golden_on_the_references_own_inputs() {
    let text = std::fs::read_to_string(fixtures().join("reference-checks.corpus"))
        .expect("the reference checker inputs");
    let inputs: Vec<(String, Vec<(String, String)>)> = bundle(&text)
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

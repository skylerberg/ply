//! The differential ADR 0052 §1 opens with: every diagnostic the port's front end raises over a
//! program, message and all, as `diag.diag_dump` frames it, against the reference chain the CLI
//! driver runs -- parse and derive expansion, the resolver, the hasher, the checker, stopping where
//! it stops -- framed the same way by `reference_diag_dump`. Compared as text, byte for byte, over
//! every corpus the other differentials read.
//!
//! No goldens: the dump is what `ply check` will read once the port's diagnostics are its own, and
//! holding it to the reference is the whole of what this comparison says.
//!
//! The port is entered in-process through `port`: the bundle the binary carries is the compiler
//! under test, and `PLY_C_EMITTER=ply:<dir>` enters a working copy `stage` has bootstrapped.

use ply_compiler_diff::{bundle, part, port, programs, reference_diag_dump};
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

/// The first line the two dumps disagree on, and then both texts whole, since a diagnostics dump
/// is short enough to read and the message is what a frame carries.
fn first_difference(reference: &str, actual: &str) -> Option<String> {
    if reference == actual {
        return None;
    }
    let want: Vec<&str> = reference.lines().collect();
    let got: Vec<&str> = actual.lines().collect();
    let at = (0..want.len().max(got.len()))
        .find(|&i| want.get(i) != got.get(i))
        .unwrap_or(0);
    let shown = |text: &str| -> String {
        const LINES: usize = 60;
        let n = text.lines().count();
        let mut out: String = text.lines().take(LINES).collect::<Vec<_>>().join("\n");
        if n > LINES {
            out.push_str(&format!("\n... {} more line(s)", n - LINES));
        }
        out
    };
    Some(format!(
        "line {at} differs\n  rust: {:?}\n  ply : {:?}\n--- reference\n{}\n--- port\n{}\n",
        want.get(at),
        got.get(at),
        shown(reference),
        shown(actual)
    ))
}

/// Runs `inputs` through both sides and names every program the two disagree on.
fn compare(label: &str, inputs: &[(String, Vec<(String, String)>)]) {
    let mut failures: Vec<String> = Vec::new();
    let mut frames = 0usize;
    for (name, program) in inputs {
        let actual = port::dump_program("diag.diag_dump", program);
        let reference = reference_diag_dump(program);
        frames += reference.lines().filter(|l| l.starts_with("diag ")).count();
        if let Some(report) = first_difference(&reference, &actual) {
            failures.push(format!(
                "{label}: the port's diagnostics differ from the reference's on {name}:\n{report}"
            ));
        }
    }
    println!(
        "  {label}: {} program(s), {frames} diagnostic(s) agree",
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
    assert!(!files.is_empty(), "{} holds no .ply files", dir.display());
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

/// Each file under `dir` as its own program on top of the standard library, named `m` as the
/// driver names a lone file.
fn each_with_std(dir: &str) -> Vec<(String, Vec<(String, String)>)> {
    let std = std_modules();
    ply_files(&repo_root().join(dir), "")
        .into_iter()
        .map(|(name, text)| {
            let mut program = std.clone();
            program.push(("m".to_string(), text));
            (format!("std + {dir}/{name}.ply"), program)
        })
        .collect()
}

#[test]
fn the_ports_diagnostics_agree_with_the_references_on_the_standard_library() {
    compare(
        "std",
        &[("the standard library".to_string(), std_modules())],
    );
}

fn the_ports_diagnostics_agree_with_the_references_on_every_example_with_the_standard_library(
    index: usize,
    of: usize,
) {
    compare("examples", &part(&each_with_std("examples"), index, of));
}

#[test]
fn the_ports_diagnostics_agree_with_the_references_on_every_example_with_the_standard_library_part_1_of_2()
 {
    the_ports_diagnostics_agree_with_the_references_on_every_example_with_the_standard_library(
        0, 2,
    );
}

#[test]
fn the_ports_diagnostics_agree_with_the_references_on_every_example_with_the_standard_library_part_2_of_2()
 {
    the_ports_diagnostics_agree_with_the_references_on_every_example_with_the_standard_library(
        1, 2,
    );
}

/// The compiler's own sources, on top of the standard library: the program the bootstrap compiles.
#[test]
fn the_ports_diagnostics_agree_with_the_references_on_the_compilers_own_sources() {
    let mut program = std_modules();
    program.extend(ply_files(&repo_root().join("crates/ply-compiler/ply"), ""));
    compare(
        "the compiler",
        &[("std + crates/ply-compiler/ply".to_string(), program)],
    );
}

/// The language fixtures the CLI suite runs, each a lone module the driver would name `m`.
#[test]
fn the_ports_diagnostics_agree_with_the_references_on_the_language_fixtures() {
    compare("language fixtures", &each_with_std("tests/fixtures/lang"));
}

/// The parser's hand-written fixtures, each alone: most exist to raise, so this is where the
/// parser's diagnostics are compared message by message.
#[test]
fn the_ports_diagnostics_agree_with_the_references_on_the_parsers_fixtures() {
    let inputs: Vec<(String, Vec<(String, String)>)> = ply_files(&here().join("fixtures"), "")
        .into_iter()
        .map(|(name, text)| {
            (
                format!("fixtures/{name}.ply"),
                vec![("m".to_string(), text)],
            )
        })
        .collect();
    compare("parser fixtures", &inputs);
}

/// The inputs mined from the reference's own parser, checker and hasher tests, each a one-module
/// program named `m`, the way the other differentials read them.
#[test]
fn the_ports_diagnostics_agree_with_the_references_on_the_mined_single_module_corpora() {
    let mut inputs: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for (file, label) in [
        ("fixtures/reference-tests.corpus", "reference-tests"),
        ("fixtures/reference-checks.corpus", "reference-checks"),
        ("fixtures/reference-hashes.corpus", "reference-hashes"),
    ] {
        let text = std::fs::read_to_string(here().join(file))
            .unwrap_or_else(|e| panic!("{file}: {e}; the mined corpora are regenerated by tools/"));
        for (i, f) in bundle(&text).into_iter().enumerate() {
            inputs.push((format!("{label}.corpus#{i}"), vec![("m".to_string(), f)]));
        }
    }
    assert!(
        inputs.len() > 700,
        "the mined corpora hold {} inputs",
        inputs.len()
    );
    compare("mined single modules", &inputs);
}

/// The whole-program bundles: the resolver's, the checker's, the deriver's hand-written programs
/// and the programs the reference's own tests build.
#[test]
fn the_ports_diagnostics_agree_with_the_references_on_the_program_bundles() {
    let mut inputs: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for (file, label) in [
        ("fixtures/resolve-programs.corpus", "resolve-programs"),
        ("fixtures/check-programs.corpus", "check-programs"),
        ("fixtures/derive-programs.corpus", "derive-programs"),
        ("fixtures/reference-programs.corpus", "reference-programs"),
    ] {
        let text =
            std::fs::read_to_string(here().join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        for (i, p) in programs(&text).into_iter().enumerate() {
            inputs.push((format!("{label}.corpus#{i}"), p));
        }
    }
    assert!(!inputs.is_empty(), "the bundles hold no program");
    compare("program bundles", &inputs);
}

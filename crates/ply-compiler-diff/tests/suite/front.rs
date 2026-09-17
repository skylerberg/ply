//! The third differential ADR 0052 §1 names: the port's whole front-end answer over a program --
//! the diagnostics, the load order, the checker's output, the hashes and bodies, the item
//! ordinals -- as `front.front_dump` frames it, against the reference chain the CLI driver runs,
//! framed by `reference_front_dump` through `ply_ty::write_front`. Compared as text, byte for
//! byte, over every corpus `diag.rs` reads.
//!
//! The writer is the protocol, so the reference's own text is also held to read back into the
//! structs it was written from: a dump that does not round-trip is one the driver could not consume.

use crate::diag::{each_with_std, first_difference, here, ply_files, repo_root, std_modules};
use ply_compiler_diff::{bundle, part, port, programs, reference_front_dump};
use ply_span::SourceId;

/// Runs `inputs` through both sides and names every program the two disagree on.
fn compare(label: &str, inputs: &[(String, Vec<(String, String)>)]) {
    let mut failures: Vec<String> = Vec::new();
    let mut frames = 0usize;
    for (name, program) in inputs {
        let reference = reference_front_dump(program);
        let ids: Vec<SourceId> = (0..program.len()).map(|i| SourceId(i as u32)).collect();
        let back = ply_ty::read_front(&reference, &ids).unwrap_or_else(|e| {
            panic!("{label}: the reference's dump of {name} does not read back: {e}")
        });
        let again = ply_ty::write_front(&back, &ids).unwrap();
        if let Some(report) = first_difference(&reference, &again) {
            panic!("{label}: the reference's dump of {name} does not round-trip:\n{report}");
        }
        frames += reference
            .lines()
            .filter(|l| {
                l.split(' ')
                    .next()
                    .is_some_and(|k| matches!(k, "diag" | "def" | "hash" | "test" | "law"))
            })
            .count();
        let actual = port::dump_program("front.front_dump", program);
        if let Some(report) = first_difference(&reference, &actual) {
            failures.push(format!(
                "{label}: the port's front end differs from the reference's on {name}:\n{report}"
            ));
        }
    }
    println!(
        "  {label}: {} program(s), {frames} frame(s) agree",
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
fn the_ports_front_end_agrees_with_the_references_on_the_standard_library() {
    compare(
        "std",
        &[("the standard library".to_string(), std_modules())],
    );
}

fn the_ports_front_end_agrees_with_the_references_on_every_example_with_the_standard_library(
    index: usize,
    of: usize,
) {
    compare("examples", &part(&each_with_std("examples"), index, of));
}

#[test]
fn the_ports_front_end_agrees_with_the_references_on_every_example_with_the_standard_library_part_1_of_2()
 {
    the_ports_front_end_agrees_with_the_references_on_every_example_with_the_standard_library(0, 2);
}

#[test]
fn the_ports_front_end_agrees_with_the_references_on_every_example_with_the_standard_library_part_2_of_2()
 {
    the_ports_front_end_agrees_with_the_references_on_every_example_with_the_standard_library(1, 2);
}

/// The compiler's own sources, on top of the standard library: the program the bootstrap compiles.
#[test]
fn the_ports_front_end_agrees_with_the_references_on_the_compilers_own_sources() {
    let mut program = std_modules();
    program.extend(ply_files(&repo_root().join("crates/ply-compiler/ply"), ""));
    compare(
        "the compiler",
        &[("std + crates/ply-compiler/ply".to_string(), program)],
    );
}

/// The language fixtures the CLI suite runs, each a lone module the driver would name `m`.
#[test]
fn the_ports_front_end_agrees_with_the_references_on_the_language_fixtures() {
    compare("language fixtures", &each_with_std("tests/fixtures/lang"));
}

/// The parser's hand-written fixtures, each alone: most exist to raise, so this is where the
/// dump's first frame is the last one.
#[test]
fn the_ports_front_end_agrees_with_the_references_on_the_parsers_fixtures() {
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
fn the_ports_front_end_agrees_with_the_references_on_the_mined_single_module_corpora() {
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
fn the_ports_front_end_agrees_with_the_references_on_the_program_bundles() {
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

//! The corpus package is the program: `ply run` over it answers the line, refuses one it cannot
//! read with exit 2, and hands a subcommand still written in Rust to the executor, whose output and
//! exit code come back as the program's own.

use crate::support::{corpus, delegated, document};

#[test]
fn a_line_the_program_cannot_read_is_refused_with_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    let out = corpus(dir.path(), &["nonsense"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("unrecognized subcommand"), "{stderr}");
}

#[test]
fn a_subcommand_the_executor_runs_comes_back_through_the_program() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("one.ply"), "fn one() -> Int = 1\n").unwrap();
    // Every table dropped, so what comes back is the executor's answer and nothing it measured.
    let out = delegated(
        dir.path(),
        &[
            "sim",
            "one.ply",
            "--trials",
            "0",
            "--rate-seeds",
            "0",
            "--no-reduction",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "the delegated run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let measured = document(&out);
    assert_eq!(measured["root"].as_str(), Some("one.ply"), "{measured:#}");
}

#[test]
fn an_executor_that_fails_fails_the_program_with_its_code_and_its_words() {
    let dir = tempfile::tempdir().unwrap();
    let out = delegated(dir.path(), &["prove", "missing.ply"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("ply-corpus: `missing.ply` did not compile"),
        "the executor's own error is what the program says: {stderr}"
    );
}

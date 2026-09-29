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
    let out = delegated(dir.path(), &["regions", "--hypothetical", "12:3", "--json"]);
    assert!(
        out.status.success(),
        "the delegated run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows = document(&out);
    assert_eq!(
        rows.as_array().map(Vec::len),
        Some(1),
        "one hypothetical asked for is one row: {rows:#}"
    );
}

#[test]
fn an_executor_that_fails_fails_the_program_with_its_code_and_its_words() {
    let dir = tempfile::tempdir().unwrap();
    let out = delegated(dir.path(), &["w6", "missing.json"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("ply-corpus: reading `missing.json`"),
        "the executor's own error is what the program says: {stderr}"
    );
}

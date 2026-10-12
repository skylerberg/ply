//! A deployed program's runnable: written from a front end's answer and read back with no compiler.

use crate::front::{empty_dump, files};
use ply_machine::runnable;

#[test]
fn a_runnable_reads_back_as_the_program_it_was_written_from() {
    let bytes = runnable::encode(
        "m.main",
        &files(&[("m.ply", "m", "fn main() -> Int = 1\n")]),
        &empty_dump(),
    )
    .expect("the runnable encodes");
    let program = runnable::decode(&bytes).expect("the runnable reads back");
    assert_eq!(program.entry, "m.main");
    let names: Vec<(&str, &str)> = program
        .front
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.name.as_str()))
        .collect();
    assert_eq!(names, [("m.ply", "m")]);
    assert_eq!(program.front.files[0].text, "fn main() -> Int = 1\n");
}

#[test]
fn bytes_that_are_no_runnable_are_refused() {
    assert!(runnable::decode(b"not compressed").is_err());
    let bytes = runnable::encode("m.main", &files(&[]), &empty_dump()).expect("it encodes");
    assert!(runnable::decode(&bytes[..bytes.len() / 2]).is_err());
}

//! A deployed program's runnable: written from a front end's answer and read back with no compiler,
//! and what a program is told of its own definitions.

use crate::front::{empty_dump, files};
use ply_eval::{Analysis, DefHash, Symbol};
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

/// What `shipped.definitions` tells a program of itself: each `fn` its entry reaches, in name order,
/// under the hash its front end gave it.
#[test]
fn a_programs_definitions_are_each_fn_its_entry_reaches_under_its_hash() {
    let mut program = Analysis::default();
    for (name, byte, deps) in [
        ("m.leaf", 1u8, &[][..]),
        ("m.other", 2, &[]),
        ("m.main", 3, &["m.leaf"]),
    ] {
        program
            .hashes
            .defs
            .insert(Symbol::new(name), DefHash([byte; 32]));
        program
            .hashes
            .deps
            .insert(Symbol::new(name), deps.iter().map(Symbol::new).collect());
    }
    let listed = ply_machine::shipped::definitions(&program, Some("m.main"));
    assert_eq!(
        listed,
        format!(
            "m.leaf {}\nm.main {}\n",
            DefHash([1; 32]).to_hex(),
            DefHash([3; 32]).to_hex()
        )
    );
    let every = ply_machine::shipped::definitions(&program, None);
    assert_eq!(every.lines().count(), 3, "with no entry, every definition");
}

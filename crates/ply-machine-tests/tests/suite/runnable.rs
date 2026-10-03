//! A program as the launcher enters it: written from a front end's answer and the C of its unit,
//! read back with no compiler, and entered.

use crate::fixture::{handed, loaded, project, unit_text};
use ply_eval::{Span, Value};
use ply_machine::artifact::{self, Binds};
use ply_machine::payload::field_of;
use ply_machine::runnable;

const EXITS_SEVEN: &str = "import std.process (process)\n\nfn main() -> Unit / {process.exit[proc]} = process.exit[proc](7)\n";

/// The fixture's runnable, as `shipped.runnable` writes one, and the C it carries.
fn written(source: &str) -> (Vec<u8>, Vec<u8>) {
    let dir = project(source);
    let front = handed(dir.path());
    let files = field_of(&front, "files", Span::DUMMY).expect("a front's files");
    let dump = field_of(&front, "dump", Span::DUMMY).expect("a front's dump");
    let unit = unit_text(&loaded(dir.path()));
    let bytes = runnable::encode("m.main", files, dump, &unit).expect("the runnable encodes");
    (bytes, unit)
}

#[test]
fn a_runnable_reads_back_as_the_program_it_was_written_from_and_enters() {
    let (bytes, unit) = written(EXITS_SEVEN);
    let program = runnable::decode(&bytes).expect("the runnable reads back");
    assert_eq!(program.entry, "m.main");
    assert_eq!(program.unit.as_bytes(), &unit[..]);
    assert!(
        program.front.files.iter().any(|f| f.name == "m"),
        "the program's own module is among the files it carries"
    );
    let opened = artifact::opened_runnable(program, std::path::Path::new("."))
        .expect("its front end's answer reads back over its own files");
    let ended = artifact::enter_runnable(opened, Vec::new(), Binds::default());
    assert_eq!(ended.into_parts().0.expect("the entry ran"), 7);
}

#[test]
fn what_a_load_walked_and_warned_of_is_no_part_of_a_runnable() {
    let dir = project(EXITS_SEVEN);
    let front = handed(dir.path());
    let files = field_of(&front, "files", Span::DUMMY).expect("a front's files");
    let dump = field_of(&front, "dump", Span::DUMMY).expect("a front's dump");
    let Value::Record(fields) = dump else {
        panic!("a dump is a record");
    };
    let other = ply_machine::payload::record(
        fields
            .iter()
            .map(|(name, value)| {
                let changed = match name.as_str() {
                    "walked" => Value::bytes(b"another walk"),
                    "diags" => Value::list(vec![Value::str("a cache's warning")]),
                    _ => value.clone(),
                };
                (name.as_str(), changed)
            })
            .collect(),
    );
    let unit = unit_text(&loaded(dir.path()));
    assert_eq!(
        runnable::encode("m.main", files, dump, &unit).expect("the runnable encodes"),
        runnable::encode("m.main", files, &other, &unit).expect("the runnable encodes"),
        "two loads of one program write one runnable"
    );
}

#[test]
fn bytes_that_are_no_runnable_are_refused() {
    assert!(runnable::decode(b"not compressed").is_err());
    let (bytes, _) = written("fn main() -> Int = 1\n");
    assert!(runnable::decode(&bytes[..bytes.len() / 2]).is_err());
}

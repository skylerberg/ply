//! The fronts `ply run` files once a load holds and reads back over a later run's walk: the walk's
//! own files where it names them, the shelf the filed answer pulled, the unit's C, and the promises
//! the filed load checked, held.

use crate::fixture::{handed, project};
use ply_eval::Span;
use ply_machine::driver::{LoadedAnalysis, loaded_analysis_of};
use ply_machine::reused::{self, Walked};
use std::path::Path;

const PULLING: &str =
    "import std.json (Null, to_string)\n\nfn main() -> Int = string_len(to_string(Null))\n";

/// The C a load that held was handed, filed beside its front.
const UNIT: &[u8] = b"/* the unit */\n";

/// The fixture's front as handed, and the entry a load that held over it files.
fn filed(dir: &Path) -> (LoadedAnalysis, Vec<u8>) {
    let value = handed(dir);
    let front = loaded_analysis_of(&value, Span::DUMMY).expect("the handed front reads");
    let placed: Vec<(String, String)> = front
        .files
        .iter()
        .map(|f| (f.path.clone(), f.name.clone()))
        .collect();
    let dump = ply_machine::payload::field_of(&value, "dump", Span::DUMMY).expect("a dump");
    let entry = reused::entry(&placed, dump, UNIT).expect("the answer encodes");
    (front, entry)
}

fn walk(dir: &Path, named: &str) -> Vec<Walked> {
    vec![Walked {
        path: named.to_string(),
        text: std::fs::read_to_string(dir.join("m.ply")).expect("the module reads"),
    }]
}

#[test]
fn a_filed_front_reads_back_over_the_walk_that_asks_for_it() {
    let dir = project(PULLING);
    let (front, entry) = filed(dir.path());
    assert!(
        front.files.len() > 1,
        "the fixture pulls a shipped module off the shelf"
    );
    let (back, unit) = reused::front(&entry, walk(dir.path(), "elsewhere/m.ply"), Vec::new())
        .expect("the entry reads back");
    assert_eq!(unit, UNIT, "the unit is read back as it was filed");
    assert_eq!(back.files.len(), front.files.len());
    let first = &back.files[0];
    assert_eq!(
        (first.path.as_str(), first.name.as_str()),
        ("elsewhere/m.ply", "m"),
        "the walk's own file is placed where this walk read it"
    );
    for (was, now) in front.files.iter().zip(&back.files).skip(1) {
        assert_eq!(
            (&was.path, &was.name, &was.text),
            (&now.path, &now.name, &now.text),
            "a shelved module is placed as the filed answer pulled it"
        );
    }
    assert_eq!(
        back.answer.check.defs.keys().collect::<Vec<_>>(),
        front.answer.check.defs.keys().collect::<Vec<_>>()
    );
}

#[test]
fn an_entry_that_does_not_read_or_does_not_fit_the_walk_is_none() {
    let dir = project(PULLING);
    let (front, entry) = filed(dir.path());
    let more: Vec<Walked> = (0..=front.files.len())
        .map(|i| Walked {
            path: format!("m{i}.ply"),
            text: String::new(),
        })
        .collect();
    assert!(
        reused::front(&entry, more, Vec::new()).is_none(),
        "a walk of more files than the entry placed is another closure"
    );
    assert!(
        reused::front(
            &entry[..entry.len() / 2],
            walk(dir.path(), "m.ply"),
            Vec::new()
        )
        .is_none(),
        "a torn entry is no entry"
    );
    assert!(reused::front(&[], walk(dir.path(), "m.ply"), Vec::new()).is_none());
}

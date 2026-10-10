//! The fronts `ply run` files once a load holds and reads back over a later run's walk: the walk's
//! own files where it names them, the shipped modules the filed answer pulled, and the library its
//! unit was compiled into.

use crate::front::{JSON, handed};
use ply_eval::Span;
use ply_machine::driver::loaded_analysis_of;
use ply_machine::reused::{self, Walked};

const OWN: &str = "import std.json (Null)\n\nfn main() -> Int = 1\n";

/// A front that pulled the shipped `std.json`, the entry a load that held over it files, and the
/// library that entry names, which a read back requires the cache still holds.
fn filed() -> (ply_machine::driver::LoadedAnalysis, Vec<u8>, tempfile::NamedTempFile) {
    let value = handed(&[("m.ply", "m", OWN), ("std/json.ply", "std.json", JSON)]);
    let front = loaded_analysis_of(&value, Span::DUMMY).expect("the handed front reads");
    let library = tempfile::NamedTempFile::new().expect("a library's file");
    let dump = ply_machine::payload::field_of(&value, "dump", Span::DUMMY).expect("a dump");
    let path = library.path().display().to_string();
    let entry = reused::entry(&front.files, dump, &path).expect("the answer encodes");
    (front, entry, library)
}

fn walk(named: &str) -> Vec<Walked> {
    vec![Walked {
        path: named.to_string(),
        text: OWN.to_string(),
    }]
}

#[test]
fn a_filed_front_reads_back_over_the_walk_that_asks_for_it() {
    let (front, entry, library) = filed();
    let (back, kept) =
        reused::front(&entry, walk("elsewhere/m.ply"), Vec::new()).expect("the entry reads back");
    assert_eq!(kept, library.path(), "the library is read back as it was filed");
    assert_eq!(back.files.len(), front.files.len());
    let first = &back.files[0];
    assert_eq!(
        (first.path.as_str(), first.name.as_str()),
        ("elsewhere/m.ply", "m"),
        "the walk's own file is placed where this walk read it"
    );
    let shipped = &back.files[1];
    assert_eq!(
        (shipped.path.as_str(), shipped.name.as_str(), shipped.text.as_str()),
        ("std/json.ply", "std.json", JSON),
        "a shipped module is placed as the filed answer pulled it"
    );
}

#[test]
fn an_entry_that_does_not_read_or_does_not_fit_the_walk_is_none() {
    let (front, entry, library) = filed();
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
        reused::front(&entry[..entry.len() / 2], walk("m.ply"), Vec::new()).is_none(),
        "a torn entry is no entry"
    );
    assert!(reused::front(&[], walk("m.ply"), Vec::new()).is_none());
    drop(library);
    assert!(
        reused::front(&entry, walk("m.ply"), Vec::new()).is_none(),
        "an entry naming a library the cache no longer holds is none"
    );
}

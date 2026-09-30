//! The fronts `ply run` files once a load holds and reads back over a later run's walk: the walk's
//! own files where it names them, the shelf the filed answer pulled, and the promises the filed
//! load checked, held.

use crate::fixture::{handed, project};
use ply_eval::Span;
use ply_machine::driver::{HandedFront, Promises, handed_front_of};
use ply_machine::reused::{self, Walked};
use std::path::{Path, PathBuf};

const PULLING: &str =
    "import std.json (Null, to_string)\n\nfn main() -> Int = string_len(to_string(Null))\n";

/// A key nothing else files under: the digest of the project's own path.
fn key_of(dir: &Path) -> String {
    blake3::hash(dir.display().to_string().as_bytes())
        .to_hex()
        .to_string()
}

/// The fixture's front filed under `key`, as a load that held files it, and the front as handed.
fn filed(dir: &Path, key: &str) -> HandedFront {
    let value = handed(dir);
    let front = handed_front_of(&value, Span::DUMMY).expect("the handed front reads");
    let placed: Vec<(String, String)> = front
        .files
        .iter()
        .map(|f| (f.path.clone(), f.name.clone()))
        .collect();
    let dump = ply_machine::payload::field_of(&value, "dump", Span::DUMMY).expect("a dump");
    reused::file(key, &placed, dump);
    front
}

fn walk(dir: &Path, named: &str) -> Vec<Walked> {
    vec![Walked {
        path: named.to_string(),
        text: std::fs::read_to_string(dir.join("m.ply")).expect("the module reads"),
    }]
}

fn entry(key: &str) -> PathBuf {
    reused::path_of(key).expect("a digest names an entry")
}

#[test]
fn a_filed_front_reads_back_over_the_walk_that_asks_for_it() {
    let dir = project(PULLING);
    let key = key_of(dir.path());
    let front = filed(dir.path(), &key);
    assert!(
        front.files.len() > 1,
        "the fixture pulls a shipped module off the shelf"
    );
    let (back, at) = reused::front(&key, walk(dir.path(), "elsewhere/m.ply"), Vec::new())
        .expect("the entry reads back");
    assert_eq!(at, entry(&key));
    assert_eq!(back.promises, Promises::Held);
    assert_eq!(front.promises, Promises::Unchecked);
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
    let _ = std::fs::remove_file(entry(&key));
}

#[test]
fn an_entry_that_does_not_read_or_does_not_fit_the_walk_is_none() {
    let dir = project(PULLING);
    let key = key_of(dir.path());
    let placed = filed(dir.path(), &key).files.len();
    let more: Vec<Walked> = (0..=placed)
        .map(|i| Walked {
            path: format!("m{i}.ply"),
            text: String::new(),
        })
        .collect();
    assert!(
        reused::front(&key, more, Vec::new()).is_none(),
        "a walk of more files than the entry placed is another closure"
    );
    let whole = std::fs::read(entry(&key)).expect("the entry was written");
    std::fs::write(entry(&key), &whole[..whole.len() / 2]).expect("the entry is torn");
    assert!(reused::front(&key, walk(dir.path(), "m.ply"), Vec::new()).is_none());
    let _ = std::fs::remove_file(entry(&key));
    assert!(reused::front(&key, walk(dir.path(), "m.ply"), Vec::new()).is_none());
    let (upper, short) = ("A".repeat(64), "0".repeat(63));
    for key in ["", "../escape", upper.as_str(), short.as_str()] {
        assert!(
            reused::path_of(key).is_none(),
            "`{key}` is not a walk's digest"
        );
    }
}

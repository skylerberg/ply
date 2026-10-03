//! A load seeded with the rows an earlier load of the same program published walks only what the
//! edit reached and answers as a load from nothing does; the front it keeps reads back as itself.

use crate::fixture::{scratch, write};
use ply_codegen::c::producer::KnownRows;
use ply_machine::driver::{kept_front, load_over_front_in};
use ply_machine::load::{Loaded, load, load_seeded};
use std::path::PathBuf;
use tempfile::TempDir;

fn manifest(name: &str, deps: &str) -> String {
    format!(
        "import std.pkg (Manifest)\nfn package() -> Manifest = {{name: \"{name}\", version: \
         {{major: 0, minor: 0, patch: 1}}, prefix: None, runtime: {{major: 0, minor: 0, patch: 1}}, \
         dependencies: [{deps}], entry: None}}\n"
    )
}

/// `app`, which reaches `util` by path; `util`'s rows outlive an edit to `app`.
fn project() -> (TempDir, PathBuf) {
    let dir = scratch();
    let app = dir.path().join("app");
    let util = "{name: \"util\", prefix: None, min: {major: 0, minor: 0, patch: 1}, \
                source: Path(\"../util\")}";
    write(&app, "ply.pkg", &manifest("app", util));
    write(
        &app,
        "main.ply",
        "import util.calc\n\nfn main() -> Int = calc::two() + 1\n",
    );
    write(&dir.path().join("util"), "ply.pkg", &manifest("util", ""));
    write(
        &dir.path().join("util"),
        "calc.ply",
        "pub fn one() -> Int = 1\n\npub fn two() -> Int = one() + one()\n\n\
         pub fn three() -> Int = two() + one()\n",
    );
    (dir, app)
}

/// The same hashes, and every definition with the same rows at the same text of the same file,
/// whatever ids the two loads gave their files.
fn same(a: &Loaded, b: &Loaded) {
    assert_eq!(a.hashes, b.hashes, "the hashes");
    assert_eq!(a.check.defs.len(), b.check.defs.len(), "the definitions");
    for (name, def) in &a.check.defs {
        let other = b.check.defs.get(name).expect("both name every definition");
        let place = |l: &Loaded, d: &ply_eval::DefInfo| {
            let file = l.sources.get(d.span.source).expect("a definition's file");
            (file.path.clone(), l.sources.snippet(d.span).into_owned())
        };
        assert_eq!(place(a, def), place(b, other), "`{name}`'s place");
        assert_eq!(
            (
                format!("{:?}", def.footprint),
                format!("{:?}", def.performed)
            ),
            (
                format!("{:?}", other.footprint),
                format!("{:?}", other.performed)
            ),
            "`{name}`'s rows"
        );
    }
}

#[test]
fn a_seeded_load_walks_what_the_edit_reached_and_answers_as_one_from_nothing() {
    let (_dir, app) = project();
    let first = load_seeded(&app, KnownRows::default()).expect("the program loads");
    assert_eq!(first.seeded, 0, "nothing was handed in");
    assert!(!first.rows.as_bytes().is_empty(), "the load published rows");

    write(
        &app,
        "main.ply",
        "import util.calc\n\nfn main() -> Int = calc::three() + 1\n",
    );
    let seeded = load_seeded(&app, first.rows).expect("the edited program loads seeded");
    let cold = load(&app).expect("the edited program loads from nothing");
    assert!(
        seeded.seeded >= 3,
        "`util.calc` comes from the rows, not a walk: {} seeded",
        seeded.seeded
    );
    same(&seeded.loaded, &cold);
}

#[test]
fn rows_that_do_not_read_seed_nothing() {
    let (_dir, app) = project();
    let seeded =
        load_seeded(&app, KnownRows::from_bytes(b"not rows".to_vec())).expect("the program loads");
    assert_eq!(seeded.seeded, 0);
    same(&seeded.loaded, &load(&app).expect("the program loads"));
}

#[test]
fn a_kept_front_reads_back_as_the_load_that_kept_it() {
    let (_dir, app) = project();
    let seeded = load_seeded(&app, KnownRows::default()).expect("the program loads");
    let bytes = seeded.front.as_ref().expect("the load kept its front");
    let handed = kept_front(bytes).expect("the kept front reads back");
    let reloaded = load_over_front_in(app.clone(), &handed).expect("it loads");
    same(&reloaded, &seeded.loaded);
}

#[test]
fn bytes_that_are_not_a_kept_front_read_as_none() {
    assert!(kept_front(b"").is_none());
    assert!(kept_front(b"not a front").is_none());
    let other = ply_eval::codec::encode(&ply_eval::Value::str("a front")).expect("it encodes");
    assert!(kept_front(&other).is_none());
}

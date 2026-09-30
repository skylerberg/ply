//! The front-end cache on the path every command takes: a load reads what the last one filed before
//! it analyses, and files what it answered after. A warm check answers what a cold one does; that a
//! seeded analysis is sound through every kind of edit is the compiler's own property, and
//! `crates/ply-compiler/ply/front.ply` tests it over sessions of edits.

use crate::harness::{ply, seeding, warm_agrees, write};
use ply_store::{ContentHash, Store};
use std::fs;
use std::path::{Path, PathBuf};

fn edit(dir: &Path, name: &str, from: &str, to: &str) {
    let path = dir.join(name);
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains(from),
        "`{from}` is not in {name}; the fixture drifted"
    );
    fs::write(path, text.replace(from, to)).unwrap();
}

const CORE: &str = r#"
pub type Money = Int

pub type Item =
  | Book(String, Money)
  | Note(String)

pub effect db {
  read  get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Int
}

pub fn price(i: Item) -> Money =
  match i {
    Book(_, p) -> p,
    Note(_) -> 0,
  }

pub fn twice<a>(x: a, f: (a) -> a) -> a = f(f(x))

pub fn label(i: Item) -> String =
  match i {
    Book(t, _) -> t,
    Note(t) -> t,
  }

test "a note is free" {
  assert_eq(price(Note("n")), 0)
}
"#;

const SHOP: &str = r#"
import core

fn total(items: List<core::Item>) -> Int =
  fold(items, 0, |acc, i: core::Item| acc + core::price(i))

fn doubled(n: Int) -> Int = core::twice(n, |x: Int| x + x)

fn stored(k: Int) -> Int / {core::db.read[cart]} = core::db.get[cart](k)

test "a shelf adds up" {
  assert_eq(total([core::Book("b", 3), core::Note("n")]), 3)
}
"#;

const LEAF: &str = r#"
pub fn one() -> Int = 1
pub fn two() -> Int = one() + one()
"#;

fn corpus() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "core.ply", CORE);
    write(dir.path(), "shop.ply", SHOP);
    write(dir.path(), "leaf.ply", LEAF);
    dir
}

/// Nothing in the corpus imports a shipped module, so every definition is the project's own.
#[test]
fn a_second_check_is_seeded_whole_and_an_edit_checks_only_what_it_moved() {
    let dir = corpus();
    let (seeded, checked) = seeding(dir.path());
    assert_eq!(seeded, 0, "a cold cache seeds nothing");
    let all = checked;
    assert!(all > 0);

    assert_eq!(
        seeding(dir.path()),
        (all, 0),
        "a warm check takes every definition from its filed rows"
    );

    // Nothing references `two`, so its own hash is the only one the edit moves.
    edit(dir.path(), "leaf.ply", "one() + one()", "one() + one() + 0");
    assert_eq!(
        seeding(dir.path()),
        (all - 1, 1),
        "the edited definition is checked and every other one is seeded"
    );
    warm_agrees(dir.path(), "after the edit");
}

#[test]
fn a_whole_editing_session_agrees_at_every_step() {
    let dir = corpus();
    warm_agrees(dir.path(), "step 0");
    warm_agrees(dir.path(), "step 0, warm");

    edit(dir.path(), "leaf.ply", "one() + one()", "one() + 1");
    warm_agrees(dir.path(), "step 1: body");

    edit(dir.path(), "core.ply", "pub fn label(", "pub fn title(");
    warm_agrees(dir.path(), "step 2: rename");

    edit(dir.path(), "core.ply", "Money", "Cost");
    warm_agrees(dir.path(), "step 3: rename a type");

    write(
        dir.path(),
        "extra.ply",
        "import leaf\npub fn four() -> Int = leaf::two() + 2\n",
    );
    warm_agrees(dir.path(), "step 4: add a file");

    edit(
        dir.path(),
        "extra.ply",
        "leaf::two() + 2",
        "leaf::two() + 3",
    );
    warm_agrees(dir.path(), "step 5: edit the new file");

    fs::remove_file(dir.path().join("extra.ply")).unwrap();
    warm_agrees(dir.path(), "step 6: delete it again");

    edit(dir.path(), "core.ply", "effect db", "effect ledger");
    edit(dir.path(), "shop.ply", "core::db.", "core::ledger.");
    warm_agrees(dir.path(), "step 7: rename an effect");
}

fn examples() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for entry in fs::read_dir(&root).expect("the example corpus must be present") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "ply") {
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            write(dir.path(), &name, &fs::read_to_string(&path).unwrap());
        }
    }
    dir
}

#[test]
fn the_example_corpus_agrees_cold_and_warm() {
    let dir = examples();
    warm_agrees(dir.path(), "cold");
    warm_agrees(dir.path(), "warm");
}

/// One cache for every mutation: an invalidation is only ever wrong in some sequence of edits.
///
/// Real code exercises handlers, regions, `nondet` effects and cross-module types the synthetic
/// corpora do not, and the session ends by undoing every edit so a wrong answer shows up as a
/// final state that is not the one it began in.
#[test]
fn a_long_session_over_the_example_corpus_agrees_at_every_step() {
    let dir = examples();
    let start = warm_agrees(dir.path(), "step 0");

    let clock = fs::read_to_string(dir.path().join("clock.ply")).unwrap();
    write(
        dir.path(),
        "clock.ply",
        &format!("{clock}\npub fn ticks() -> Int = 0\n"),
    );
    warm_agrees(dir.path(), "step 1: a definition appeared");

    edit(
        dir.path(),
        "report.ply",
        "fn assets() -> List<String>",
        "// a note\nfn assets() -> List<String>",
    );
    warm_agrees(dir.path(), "step 2: a comment");

    edit(dir.path(), "ledger.ply", "presented", "presented_value");
    edit(dir.path(), "report.ply", "presented", "presented_value");
    warm_agrees(dir.path(), "step 3: a rename across modules");

    edit(dir.path(), "report.ply", "type Line = ", "type Row = ");
    edit(dir.path(), "report.ply", "-> Line =", "-> Row =");
    edit(dir.path(), "report.ply", "List<Line>", "List<Row>");
    edit(dir.path(), "report.ply", "l: Line|", "l: Row|");
    warm_agrees(dir.path(), "step 4: a type rename");

    write(dir.path(), "spare.ply", "pub fn spare() -> Int = 9\n");
    warm_agrees(dir.path(), "step 5: a module appeared");

    fs::rename(dir.path().join("spare.ply"), dir.path().join("kept.ply")).unwrap();
    warm_agrees(dir.path(), "step 6: it was renamed");

    fs::remove_file(dir.path().join("kept.ply")).unwrap();
    warm_agrees(dir.path(), "step 7: and deleted");

    write(dir.path(), "clock.ply", &clock);
    edit(
        dir.path(),
        "report.ply",
        "// a note\nfn assets()",
        "fn assets()",
    );
    edit(dir.path(), "ledger.ply", "presented_value", "presented");
    edit(dir.path(), "report.ply", "presented_value", "presented");
    edit(dir.path(), "report.ply", "type Row = ", "type Line = ");
    edit(dir.path(), "report.ply", "-> Row =", "-> Line =");
    edit(dir.path(), "report.ply", "List<Row>", "List<Line>");
    edit(dir.path(), "report.ply", "l: Row|", "l: Line|");
    let end = warm_agrees(dir.path(), "step 8: back to where it started");
    assert_eq!(
        start, end,
        "an undone session must land on the state it began in"
    );
}

/// A deleted import is an error however much of the program the cache still holds rows for.
#[test]
fn deleting_a_file_is_reported_rather_than_skipped_past() {
    let dir = corpus();
    write(
        dir.path(),
        "extra.ply",
        "import leaf\npub fn three() -> Int = leaf::two()\n",
    );
    warm_agrees(dir.path(), "cold");

    fs::remove_file(dir.path().join("leaf.ply")).unwrap();
    let answer = warm_agrees(dir.path(), "a dangling import");
    assert_eq!(answer["exit_code"], 2, "{answer}");
    assert!(
        answer["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == ply_eval::codes::UNKNOWN_MODULE),
        "{answer}"
    );
}

/// Structurally identical definitions in two modules share a hash while their schemes name
/// different types: each is filed under its own name, and each is seeded with its own row.
#[test]
fn two_definitions_that_share_a_hash_each_keep_their_own_interface() {
    let dir = tempfile::tempdir().unwrap();
    let body =
        "pub type Thing = | Wrap(Int)\npub fn peel(t: Thing) -> Int = match t { Wrap(n) -> n }\n";
    write(dir.path(), "a.ply", body);
    write(dir.path(), "b.ply", body);

    warm_agrees(dir.path(), "cold");
    let warm = warm_agrees(dir.path(), "warm");
    let type_of = |name: &str| {
        warm["definitions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == name)
            .map(|d| d["type"].clone())
            .unwrap_or_else(|| panic!("`{name}` is in the answer"))
    };
    assert_ne!(
        type_of("a.peel"),
        type_of("b.peel"),
        "each definition's scheme must name its own module's type"
    );

    let store = Store::open(dir.path()).unwrap();
    let hash_of = |file: &str, name: &str| {
        store
            .fingerprint(&dir.path().join(file))
            .expect("the file is on record")
            .defs
            .iter()
            .find(|e| e.name.as_str() == name)
            .expect("the definition is on record")
            .hash
    };
    let shared = hash_of("a.ply", "a.peel");
    assert_eq!(
        shared,
        hash_of("b.ply", "b.peel"),
        "the fixture is only interesting while the two hash alike"
    );
    for name in ["a.peel", "b.peel"] {
        assert!(
            store.def_of(shared, &ply_eval::Symbol::new(name)).is_some(),
            "`{name}` has a slot of its own under the shared hash"
        );
    }
}

#[test]
fn a_corrupt_front_end_cache_degrades_to_a_cold_check_and_is_repaired() {
    let dir = corpus();
    warm_agrees(dir.path(), "cold");
    fs::write(
        dir.path().join(".ply-cache/frontend.idx"),
        "not an index at all",
    )
    .unwrap();

    let out = ply(dir.path()).args(["check", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let answer: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        answer["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"].as_str().unwrap_or("").starts_with("W06")),
        "a discarded cache is reported: {answer}"
    );
    assert_eq!(
        answer["front_end"]["definitions"]["seeded"], 0,
        "nothing is taken from a cache that did not read"
    );
    warm_agrees(dir.path(), "after the cache was rewritten");
    let (_, checked) = seeding(dir.path());
    assert_eq!(
        checked, 0,
        "the run that met the damage filed a whole cache"
    );
}

/// Mangled in the payload rather than replaced: a valid header over undecodable entries is what a
/// half-written append leaves.
#[test]
fn a_cache_mangled_mid_session_degrades_and_recovers() {
    let dir = corpus();
    warm_agrees(dir.path(), "cold");
    edit(
        dir.path(),
        "leaf.ply",
        "pub fn one() -> Int = 1",
        "pub fn one() -> Int = 2",
    );
    warm_agrees(dir.path(), "edited");

    let data = dir.path().join(".ply-cache/frontend.dat");
    let mut bytes = fs::read(&data).unwrap();
    for byte in bytes.iter_mut().skip(64) {
        *byte ^= 0x5a;
    }
    fs::write(&data, &bytes).unwrap();
    warm_agrees(dir.path(), "the cache was mangled");
    warm_agrees(dir.path(), "and the run after that");
}

/// The shape a half-finished garbage collection would leave: an index whose data file is gone.
#[test]
fn fingerprints_without_their_interfaces_are_refused_rather_than_believed() {
    let dir = corpus();
    warm_agrees(dir.path(), "cold");
    fs::remove_file(dir.path().join(".ply-cache/frontend.dat")).unwrap();
    warm_agrees(dir.path(), "fingerprints with no interfaces behind them");
    warm_agrees(dir.path(), "and the run after that");
}

/// Only a load of the whole project drops what the cache holds for files it did not read.
#[test]
fn a_single_file_run_does_not_spoil_the_whole_project_run_after_it() {
    let dir = corpus();
    warm_agrees(dir.path(), "cold");
    let out = ply(dir.path())
        .args(["check", "leaf.ply"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        Store::open(dir.path()).unwrap().sources_len() >= 3,
        "a single-file run must not have pruned the rest of the project"
    );
    warm_agrees(dir.path(), "whole project after a single-file run");
}

#[test]
fn two_overlapping_runs_leave_a_cache_that_still_agrees() {
    let dir = corpus();
    let root = dir.path();
    std::thread::scope(|scope| {
        let first = scope.spawn(|| ply(root).arg("check").output().unwrap());
        let second = scope.spawn(|| ply(root).args(["check", "."]).output().unwrap());
        assert_eq!(first.join().unwrap().status.code(), Some(0));
        assert_eq!(second.join().unwrap().status.code(), Some(0));
    });
    warm_agrees(root, "after two overlapping runs");
    edit(
        root,
        "leaf.ply",
        "pub fn one() -> Int = 1",
        "pub fn one() -> Int = 3",
    );
    warm_agrees(root, "and an edit after them");
}

/// What turns caching off files nothing: `ply build`, `ply hosts`, `--no-cache` and a program's own
/// load of a program.
#[test]
fn a_load_that_does_not_cache_files_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "m.ply",
        "pub fn main() -> Int = 1\ntest \"one\" { assert_eq(main(), 1) }\n",
    );
    let untouched = |what: &str| {
        let cache = dir.path().join(".ply-cache");
        let filed = cache.join("frontend.idx").exists() || cache.join("frontend.dat").exists();
        assert!(!filed, "{what} filed a front-end cache");
    };
    ply_machine::load::load(dir.path()).expect("the program loads");
    untouched("a program's own load");
    let out = ply(dir.path())
        .args(["test", "--no-cache"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    untouched("`ply test --no-cache`");
    let out = ply(dir.path()).args(["hosts"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    untouched("`ply hosts`");
    let out = ply(dir.path())
        .args(["build", ".", "--digest"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    untouched("`ply build`");
}

/// A dependency's module is filed under its package's identity, so moving the package keeps what
/// was filed for it, and a changed dependency is checked again.
#[test]
fn a_dependency_module_is_filed_under_its_package_and_survives_the_package_moving() {
    let manifest = |name: &str, deps: &str| {
        format!(
            "import std.pkg (Manifest)\nfn package() -> Manifest = {{name: \"{name}\", version: {{major: 0, minor: 0, patch: 1}}, prefix: None, runtime: {{major: 0, minor: 0, patch: 1}}, dependencies: [{deps}], entry: None}}\n"
        )
    };
    let dep = |path: &str| {
        format!(
            "{{name: \"lib\", prefix: None, min: {{major: 0, minor: 0, patch: 1}}, source: Path(\"{path}\")}}"
        )
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "app/ply.pkg", &manifest("app", &dep("../lib")));
    write(
        root,
        "app/main.ply",
        "import lib.answer\nfn main() -> Int = answer::answer()\n",
    );
    let lib_manifest = manifest("lib", "");
    write(root, "lib/ply.pkg", &lib_manifest);
    write(root, "lib/answer.ply", "pub fn answer() -> Int = 42\n");
    let app = root.join("app");

    let out = ply(&app).arg("check").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let identity = ContentHash::of(lib_manifest.as_bytes()).to_hex();
    let mut store = Store::open(&app).unwrap();
    assert!(
        store
            .source_keys()
            .contains(&format!("{identity}/answer.ply")),
        "the dependency's module is keyed by its package's identity: {:?}",
        store.source_keys()
    );
    store.set_packages(vec![(PathBuf::from("/anywhere"), identity.clone())]);
    let filed = store
        .fingerprint(Path::new("/anywhere/answer.ply"))
        .expect("found by the package it belongs to");
    assert_eq!(
        filed.module, "lib.answer",
        "filed under the name the front end gave it"
    );
    drop(store);

    // The package is checked out somewhere else, and its manifest is unchanged: the rows filed for
    // it are the moved package's too.
    fs::create_dir_all(root.join("vendor")).unwrap();
    fs::rename(root.join("lib"), root.join("vendor/lib")).unwrap();
    write(root, "app/ply.pkg", &manifest("app", &dep("../vendor/lib")));
    let (_, checked) = seeding(&app);
    assert_eq!(checked, 0, "a moved package keeps the rows filed for it");

    // A changed dependency is a different program: its edited definition and the one calling it
    // are checked.
    write(
        root,
        "vendor/lib/answer.ply",
        "pub fn answer() -> Int = 43\n",
    );
    let (_, checked) = seeding(&app);
    assert_eq!(
        checked, 2,
        "the edit and its caller are checked, and nothing else"
    );
}

/// `ply prove` lowers a module's claims only when it has something to discharge there, and keeps
/// what it lowered keyed by the texts the module reaches.
#[test]
fn prove_asks_for_claims_only_when_something_is_discharged_and_only_where_an_edit_reached() {
    use ply_codegen::c::producer;
    use ply_machine::driver;
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "base.ply",
        "\
pub fn one() -> Int = 1

pub fn bump(x: Int) -> Int
  requires x < 100
  ensures result > x
= x + one()
",
    );
    write(
        dir.path(),
        "app.ply",
        "\
import base

fn more(x: Int) -> Int
  requires x < 50
  ensures result > x
= base::bump(x)
",
    );
    write(
        dir.path(),
        "side.ply",
        "\
fn zero(x: Int) -> Int
  ensures result == 0
= x - x
",
    );

    // What a load the CLI cached answers with: its claims are kept beside the front-end cache.
    let cached = || {
        let mut loaded = ply_machine::load::load(dir.path()).expect("the project loads");
        loaded.frontend.incremental = true;
        loaded
    };
    // `ply prove` lowers claims on the thread its prover runs on, and the port's census is that
    // thread's, so it is read here, where the claims are asked for.
    let lowered = |what: &str, claimed: usize| {
        let mut store = Store::open(dir.path()).unwrap();
        let loaded = cached();
        producer::reset_census();
        driver::claims(&loaded, Some(&mut store)).expect("the claims lower");
        assert_eq!(
            producer::census().claimed,
            claimed,
            "{what}: modules whose claims the port was asked for"
        );
        store.flush().unwrap();
    };

    lowered("cold", 3);
    lowered("nothing edited", 0);
    edit(dir.path(), "side.ply", "x - x", "x - x + 0");
    lowered("an edit to a module nothing imports", 1);
    edit(dir.path(), "base.ply", "x + one()", "x + one() + 0");
    lowered("an edit to a module another imports", 2);

    ply(dir.path()).args(["prove", "."]).assert().success();
    ply(dir.path()).args(["prove", "."]).assert().success();

    // What the run reads the cache for: every obligation is answered from it, whatever became of
    // the claims the prover lowered.
    fs::remove_file(dir.path().join(".ply-cache/claims.answer")).unwrap();
    let out = ply(dir.path())
        .args(["prove", ".", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("one JSON object");
    assert_eq!(v["obligations"].as_array().map(Vec::len), Some(3));
    assert_eq!(
        v["cached"], 3,
        "every obligation is answered from the cache: {v}"
    );
}

/// A `reuse fn` is checked whole-program, so a warm check must refuse a broken promise as a cold one
/// does.
#[test]
fn a_promise_is_known_on_every_run_and_still_refused() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "grow.ply",
        "reuse fn grow(xs: List<Int>, n: Int) -> List<Int> = {\n\
         \x20 let ys = push(xs, n);\n\
         \x20 if len(xs) < 0 { xs } else { ys }\n\
         }\n",
    );
    for run in ["first", "second"] {
        let answer = warm_agrees(dir.path(), run);
        assert_eq!(answer["exit_code"], 2, "{run}: {answer}");
        assert!(
            answer["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == ply_eval::codes::REUSE_BROKEN),
            "{run}: {answer}"
        );
    }
}

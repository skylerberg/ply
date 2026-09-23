use assert_cmd::Command;
use ply_machine::driver;
use ply_machine::load::{LoadError, Loaded};
use ply_span::Symbol;
use ply_store::Store;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// `{:?}`, not a printed signature: printing renames variables per item and would hide a numbering divergence.
fn snapshot(loaded: &Loaded) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, hash) in &loaded.hashes.defs {
        out.insert(format!("hash {name}"), hash.to_hex());
    }
    for (name, hash) in &loaded.hashes.decls {
        out.insert(format!("decl {name}"), hash.to_hex());
    }
    for (name, def) in &loaded.check.defs {
        out.insert(format!("scheme {name}"), format!("{:?}", def.scheme));
        out.insert(format!("footprint {name}"), def.footprint.to_string());
    }
    for (name, ctor) in &loaded.check.ctors {
        out.insert(
            format!("ctor {name}"),
            format!("{} {:?} {:?}", ctor.index, ctor.fields, ctor.scheme),
        );
    }
    for (name, effect) in &loaded.check.effects {
        let ops: Vec<String> = effect
            .ops
            .values()
            .map(|op| format!("{} {:?} {:?} {:?}", op.name, op.mode, op.params, op.ret))
            .collect();
        out.insert(
            format!("effect {name}"),
            format!("{} {ops:?}", effect.nondet),
        );
    }
    for (i, test) in loaded.check.tests.iter().enumerate() {
        let hash = loaded
            .hashes
            .tests
            .get(i)
            .map(|h| h.to_hex())
            .unwrap_or_default();
        out.insert(
            format!("test {i}"),
            format!("{} {} {} {hash}", test.key, test.nondet, test.footprint),
        );
    }
    out
}

/// The incremental run goes first so it sees the store as an edit-test loop would.
#[track_caller]
fn agree(dir: &Path, what: &str) -> Loaded {
    let mut store = Store::open(dir).expect("the cache directory must be creatable");
    let incremental = driver::load_incremental(dir, &mut store)
        .unwrap_or_else(|e| panic!("{what}: the incremental path failed: {:?}", codes(&e)));
    let full = driver::load_full(dir)
        .unwrap_or_else(|e| panic!("{what}: the from-scratch path failed: {:?}", codes(&e)));

    let a = snapshot(&full);
    let b = snapshot(&incremental);
    let mut differences = Vec::new();
    for key in a.keys().chain(b.keys()) {
        if a.get(key) != b.get(key) {
            differences.push(format!(
                "  {key}\n    from scratch {:?}\n    incremental  {:?}",
                a.get(key),
                b.get(key)
            ));
        }
    }
    differences.dedup();
    assert!(
        differences.is_empty(),
        "{what}: the incremental path disagreed with a from-scratch check:\n{}",
        differences.join("\n")
    );
    incremental
}

fn codes(e: &LoadError) -> Vec<String> {
    e.diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect()
}

fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn hash_of(loaded: &Loaded, name: &str) -> String {
    loaded.hashes.defs[&Symbol::new(name)].to_hex()
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

fn stored(k: Int) -> Int / {core::db.read[cart]} = core::db.get[cart](k)

test "a shelf adds up" {
  assert_eq(total([core::Book("b", 3), core::Note("n")]), 3)
}
"#;

const LEAF: &str = "pub fn one() -> Int = 1\npub fn two() -> Int = one() + one()\n";

fn corpus() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "core.ply", CORE);
    write(dir.path(), "shop.ply", SHOP);
    write(dir.path(), "leaf.ply", LEAF);
    dir
}

const EFFECT_DECL: &str = "pub effect db { read get[r](key: Int) -> Int }\n";

/// A module that performs an effect declared elsewhere and declares none of its own.
fn performer(module: &str) -> String {
    format!(
        "import {module}\n\
         pub fn look(k: Int) -> Int / {{{module}::db.read[t]}} = {module}::db.get[t](k)\n\
         test \"looks\" {{ handle {{ assert_eq(look(1), 0) }} with {{ {module}::db.get[t](k) -> 0 }} }}\n"
    )
}

/// Normalization writes an effect's rank among its structurally identical twins into every performer's hash.
#[test]
fn adding_an_identically_declared_effect_reranks_a_skipped_performer() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "x.ply", EFFECT_DECL);
    write(dir.path(), "p.ply", &performer("x"));
    agree(dir.path(), "cold");
    agree(dir.path(), "warm");

    write(dir.path(), "a.ply", EFFECT_DECL);
    agree(
        dir.path(),
        "a module declaring an identical effect appeared",
    );
}

/// The rank is decided by sorting the twins' names, so renaming one of them moves the other.
#[test]
fn renaming_an_effect_reranks_its_structural_twin() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.ply", EFFECT_DECL);
    write(dir.path(), "x.ply", EFFECT_DECL);
    write(dir.path(), "p.ply", &performer("a"));
    agree(dir.path(), "cold");

    write(
        dir.path(),
        "x.ply",
        "pub effect audit { read get[r](key: Int) -> Int }\n",
    );
    agree(dir.path(), "the twin was renamed to sort first");
}

#[test]
fn deleting_an_effect_declaring_module_reranks_the_survivor() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.ply", EFFECT_DECL);
    write(dir.path(), "x.ply", EFFECT_DECL);
    write(dir.path(), "p.ply", &performer("x"));
    agree(dir.path(), "cold");

    fs::remove_file(dir.path().join("a.ply")).unwrap();
    agree(dir.path(), "the module that ranked first was deleted");
}

/// Operations are sorted before hashing, so reordering is a `DefHash` no-op, but each signature must stay on its own operation.
#[test]
fn reordering_an_effects_operations_keeps_every_signature_on_its_operation() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "m.ply",
        "pub effect db {\n\
         \x20 read  get[r](key: Int) -> Int\n\
         \x20 write put[r](key: Int, value: String) -> Unit\n\
         }\npub fn f() -> Int = 1\n",
    );
    agree(dir.path(), "cold");

    write(
        dir.path(),
        "m.ply",
        "pub effect db {\n\
         \x20 write put[r](key: Int, value: String) -> Unit\n\
         \x20 read  get[r](key: Int) -> Int\n\
         }\npub fn f() -> Int = 1\n",
    );
    agree(dir.path(), "the operations were reordered");
}

/// A type's variants are *not* sorted away, so the same restore is safe for them.
#[test]
fn reordering_a_types_variants_changes_its_hash_and_so_costs_a_recheck() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "m.ply",
        "pub type T =\n  | A(Int)\n  | B(String)\n",
    );
    let cold = agree(dir.path(), "cold");
    let before = cold.hashes.decls[&Symbol::new("m.T")];

    write(
        dir.path(),
        "m.ply",
        "pub type T =\n  | B(String)\n  | A(Int)\n",
    );
    let after = agree(dir.path(), "the variants were reordered");
    assert_ne!(after.hashes.decls[&Symbol::new("m.T")], before);
}

/// A test's hash covers its body and `nondet` marker, never its label, yet the reported label must be the source's.
#[test]
fn renaming_a_test_label_is_reported_by_a_from_scratch_check() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "m.ply",
        "fn f() -> Int = 1\ntest \"f is one\" { assert_eq(f(), 1) }\n",
    );
    agree(dir.path(), "cold");

    write(
        dir.path(),
        "m.ply",
        "fn f() -> Int = 1\ntest \"f is really one\" { assert_eq(f(), 1) }\n",
    );
    agree(dir.path(), "the label changed");
    agree(dir.path(), "and the run after that, in case it self-heals");
}

/// `pub` is erased by normalization, so no hash can show this: the error has to come from checking.
#[test]
fn removing_pub_from_an_imported_name_is_still_an_error() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.ply", "pub fn one() -> Int = 1\n");
    write(
        dir.path(),
        "b.ply",
        "import a\nfn two() -> Int = a::one() + a::one()\n",
    );
    agree(dir.path(), "cold");

    write(dir.path(), "a.ply", "fn one() -> Int = 1\n");
    let mut store = Store::open(dir.path()).unwrap();
    let incremental = driver::load_incremental(dir.path(), &mut store);
    let full = driver::load_full(dir.path());
    assert_eq!(
        incremental.is_err(),
        full.is_err(),
        "the two paths disagreed about whether the program compiles; \
         from scratch: {:?}, incremental: {:?}",
        full.err().map(|e| codes(&e)),
        incremental.err().map(|e| codes(&e)),
    );
}

#[test]
fn editing_a_body_and_reverting_it_restores_every_hash() {
    let dir = corpus();
    let before = snapshot(&agree(dir.path(), "cold"));

    write(
        dir.path(),
        "leaf.ply",
        "pub fn one() -> Int = 1\npub fn two() -> Int = one() + 1\n",
    );
    agree(dir.path(), "edited");
    write(dir.path(), "leaf.ply", LEAF);
    assert_eq!(before, snapshot(&agree(dir.path(), "reverted")));
}

#[test]
fn adding_a_definition_and_deleting_it_restores_every_hash() {
    let dir = corpus();
    let before = snapshot(&agree(dir.path(), "cold"));

    write(
        dir.path(),
        "leaf.ply",
        &format!("{LEAF}pub fn three() -> Int = two() + one()\n"),
    );
    agree(dir.path(), "a definition was added");
    write(dir.path(), "leaf.ply", LEAF);
    assert_eq!(before, snapshot(&agree(dir.path(), "and deleted again")));
}

#[test]
fn moving_a_definition_between_modules_and_back_changes_no_hash() {
    let dir = corpus();
    write(dir.path(), "a.ply", "pub fn shared() -> Int = 41 + 1\n");
    write(dir.path(), "b.ply", "pub fn other() -> Int = 7\n");
    let shared = hash_of(&agree(dir.path(), "cold"), "a.shared");

    write(dir.path(), "a.ply", "");
    write(
        dir.path(),
        "b.ply",
        "pub fn shared() -> Int = 41 + 1\npub fn other() -> Int = 7\n",
    );
    assert_eq!(
        hash_of(&agree(dir.path(), "moved to b"), "b.shared"),
        shared
    );

    write(dir.path(), "a.ply", "pub fn shared() -> Int = 41 + 1\n");
    write(dir.path(), "b.ply", "pub fn other() -> Int = 7\n");
    assert_eq!(
        hash_of(&agree(dir.path(), "moved back"), "a.shared"),
        shared
    );
}

/// Moving the file changes the fingerprint's key as well as every program-wide name derived from it.
#[test]
fn renaming_a_module_and_renaming_it_back_changes_no_hash() {
    let dir = corpus();
    let one = hash_of(&agree(dir.path(), "cold"), "leaf.one");

    fs::rename(dir.path().join("leaf.ply"), dir.path().join("frond.ply")).unwrap();
    assert_eq!(
        hash_of(&agree(dir.path(), "module renamed"), "frond.one"),
        one
    );

    fs::rename(dir.path().join("frond.ply"), dir.path().join("leaf.ply")).unwrap();
    assert_eq!(hash_of(&agree(dir.path(), "renamed back"), "leaf.one"), one);
}

#[test]
fn deleting_an_imported_file_and_restoring_it_restores_every_hash() {
    let dir = corpus();
    let before = snapshot(&agree(dir.path(), "cold"));
    let core = fs::read_to_string(dir.path().join("core.ply")).unwrap();

    fs::remove_file(dir.path().join("core.ply")).unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    driver::load_incremental(dir.path(), &mut store)
        .expect_err("an import of a deleted module must be reported, not skipped past");
    drop(store);

    write(dir.path(), "core.ply", &core);
    assert_eq!(before, snapshot(&agree(dir.path(), "restored")));
}

/// Nothing may consult mtime: a file rewritten with the bytes it had publishes what it published.
#[test]
fn rewriting_a_file_with_identical_bytes_invalidates_nothing() {
    let dir = corpus();
    agree(dir.path(), "cold");
    let before = snapshot(&agree(dir.path(), "warm"));

    let text = fs::read_to_string(dir.path().join("leaf.ply")).unwrap();
    fs::write(dir.path().join("leaf.ply"), text).unwrap();
    assert_eq!(
        before,
        snapshot(&agree(dir.path(), "rewritten with identical bytes"))
    );
}

#[test]
fn a_comment_moves_no_hash_anywhere() {
    let dir = corpus();
    let before = snapshot(&agree(dir.path(), "cold"));
    write(dir.path(), "leaf.ply", &format!("// a note\n{LEAF}"));
    assert_eq!(before, snapshot(&agree(dir.path(), "a comment was added")));
}

#[test]
fn aliasing_an_import_differently_and_back_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    let shop = fs::read_to_string(dir.path().join("shop.ply")).unwrap();

    write(
        dir.path(),
        "shop.ply",
        &shop
            .replace("import core", "import core as c")
            .replace("core::", "c::"),
    );
    agree(dir.path(), "the import was aliased");
    write(dir.path(), "shop.ply", &shop);
    agree(dir.path(), "the alias was removed");
}

#[test]
fn switching_between_a_module_import_and_a_selective_one_agrees() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.ply", "pub fn one() -> Int = 1\n");
    write(
        dir.path(),
        "b.ply",
        "import a\nfn two() -> Int = a::one() + a::one()\n",
    );
    agree(dir.path(), "cold");

    write(
        dir.path(),
        "b.ply",
        "import a (one)\nfn two() -> Int = one() + one()\n",
    );
    agree(dir.path(), "selective");
    write(
        dir.path(),
        "b.ply",
        "import a\nfn two() -> Int = a::one() + a::one()\n",
    );
    agree(dir.path(), "back to a module import");
}

/// Nothing but the resolution witness moves: every scheme mentioning the type is written in the old name.
#[test]
fn renaming_a_type_two_modules_away_agrees() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "a.ply",
        "pub type T = | Mk(Int)\npub fn mk() -> T = Mk(1)\n",
    );
    write(
        dir.path(),
        "b.ply",
        "import a\npub fn get() -> a::T = a::mk()\n",
    );
    // `top` does not republish `b::get`'s type, so the rename need not touch `c.ply`.
    write(
        dir.path(),
        "c.ply",
        "import b\npub fn top() -> Bool = { b::get(); true }\n",
    );
    agree(dir.path(), "cold");

    write(
        dir.path(),
        "a.ply",
        "pub type Renamed = | Mk(Int)\npub fn mk() -> Renamed = Mk(1)\n",
    );
    write(
        dir.path(),
        "b.ply",
        "import a\npub fn get() -> a::Renamed = a::mk()\n",
    );
    agree(dir.path(), "the type was renamed two modules away");
}

#[test]
fn narrowing_an_effect_annotation_agrees() {
    let dir = tempfile::tempdir().unwrap();
    let source = |row: &str| {
        format!(
            "pub effect db {{ read get[r](key: Int) -> Int }}\n\
             pub fn look(k: Int) -> Int / {row} = db.get[a](k)\n"
        )
    };
    write(dir.path(), "m.ply", &source("{db.read[a], db.read[b]}"));
    agree(dir.path(), "cold");
    write(dir.path(), "m.ply", &source("{db.read[a]}"));
    agree(dir.path(), "the annotation narrowed");
}

#[test]
fn changing_only_a_resource_label_reaches_the_importer() {
    let dir = tempfile::tempdir().unwrap();
    let m = |label: &str| {
        format!(
            "pub effect db {{ read get[r](key: Int) -> Int }}\n\
             pub fn look(k: Int) -> Int / {{db.read[{label}]}} = db.get[{label}](k)\n"
        )
    };
    let u = |label: &str| {
        format!("import m\nfn use_it() -> Int / {{m::db.read[{label}]}} = m::look(1)\n")
    };
    write(dir.path(), "m.ply", &m("a"));
    write(dir.path(), "u.ply", &u("a"));
    let cold = agree(dir.path(), "cold");
    let before = hash_of(&cold, "u.use_it");

    write(dir.path(), "m.ply", &m("z"));
    write(dir.path(), "u.ply", &u("z"));
    let after = agree(dir.path(), "the resource label changed");
    assert_ne!(
        hash_of(&after, "u.use_it"),
        before,
        "a resource is part of the atom"
    );
}

#[test]
fn a_cache_mangled_mid_session_degrades_to_the_full_path_and_recovers() {
    let dir = corpus();
    agree(dir.path(), "cold");
    write(
        dir.path(),
        "leaf.ply",
        "pub fn one() -> Int = 2\npub fn two() -> Int = one() + one()\n",
    );
    agree(dir.path(), "edited");

    // Mangled in the payload rather than replaced: a valid header over undecodable entries is what a half-written append leaves.
    let data = dir.path().join(".ply-cache/frontend.dat");
    let mut bytes = fs::read(&data).unwrap();
    for byte in bytes.iter_mut().skip(64) {
        *byte ^= 0x5a;
    }
    fs::write(&data, &bytes).unwrap();
    agree(dir.path(), "the cache was mangled");
    agree(dir.path(), "and the run after that");
}

/// The shape a half-finished garbage collection would leave.
#[test]
fn fingerprints_without_their_interfaces_are_refused_rather_than_believed() {
    let dir = corpus();
    agree(dir.path(), "cold");

    // The index still names every fingerprint and interface; the file its offsets point into is gone.
    fs::remove_file(dir.path().join(".ply-cache/frontend.dat")).unwrap();

    agree(dir.path(), "fingerprints with no interfaces behind them");
    agree(dir.path(), "and the run after that");
}

#[test]
fn a_single_file_run_does_not_spoil_the_whole_project_run_after_it() {
    let dir = corpus();
    let mut store = Store::open(dir.path()).unwrap();
    let _ = driver::load_incremental(&dir.path().join("leaf.ply"), &mut store);
    store.flush().unwrap();
    drop(store);

    agree(dir.path(), "whole project after a single-file run");
    let store = Store::open(dir.path()).unwrap();
    assert!(
        store.sources_len() >= 3,
        "a single-file run must not have pruned the rest of the project"
    );
}

#[test]
fn two_stores_open_at_once_leave_a_cache_that_still_agrees() {
    let dir = corpus();
    let mut first = Store::open(dir.path()).unwrap();
    let mut second = Store::open(dir.path()).unwrap();

    driver::load_incremental(dir.path(), &mut first).unwrap();
    write(
        dir.path(),
        "leaf.ply",
        "pub fn one() -> Int = 3\npub fn two() -> Int = one() + one()\n",
    );
    driver::load_incremental(dir.path(), &mut second).unwrap();
    drop(first);
    drop(second);

    agree(dir.path(), "after two overlapping runs");
}

#[test]
fn a_nested_module_and_a_flat_one_sharing_a_prefix_agree() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.ply", "pub fn x() -> Int = 1\n");
    write(dir.path(), "a/b.ply", "pub fn y() -> Int = 2\n");
    let cold = agree(dir.path(), "cold");
    assert!(cold.hashes.defs.contains_key(&Symbol::new("a.x")));
    assert!(cold.hashes.defs.contains_key(&Symbol::new("a.b.y")));

    write(dir.path(), "a/b.ply", "pub fn y() -> Int = 3\n");
    agree(dir.path(), "the nested module changed");
}

#[test]
fn an_empty_file_appearing_and_disappearing_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    write(dir.path(), "empty.ply", "");
    agree(dir.path(), "an empty module appeared");
    write(dir.path(), "empty.ply", "pub fn now() -> Int = 1\n");
    agree(dir.path(), "and gained a definition");
    fs::remove_file(dir.path().join("empty.ply")).unwrap();
    agree(dir.path(), "and went away");
}

/// One store for every mutation: an invalidation is only ever wrong in some sequence of edits.
#[test]
fn a_long_session_over_the_example_corpus_agrees_at_every_step() {
    let dir = tempfile::tempdir().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for entry in fs::read_dir(&root).expect("the example corpus must be present") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "ply") {
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            write(dir.path(), &name, &fs::read_to_string(&path).unwrap());
        }
    }
    let start = snapshot(&agree(dir.path(), "step 0"));

    let clock = fs::read_to_string(dir.path().join("clock.ply")).unwrap();
    write(
        dir.path(),
        "clock.ply",
        &format!("{clock}\npub fn ticks() -> Int = 0\n"),
    );
    agree(dir.path(), "step 1: a definition appeared");

    write(dir.path(), "spare.ply", "pub fn spare() -> Int = 9\n");
    agree(dir.path(), "step 2: a module appeared");

    fs::rename(dir.path().join("spare.ply"), dir.path().join("kept.ply")).unwrap();
    agree(dir.path(), "step 3: it was renamed");

    fs::remove_file(dir.path().join("kept.ply")).unwrap();
    agree(dir.path(), "step 4: and deleted");

    write(dir.path(), "clock.ply", &clock);
    let end = snapshot(&agree(dir.path(), "step 5: back to where it started"));
    assert_eq!(
        start, end,
        "an undone session must land on the state it began in"
    );
}

/// Seeded, so a divergence is reproducible from the failure message alone.
#[test]
fn a_shuffle_of_module_states_agrees_at_every_step() {
    let variants: [(&str, [String; 3]); 3] = [
        (
            "leaf.ply",
            [
                LEAF.to_string(),
                format!("// reformatted\n\n{LEAF}"),
                "pub fn one() -> Int = 1\npub fn two() -> Int = one() + 1\n".to_string(),
            ],
        ),
        (
            "core.ply",
            [
                CORE.to_string(),
                CORE.replace("Money", "Cost"),
                CORE.replace("pub fn label(", "pub fn title("),
            ],
        ),
        (
            "shop.ply",
            [
                SHOP.to_string(),
                SHOP.replace("import core\n", "import core\nimport leaf\n"),
                SHOP.replace("acc + core::price(i)", "acc + core::price(i) + 0"),
            ],
        ),
    ];

    let dir = corpus();
    let mut state = [0usize; 3];
    let mut seed: u64 = 0xa11d_1700_0bad_f00d;
    for step in 0..60 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let file = (seed >> 33) as usize % variants.len();
        let pick = (seed >> 17) as usize % 3;
        if state[file] == pick {
            continue;
        }
        state[file] = pick;
        let (name, bodies) = &variants[file];
        write(dir.path(), name, &bodies[pick]);
        agree(
            dir.path(),
            &format!("shuffle step {step}: {name} -> {pick}"),
        );
    }
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn test_hashes(dir: &Path, extra: &[&str]) -> Vec<String> {
    let out = ply(dir)
        .arg("test")
        .arg("--json")
        .args(extra)
        .output()
        .unwrap();
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json emits one object");
    v["selection"]["tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| format!("{} {}", t["key"], t["hash"]))
        .collect()
}

/// The front end is serial, so the worker count must not reach a hash.
#[test]
fn the_worker_count_does_not_reach_a_test_hash() {
    let dir = corpus();
    let one = test_hashes(dir.path(), &["--jobs", "1"]);
    let many = test_hashes(dir.path(), &["--jobs", "10"]);
    assert_eq!(one, many);
    assert_eq!(one, test_hashes(dir.path(), &["--no-cache"]));
}

#[test]
fn a_relative_and_an_absolute_path_share_one_cache() {
    let dir = corpus();
    // The front-end cache is opened by the load, so nothing has to run for this to be about it.
    ply(dir.path())
        .args(["test", "--filter", "nonexistent"])
        .output()
        .unwrap();

    let out = ply(dir.path())
        .args(["test", "--filter", "nonexistent"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    // One cache under the project root either way: an absolute path must not open a second one.
    let caches: Vec<_> = walkdir(dir.path())
        .into_iter()
        .filter(|p| p.ends_with(".ply-cache"))
        .collect();
    assert_eq!(caches.len(), 1, "{caches:?}");
}

/// Every directory under `root`, so a second cache anywhere below it is visible.
fn walkdir(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.push(path.clone());
                stack.push(path);
            }
        }
    }
    out
}

#[test]
fn every_file_contributes_its_reference_graph() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "a.ply",
        "pub fn one() -> Int = 1\npub fn two() -> Int = one()\n",
    );

    let mut store = Store::open(dir.path()).unwrap();
    driver::load_incremental(dir.path(), &mut store).unwrap();
    drop(store);

    let mut store = Store::open(dir.path()).unwrap();
    let warm = driver::load_incremental(dir.path(), &mut store).unwrap();
    let full = driver::load_full(dir.path()).unwrap();

    assert_eq!(warm.hashes.deps, full.hashes.deps);
    assert_eq!(warm.hashes.closure, full.hashes.closure);
}

/// `base` performs from the start, so an edit below it moves the row and nothing else.
fn chain(base: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lib.ply", &chain_lib(base));
    write(
        dir.path(),
        "top.ply",
        "import lib\npub fn top(n: Int) -> Int = lib::mid(n) + 1\n",
    );
    dir
}

fn chain_lib(base: &str) -> String {
    format!(
        "pub effect log {{ write note[r](msg: String) -> Int }}\n\
         pub fn base(n: Int) -> Int = {base}\n\
         pub fn mid(n: Int) -> Int = base(n) + 1\n"
    )
}

/// `top`'s declared row permits where `mid` now performs, so an answer against a stale `mid` reports `E0302` for a program that checks.
#[test]
fn a_row_that_moves_under_a_caller_agrees() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lib.ply", &chain_lib(r#"log.note[a]("n") + n"#));
    write(
        dir.path(),
        "top.ply",
        "import lib\npub fn top(n: Int) -> Int / {lib::log.write[a]} = lib::mid(n) + 1\n",
    );
    agree(dir.path(), "cold");

    write(dir.path(), "lib.ply", &chain_lib(r#"log.note[b]("n") + n"#));
    write(
        dir.path(),
        "top.ply",
        "import lib\npub fn top(n: Int) -> Int / {lib::log.write[b]} = lib::mid(n) + 2\n",
    );
    agree(
        dir.path(),
        "the row moved under a caller checked in the same wave",
    );
}

#[test]
fn a_body_edit_that_moves_the_published_row_still_reaches_every_caller() {
    let dir = chain(r#"log.note[a]("n") + n"#);
    agree(dir.path(), "cold");

    write(dir.path(), "lib.ply", &chain_lib(r#"log.note[b]("n") + n"#));
    let after = agree(dir.path(), "the edited body performs elsewhere");

    assert_eq!(
        after.check.defs[&Symbol::new("top.top")]
            .footprint
            .to_string(),
        "{lib.log.note[b]}"
    );
}

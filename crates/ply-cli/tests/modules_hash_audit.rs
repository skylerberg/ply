//! Where content addressing under modules meets the incremental front end.
//!
//! The equivalence property is the front end's whole safety argument: for every
//! corpus and after every edit, the incremental path must produce
//! byte-identical `DefHash`es to a from-scratch check.

use ply_cli::driver;
use ply_cli::load::Loaded;
use ply_store::Store;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn snapshot(loaded: &Loaded) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, hash) in &loaded.hashes.defs {
        out.insert(format!("hash {name}"), hash.to_hex());
    }
    for (name, hash) in &loaded.hashes.decls {
        out.insert(format!("decl {name}"), hash.to_hex());
    }
    for (i, test) in loaded.check.tests.iter().enumerate() {
        let hash = loaded.hashes.tests.get(i).map(|h| h.to_hex()).unwrap_or_default();
        out.insert(format!("test {}", test.key), hash);
    }
    out
}

#[track_caller]
fn agree(dir: &Path, what: &str) {
    let mut store = Store::open(dir).expect("the cache directory must be creatable");
    let incremental = driver::load_incremental(dir, &mut store)
        .unwrap_or_else(|e| panic!("{what}: the incremental path failed: {:?}", codes(&e)));
    let full = driver::load_full(dir)
        .unwrap_or_else(|e| panic!("{what}: the full path failed: {:?}", codes(&e)));

    let a = snapshot(&full);
    let b = snapshot(&incremental);
    let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    let differences: Vec<String> = keys
        .into_iter()
        .filter(|key| a.get(*key) != b.get(*key))
        .map(|key| {
            format!("  {key}\n    full        {:?}\n    incremental {:?}", a.get(key), b.get(key))
        })
        .collect();
    assert!(
        differences.is_empty(),
        "{what}: the incremental path disagreed with a from-scratch check:\n{}",
        differences.join("\n")
    );
}

fn codes(e: &ply_cli::load::LoadError) -> Vec<String> {
    e.diagnostics.iter().map(|d| format!("{}: {}", d.code, d.message)).collect()
}

fn write(dir: &Path, name: &str, text: &str) {
    fs::write(dir.join(name), text).unwrap();
}

const DECLARER: &str = "\
pub effect db {
  read get[r](key: Int) -> Int
}
";

const PERFORMER: &str = "\
import core
pub fn touch(k: Int) -> Int / {core::db.read[cart]} = core::db.get[cart](k)
test \"touch is handled\" {
  assert_eq(handle touch(1) with { core::db.get[cart](k) -> k + 1, }, 2)
}
";

/// The shape that used to desynchronize the two front ends. A file that merely
/// *performs* an effect passes both of gate 1's checks when a new module
/// declares a look-alike — its bytes are unchanged and the declaration it
/// depends on still hashes the same — so it is skipped and keeps whatever it
/// was hashed under last time. Nothing about an effect's identity may therefore
/// depend on which other effects the program happens to declare.
#[test]
fn adding_a_look_alike_effect_keeps_the_two_front_ends_in_agreement() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "core.ply", DECLARER);
    write(dir.path(), "shop.ply", PERFORMER);
    agree(dir.path(), "cold");
    agree(dir.path(), "warm");

    write(dir.path(), "aaa.ply", "pub effect alpha {\n  read get[r](key: Int) -> Int\n}\n");
    agree(dir.path(), "after a new file declared a look-alike effect");
}

/// The control: the same corpus, the same warm store, and a new file whose
/// effect is *not* a look-alike. If this ever fails the cause is not the rank.
#[test]
fn adding_an_unrelated_effect_keeps_the_two_front_ends_in_agreement() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "core.ply", DECLARER);
    write(dir.path(), "shop.ply", PERFORMER);
    agree(dir.path(), "cold");
    agree(dir.path(), "warm");

    write(dir.path(), "aaa.ply", "pub effect alpha {\n  write put(key: Int) -> Int\n}\n");
    agree(dir.path(), "after a new file declared an unrelated effect");
}

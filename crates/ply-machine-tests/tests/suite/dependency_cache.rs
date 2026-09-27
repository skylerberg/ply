//! A dependency's own modules are cached, and under the package rather than the path.

use crate::fixture::write;
use ply_machine::driver;
use ply_machine::load::Loaded;
use ply_store::Store;
use std::collections::BTreeMap;
use std::path::Path;

fn manifest(name: &str, deps: &str) -> String {
    format!(
        "import std.pkg (Manifest)\nfn package() -> Manifest = {{name: \"{name}\", version: {{major: 0, minor: 0, patch: 1}}, prefix: None, runtime: {{major: 0, minor: 0, patch: 1}}, dependencies: [{deps}], entry: None}}\n"
    )
}

fn dep(name: &str, path: &str) -> String {
    format!(
        "{{name: \"{name}\", prefix: None, min: {{major: 0, minor: 0, patch: 1}}, source: Path(\"{path}\")}}"
    )
}

/// A root over a dependency, with the dependency's directory where the importer's path says.
fn layout(root: &Path, lib_path: &str) {
    write(root, "app/ply.pkg", &manifest("app", &dep("lib", lib_path)));
    write(
        root,
        "app/main.ply",
        "import lib.answer\nfn main() -> Int = answer::answer()\n",
    );
    write(root, "lib/ply.pkg", &manifest("lib", ""));
    write(root, "lib/answer.ply", "pub fn answer() -> Int = 42\n");
}

/// Every definition and test hash the load answered with, as a comparable value.
fn snapshot(loaded: &Loaded) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, hash) in &loaded.hashes.defs {
        out.insert(format!("hash {name}"), hash.to_hex());
    }
    for (i, test) in loaded.check.tests.iter().enumerate() {
        let hash = loaded
            .hashes
            .tests
            .get(i)
            .map(|h| h.to_hex())
            .unwrap_or_default();
        out.insert(format!("test {}", test.key), hash);
    }
    out
}

#[test]
fn a_dependency_module_is_filed_under_its_package_and_survives_the_package_moving() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    layout(root, "../lib");
    let app = root.join("app");

    let mut store = Store::open(&app).expect("a store");
    let before = driver::load_incremental(&app, &mut store).expect("the project loads");
    let file = root.join("lib/answer.ply");
    let filed = store
        .fingerprint(&file)
        .expect("a dependency's module is on record, not skipped for living outside the root");
    assert_eq!(
        filed.module, "lib.answer",
        "the file was filed under the name the front end gave it"
    );
    let key = store
        .source_keys()
        .into_iter()
        .find(|k| k.ends_with("/answer.ply"))
        .unwrap_or_else(|| {
            panic!(
                "no key names the dependency's module: {:?}",
                store.source_keys()
            )
        });
    assert!(
        !key.contains("lib/"),
        "the key carries the package's identity rather than where it happens to live: {key}"
    );
    let content = filed.content_hash;
    store.flush().expect("the store flushes");

    // The package is checked out somewhere else and the importer points at the new place. Its own
    // manifest is unchanged, so its identity is, and the entry filed under it is still the one for
    // this file — which is what makes the cache survive a checkout moving.
    std::fs::create_dir_all(root.join("vendor")).unwrap();
    std::fs::rename(root.join("lib"), root.join("vendor/lib")).unwrap();
    write(
        root,
        "app/ply.pkg",
        &manifest("app", &dep("lib", "../vendor/lib")),
    );

    let mut store = Store::open(&app).expect("a store");
    let after = driver::load_incremental(&app, &mut store).expect("the moved project loads");
    let moved = store
        .fingerprint(&root.join("vendor/lib/answer.ply"))
        .expect("the entry is found by the package it belongs to");
    assert_eq!(moved.content_hash, content);
    assert_eq!(moved.module, "lib.answer");
    assert_eq!(
        snapshot(&before),
        snapshot(&after),
        "the same program, checked from a different place, hashes the same"
    );

    // And a changed dependency is a different program: the entry is not trusted past its bytes.
    std::fs::write(
        root.join("vendor/lib/answer.ply"),
        "pub fn answer() -> Int = 43\n",
    )
    .unwrap();
    let mut store = Store::open(&app).expect("a store");
    let edited = driver::load_incremental(&app, &mut store).expect("the edited project loads");
    assert_ne!(
        snapshot(&before),
        snapshot(&edited),
        "the cache answered for bytes it did not see"
    );
}

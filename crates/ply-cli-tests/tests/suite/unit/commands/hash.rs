use ply_cli::commands::hash::*;
use ply_cli::load::{Loaded, load};
use ply_span::Symbol;
use ply_ty::HashOutput;
use serde_json::json;

fn write(dir: &std::path::Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn fixture(text: &str) -> (tempfile::TempDir, Loaded, HashOutput) {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "m.ply", text);
    let loaded = load(dir.path()).unwrap();
    let hashes = loaded.hashes().unwrap();
    (dir, loaded, hashes)
}

const SOURCE: &str = "fn one() -> Int = 1\n\
                      fn two() -> Int = one() + one()\n\
                      test \"two is two\" { assert_eq(two(), 2) }\n";

#[test]
fn every_definition_and_test_gets_a_full_hex_hash() {
    let (_dir, loaded, hashes) = fixture(SOURCE);
    let v = report_json(&loaded, &hashes, false);
    assert_eq!(v["definitions"].as_array().unwrap().len(), 2);
    assert_eq!(v["definitions"][0]["hash"].as_str().unwrap().len(), 64);
    assert_eq!(v["definitions"][0]["short"].as_str().unwrap().len(), 12);
    assert_eq!(v["tests"][0]["hash"].as_str().unwrap().len(), 64);
    assert!(v["definitions"][0].get("deps").is_none());
}

#[test]
fn every_entry_says_which_module_it_came_from_and_that_the_module_is_not_hashed() {
    let (_dir, loaded, hashes) = fixture(SOURCE);
    let v = report_json(&loaded, &hashes, false);
    assert_eq!(v["module_is_hashed"], false);
    assert_eq!(v["definitions"][0]["module"], "m");
    assert_eq!(v["definitions"][0]["name"], "m.one");
    assert_eq!(v["definitions"][0]["simple_name"], "one");
    assert_eq!(v["tests"][0]["module"], "m");
    assert_eq!(v["tests"][0]["key"], "m.two is two");
    assert_eq!(v["modules"][0]["name"], "m");
}

#[test]
fn deps_adds_the_graph_and_the_closure() {
    let (_dir, loaded, hashes) = fixture(SOURCE);
    let v = report_json(&loaded, &hashes, true);
    let two = v["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "m.two")
        .unwrap();
    assert_eq!(two["deps"], json!(["m.one"]));
    assert_eq!(two["closure"], json!(["m.one", "m.two"]));
    assert_eq!(
        v["tests"][0]["closure"],
        json!(["m.one", "m.two", "m.two is two"])
    );
}

fn def(hashes: &HashOutput, name: &str) -> ply_ty::DefHash {
    hashes.defs[&Symbol::new(name)]
}

#[test]
fn renaming_a_definition_changes_no_hash_in_the_report() {
    let (_dir, _l, before) = fixture(SOURCE);
    let (_dir2, _l2, after) = fixture(
        "fn uno() -> Int = 1\n\
         fn two() -> Int = uno() + uno()\n\
         test \"two is two\" { assert_eq(two(), 2) }\n",
    );
    assert_eq!(def(&before, "m.one"), def(&after, "m.uno"));
    assert_eq!(def(&before, "m.two"), def(&after, "m.two"));
    assert_eq!(before.tests, after.tests);
}

#[test]
fn editing_a_body_moves_that_hash_and_its_dependents() {
    let (_dir, _l, before) = fixture(SOURCE);
    let (_dir2, _l2, after) = fixture(
        "fn one() -> Int = 2\n\
         fn two() -> Int = one() + one()\n\
         test \"two is two\" { assert_eq(two(), 2) }\n",
    );
    assert_ne!(def(&before, "m.one"), def(&after, "m.one"));
    assert_ne!(def(&before, "m.two"), def(&after, "m.two"));
    assert_ne!(before.tests[0], after.tests[0]);
}

#[test]
fn moving_a_definition_between_modules_changes_no_hash_in_the_report() {
    let together = tempfile::tempdir().unwrap();
    write(
        together.path(),
        "app.ply",
        "fn one() -> Int = 1\n\
         fn two() -> Int = one() + one()\n\
         test \"two is two\" { assert_eq(two(), 2) }\n",
    );
    let l = load(together.path()).unwrap();
    let before = l.hashes().unwrap();

    let apart = tempfile::tempdir().unwrap();
    write(apart.path(), "lib.ply", "pub fn one() -> Int = 1\n");
    write(
        apart.path(),
        "app.ply",
        "import lib\n\
         fn two() -> Int = lib::one() + lib::one()\n\
         test \"two is two\" { assert_eq(two(), 2) }\n",
    );
    let l2 = load(apart.path()).unwrap();
    let after = l2.hashes().unwrap();

    assert_eq!(def(&before, "app.one"), def(&after, "lib.one"));
    assert_eq!(def(&before, "app.two"), def(&after, "app.two"));
    assert_eq!(before.tests, after.tests);
}

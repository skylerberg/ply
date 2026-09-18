use ply_cli::driver;
use ply_cli::load::{Loaded, load};
use ply_span::{Symbol, codes};
use ply_store::Store;
use std::fs;
use std::path::Path;

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

#[track_caller]
fn incremental(dir: &Path) -> Loaded {
    let mut store = Store::open(dir).expect("the cache directory is writable");
    driver::load_incremental(dir, &mut store).expect("the corpus checks")
}

/// An imported but unused name is in no `deps` entry, so deleting it leaves every hash the importer names untouched.
#[test]
fn deleting_an_unused_selectively_imported_name_is_reported_not_skipped_past() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lib.ply",
        "pub fn used() -> Int = 1\npub fn spare() -> Int = 2\n",
    );
    write(
        dir.path(),
        "app.ply",
        "import lib (used, spare)\nfn go() -> Int = used()\n",
    );

    incremental(dir.path());
    incremental(dir.path());

    write(dir.path(), "lib.ply", "pub fn used() -> Int = 1\n");
    let mut store = Store::open(dir.path()).unwrap();
    let err = driver::load_incremental(dir.path(), &mut store)
        .expect_err("an import of a deleted name must be an error, not a skipped file");
    assert!(
        err.diagnostics
            .iter()
            .any(|d| d.code == codes::UNKNOWN_NAME),
        "codes: {:?}",
        err.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

/// Inference walks modules dependency-first, so check order depends on what the cache held.
#[test]
fn the_published_order_is_the_same_warm_as_cold() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        "import lib\n\
         pub effect audit { write note[log](m: Int) -> Unit }\n\
         pub type Wrapped = | Wrap(Int) | Empty\n\
         fn second() -> Int = lib::base()\n\
         fn first() -> Int = second()\n",
    );
    write(
        dir.path(),
        "lib.ply",
        "pub effect db { read get[t](k: Int) -> Int }\n\
         pub type Row = | Cell(Int)\n\
         pub fn base() -> Int = 1\n",
    );

    incremental(dir.path());
    let warm = incremental(dir.path());

    let full = load(dir.path()).unwrap();
    let keys = |l: &Loaded| {
        (
            l.check.defs.keys().cloned().collect::<Vec<Symbol>>(),
            l.check.effects.keys().cloned().collect::<Vec<Symbol>>(),
            l.check.ctors.keys().cloned().collect::<Vec<Symbol>>(),
            l.check.modules.keys().cloned().collect::<Vec<Symbol>>(),
        )
    };
    assert_eq!(keys(&warm), keys(&full));

    // The run's own order: files sorted, then each file's items as written.
    let defs: Vec<&str> = full.check.defs.keys().map(|k| k.as_str()).collect();
    assert_eq!(defs, ["app.second", "app.first", "lib.base"]);
}

#[test]
fn a_result_cache_write_failure_is_not_blamed_on_the_front_end() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "m.ply", "pub fn f() -> Int = 1\n");
    // `rename` cannot replace a directory, so only the result cache's atomic write fails.
    fs::create_dir_all(dir.path().join(".ply-cache/results.json")).unwrap();

    let mut store = Store::open(dir.path()).unwrap();
    let loaded = driver::load_incremental(dir.path(), &mut store)
        .expect("an unwritable cache never fails a compile");

    let warning = loaded
        .frontend
        .warnings
        .first()
        .expect("an unwritable cache has to be reported");
    assert_eq!(warning.code, codes::CACHE_UNREADABLE);
    assert!(
        warning.message.contains("result cache"),
        "the failing cache has to be named: {}",
        warning.message
    );
    assert!(
        !warning.message.contains("front-end cache"),
        "the front-end cache is not what failed: {}",
        warning.message
    );
}

#[test]
fn three_operations_sharing_one_atom_are_three_reachable_clauses() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        "import std.net (net)\n\
         pub fn touch(c: Int) -> Int / {net.write[conn]} = {\n\
           let _ = net.recv[conn](c, 16, 1000);\n\
           0\n\
         }\n\
         pub fn boom() -> Int = nope(1)\n\
         test \"three operations, one atom\" {\n\
           handle { assert_eq(touch(3), 0) } with {\n\
             net.recv[conn](c, m, t) -> Some(b\"\"),\n\
             net.send[conn](c, p, t) -> Some(bytes_len(p)),\n\
             net.close[conn](c) -> (),\n\
           }\n\
         }\n",
    );

    let err = load(dir.path()).expect_err("`nope` is unknown");
    let duplicates: Vec<&str> = err
        .diagnostics
        .iter()
        .filter(|d| d.code == codes::DUPLICATE_DEFINITION)
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        duplicates.is_empty(),
        "no clause here is unreachable: {duplicates:?}"
    );
    assert_eq!(
        err.diagnostics
            .iter()
            .filter(|d| d.severity == ply_span::Severity::Error)
            .count(),
        1,
        "only `nope` is an error"
    );
}

/// The warning names the operation, not the atom: the atom is not what the second clause lost to.
#[test]
fn the_same_operation_handled_twice_is_still_reported() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        "import std.net (net)\n\
         pub fn touch(c: Int) -> Int / {net.write[conn]} = {\n\
           let _ = net.recv[conn](c, 16, 1000);\n\
           0\n\
         }\n\
         pub fn boom() -> Int = nope(1)\n\
         test \"one operation, twice\" {\n\
           handle { assert_eq(touch(3), 0) } with {\n\
             net.recv[conn](c, m, t) -> Some(b\"\"),\n\
             net.recv[conn](c, m, t) -> Some(b\"x\"),\n\
           }\n\
         }\n",
    );

    let err = load(dir.path()).expect_err("`nope` is unknown");
    let d = err
        .diagnostics
        .iter()
        .find(|d| d.code == codes::DUPLICATE_DEFINITION)
        .expect("the second clause is unreachable");
    assert!(
        d.message.contains("net.recv[conn]"),
        "the operation is what was duplicated: {}",
        d.message
    );
}

#[test]
fn a_failure_is_placed_in_the_text_that_ran_after_its_definition_moved() {
    let dir = tempfile::tempdir().unwrap();
    let run = |text: &str| -> serde_json::Value {
        write(dir.path(), "m.ply", text);
        let out = assert_cmd::Command::cargo_bin("ply")
            .unwrap()
            .args(["--color", "never", "run", "m.ply", "--json"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
    };
    // The hash of `main` is the same in both texts; only where it sits moved.
    let below = run("\n\n\nfn main() -> Int = 1 / 0\n");
    assert_eq!(
        below["diagnostics"][0]["labels"][0]["start"]["line"], 4,
        "{below}"
    );
    let above = run("fn main() -> Int = 1 / 0\n");
    assert_eq!(
        above["diagnostics"][0]["labels"][0]["start"]["line"], 1,
        "{above}"
    );
}

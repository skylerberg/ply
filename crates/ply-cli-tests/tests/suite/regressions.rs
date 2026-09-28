use crate::harness::{ply, write};
use ply_machine::driver;
use ply_machine::load::{Loaded, load};
use ply_span::{Symbol, codes};
use ply_store::Store;
use std::fs;
use std::path::Path;

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
         pub fn touch(c: Int) -> Int / {net.recv[conn]} = {\n\
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
         pub fn touch(c: Int) -> Int / {net.recv[conn]} = {\n\
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
        let out = ply(dir.path())
            .args(["run", "m.ply", "--json"])
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

/// A constructor is the one its own module declares, even when another imported module declares a
/// constructor of the same name and is imported first.
///
/// `tag_of` resolves a name the import list did not qualify by searching the module that *holds*
/// it, and the search took the first import whose module declared the simple name — so `Nondet`
/// here resolved to `second`, whose `Skipped` has a `Nondet` of its own, and `first`'s arm never
/// matched. The name has to come from the import that bound it, not from any module that happens to
/// declare it.
#[test]
fn a_constructor_name_is_the_module_it_was_imported_from_and_not_any_module_that_declares_it() {
    let dir = crate::harness::project_files(&[
        (
            "first.ply",
            "pub type Colour = | Nondet | Other\n\npub fn code(c: Colour) -> Int =\n  match c {\n    Nondet -> 11,\n    Other -> 12,\n  }\n",
        ),
        (
            "second.ply",
            "pub type Skipped = | Nondet | Panicked\n\npub fn skipped_code(s: Skipped) -> Int =\n  match s {\n    Nondet -> 21,\n    Panicked -> 22,\n  }\n",
        ),
        (
            "use.ply",
            "// `second` comes first and declares a `Nondet` of its own.\nimport second (Panicked, Skipped, skipped_code)\nimport first (Colour, Nondet, Other, code)\n\ntest \"the imported constructor is the one that was imported\" {\n  assert_eq(code(Nondet), 11);\n  assert_eq(code(Other), 12)\n}\n",
        ),
    ]);
    let out = ply(dir.path()).args(["test", "-j", "1"]).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    assert!(text.contains("1 passed"), "{text}");
}

/// An `Int` at or beyond `2^62` does not fit an immediate and is boxed, and a self tail call used
/// to release that boxed word on every restart and again on the way out: the compiled tier aborted
/// in `heap::release_last`, one iteration in, because the emitter bound the parameter local with
/// no count and the release dropped it (card `6298c4dd`). The whole pipeline is the point here —
/// the front end, the emitted C, the launcher and the exit code — so it is a `ply run` and not a
/// unit test.
#[test]
fn a_boxed_int_passes_through_a_self_tail_call() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "m.ply",
        "fn countdown(n: Int, k: Int) -> Int = if k == 0 { n } else { countdown(n, k - 1) }\n\
         pub fn main() -> Unit {\n\
           assert_eq(countdown(4611686018427387904, 1), 4611686018427387904);\n\
           assert_eq(countdown(4611686018427387904, 1000), 4611686018427387904);\n\
           assert_eq(countdown(0 - 4611686018427387905, 1), 0 - 4611686018427387905)\n\
         }\n",
    );
    let out = ply(dir.path())
        .args(["run", "--color", "never"])
        .output()
        .unwrap();
    let said = || {
        format!(
            "exit {:?}\nstdout: {}\nstderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };
    assert_eq!(out.status.code(), Some(0), "{}", said());
}

//! Regressions for defects that survived a previous round of audits.
//!
//! Each one is a case where a run produced a wrong or unstable *answer* while
//! looking healthy: a skip that swallowed an error, an ordering that moved with
//! the cache, and a warning that named the wrong file.

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

#[track_caller]
fn file_report(loaded: &Loaded, name: &str) -> ply_cli::driver::FileReport {
    loaded
        .frontend
        .files
        .iter()
        .find(|f| f.path.ends_with(name))
        .unwrap_or_else(|| panic!("{name} was not reported"))
        .clone()
}

/// A name a file imports but never uses appears in no `deps` entry — nothing
/// references it — so deleting it downstream leaves the importer's bytes and
/// every hash it names untouched. Only the declaring module's export digest
/// moves, and gate 1 has to be reading it, or the dangling import is never
/// reported and the program silently "compiles".
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
    let warm = incremental(dir.path());
    assert!(
        !file_report(&warm, "app.ply").parsed,
        "the fixture is only interesting while the importer skips"
    );

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

/// The mechanism on its own, with the program still compiling either way: a new
/// export in `lib` reaches nothing in `app`, so every other gate-1 condition
/// still holds and only the digest can refuse the skip.
#[test]
fn a_changed_export_set_refuses_an_importers_skip_by_digest() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lib.ply", "pub fn used() -> Int = 1\n");
    write(
        dir.path(),
        "app.ply",
        "import lib\nfn go() -> Int = lib::used()\n",
    );

    incremental(dir.path());
    let warm = incremental(dir.path());
    assert!(!file_report(&warm, "app.ply").parsed);

    write(
        dir.path(),
        "lib.ply",
        "pub fn used() -> Int = 1\npub fn extra() -> Int = 9\n",
    );
    let after = incremental(dir.path());
    let app = file_report(&after, "app.ply");
    assert!(app.parsed, "the importer must be re-parsed");
    assert_eq!(
        app.refusal.describe(),
        "import `lib` changed",
        "the digest is what refused it, not some other condition"
    );
}

/// A module nothing parsed touches keeps skipping: the digest may not be a
/// licence to re-parse the world on any edit anywhere.
#[test]
fn an_edit_in_an_unimported_module_leaves_the_digest_alone() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lib.ply", "pub fn used() -> Int = 1\n");
    write(
        dir.path(),
        "app.ply",
        "import lib\nfn go() -> Int = lib::used()\n",
    );
    write(dir.path(), "lone.ply", "fn lone() -> Int = 1\n");

    incremental(dir.path());
    incremental(dir.path());

    write(dir.path(), "lone.ply", "fn lone() -> Int = 2\n");
    let after = incremental(dir.path());
    assert!(!file_report(&after, "app.ply").parsed);
    assert!(!file_report(&after, "lib.ply").parsed);
}

/// Inference walks modules dependency-first and never walks a skipped one at
/// all, so a `CheckOutput` assembled in check order lists a project's
/// definitions differently depending on what the cache held. Two runs of
/// `ply check --types` over one unchanged tree would then diff against each
/// other.
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
    assert!(warm.frontend.skipped() > 0, "gate 1 never fired");

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

    // And it is the run's own order, so a reader can predict it: files sorted,
    // then each file's items as written.
    let defs: Vec<&str> = full.check.defs.keys().map(|k| k.as_str()).collect();
    assert_eq!(defs, ["app.second", "app.first", "lib.base"]);
}

/// `Store::flush` writes the result cache and the front-end cache, and the
/// driver used to label either failure as the front end's. A person told the
/// wrong cache failed clears the wrong one and sees the same warning again.
#[test]
fn a_result_cache_write_failure_is_not_blamed_on_the_front_end() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "m.ply", "fn f() -> Int = 1\n");
    // A directory cannot be replaced by `rename`, so the result cache's atomic
    // write fails while everything else about the run is fine.
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

/// `Stack::find_handler` selects a clause by `(effect, operation, resource)`.
/// The duplicate-clause check was keyed on the *atom*, so a handler for
/// `net.recv[conn]`, `net.send[conn]` and `net.close[conn]` — one atom, three
/// operations, which is every serve loop in `std.http` — reported two of its
/// three clauses unreachable. It was invisible on a clean run only because a
/// successful check drops its warnings, so a user's unrelated typo turned into
/// 22 warnings blaming the shipped stdlib.
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

/// The other half: the same operation twice really is unreachable, and the
/// warning names the operation rather than the atom, because the atom is not
/// what the second clause lost to.
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

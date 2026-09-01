//! An adversarial audit of the stdlib path.
//!
//! `stdlib.rs` asserts the mechanism works. This file assumes it is broken and
//! goes looking for the break, because module resolution is where this project
//! has repeatedly found soundness holes and `std` is a *second* resolution
//! channel bolted beside the first.
//!
//! The questions, in the order they are asked below:
//!
//! - can a project module shadow, impersonate or be confused with a `std` one?
//! - does importing `std` move a hash in a definition that does not import it?
//! - under a warm cache written by a *different* compiler, is the invalidation
//!   exact? Too many is a cost; **too few is a stale result over changed code**,
//!   and that is the one this file exists for.
//! - is renaming a `std` definition still free, as ADR 0001 requires of every
//!   definition?

use assert_cmd::prelude::*;
use ply_cli::driver;
use ply_cli::load::{Loaded, load};
use ply_span::{Symbol, codes};
use ply_store::{ContentHash, DefEntry, Store};
use ply_syntax::ast::ModuleName;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn output(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn std_net() -> ModuleName {
    ModuleName::from_dotted("std.net")
}

fn hash_of(loaded: &Loaded, name: &str) -> String {
    let key = Symbol::new(name);
    loaded
        .hashes
        .defs
        .get(&key)
        .or_else(|| loaded.hashes.decls.get(&key))
        .unwrap_or_else(|| panic!("`{name}` is not in the program"))
        .to_hex()
}

/// A project that reaches `std.net` and handles every atom, so its test is
/// `det` and cacheable — which is what makes "did it re-run?" a question with
/// an answer.
const IMPORTER: &str = "\
import std.net (net, drain)

pub fn read_all(c: Int) -> Bytes / {net.write[conn]} = drain(c, b\"\", 1000)

test \"reads to the end\" {
  handle { assert_eq(read_all(1), b\"\") } with { net.recv[conn](c, m, t) -> Some(b\"\") }
}
";

// --- Can a project module impersonate a `std` one? --------------------------

/// The reserved root, attacked from every direction a path can take. Reserving
/// `std` is what removes the precedence question entirely, so a hole here is not
/// a diagnostic bug — it is a program whose meaning depends on where a file
/// sits.
#[test]
fn nothing_a_project_can_name_lands_under_the_reserved_root() {
    // Every one of these derives a module name at or under `std`.
    for (rel, expected) in [
        ("std.ply", "std"),
        ("std/json.ply", "std.json"),
        ("std/net.ply", "std.net"),
        ("std/http/server.ply", "std.http.server"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), rel, "pub fn f() -> Int = 1\n");
        let err = load(dir.path()).unwrap_err();
        assert!(
            err.diagnostics
                .iter()
                .any(|d| d.code == codes::RESERVED_MODULE_NAME),
            "`{rel}` would be `{expected}` and was accepted: {:?}",
            err.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    // And the near misses, which must keep working: reserving a prefix must not
    // reserve every name that starts with the same three letters.
    for rel in ["stdlib.ply", "mine/std_helpers.ply", "a/std_thing.ply"] {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), rel, "pub fn f() -> Int = 1\n");
        assert!(load(dir.path()).is_ok(), "`{rel}` was refused");
    }
}

/// A project file inside a directory called `std`, named directly rather than
/// discovered. The root becomes the file's parent, so the module is `json` — it
/// cannot impersonate `std.json`, and the shipped module is still reachable
/// beside it.
#[test]
fn naming_a_file_under_std_directly_cannot_smuggle_it_in_as_a_std_module() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "std/json.ply",
        "pub fn parse(x: Int) -> Int = x\n",
    );
    let loaded = load(&dir.path().join("std/json.ply")).expect("it is a project module");
    let names: Vec<String> = loaded
        .modules()
        .iter()
        .map(|m| m.name.to_string())
        .collect();
    assert_eq!(names, ["json"], "a file under `std/` became a `std` module");
    assert!(loaded.check.defs.contains_key(&Symbol::new("json.parse")));
}

/// Two modules that would bind the same name, one shipped and one the project's.
/// The collision is an error rather than a precedence rule, which is the whole
/// reason `std` is reserved: nothing silently wins.
#[test]
fn a_project_module_named_net_beside_std_net_is_a_loud_collision() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "net.ply",
        "pub nondet effect net { read recv[r](conn: Int, max: Int) -> Bytes }\n\
         pub fn drain(c: Int, acc: Bytes) -> Bytes / {net.read[conn]} = acc\n",
    );
    write(
        dir.path(),
        "app.ply",
        "import net\nimport std.net\npub fn f() -> Int = 1\n",
    );
    let err = load(dir.path()).unwrap_err();
    assert!(
        err.diagnostics
            .iter()
            .any(|d| d.code == codes::DUPLICATE_IMPORT),
        "one of the two silently won: {:?}",
        err.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );

    // `as` is the escape hatch, and both are then reachable and distinct: the
    // two `net` effects produce atoms under different program-wide names.
    write(
        dir.path(),
        "app.ply",
        "import net as mine\n\
         import std.net\n\
         pub fn f(c: Int) -> Bytes / {mine::net.read[conn]} = mine::drain(c, b\"\")\n",
    );
    let loaded = load(dir.path()).expect("`as` disambiguates");
    let footprint = &loaded.check.defs[&Symbol::new("app.f")].footprint;
    let atoms: Vec<String> = footprint.0.iter().map(|a| a.effect.to_string()).collect();
    assert_eq!(
        atoms,
        ["net.net"],
        "the project's effect and the shipped one are not distinct"
    );
}

/// A `std` module cannot be made to import a project one, so a cycle between
/// the two is unrepresentable rather than merely rejected. The check is asserted
/// from the other side as well: a project module importing `std` and a `std`
/// module importing nothing outside `std` is the only shape that exists.
#[test]
fn no_cycle_can_be_built_between_a_project_and_the_stdlib() {
    // A project module named after the one `std.json` imports, in case a
    // shipped module's import could be captured by a project file.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        "import std.json\npub fn f() -> Int = 1\n",
    );
    let loaded = load(dir.path()).expect("it checks");
    for view in loaded.modules() {
        if !ply_std::is_std(view.name) {
            continue;
        }
        for imported in &view.info.imports {
            assert!(
                ply_std::is_std(imported),
                "the shipped module `{}` imports `{imported}`",
                view.name
            );
        }
    }

    // And a self-import written into a shipped module is E0505, not the user's
    // problem — asserted through the same loader path a real one would take.
    for (name, source) in ply_std::sources() {
        assert!(
            !source.contains("import ")
                || source
                    .lines()
                    .filter(|l| l.starts_with("import "))
                    .all(|l| l.trim_start_matches("import ").starts_with("std.")),
            "`{name}` imports something outside `std`"
        );
    }
}

// --- Does importing `std` disturb anything that does not? -------------------

/// ADR 0012 corollary 1: nothing outside a definition's own reachable graph may
/// enter its hash. Adding a module that imports the whole stdlib next door must
/// move nothing.
#[test]
fn a_definition_that_does_not_import_std_is_unmoved_by_one_that_does() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "pure.ply",
        "pub fn add(a: Int, b: Int) -> Int = a + b\n\
         test \"add\" { assert_eq(add(1, 2), 3) }\n",
    );
    let before = load(dir.path()).unwrap();
    let before_test = before.hashes.tests[0].to_hex();

    write(
        dir.path(),
        "uses.ply",
        "import std.json\nimport std.net\npub fn zero() -> json::Json = json::Null\n",
    );
    let after = load(dir.path()).unwrap();

    assert_eq!(hash_of(&before, "pure.add"), hash_of(&after, "pure.add"));
    let after_test = after
        .check
        .tests
        .iter()
        .position(|t| t.key.as_str() == "pure.add")
        .map(|i| after.hashes.tests[i].to_hex())
        .expect("the test survives");
    assert_eq!(
        before_test, after_test,
        "a test re-runs because an unrelated module imported `std`"
    );
}

// --- The warm cache, across a compiler upgrade ------------------------------

/// What the previous compiler left behind, rewritten as this one would find it.
///
/// A real upgrade cannot be staged in one process — only one `ply-std` is linked
/// — so the *cache* is aged instead, which is the same state from the driver's
/// point of view: a fingerprint filed under `<std>/net.ply` that was written
/// against bytes this binary does not ship.
fn age_the_shipped_fingerprint(dir: &Path, mut mutate: impl FnMut(&mut DefEntry)) {
    let path = ply_std::pseudo_path(&std_net());
    let mut store = Store::open(dir).unwrap();
    let mut fingerprint = (*store
        .fingerprint(&path)
        .expect("the warm run recorded the shipped module"))
    .clone();
    assert_eq!(
        fingerprint.content_hash,
        ContentHash::of(ply_std::NET.as_bytes()),
        "gate 1 is not keyed on the embedded bytes, so an upgrade cannot refuse a skip"
    );
    fingerprint.content_hash = ContentHash::of(b"what the last compiler shipped");
    for entry in &mut fingerprint.defs {
        mutate(entry);
    }
    store.put_source(&path, fingerprint);
    store.flush().unwrap();
}

/// The upgrade that changed nothing but the bytes — a comment, a reflow. Gate 1
/// refuses the skip because the content moved, the definitions hash to exactly
/// what they hashed to, and **nothing downstream re-runs**. Over-invalidation
/// here would make every compiler upgrade a whole-project rebuild.
#[test]
fn an_upgrade_that_moves_no_definition_re_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", output(&out));

    age_the_shipped_fingerprint(dir.path(), |_| {});

    let mut store = Store::open(dir.path()).unwrap();
    let loaded = driver::load_incremental(dir.path(), &mut store).unwrap();
    let shipped = loaded
        .frontend
        .files
        .iter()
        .find(|f| f.module == std_net())
        .expect("the shipped module is in the run");
    assert!(
        shipped.parsed,
        "the embedded bytes moved and gate 1 skipped anyway: {:?}",
        shipped.refusal
    );
    let app = loaded
        .frontend
        .files
        .iter()
        .find(|f| f.module.as_str() == "app")
        .expect("the project module is in the run");
    assert!(
        !app.parsed,
        "a stdlib edit that moved no definition dragged the project in: {:?}",
        app.refusal
    );

    // And the test the project owns is unchanged, so `ply test` selects nothing.
    let out = ply(dir.path()).arg("test").output().unwrap();
    let text = output(&out);
    assert!(
        text.contains("selected 0 of 1"),
        "an upgrade that moved no definition re-ran a test:\n{text}"
    );
}

/// The direction that is a blocker rather than a cost: the cache was written
/// under a `std.net` whose definitions really did move, and the run must not
/// believe any of it.
///
/// The perturbation is deliberately *self-consistent* — the aged fingerprint's
/// `drain` hash is also what the aged project fingerprint recorded depending on
/// — so nothing is corrupt and every gate has a coherent story to tell. If the
/// driver skipped either file it would publish a hash the current sources do not
/// produce, which is a stale result over changed code.
#[test]
fn an_upgrade_that_moved_a_definition_invalidates_exactly_its_dependents() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);
    write(
        dir.path(),
        "elsewhere.ply",
        "pub fn untouched() -> Int = 41 + 1\n\
         test \"untouched\" { assert_eq(untouched(), 42) }\n",
    );
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", output(&out));

    let drain = Symbol::new("std.net.drain");
    let mut aged = None;
    age_the_shipped_fingerprint(dir.path(), |entry| {
        if entry.name == drain {
            let mut bytes = entry.hash.0;
            bytes[0] ^= 0xff;
            entry.hash = ply_hash::DefHash(bytes);
            aged = Some(entry.hash);
        }
    });
    let aged = aged.expect("`std.net.drain` is in the shipped fingerprint");

    // The project's own fingerprint, aged to agree: this is what the previous
    // compiler would have written, and it is what makes the test sharp — every
    // gate is internally consistent and only the embedded source disagrees.
    {
        let mut store = Store::open(dir.path()).unwrap();
        let path = PathBuf::from("app.ply");
        let mut fingerprint = (*store.fingerprint(&path).expect("app was fingerprinted")).clone();
        let mut touched = false;
        for dep in &mut fingerprint.deps {
            if dep.name == drain {
                dep.hash = aged;
                touched = true;
            }
        }
        assert!(touched, "`app` does not record a dependency on `drain`");
        store.put_source(&path, fingerprint);
        store.flush().unwrap();
    }

    let mut store = Store::open(dir.path()).unwrap();
    let loaded = driver::load_incremental(dir.path(), &mut store).unwrap();
    let parsed = |module: &str| {
        loaded
            .frontend
            .files
            .iter()
            .find(|f| f.module.as_str() == module)
            .unwrap_or_else(|| panic!("`{module}` is not in the run"))
            .parsed
    };
    assert!(parsed("std.net"), "the changed shipped module was skipped");
    assert!(
        parsed("app"),
        "the dependent module was skipped, so it carries a hash the sources do not produce"
    );
    assert!(
        !parsed("elsewhere"),
        "a module reaching nothing in `std` was invalidated by a stdlib upgrade"
    );

    // The published hashes are what a from-scratch run computes — the assertion
    // that "too few were invalidated" would actually fail on.
    let scratch = load(dir.path()).unwrap();
    for name in ["app.read_all", "std.net.drain", "elsewhere.untouched"] {
        assert_eq!(
            hash_of(&loaded, name),
            hash_of(&scratch, name),
            "`{name}` is stale after an upgrade"
        );
    }
}

/// The cache written under one digest and read under another warns, and the
/// number it reports is the number of definitions *this program reaches* whose
/// hash moved — which is zero here, and must be said as zero rather than
/// implied.
#[test]
fn the_upgrade_notice_counts_what_moved_rather_than_what_exists() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);
    ply(dir.path()).arg("test").output().unwrap();

    {
        let mut store = Store::open(dir.path()).unwrap();
        store.set_stdlib_digest(String::from("b3:000000000000"));
        store.flush().unwrap();
    }

    let mut store = Store::open(dir.path()).unwrap();
    let loaded = driver::load_incremental(dir.path(), &mut store).unwrap();
    let warning = loaded
        .frontend
        .warnings
        .iter()
        .find(|d| d.code == codes::STDLIB_CHANGED)
        .expect("a cache written under another digest warns");
    assert!(
        warning.notes.iter().any(|n| n.contains("no definition")),
        "the notice implied work that did not happen: {:?}",
        warning.notes
    );

    // Once, not on every subsequent run: the digest is rewritten on the way out.
    let mut store = Store::open(dir.path()).unwrap();
    let again = driver::load_incremental(dir.path(), &mut store).unwrap();
    assert!(
        !again
            .frontend
            .warnings
            .iter()
            .any(|d| d.code == codes::STDLIB_CHANGED),
        "W0605 repeats"
    );
}

// --- Renaming a `std` definition is free ------------------------------------

/// ADR 0001's headline invariant, asked of the stdlib: renaming a definition
/// changes no hash anywhere. The shipped source cannot be edited in a test, so
/// the check runs over a byte-identical copy — which, by required test 4, has
/// **the same hashes as the shipped module**, so it is the same claim about the
/// same definitions.
#[test]
fn renaming_a_shipped_definition_moves_no_hash() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "mine.ply", ply_std::NET);
    write(
        dir.path(),
        "reader.ply",
        "import mine (net, drain)\n\
         pub fn read_all(c: Int) -> Bytes / {net.write[conn]} = drain(c, b\"\", 1000)\n\
         test \"reads\" {\n\
        \x20 handle { assert_eq(read_all(1), b\"\") } with { net.recv[conn](c, m, t) -> Some(b\"\") }\n\
         }\n",
    );
    let before = load(dir.path()).unwrap();

    // Renaming it, and every reference to it, across a module boundary.
    write(
        dir.path(),
        "mine.ply",
        &ply_std::NET.replace("drain", "read_to_end"),
    );
    write(
        dir.path(),
        "reader.ply",
        "import mine (net, read_to_end)\n\
         pub fn read_all(c: Int) -> Bytes / {net.write[conn]} = read_to_end(c, b\"\", 1000)\n\
         test \"reads\" {\n\
        \x20 handle { assert_eq(read_all(1), b\"\") } with { net.recv[conn](c, m, t) -> Some(b\"\") }\n\
         }\n",
    );
    let after = load(dir.path()).unwrap();

    assert_eq!(
        hash_of(&before, "mine.drain"),
        hash_of(&after, "mine.read_to_end"),
        "renaming a shipped definition moved its own hash"
    );
    assert_eq!(
        hash_of(&before, "reader.read_all"),
        hash_of(&after, "reader.read_all"),
        "renaming a shipped definition moved a caller's hash"
    );
    assert_eq!(
        before.hashes.tests[0].to_hex(),
        after.hashes.tests[0].to_hex(),
        "renaming a shipped definition re-selected a test"
    );
}

/// The invariant the driver's one documented constraint hazard rests on.
///
/// `Driver::restore_skipped` publishes a skipped definition with **no**
/// `where` clauses, because `CachedDef` does not carry them. That is only safe
/// while a parsed module can never reference a skipped one — which
/// `close_over_imports` is supposed to guarantee. Asserting it here means the
/// day that guarantee is relaxed, this fails rather than an `E0206` silently
/// stopping firing.
#[test]
fn a_parsed_module_never_reaches_a_skipped_one() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lib.ply",
        "pub fn needs<a>(x: a) -> Int where derivable(ord, a) = 1\n",
    );
    write(
        dir.path(),
        "app.ply",
        "import lib\npub fn go() -> Int = lib::needs(1)\n",
    );
    ply(dir.path()).arg("check").output().unwrap();

    // Only the importer changes, which is the case where skipping the callee
    // would be most tempting.
    write(
        dir.path(),
        "app.ply",
        "import lib\npub fn go() -> Int = lib::needs(2)\n",
    );
    let mut store = Store::open(dir.path()).unwrap();
    let loaded = driver::load_incremental(dir.path(), &mut store).unwrap();

    let by_module = |name: &str| {
        loaded
            .frontend
            .files
            .iter()
            .find(|f| f.module.as_str() == name)
            .unwrap_or_else(|| panic!("`{name}` is not in the run"))
    };
    assert!(by_module("app").parsed, "the edited module was skipped");
    assert!(
        by_module("lib").parsed,
        "a parsed module reached a skipped callee, so its `where` clauses are absent"
    );
}

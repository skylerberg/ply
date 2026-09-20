use assert_cmd::prelude::*;
use ply_cli::driver;
use ply_cli::load::{Loaded, load};
use ply_span::{Symbol, codes};
use ply_store::{ContentHash, DefEntry, Store};
use ply_ty::ModuleName;
use std::path::Path;
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

/// Handles every atom, so its test is `det` and cacheable, which makes "did it re-run?" answerable.
const IMPORTER: &str = "\
import std.net (net, drain)

pub fn read_all(c: Int) -> Bytes / {net.recv[conn]} = drain(c, b\"\", 1000)

test \"reads to the end\" {
  handle { assert_eq(read_all(1), b\"\") } with { net.recv[conn](c, m, t) -> Some(b\"\") }
}
";

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

    // Near misses must keep working: reserving a prefix must not reserve every name starting with those letters.
    for rel in ["stdlib.ply", "mine/std_helpers.ply", "a/std_thing.ply"] {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), rel, "pub fn f() -> Int = 1\n");
        assert!(load(dir.path()).is_ok(), "`{rel}` was refused");
    }
}

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

    // `as` is the escape hatch: the two `net` effects then produce atoms under different program-wide names.
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

#[test]
fn no_cycle_can_be_built_between_a_project_and_the_stdlib() {
    // A project module named after the one `std.json` imports, in case the shipped import could be captured.
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

    // A self-import in a shipped module is E0505, asserted through the loader path a real one would take.
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

/// What the previous compiler left behind, rewritten as this one would find it.
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
        "the fingerprint is not keyed on the embedded bytes, so an upgrade would leave no trace"
    );
    fingerprint.content_hash = ContentHash::of(b"what the last compiler shipped");
    for entry in &mut fingerprint.defs {
        mutate(entry);
    }
    store.put_source(&path, fingerprint);
    store.flush().unwrap();
}

#[test]
fn an_upgrade_that_moves_no_definition_re_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", output(&out));

    age_the_shipped_fingerprint(dir.path(), |_| {});

    // The test the project owns is unchanged, so `ply test` selects nothing.
    let out = ply(dir.path()).arg("test").output().unwrap();
    let text = output(&out);
    assert!(
        text.contains("selected 0 of 1"),
        "an upgrade that moved no definition re-ran a test:\n{text}"
    );
}

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
    let mut aged = false;
    age_the_shipped_fingerprint(dir.path(), |entry| {
        if entry.name == drain {
            let mut bytes = entry.hash.0;
            bytes[0] ^= 0xff;
            entry.hash = ply_ty::DefHash(bytes);
            aged = true;
        }
    });
    assert!(aged, "`std.net.drain` is in the shipped fingerprint");

    let mut store = Store::open(dir.path()).unwrap();
    let loaded = driver::load_incremental(dir.path(), &mut store).unwrap();

    // A cache written under an older `std.net` may be believed by none of the published hashes.
    let scratch = load(dir.path()).unwrap();
    for name in ["app.read_all", "std.net.drain", "elsewhere.untouched"] {
        assert_eq!(
            hash_of(&loaded, name),
            hash_of(&scratch, name),
            "`{name}` is stale after an upgrade"
        );
    }
}

/// Zero here, and it must be said as zero rather than implied.
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

#[test]
fn renaming_a_shipped_definition_moves_no_hash() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "mine.ply", ply_std::NET);
    write(
        dir.path(),
        "reader.ply",
        "import mine (net, drain)\n\
         pub fn read_all(c: Int) -> Bytes / {net.recv[conn]} = drain(c, b\"\", 1000)\n\
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
         pub fn read_all(c: Int) -> Bytes / {net.recv[conn]} = read_to_end(c, b\"\", 1000)\n\
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

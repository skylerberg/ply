//! The stdlib path: `import std.net` from a project, and what it does — and does not do — to
//! content addressing.

use assert_cmd::prelude::*;
use ply_cli::driver;
use ply_cli::load::{Loaded, load};
use ply_span::{Symbol, codes};
use ply_store::{ContentHash, Store};
use ply_syntax::ast::ModuleName;
use serde_json::Value;
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

/// A project that reaches a socket through the imported declaration and handles every atom it can
/// perform, so its test is `det` and cacheable.
const IMPORTER: &str = "\
import std.net (net)

fn touch(port: Int) -> Int / {net.write[listener]} {
  let l = net.listen[listener](port);
  net.close[listener](l);
  l
}

test \"the imported effect is handled in memory\" {
  handle {
    assert_eq(touch(8080), 3)
  } with {
    net.listen[listener](p) -> 3,
    net.close[listener](l) -> (),
  }
}
";

fn hash_of(loaded: &Loaded, name: &str) -> String {
    let key = Symbol::new(name);
    loaded
        .hashes
        .defs
        .get(&key)
        .or_else(|| loaded.hashes.decls.get(&key))
        .unwrap_or_else(|| {
            panic!(
                "`{name}` is not in the program; it holds {:?}",
                loaded.hashes.defs.keys().collect::<Vec<_>>()
            )
        })
        .to_hex()
}

// --- Resolution -------------------------------------------------------------

#[test]
fn a_project_module_can_import_std_net_and_it_checks() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);

    let loaded = load(dir.path()).expect("`import std.net` resolves and checks");
    assert!(
        loaded.check.modules.contains_key(&Symbol::new("std.net")),
        "the shipped module is not in the program: {:?}",
        loaded.check.modules.keys().collect::<Vec<_>>()
    );
    // The effect's program-wide name is what `ply_host::tcp` registers against, and it is qualified
    // by the module that declares it like any other.
    assert!(
        loaded
            .check
            .effects
            .contains_key(&Symbol::new("std.net.net")),
        "{:?}",
        loaded.check.effects.keys().collect::<Vec<_>>()
    );
    assert!(
        loaded
            .check
            .defs
            .contains_key(&Symbol::new("std.net.drain"))
    );
    assert!(loaded.check.defs.contains_key(&Symbol::new("app.touch")));

    // The row a host handler binds against, written in the qualified name.
    let touch = &loaded.check.defs[&Symbol::new("app.touch")];
    assert_eq!(touch.footprint.to_string(), "{std.net.net.write[listener]}");
}

/// And it runs: `ply test` goes green over the in-memory handlers, which is the end-to-end claim
/// the resolution above only sets up.
#[test]
fn a_project_that_imports_std_net_tests_green() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);

    let out = ply(dir.path()).arg("test").output().unwrap();
    let text = output(&out);
    assert_eq!(out.status.code(), Some(0), "got:\n{text}");
    assert!(text.contains("the imported effect is handled"), "{text}");
}

/// The handler binds against the shipped declaration, so `ply hosts` names the qualified effect.
#[test]
fn ply_hosts_binds_the_shipped_declaration_under_its_qualified_name() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);

    let out = ply(dir.path())
        .args(["hosts", "--host", "--json"])
        .output()
        .unwrap();
    let text = output(&out);
    assert_eq!(out.status.code(), Some(0), "got:\n{text}");
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let rows = v["hosts"].as_array().expect("rows");
    assert!(!rows.is_empty(), "nothing bound: {text}");
    assert!(
        rows.iter().all(|r| r["effect"] == "std.net.net"),
        "got:\n{text}"
    );
    assert!(
        rows.iter()
            .any(|r| r["triple"] == "std.net.net.listen[listener]"),
        "got:\n{text}"
    );

    // A program that declares its own `net` instead reaches no handler: the registration names
    // `std.net.net` and nothing else answers to it.
    let other = tempfile::tempdir().unwrap();
    write(
        other.path(),
        "app.ply",
        "nondet effect net {\n  write listen[s](port: Int) -> Int\n}\n\
         fn f() -> Int / {net.write[listener]} = net.listen[listener](1)\n",
    );
    let out = ply(other.path())
        .args(["hosts", "--host"])
        .output()
        .unwrap();
    let text = output(&out);
    assert!(
        text.contains("none serves an atom this program performs"),
        "a copied declaration must not bind:\n{text}"
    );
}

/// Required test 2, in the loader: `std` is reserved, so a project file that would claim it is
/// refused against the file rather than silently shadowing or being shadowed.
#[test]
fn a_project_file_under_std_is_e0113_against_the_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "std/json.ply", "pub fn parse() -> Int = 1\n");

    let err = load(dir.path()).expect_err("`std` is reserved");
    assert_eq!(err.diagnostics.len(), 1);
    assert_eq!(err.diagnostics[0].code, codes::RESERVED_MODULE_NAME);
    assert!(
        err.diagnostics[0].message.contains("std.json"),
        "{:?}",
        err.diagnostics[0].message
    );
    let span = err.diagnostics[0].primary_span().unwrap();
    assert!(!span.is_dummy(), "E0113 must point at the file it is about");
    assert!(err.sources.get(span.source).is_some());
}

/// A file named `std.ply` claims the root itself, which is the same error and is easy to miss when
/// the rule is written as a prefix check.
#[test]
fn a_project_file_named_std_is_also_e0113() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "std.ply", "pub fn f() -> Int = 1\n");
    let err = load(dir.path()).expect_err("`std` is reserved");
    assert_eq!(err.diagnostics[0].code, codes::RESERVED_MODULE_NAME);

    // And a name that merely starts with the letters is not reserved.
    let ok = tempfile::tempdir().unwrap();
    write(ok.path(), "stdlib.ply", "pub fn f() -> Int = 1\n");
    load(ok.path()).expect("`stdlib` is an ordinary module name");
}

/// A `std.x` this build does not ship names what it does, rather than reporting that a module is
/// missing from a project the user cannot add it to.
#[test]
fn importing_a_module_that_does_not_ship_lists_the_ones_that_do() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        "import std.nonesuch\nfn f() -> Int = 1\n",
    );
    let err = load(dir.path()).expect_err("`std.nonesuch` does not ship");
    assert_eq!(err.diagnostics[0].code, codes::UNKNOWN_MODULE);
    let rendered = format!("{:?}", err.diagnostics[0]);
    assert!(rendered.contains("std.net"), "{rendered}");
    let span = err.diagnostics[0].primary_span().unwrap();
    assert!(!span.is_dummy(), "the diagnostic must point at the import");
}

// --- Content addressing ------------------------------------------------------

#[test]
fn a_program_importing_nothing_from_std_loads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", "fn f() -> Int = 1\n");

    let loaded = load(dir.path()).unwrap();
    assert_eq!(loaded.check.modules.len(), 1);
    assert!(
        !loaded
            .check
            .defs
            .keys()
            .any(|k| k.as_str().starts_with("std.")),
        "{:?}",
        loaded.check.defs.keys().collect::<Vec<_>>()
    );
    assert_eq!(loaded.files.len(), 1, "{:?}", loaded.files);
}

#[test]
fn copying_a_shipped_module_into_a_project_produces_identical_hashes() {
    let shipped = tempfile::tempdir().unwrap();
    write(shipped.path(), "app.ply", IMPORTER);
    let shipped = load(shipped.path()).unwrap();

    let copied = tempfile::tempdir().unwrap();
    write(copied.path(), "mine.ply", ply_std::NET);
    write(
        copied.path(),
        "app.ply",
        &IMPORTER.replace("import std.net (net)", "import mine (net)"),
    );
    let copied = load(copied.path()).unwrap();

    assert_eq!(
        hash_of(&shipped, "std.net.drain"),
        hash_of(&copied, "mine.drain"),
        "a stdlib definition must hash like a project one"
    );
    assert_eq!(
        hash_of(&shipped, "std.net.net"),
        hash_of(&copied, "mine.net"),
        "an effect declaration is a declaration like any other"
    );
    // And the definition that *reaches* it: a reference contributes the referent's hash, so the
    // importer is one definition in both programs.
    assert_eq!(
        hash_of(&shipped, "app.touch"),
        hash_of(&copied, "app.touch")
    );
}

/// The pseudo-path is what lets gate 1 key an embedded module on the bytes it was compiled from,
/// with no new mechanism and no file on disk.
#[test]
fn a_shipped_module_is_fingerprinted_under_its_pseudo_path() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);

    let mut store = Store::open(dir.path()).unwrap();
    driver::load_incremental(dir.path(), &mut store).unwrap();
    store.flush().unwrap();

    let path = ply_std::pseudo_path(&std_net());
    assert_eq!(path, PathBuf::from("<std>/net.ply"));
    let fingerprint = store
        .fingerprint(&path)
        .expect("the shipped module is filed under its pseudo-path");
    assert_eq!(
        fingerprint.content_hash,
        ContentHash::of(ply_std::NET.as_bytes()),
        "gate 1 must key on the embedded source bytes"
    );

    // Gate 1 then fires for it on the next run, exactly as for a file.
    let second = driver::load_incremental(dir.path(), &mut store).unwrap();
    let report = second
        .frontend
        .files
        .iter()
        .find(|f| f.module.as_str() == "std.net")
        .expect("the shipped module is reported like any other");
    assert!(
        !report.parsed,
        "the shipped module was parsed again: {:?}",
        report.refusal
    );
}

#[test]
fn incremental_and_full_agree_over_a_program_that_imports_std() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);

    let mut store = Store::open(dir.path()).unwrap();
    let cold = driver::load_incremental(dir.path(), &mut store).unwrap();
    store.flush().unwrap();
    let warm = driver::load_incremental(dir.path(), &mut store).unwrap();
    let full = driver::load_full(dir.path()).unwrap();

    for name in ["std.net.drain", "std.net.net", "app.touch"] {
        assert_eq!(hash_of(&cold, name), hash_of(&full, name), "{name}");
        assert_eq!(hash_of(&warm, name), hash_of(&full, name), "{name}");
    }
    assert_eq!(
        format!(
            "{:?}",
            warm.check.defs[&Symbol::new("std.net.drain")].scheme
        ),
        format!(
            "{:?}",
            full.check.defs[&Symbol::new("std.net.drain")].scheme
        ),
    );
}

#[test]
fn renaming_a_project_definition_that_calls_std_moves_no_hash() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);
    let before = load(dir.path()).unwrap();

    write(dir.path(), "app.ply", &IMPORTER.replace("touch", "poke"));
    let after = load(dir.path()).unwrap();

    assert_eq!(
        hash_of(&before, "app.touch"),
        hash_of(&after, "app.poke"),
        "renaming a definition changed its hash"
    );
    assert_eq!(
        hash_of(&before, "std.net.drain"),
        hash_of(&after, "std.net.drain")
    );
    assert_eq!(before.hashes.tests, after.hashes.tests, "a test re-runs");
}

// --- Selection ---------------------------------------------------------------

#[test]
fn a_shipped_modules_tests_are_not_a_projects() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);

    let loaded = load(dir.path()).unwrap();
    let shipped = loaded
        .check
        .tests
        .iter()
        .filter(|t| t.module.as_str() == "std.net")
        .count();
    assert!(shipped > 0, "the fixture needs a shipped test to hide");

    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("{e}: {}", output(&out)));
    assert_eq!(v["selection"]["total"], 1, "{v}");
    // Not counted as filtered out either: a shipped test was never in this project's denominator,
    // and reporting it as excluded would invite someone to go looking for it.
    assert_eq!(v["selection"]["filtered_out"], 0, "{v}");

    let out = ply(dir.path())
        .args(["test", "--std", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("{e}: {}", output(&out)));
    assert_eq!(v["selection"]["total"], 1 + shipped, "{v}");
    assert_eq!(v["exit_code"], 0, "{v}");
    // And the run above wrote **nothing** about them: the project's test was cached by it, the
    // shipped one was not, so `--std` still has work to do.
    assert_eq!(v["selection"]["cached"], 1, "{v}");
    assert_eq!(v["selection"]["selected"], shipped, "{v}");
}

/// `ply run` in a directory holding a shipped module still has one entry point: `main` is a
/// project's, and a stdlib `main` would make the command ambiguous in a directory the user did not
/// write.
#[test]
fn entry_points_exclude_the_shipped_modules() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        "import std.net (net)\nfn main() -> Int = 1\n",
    );
    let loaded = load(dir.path()).unwrap();
    let mains: Vec<&str> = loaded
        .entry_points()
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(mains, ["app.main"]);
}

// --- `ply std` and the digest -------------------------------------------------

#[test]
fn ply_std_lists_the_modules_and_prints_a_stable_digest() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", "fn f() -> Int = 1\n");

    let out = ply(dir.path()).arg("std").output().unwrap();
    let text = output(&out);
    assert_eq!(out.status.code(), Some(0), "got:\n{text}");
    assert!(text.contains("std.net"), "got:\n{text}");
    assert!(text.contains("std.json"), "got:\n{text}");
    assert!(text.contains("digest: b3:"), "got:\n{text}");

    let digest = || {
        let out = ply(dir.path()).args(["std", "--digest"]).output().unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let once = digest();
    assert_eq!(once, digest(), "the digest moved between two runs");
    assert!(once.starts_with("b3:"), "{once}");
    assert_eq!(once.trim().len(), 15, "{once}");
    assert!(
        text.contains(once.trim()),
        "the two forms disagree:\n{text}"
    );

    let out = ply(dir.path()).args(["std", "--json"]).output().unwrap();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["digest"], once.trim());

    // Every shipped module, in the table's order, which is the order `ply-std`'s own suite pins as
    // sorted and unique.
    let listed: Vec<&str> = v["modules"]
        .as_array()
        .expect("an array of modules")
        .iter()
        .map(|m| m["module"].as_str().expect("a name"))
        .collect();
    let shipped: Vec<String> = ply_std::modules().map(|m| m.to_string()).collect();
    assert_eq!(listed, shipped, "got:\n{text}");
    for module in v["modules"].as_array().unwrap() {
        assert!(
            module["definitions"].as_u64().unwrap() >= 2,
            "`{}` lists no definitions",
            module["module"]
        );
    }
}

#[test]
fn a_cache_written_under_another_digest_warns_once_and_says_how_much_moved() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);

    let mut store = Store::open(dir.path()).unwrap();
    driver::load_incremental(dir.path(), &mut store).unwrap();
    store.flush().unwrap();
    assert_eq!(
        store.stdlib_digest().as_deref(),
        Some(ply_std::digest_short().as_str())
    );

    // A warm cache written by a build whose stdlib was something else.
    std::fs::write(dir.path().join(".ply-cache/stdlib"), "b3:000000000000\n").unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let loaded = driver::load_incremental(dir.path(), &mut store).unwrap();
    let warnings: Vec<_> = loaded
        .frontend
        .warnings
        .iter()
        .filter(|d| d.code == codes::STDLIB_CHANGED)
        .collect();
    assert_eq!(warnings.len(), 1, "{:?}", loaded.frontend.warnings);
    let rendered = format!("{:?}", warnings[0]);
    assert!(rendered.contains("b3:000000000000"), "{rendered}");
    assert!(rendered.contains(&ply_std::digest_short()), "{rendered}");
    // Nothing actually moved: the sources this build ships are the ones it compiled the last run
    // with, only the recorded digest was a lie.
    assert!(
        rendered.contains("no definition this program reaches changed"),
        "{rendered}"
    );
    store.flush().unwrap();

    // Once. The run that saw it recorded the digest, so the next is quiet.
    let mut store = Store::open(dir.path()).unwrap();
    let again = driver::load_incremental(dir.path(), &mut store).unwrap();
    assert!(
        !again
            .frontend
            .warnings
            .iter()
            .any(|d| d.code == codes::STDLIB_CHANGED),
        "{:?}",
        again.frontend.warnings
    );
}

/// A cold cache has nothing to compare against, so it says nothing.
#[test]
fn a_cold_cache_does_not_warn_about_the_stdlib() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);
    let mut store = Store::open(dir.path()).unwrap();
    let loaded = driver::load_incremental(dir.path(), &mut store).unwrap();
    assert!(
        !loaded
            .frontend
            .warnings
            .iter()
            .any(|d| d.code == codes::STDLIB_CHANGED),
        "{:?}",
        loaded.frontend.warnings
    );
}

/// The shipped sources may import only `std.*`
#[test]
fn a_shipped_module_importing_outside_std_is_ply_s_fault() {
    let d = ply_std::foreign_import(&std_net(), &Symbol::new("app"), ply_span::Span::DUMMY);
    assert_eq!(d.code, codes::INTERNAL_ERROR);
    let rendered = format!("{d:?}");
    assert!(rendered.contains("not in this program"), "{rendered}");
}

#[test]
fn editing_one_shipped_definition_moves_exactly_what_reaches_it() {
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
    write(
        dir.path(),
        "elsewhere.ply",
        "pub fn untouched() -> Int = 41 + 1\n\
         test \"untouched\" { assert_eq(untouched(), 42) }\n",
    );

    let before = load(dir.path()).unwrap();
    // The edit a compiler upgrade would make: one definition's body, nothing else in the module.
    write(
        dir.path(),
        "mine.ply",
        &ply_std::NET.replace(
            "net.recv[conn](c, 4096, timeout_ms)",
            "net.recv[conn](c, 8192, timeout_ms)",
        ),
    );
    let after = load(dir.path()).unwrap();

    assert_ne!(
        hash_of(&before, "mine.drain"),
        hash_of(&after, "mine.drain"),
        "the edited definition"
    );
    assert_ne!(
        hash_of(&before, "reader.read_all"),
        hash_of(&after, "reader.read_all"),
        "a reference contributes the referent's hash, so a caller moves with it"
    );
    // Everything else stands: the effect declaration, the module's other definitions, and a module
    // that reaches none of them.
    for name in ["mine.net", "mine.head", "mine.tail", "elsewhere.untouched"] {
        assert_eq!(hash_of(&before, name), hash_of(&after, name), "{name}");
    }

    // And the tests: exactly the one that reaches the edit is re-selected.
    let key_of = |loaded: &Loaded, label: &str| {
        loaded
            .check
            .tests
            .iter()
            .position(|t| t.key.as_str().ends_with(label))
            .map(|i| loaded.hashes.tests[i].to_hex())
            .unwrap_or_else(|| panic!("no test labelled `{label}`"))
    };
    assert_ne!(key_of(&before, "reads"), key_of(&after, "reads"));
    assert_eq!(key_of(&before, "untouched"), key_of(&after, "untouched"));
}

/// `ply cache compact` walks the files on disk, and a shipped module has none.
#[test]
fn compaction_keeps_the_shipped_modules_it_loaded() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);
    ply(dir.path()).arg("test").output().unwrap();

    let path = ply_std::pseudo_path(&std_net());
    assert!(
        Store::open(dir.path())
            .unwrap()
            .fingerprint(&path)
            .is_some(),
        "the run recorded nothing to compact"
    );

    let out = ply(dir.path()).args(["cache", "compact"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "got:\n{}", output(&out));
    assert!(
        Store::open(dir.path())
            .unwrap()
            .fingerprint(&path)
            .is_some(),
        "compaction dropped a module this binary still ships"
    );

    // And the run after it still skips: the point of keeping the entry.
    let mut store = Store::open(dir.path()).unwrap();
    let loaded = driver::load_incremental(dir.path(), &mut store).unwrap();
    let report = loaded
        .frontend
        .files
        .iter()
        .find(|f| f.module.as_str() == "std.net")
        .expect("the shipped module is in the run");
    assert!(
        !report.parsed,
        "reparsed after compaction: {:?}",
        report.refusal
    );
}

/// The compiler's own suite for what it ships.
#[test]
fn the_shipped_modules_own_tests_and_laws_all_pass() {
    let dir = tempfile::tempdir().unwrap();
    let imports: String = ply_std::modules()
        .map(|m| format!("import {m}\n"))
        .collect();
    write(dir.path(), "all.ply", &imports);

    let out = ply(dir.path())
        .args(["test", "--std", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("{e}: {}", output(&out)));
    assert_eq!(v["exit_code"], 0, "{v:#}");
    assert_eq!(v["summary"]["failed"], 0, "{v:#}");
    assert!(
        v["summary"]["passed"].as_u64().unwrap() > 0,
        "the shipped modules declare no test: {v:#}"
    );

    let out = ply(dir.path())
        .args(["prove", "--std", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("{e}: {}", output(&out)));
    assert_eq!(v["exit_code"], 0, "{v:#}");
    assert!(
        v["obligations"].as_array().is_some_and(|o| !o.is_empty()),
        "the shipped modules carry no obligation: {v:#}"
    );
}

/// A project's `ply prove` reports what the project claimed.
#[test]
fn a_shipped_modules_laws_are_not_a_projects() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        "import std.json\n\
         pub fn twice(n: Int) -> Int ensures result == n + n = n * 2\n",
    );

    let project = |args: &[&str]| -> Value {
        let out = ply(dir.path()).args(args).output().unwrap();
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
            .unwrap_or_else(|e| panic!("{e}: {}", output(&out)))
    };

    let mine = project(&["prove", "--json"]);
    let all = project(&["prove", "--std", "--json"]);
    assert_eq!(mine["exit_code"], 0, "{mine:#}");

    let owners: Vec<&str> = mine["obligations"]
        .as_array()
        .expect("obligations")
        .iter()
        .map(|o| o["owner"].as_str().expect("an owner"))
        .collect();
    assert_eq!(owners, ["app.twice"], "{mine:#}");
    assert!(
        all["obligations"].as_array().unwrap().len() > owners.len(),
        "`--std` adds nothing: {all:#}"
    );

    // And the coverage denominator is the project's definitions, not the stdlib's — an unspecified
    // count of two hundred shipped functions is a reader's reason to stop reading the line that
    // matters most.
    assert_eq!(mine["coverage"]["definitions"], 1, "{mine:#}");
    assert!(
        all["coverage"]["definitions"].as_u64().unwrap() > 100,
        "{all:#}"
    );
}

/// `observe_definitions` withholds every definition an unresolved test reached, and a shipped
/// module's tests are never resolved without `--std` — so `std.router`'s and `std.http`'s
/// definitions were never recorded, stayed permanently "changed", and landed in the suspect set of
/// every failure in every project that imported them.
#[test]
fn a_shipped_definition_the_project_never_touched_is_not_a_suspect() {
    let dir = tempfile::tempdir().unwrap();
    let source = |literal: &str| {
        format!(
            "import std.router\n\
             import std.http\n\
             pub type Endpoint = Health | GetItem\n\
             pub fn table() -> List<router::Route<Endpoint>> = [\n\
               {{method: http::Get, path: router::pattern_of_string(\"/health\"), endpoint: Health}},\n\
               {{method: http::Get, path: router::pattern_of_string(\"/items/{{sku}}\"), endpoint: GetItem}},\n\
             ]\n\
             pub fn slug(s: String) -> String = string_concat(\"{literal}\", s)\n\
             pub fn hits(p: String) -> Bool =\n\
               match router::route(table(), http::Get, p) {{\n\
                 router::Found(_) -> true,\n\
                 _ -> false,\n\
               }}\n\
             test \"the table routes an item\" {{ assert_eq(hits(slug(\"bolt\")), true) }}\n"
        )
    };

    write(dir.path(), "app.ply", &source("/items/"));
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert!(out.status.success(), "{}", output(&out));

    // One edit, to one definition the project owns.
    write(dir.path(), "app.ply", &source("/goods/"));
    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("{e}: {}", output(&out)));

    let suspects: Vec<String> = v["failures"][0]["suspects"]
        .as_array()
        .unwrap_or_else(|| panic!("a failure with an attribution: {v}"))
        .iter()
        .map(|s| s["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !suspects.is_empty(),
        "the edit has to be attributed to something: {v}"
    );
    let shipped: Vec<&String> = suspects.iter().filter(|n| n.starts_with("std.")).collect();
    assert!(
        shipped.is_empty(),
        "nothing under `std` moved, so nothing under `std` is a suspect: {shipped:?}"
    );
    assert_eq!(
        v["failures"][0]["culprit"]["definitions"][0], "app.slug",
        "{v}"
    );
}

// --- The `Limits` mispairing guard ------------------------------------------

use ply_syntax::ast::{Expr, ExprKind, Ident, Item, Module, Stmt, TypeDefBody, TypeExpr};

fn shipped_http() -> Module {
    use ply_span::SourceId;
    let source = ply_std::source(&ModuleName::from_dotted("std.http")).expect("std.http ships");
    ply_syntax::parse_module(SourceId(0), ModuleName::from_dotted("std.http"), source)
        .expect("the shipped module parses")
}

fn limits_fields(module: &Module) -> Vec<String> {
    let fields = module
        .items
        .iter()
        .find_map(|i| match i {
            Item::Type(d) if d.name.name.as_str() == "Limits" => match &d.body {
                TypeDefBody::Alias(TypeExpr::Record { fields, .. }) => Some(fields),
                _ => None,
            },
            _ => None,
        })
        .expect("`type Limits` is a record");
    assert!(
        fields.len() >= 13,
        "`Limits` shrank to {} fields; these tests are about the cost of it growing",
        fields.len()
    );
    fields.iter().map(|(n, _)| n.name.to_string()).collect()
}

/// A dotted path rendered back to source — `state.limits.max_body` — or `None` for anything that is
/// not one.
fn dotted(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Var(v) if v.is_bare() => Some(v.name.name.to_string()),
        ExprKind::Field { base, field } => Some(format!("{}.{}", dotted(base)?, field.name)),
        _ => None,
    }
}

/// The field of `base` this expression reads, if it reads one directly.
fn read_of_base(e: &Expr, base: &str) -> Option<String> {
    let path = dotted(e)?;
    let rest = path.strip_prefix(base)?.strip_prefix('.')?;
    (!rest.contains('.')).then(|| rest.to_string())
}

/// The record literal `func` evaluates to: the value of `let <binder>` when one is named, otherwise
/// the block's tail.
fn limits_literal<'a>(module: &'a Module, func: &str, binder: Option<&str>) -> &'a [(Ident, Expr)] {
    let body = module
        .items
        .iter()
        .find_map(|i| match i {
            Item::Fn(d) if d.name.name.as_str() == func => Some(&d.body),
            _ => None,
        })
        .unwrap_or_else(|| panic!("`{func}` is defined"));
    let ExprKind::Block { stmts, tail } = &body.kind else {
        panic!("`{func}` is a block")
    };
    let record = match binder {
        Some(b) => stmts
            .iter()
            .find_map(|s| match s {
                Stmt::Let { pat, value, .. } if format!("{:?}", pat.kind).contains(b) => {
                    Some(&**value)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("`let {b} = ...` in `{func}`")),
        None => tail
            .as_deref()
            .unwrap_or_else(|| panic!("`{func}` has a tail")),
    };
    let ExprKind::Record { fields } = &record.kind else {
        panic!("`{func}`'s result is a record literal, expanded from `{{..base, ..}}`")
    };
    fields
}

/// Asserts that `func` builds a full-width `Limits` in which every field it does not deliberately
/// vary is `base.<that same field>`.
#[track_caller]
fn copies_every_limit_it_does_not_vary(
    func: &str,
    binder: Option<&str>,
    base: &str,
    varied: &[(&str, Option<&str>)],
) {
    let module = shipped_http();
    let fields = limits_literal(&module, func, binder);

    let mut names: Vec<String> = fields.iter().map(|(n, _)| n.name.to_string()).collect();
    let mut expected = limits_fields(&module);
    names.sort();
    expected.sort();
    assert_eq!(
        names, expected,
        "`{func}` does not build exactly `Limits`, so it would not type-check as one"
    );

    let mut actual: Vec<(String, Option<String>)> = Vec::new();
    for (name, value) in fields {
        let name = name.name.to_string();
        match read_of_base(value, base) {
            Some(from) if from == name => {}
            other => actual.push((name, other)),
        }
    }
    let want: Vec<(String, Option<String>)> = varied
        .iter()
        .map(|(n, f)| (n.to_string(), f.map(str::to_string)))
        .collect();
    assert_eq!(
        actual, want,
        "in `{func}`, exactly these bounds may differ from `{base}`'s bound of their own name; \
         anything else here is a mispaired limit"
    );
}

/// **The guard the `chunk_trailers` rewrite is worth.**
#[test]
fn chunk_trailers_copies_every_limit_it_does_not_replace() {
    copies_every_limit_it_does_not_vary(
        "chunk_trailers",
        Some("trailer_limits"),
        "state.limits",
        &[("max_header_bytes", Some("max_trailer_bytes"))],
    );
}

#[test]
fn the_limits_helpers_vary_only_the_bounds_they_are_named_for() {
    copies_every_limit_it_does_not_vary(
        "limits_keeping",
        None,
        "base",
        &[("max_keep_alive", None)],
    );
    copies_every_limit_it_does_not_vary(
        "limits_streaming",
        None,
        "base",
        &[("max_stream_chunks", None)],
    );
    copies_every_limit_it_does_not_vary(
        "limits_with",
        None,
        "base",
        &[
            ("max_request_line", None),
            ("max_header_bytes", None),
            ("max_header_count", None),
            ("max_body", None),
            ("max_chunk_size", None),
            ("max_chunk_line", None),
            ("max_trailer_bytes", None),
        ],
    );
}

/// `limits_with` is the one converted site where a mispairing survives the rewrite, because the
/// seven bounds it *does* write are seven `Int` parameters and `max_chunk_size: chunk_line`
/// type-checks.
#[test]
fn limits_with_pairs_each_bound_with_the_parameter_named_after_it() {
    let module = shipped_http();
    let params: Vec<String> = module
        .items
        .iter()
        .find_map(|i| match i {
            Item::Fn(d) if d.name.name.as_str() == "limits_with" => Some(&d.params),
            _ => None,
        })
        .expect("`limits_with` is defined")
        .iter()
        .map(|p| p.name.name.to_string())
        .collect();

    let mut paired = 0;
    for (name, value) in limits_literal(&module, "limits_with", None) {
        let name = name.name.to_string();
        let Some(arg) = dotted(value).filter(|a| params.contains(a)) else {
            continue;
        };
        assert_eq!(
            name,
            format!("max_{arg}"),
            "`limits_with` passes `{arg}` as `{name}`, and every `Limits` field is `Int`, \
             so nothing else would have caught it"
        );
        paired += 1;
    }
    assert_eq!(
        paired,
        params.len(),
        "every parameter of `limits_with` must reach a bound, or one of them is dead"
    );
}

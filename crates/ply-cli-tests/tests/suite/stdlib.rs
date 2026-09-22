use assert_cmd::prelude::*;
use ply_cli::driver;
use ply_cli::load::{LoadError, Loaded, load};
use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};
use ply_store::{ContentHash, Store};
use ply_ty::ModuleName;
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

/// Handles every atom it can perform, so its test is `det` and cacheable.
const IMPORTER: &str = "\
import std.net (net)

fn touch(port: Int) -> Int / {net.listen[listener], net.close[listener]} {
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
    // `ply_host::tcp` registers against the program-wide name, qualified by the declaring module.
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
    assert_eq!(
        touch.footprint.to_string(),
        "{std.net.net.close[listener], std.net.net.listen[listener]}"
    );
}

#[test]
fn a_project_that_imports_std_net_tests_green() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", IMPORTER);

    let out = ply(dir.path()).arg("test").output().unwrap();
    let text = output(&out);
    assert_eq!(out.status.code(), Some(0), "got:\n{text}");
    assert!(text.contains("the imported effect is handled"), "{text}");
}

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

    // A program declaring its own `net` reaches no handler: the registration names `std.net.net` only.
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

/// Easy to miss when the rule is written as a prefix check.
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
    // A reference contributes the referent's hash, so the importer is one definition in both programs.
    assert_eq!(
        hash_of(&shipped, "app.touch"),
        hash_of(&copied, "app.touch")
    );
}

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
        "the fingerprint must key on the embedded source bytes"
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
    // A shipped test was never in this project's denominator, so it is not "filtered out" either.
    assert_eq!(v["selection"]["filtered_out"], 0, "{v}");

    let out = ply(dir.path())
        .args(["test", "--std", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("{e}: {}", output(&out)));
    assert_eq!(v["selection"]["total"], 1 + shipped, "{v}");
    assert_eq!(v["exit_code"], 0, "{v}");
    // The run wrote nothing about the shipped test, so `--std` still has work to do.
    assert_eq!(v["selection"]["cached"], 1, "{v}");
    assert_eq!(v["selection"]["selected"], shipped, "{v}");
}

/// A stdlib `main` would make `ply run` ambiguous in a directory the user did not write.
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

    // In the table's order, which `ply-std`'s own suite pins as sorted and unique.
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
    // Nothing moved: the shipped sources are the last run's; only the recorded digest was a lie.
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

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
        .canonicalize()
        .expect("the repository path exists")
}

/// The port's answer over a flat directory, pulling in the shipped modules itself, and the driver's.
fn pulled_and_loaded(dir: &Path) -> (Vec<String>, ply_ty::Front, Result<Loaded, LoadError>) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    files.sort();
    let user: Vec<(String, String)> = files
        .iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(p).unwrap())
        })
        .collect();
    let shipped: Vec<(String, String)> = ply_std::sources()
        .map(|(m, t)| (m.to_string(), t.to_string()))
        .collect();
    let pulled = ply_codegen::c::producer::front_pulling_std(&user, &shipped)
        .unwrap_or_else(|e| panic!("{}: the port does not answer: {e:#}", dir.display()));
    let ids: Vec<SourceId> = (0..user.len() + pulled.modules.len())
        .map(|i| SourceId(i as u32))
        .collect();
    let ours = ply_ty::read_front(&pulled.dump, &ids)
        .unwrap_or_else(|e| panic!("{}: the port's answer does not read: {e}", dir.display()));

    let theirs = load(dir);
    let sources = match &theirs {
        Ok(loaded) => &loaded.sources,
        Err(err) => &err.sources,
    };
    let placed: Vec<PathBuf> = sources.files().iter().map(|f| f.path.clone()).collect();
    let expected: Vec<PathBuf> = files
        .into_iter()
        .chain(
            pulled
                .modules
                .iter()
                .map(|m| ply_std::pseudo_path(&ModuleName::from_dotted(m))),
        )
        .collect();
    assert_eq!(placed, expected, "{}: the modules, in order", dir.display());
    (pulled.modules, ours, theirs)
}

#[test]
fn the_driver_places_the_shipped_modules_the_port_pulls_in_and_answers_as_it_does() {
    let chain = tempfile::tempdir().unwrap();
    write(
        chain.path(),
        "app.ply",
        "import std.router\nimport util\n\nfn f() -> Int = util::one()\n",
    );
    write(
        chain.path(),
        "util.ply",
        "import std.trace\n\npub fn one() -> Int = 1\n",
    );
    let plain = tempfile::tempdir().unwrap();
    write(plain.path(), "a.ply", "pub fn a() -> Int = 1\n");
    write(
        plain.path(),
        "b.ply",
        "import a\n\nfn b() -> Int = a::a()\n",
    );

    // A round's imports follow the round before it, so the whole is not in byte order.
    let rounds: &[&str] = &["std.router", "std.trace", "std.http", "std.json", "std.net"];
    for (dir, pulls) in [
        (repo("examples"), None),
        (chain.path().to_path_buf(), Some(rounds)),
        (plain.path().to_path_buf(), Some(&[][..])),
    ] {
        let (pulled, ours, theirs) = pulled_and_loaded(&dir);
        if let Some(pulls) = pulls {
            assert_eq!(pulled, pulls, "{}", dir.display());
        }
        let loaded = theirs.unwrap_or_else(|e| panic!("{}: {:?}", dir.display(), e.diagnostics));
        assert!(
            ours.hashes == loaded.hashes,
            "{}: the hashes differ",
            dir.display()
        );
        assert!(
            format!("{ours:?}") == format!("{:?}", loaded.front),
            "{}: the answers differ",
            dir.display()
        );
    }
}

fn headlines(ds: &[Diagnostic]) -> Vec<(&'static str, &str, Option<Span>)> {
    ds.iter()
        .map(|d| (d.code, d.message.as_str(), d.primary_span()))
        .collect()
}

#[test]
fn the_driver_refuses_with_the_port_s_diagnostics_alone() {
    let unshipped = tempfile::tempdir().unwrap();
    write(
        unshipped.path(),
        "app.ply",
        "import std.json\nimport std.nonesuch\nfn f() -> Int = 1\n",
    );
    let unknown = tempfile::tempdir().unwrap();
    write(
        unknown.path(),
        "app.ply",
        "import std.json\nimport nowhere\nfn f() -> Int = 1\n",
    );
    for dir in [unshipped.path(), unknown.path()] {
        let (_, ours, theirs) = pulled_and_loaded(dir);
        let err = theirs.expect_err("an import nothing answers for is refused");
        assert_eq!(
            ours.diagnostics.first().map(|d| d.code),
            Some(codes::UNKNOWN_MODULE)
        );
        assert_eq!(
            format!("{:?}", ours.diagnostics),
            format!("{:?}", err.diagnostics)
        );
    }

    let broken = tempfile::tempdir().unwrap();
    write(
        broken.path(),
        "app.ply",
        "import std.json\nfn f() -> Int = )\n",
    );
    let fixtures = ["ambiguous_import", "module_cycle", "duplicate_import"]
        .map(|f| repo(&format!("tests/fixtures/{f}")));
    for dir in fixtures.iter().map(PathBuf::as_path).chain([broken.path()]) {
        let (_, ours, theirs) = pulled_and_loaded(dir);
        let err = theirs
            .err()
            .unwrap_or_else(|| panic!("{}: the driver accepts it", dir.display()));
        assert_eq!(
            headlines(&ours.diagnostics),
            headlines(&err.diagnostics),
            "{}",
            dir.display()
        );
    }
}

#[test]
fn editing_one_shipped_definition_moves_exactly_what_reaches_it() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "mine.ply", ply_std::NET);
    write(
        dir.path(),
        "reader.ply",
        "import mine (net, drain)\n\
         pub fn read_all(c: Int) -> Bytes / {net.recv[conn]} = drain[conn](c, b\"\", 1000)\n\
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
            "net.recv[l](c, 4096, timeout_ms)",
            "net.recv[l](c, 8192, timeout_ms)",
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
    // Everything else stands: the effect, the module's other definitions, and a module that reaches none of them.
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
}

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

    // The coverage denominator is the project's definitions, not the stdlib's.
    assert_eq!(mine["coverage"]["definitions"], 1, "{mine:#}");
    assert!(
        all["coverage"]["definitions"].as_u64().unwrap() > 100,
        "{all:#}"
    );
}

/// A shipped module's tests are never resolved without `--std`, so its definitions must not stay permanently "changed".
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

/// `std.http` with each run of whitespace squeezed to one space, and to none inside a brace, so
/// that the scrapes below ask what it says rather than how the formatter laid it out.
fn shipped_http() -> String {
    let source = ply_std::source(&ModuleName::from_dotted("std.http")).expect("std.http ships");
    let mut out = String::with_capacity(source.len());
    for c in source.chars() {
        if c.is_whitespace() {
            if !(out.is_empty() || out.ends_with(' ') || out.ends_with('{')) {
                out.push(' ');
            }
        } else {
            if c == '}' && out.ends_with(' ') {
                out.pop();
            }
            out.push(c);
        }
    }
    out
}

fn limits_fields() -> Vec<String> {
    let head = "pub type Limits = {";
    let source = shipped_http();
    let at = source.find(head).expect("`type Limits` is a record") + head.len();
    let block = &source[at..at + source[at..].find('}').expect("`Limits` closes")];
    let fields: Vec<String> = block
        .split(',')
        .filter_map(|field| field.split_once(':'))
        .map(|(name, _)| name.trim().to_string())
        .collect();
    assert!(
        fields.len() >= 13,
        "`Limits` shrank to {} fields; these tests are about the cost of it growing",
        fields.len()
    );
    fields
}

/// `func`'s `{..base, name: value, ..}` as written: the base, then each field it names.
fn record_update(func: &str) -> (String, Vec<(String, String)>) {
    let source = shipped_http();
    let at = source
        .find(&format!("fn {func}("))
        .unwrap_or_else(|| panic!("`{func}` is defined"));
    let body = &source[at..];
    let open = body
        .find("{..")
        .unwrap_or_else(|| panic!("`{func}` builds its `Limits` by spreading one"))
        + "{..".len();
    let inner = &body[open..open + body[open..].find('}').expect("the spread closes")];
    let mut parts = inner.split(',').map(str::trim).filter(|p| !p.is_empty());
    let base = parts.next().expect("a spread names its base").to_string();
    let fields = parts
        .map(|part| {
            let (name, value) = part
                .split_once(':')
                .unwrap_or_else(|| panic!("`{part}` in `{func}` is not `name: value`"));
            (name.trim().to_string(), value.trim().to_string())
        })
        .collect();
    (base, fields)
}

/// The field of `base` this value reads, if it reads one directly.
fn read_of_base(value: &str, base: &str) -> Option<String> {
    let rest = value.strip_prefix(base)?.strip_prefix('.')?;
    (!rest.contains('.')).then(|| rest.to_string())
}

/// Every `Limits` field `func` names must be one it deliberately varies; the spread copies the rest.
#[track_caller]
fn copies_every_limit_it_does_not_vary(func: &str, base: &str, varied: &[(&str, Option<&str>)]) {
    let (spread, fields) = record_update(func);
    assert_eq!(spread, base, "`{func}` spreads `{spread}`, not `{base}`");
    let limits = limits_fields();
    for (name, _) in &fields {
        assert!(
            limits.contains(name),
            "`{func}` sets `{name}`, which `Limits` does not have"
        );
    }

    let mut actual: Vec<(String, Option<String>)> = Vec::new();
    for (name, value) in fields {
        match read_of_base(&value, base) {
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

#[test]
fn chunk_trailers_copies_every_limit_it_does_not_replace() {
    copies_every_limit_it_does_not_vary(
        "chunk_trailers",
        "state.limits",
        &[("max_header_bytes", Some("max_trailer_bytes"))],
    );
}

#[test]
fn the_limits_helpers_vary_only_the_bounds_they_are_named_for() {
    copies_every_limit_it_does_not_vary("limits_keeping", "base", &[("max_keep_alive", None)]);
    copies_every_limit_it_does_not_vary("limits_streaming", "base", &[("max_stream_chunks", None)]);
    copies_every_limit_it_does_not_vary(
        "limits_with",
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

/// Its seven written bounds are all `Int` parameters, so `max_chunk_size: chunk_line` would type-check.
#[test]
fn limits_with_pairs_each_bound_with_the_parameter_named_after_it() {
    let source = shipped_http();
    let at = source
        .find("fn limits_with(")
        .expect("`limits_with` is defined")
        + "fn limits_with(".len();
    let params: Vec<String> = source
        [at..at + source[at..].find(')').expect("the parameters close")]
        .split(',')
        .filter_map(|param| param.split_once(':'))
        .map(|(name, _)| name.trim().to_string())
        .collect();

    let mut paired = 0;
    for (name, arg) in record_update("limits_with").1 {
        if !params.contains(&arg) {
            continue;
        }
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

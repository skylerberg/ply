//! The committed `ply` program, and the shelf it is built against.
//!
//! CI's `refresh` job rebuilds the artifact and its digest on main with `ply build`, so no pull
//! request carries either.

use crate::harness::{ply, write};
use ply_launcher::shipped;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[test]
fn the_cli_tree_is_a_package() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ply-cli/ply");
    let loaded = ply_machine::load::load(&dir).expect("the CLI tree loads");
    assert_eq!(
        loaded.front.packages,
        vec![
            (
                "cli".to_string(),
                vec!["suite".to_string(), "proof".to_string()]
            ),
            ("suite".to_string(), Vec::new()),
            ("proof".to_string(), Vec::new()),
            ("std".to_string(), Vec::new()),
            ("compiler".to_string(), vec!["std".to_string()]),
        ],
        "the CLI tree's ply.pkg names it the `cli` package, closed over the suite, the prove package \
         and the two built-ins"
    );
}

#[test]
fn the_corpus_tree_is_a_package_over_the_cli() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ply-corpus/ply");
    let loaded = ply_machine::load::load(&dir).expect("the corpus tree loads");
    assert_eq!(
        loaded.front.packages,
        vec![
            (
                "corpus".to_string(),
                vec!["cli".to_string(), "suite".to_string(), "proof".to_string()]
            ),
            (
                "cli".to_string(),
                vec!["suite".to_string(), "proof".to_string()]
            ),
            ("suite".to_string(), Vec::new()),
            ("proof".to_string(), Vec::new()),
            ("std".to_string(), Vec::new()),
            ("compiler".to_string(), vec!["std".to_string()]),
        ],
        "the corpus tree's ply.pkg names it the `corpus` package over the CLI's and the two packages \
         it reads as the CLI does, the suite and the prove package"
    );
}

#[test]
fn the_committed_program_is_what_these_sources_build() {
    let identity = shipped::identity();
    let built = shipped::build().expect("the `ply` program builds");

    // Whatever else moves, the artifact has to carry the entry point the runner enters, and its
    // unit has to hold a body for it: the runner enters the unit and nothing else.
    let named = PathBuf::from(shipped::ARTIFACT);
    let (decoded, _) = ply_machine::artifact::decode(&built, &named).expect("it decodes");
    assert_eq!(decoded.entry_name(), Some("ply.main"));
    let unit = decoded
        .unit
        .as_ref()
        .expect("no compiled unit was embedded");
    let text = ply_codegen::c::bundle::unpack(&unit.text).expect("the unit unpacks");
    // The symbol as the unit's own table publishes it: `<name> <arity> <symbol> <entry>`.
    let symbol = text
        .lines()
        .find_map(|l| l.strip_prefix("\"ply.main ")?.split(' ').nth(1))
        .expect("the embedded unit says what it calls `ply.main`")
        .to_string();
    assert!(
        text.contains(&format!("Word {symbol}(PlyCtx *ctx")),
        "the embedded unit holds no body for `ply.main`, so nothing can be entered from it"
    );
    ply_machine::artifact::open(&decoded, &named).expect("it opens as the program it names");

    let artifact = shipped::committed();
    let digest = Path::new(shipped::DIR).join(shipped::DIGEST);
    if !artifact.is_file() {
        eprintln!(
            "no committed program at {}: a pull request carries none, and CI's `refresh` job \
             writes it on main. Build it here with `ply build crates/ply-cli/ply --entry ply.main \
             -o crates/ply-cli/bootstrap/ply.plyx --stamp crates/ply-cli/bootstrap/ply.digest`.",
            artifact.display()
        );
        return;
    }

    // A pull request that touches the compiler moves the identity, so the committed program is
    // behind it and the stage carries the run; only main, after `refresh`, has the two agreeing.
    if shipped::committed_digest().as_deref() != Some(identity.as_str()) {
        eprintln!(
            "{} names other sources than the ones in this tree; CI's `refresh` job rebuilds it on \
             main, and this run took the staged program",
            digest.display()
        );
        return;
    }
    let committed = std::fs::read(&artifact).unwrap();
    assert_eq!(
        committed.len(),
        built.len(),
        "the committed program is {} bytes and these sources build {}",
        committed.len(),
        built.len()
    );
    assert!(
        committed == built,
        "the committed program differs from what these sources build, at the same length"
    );
}

#[test]
fn the_compiler_is_on_the_shelf_under_its_own_root_and_nothing_may_shadow_it() {
    let names: Vec<&str> = ply_machine::shelf::sources()
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert!(names.contains(&"compiler.fmt"), "{names:?}");
    assert!(names.contains(&"std.path"), "{names:?}");
    assert!(ply_machine::shelf::is_shipped_name("compiler.fmt"));
    assert!(ply_machine::shelf::is_shipped_name("std.fs"));
    assert!(!ply_machine::shelf::is_shipped_name("compilers.fmt"));
    assert!(!ply_machine::shelf::is_shipped_name("fmt"));

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("compiler")).unwrap();
    write(dir.path(), "compiler/fmt.ply", "pub fn f() -> Int = 1\n");
    let err = ply_machine::load::load(dir.path())
        .expect_err("`compiler` is the built-in package's prefix");
    assert_eq!(err.diagnostics.len(), 1);
    assert_eq!(err.diagnostics[0].code, ply_eval::codes::PREFIX_COLLISION);
    assert!(
        err.diagnostics[0].message.contains("compiler.fmt"),
        "{:?}",
        err.diagnostics[0].message
    );

    // A name that merely starts with the letters is no collision.
    let ok = tempfile::tempdir().unwrap();
    write(ok.path(), "compilers.ply", "pub fn f() -> Int = 1\n");
    ply_machine::load::load(ok.path()).expect("`compilers` is an ordinary module name");
}

/// The shelf hands over text, and the front end and the emitter each parse it: every import a
/// shelved module makes must resolve there — an import of another package in full, or the
/// compiler package's own sibling.
#[test]
fn every_shelved_module_imports_the_shelf_under_the_names_it_files_them_under() {
    let filed: Vec<&str> = ply_machine::shelf::sources()
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    for (module, text) in ply_machine::shelf::sources() {
        for line in text.lines().filter_map(|l| l.strip_prefix("import ")) {
            let path: &str = line
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
                .next()
                .unwrap_or("");
            let resolves = filed.contains(&path)
                || (module.starts_with("compiler.")
                    && filed.contains(&format!("compiler.{path}").as_str()));
            assert!(
                resolves,
                "`{module}` imports `{path}`, which the shelf files under no such name: {filed:?}"
            );
        }
    }
}

#[test]
fn a_program_may_import_the_formatter_off_the_shelf() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        "import compiler.fmt (fmt_source)\n\n\
         fn main() -> Bool =\n  \
           match fmt_source(b\"fn  f()->Int=1\") {\n    \
             Ok(text) -> text != b\"fn  f()->Int=1\",\n    \
             Err(_) -> false,\n  \
           }\n",
    );
    let out = ply(dir.path()).args(["check", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(out.status.code(), Some(0), "{v:#}");
}

/// The program's own tests are the specification of every command it carries, and they need no
/// filesystem: each runs over a tree and a process the test hands it.
#[test]
fn the_programs_own_tests_pass() {
    // The whole closure, laid out the way a checkout has it: the CLI depends on the suite by path,
    // so a stage that carried only the CLI's own modules would not load.
    let dir = tempfile::tempdir().unwrap();
    shipped::lay_out(dir.path()).expect("the shipped closure lays out");

    let out = ply(&dir.path().join(shipped::ROOT))
        .args(["test", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(v["exit_code"], 0, "{}", red(&v));
    assert_eq!(v["summary"]["failed"], 0, "{}", red(&v));
    assert!(
        v["summary"]["passed"].as_u64().unwrap() > 0,
        "the program declares no test: {}",
        v["summary"]
    );
}

/// The whole report is a compiled unit's worth of timings; name the tests that failed and what
/// each said instead.
fn red(report: &Value) -> String {
    let Some(failures) = report["failures"].as_array() else {
        return report["summary"].to_string();
    };
    if failures.is_empty() {
        return report["summary"].to_string();
    }
    failures
        .iter()
        .map(|f| {
            format!(
                "{}: {} {}",
                f["name"].as_str().unwrap_or("?"),
                f["diagnostic"]["code"].as_str().unwrap_or(""),
                f["diagnostic"]["message"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The path argument every command defaults to is `.`, and a root bound to what that tidies to has
/// to be a directory that resolves: an empty one is `E0454` before the program runs at all.
#[test]
fn every_ported_command_answers_with_no_path_argument() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "m.ply", "pub fn one() -> Int = 1\n");

    // `ply fmt --check` over a tree that needs no rewriting writes nothing; every other row does.
    for (args, writes) in [
        (vec!["defs"], true),
        (vec!["defs", "--json"], true),
        (vec!["hash"], true),
        (vec!["hash", "--json"], true),
        (vec!["doc", "one"], true),
        (vec!["doc", "one", "--json"], true),
        (vec!["show", "one"], true),
        (vec!["show", "one", "--json"], true),
        (vec!["callers", "one"], true),
        (vec!["callers", "one", "--json"], true),
        (vec!["std"], true),
        (vec!["std", "--json"], true),
        (vec!["std", "--digest"], true),
        (vec!["explain", "E0001"], true),
        (vec!["hosts"], true),
        (vec!["hosts", "--json"], true),
        (vec!["hosts", "--digest"], true),
        (vec!["cache", "stats"], true),
        (vec!["cache", "stats", "--json"], true),
        (vec!["fmt", "--check"], false),
    ] {
        let out = ply(dir.path()).args(&args).output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "`ply {}` exited {:?}\n{}",
            args.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !writes || !out.stdout.is_empty(),
            "`ply {}` wrote nothing",
            args.join(" ")
        );
    }
}

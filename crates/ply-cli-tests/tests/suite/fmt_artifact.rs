//! The committed `ply fmt` program, and the shelf it is built against.
//!
//! `PLY_C_BOOTSTRAP_REFRESH=1` rewrites `crates/ply-cli/bootstrap` with what these sources build;
//! CI does so on main after each merge, so no pull request carries the artifact.

use assert_cmd::Command;
use ply_cli::shipped;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

#[test]
fn the_committed_program_is_what_these_sources_build() {
    let identity = shipped::identity();
    let built = shipped::build().expect("the `ply fmt` program builds");

    // Whatever else moves, the artifact has to carry the entry point the runner enters, and its
    // unit has to hold a body for it: the runner enters the unit and nothing else.
    let named = PathBuf::from(shipped::ARTIFACT);
    let (decoded, _) = ply_cli::artifact::decode(&built, &named).expect("it decodes");
    assert_eq!(decoded.entry_name(), Some("fmt.main"));
    let unit = decoded
        .unit
        .as_ref()
        .expect("no compiled unit was embedded");
    let text = ply_codegen::c::bundle::unpack(&unit.text).expect("the unit unpacks");
    assert!(
        text.contains("ply_fmt_main("),
        "the embedded unit holds no body for `fmt.main`, so nothing can be entered from it"
    );
    ply_cli::artifact::open(&decoded, &named).expect("it opens as the program it names");

    let artifact = shipped::committed();
    let digest = Path::new(shipped::DIR).join(shipped::DIGEST);
    if std::env::var("PLY_C_BOOTSTRAP_REFRESH").is_ok() {
        std::fs::create_dir_all(shipped::DIR).unwrap();
        std::fs::write(&artifact, &built).unwrap();
        std::fs::write(&digest, format!("{identity}\n")).unwrap();
        eprintln!(
            "the `ply fmt` program was written to {} ({} bytes)",
            artifact.display(),
            built.len()
        );
        return;
    }
    if !artifact.is_file() {
        eprintln!(
            "no committed program at {}: a pull request carries none, and CI's `refresh` job \
             writes it on main. Build it here with PLY_C_BOOTSTRAP_REFRESH=1.",
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
    let names: Vec<&str> = shipped::sources().iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"compiler.fmt"), "{names:?}");
    assert!(names.contains(&"std.path"), "{names:?}");
    assert!(shipped::is_shipped_name("compiler.fmt"));
    assert!(shipped::is_shipped_name("std.fs"));
    assert!(!shipped::is_shipped_name("compilers.fmt"));
    assert!(!shipped::is_shipped_name("fmt"));

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("compiler")).unwrap();
    write(dir.path(), "compiler/fmt.ply", "pub fn f() -> Int = 1\n");
    let err = ply_cli::load::load(dir.path()).expect_err("`compiler` is reserved");
    assert_eq!(err.diagnostics.len(), 1);
    assert_eq!(
        err.diagnostics[0].code,
        ply_span::codes::RESERVED_MODULE_NAME
    );
    assert!(
        err.diagnostics[0].message.contains("compiler.fmt"),
        "{:?}",
        err.diagnostics[0].message
    );

    // A name that merely starts with the letters is not reserved.
    let ok = tempfile::tempdir().unwrap();
    write(ok.path(), "compilers.ply", "pub fn f() -> Int = 1\n");
    ply_cli::load::load(ok.path()).expect("`compilers` is an ordinary module name");
}

/// The shelf hands over text, and the front end and the emitter each parse it: a module filed
/// under `compiler.x` whose text still imports `x` resolves one way for one reader and another
/// way for the other, and every body that calls across it is refused.
#[test]
fn every_shelved_module_imports_the_shelf_under_the_names_it_files_them_under() {
    let filed: Vec<&str> = shipped::sources().iter().map(|(n, _)| n.as_str()).collect();
    for (module, text) in shipped::sources() {
        for line in text.lines().filter_map(|l| l.strip_prefix("import ")) {
            let path: &str = line
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
                .next()
                .unwrap_or("");
            assert!(
                filed.contains(&path),
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

/// The program's own tests are the walk's specification, and they need no filesystem.
#[test]
fn the_fmt_programs_own_tests_pass() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .join("crates/ply-cli/ply/fmt.ply");
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(&source, dir.path().join("fmt.ply")).unwrap();

    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(v["exit_code"], 0, "{v:#}");
    assert_eq!(v["summary"]["failed"], 0, "{v:#}");
    assert!(
        v["summary"]["passed"].as_u64().unwrap() > 0,
        "the program declares no test: {v:#}"
    );
}

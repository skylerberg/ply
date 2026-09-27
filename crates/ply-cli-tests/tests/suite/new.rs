//! `ply new`: the first thirty seconds of a package. The scaffold is only as good as what it
//! makes, so every test here goes on to build over the tree a run created.

use crate::harness::{json_of, ply};
use serde_json::Value;
use tempfile::TempDir;

/// A scratch directory, no project of its own.
fn scratch() -> TempDir {
    tempfile::tempdir().expect("a scratch directory")
}

fn read(dir: &std::path::Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name))
        .unwrap_or_else(|e| panic!("`{name}` was not written: {e}"))
}

#[test]
fn a_new_package_checks_tests_and_formats_as_it_stands() {
    let dir = scratch();
    let out = ply(dir.path()).args(["new", "demo"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "   created demo · ply.pkg, main.ply\n   next: cd demo && ply test\n"
    );

    let manifest = read(dir.path(), "demo/ply.pkg");
    assert!(manifest.contains("name: \"demo\""), "{manifest}");
    assert!(manifest.contains("runtime: { major: 0,"), "{manifest}");
    let module = read(dir.path(), "demo/main.ply");
    assert!(module.contains("fn main() -> Unit"), "{module}");
    assert!(module.contains("test \"the first test\""), "{module}");

    // The scaffold is what the language already accepts: a check, a green test, and `fmt`-clean.
    for (args, want) in [
        (vec!["check", "demo"], "checked 2 modules"),
        (vec!["test", "demo"], "1 passed"),
        (vec!["fmt", "--check", "demo"], ""),
    ] {
        let out = ply(dir.path()).args(&args).output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.status.code(), Some(0), "{args:?}: {text}");
        assert!(text.contains(want), "{args:?}: {text}");
    }
}

#[test]
fn a_library_has_no_main_and_one_public_definition() {
    let dir = scratch();
    let out = ply(dir.path())
        .args(["new", "core", "--lib"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let module = read(dir.path(), "core/lib.ply");
    assert!(module.contains("pub fn double"), "{module}");
    assert!(!module.contains("fn main"), "{module}");
    let out = ply(dir.path()).args(["test", "core"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    // Nothing to run, and the run says so rather than picking the library's first definition.
    let out = ply(dir.path()).args(["run", "core"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no `main` to run"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_json_answer_names_the_package_the_directory_and_the_files() {
    let dir = scratch();
    let out = ply(dir.path())
        .args(["new", "store/orders", "--json"])
        .output()
        .unwrap();
    let v: Value = json_of(&out);
    assert_eq!(v["command"], "new");
    assert_eq!(v["ok"], true);
    assert_eq!(v["exit_code"], 0);
    assert_eq!(v["name"], "orders");
    assert_eq!(v["path"], "store/orders");
    assert_eq!(v["lib"], false);
    assert_eq!(v["files"], serde_json::json!(["ply.pkg", "main.ply"]));
    assert!(dir.path().join("store/orders/ply.pkg").exists());
}

#[test]
fn a_name_the_package_shape_does_not_allow_is_refused_before_anything_is_written() {
    let dir = scratch();
    for bad in ["Bad", "my-app", "9lives", "a..b"] {
        let out = ply(dir.path()).args(["new", bad]).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{bad}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("is not a package name"), "{bad}: {stderr}");
        assert!(stderr.contains("E0502"), "{bad}: {stderr}");
        assert!(!dir.path().join(bad).exists(), "`{bad}` was made anyway");
    }

    // The name can be given instead of derived, which is how a directory that is not a package
    // name becomes one.
    let out = ply(dir.path())
        .args(["new", "lib-src", "--name", "core"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(read(dir.path(), "lib-src/ply.pkg").contains("name: \"core\""));
}

#[test]
fn a_directory_that_is_already_there_is_never_written_into() {
    let dir = scratch();
    std::fs::write(dir.path().join("demo.ply"), "fn main() -> Int = 1\n").unwrap();
    std::fs::create_dir(dir.path().join("demo")).unwrap();
    std::fs::write(dir.path().join("demo/keep"), "mine\n").unwrap();

    let out = ply(dir.path()).args(["new", "demo"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("is already there"), "{stderr}");
    assert_eq!(read(dir.path(), "demo/keep"), "mine\n");
    assert!(!dir.path().join("demo/ply.pkg").exists());
}

#[test]
fn a_scaffolded_project_is_a_package_a_dependency_can_be_added_to() {
    let dir = scratch();
    for (args, want) in [(vec!["new", "core", "--lib"], 0), (vec!["new", "app"], 0)] {
        let out = ply(dir.path()).args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(want), "{args:?}");
    }
    // The generated manifest is a manifest: adding a dependency to it is an edit a reader can
    // make, and the root over it then reaches the library's own module.
    std::fs::write(
        dir.path().join("app/ply.pkg"),
        "import std.pkg (Manifest)\n\
         fn package() -> Manifest = {name: \"app\", version: {major: 0, minor: 1, patch: 0}, prefix: None, runtime: {major: 0, minor: 1, patch: 0}, dependencies: [{name: \"core\", prefix: None, min: {major: 0, minor: 1, patch: 0}, source: Path(\"../core\")}], entry: None}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("app/main.ply"),
        "import core.lib\n\nfn main() -> Int = lib::double(21)\n",
    )
    .unwrap();
    let out = ply(dir.path()).args(["run", "app"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("42"));
}

use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").expect("the binary is built");
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn manifest(name: &str, deps: &str) -> String {
    format!(
        "import std.pkg (Manifest)\nfn package() -> Manifest = {{name: \"{name}\", version: {{major: 0, minor: 0, patch: 1}}, prefix: None, runtime: {{major: 0, minor: 0, patch: 1}}, dependencies: [{deps}], entry: None}}\n"
    )
}

fn dep(name: &str, path: &str) -> String {
    format!(
        "{{name: \"{name}\", prefix: None, min: {{major: 0, minor: 0, patch: 1}}, source: Path(\"{path}\")}}"
    )
}

/// app -> lib -> base, with lib using base and a sibling of its own.
fn graph() -> TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    for pkg in ["app", "lib", "base"] {
        std::fs::create_dir(root.join(pkg)).unwrap();
    }
    std::fs::write(
        root.join("app/ply.pkg"),
        manifest("app", &dep("lib", "../lib")),
    )
    .unwrap();
    std::fs::write(
        root.join("app/main.ply"),
        "import lib.answer\nfn main() -> Int = answer::answer()\ntest \"answers\" { assert_eq(answer::answer(), 42) }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("lib/ply.pkg"),
        manifest("lib", &dep("base", "../base")),
    )
    .unwrap();
    std::fs::write(
        root.join("lib/answer.ply"),
        "import base.deep\nimport extra\npub fn answer() -> Int = deep::deep() + extra::hidden()\n",
    )
    .unwrap();
    std::fs::write(root.join("lib/extra.ply"), "pub fn hidden() -> Int = 35\n").unwrap();
    std::fs::write(root.join("base/ply.pkg"), manifest("base", "")).unwrap();
    std::fs::write(root.join("base/deep.ply"), "pub fn deep() -> Int = 7\n").unwrap();
    dir
}

#[test]
fn a_path_dependency_serves_its_modules_and_runs_them() {
    let dir = graph();
    for (args, want) in [
        (vec!["check", "app"], "checked 4 modules"),
        (vec!["run", "app"], "42"),
        (vec!["test", "app"], "1 passed"),
        (vec!["build", "app", "-o", "app.plyx"], "built main.main"),
    ] {
        let out = ply(dir.path()).args(&args).output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.status.success() && text.contains(want),
            "{args:?}: {text}"
        );
    }
}

#[test]
fn a_transitive_dependency_the_importer_does_not_declare_is_refused() {
    let dir = graph();
    std::fs::write(
        dir.path().join("app/main.ply"),
        "import base.deep\nfn main() -> Int = deep::deep()\n",
    )
    .unwrap();
    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0132"), "{err}");
    assert!(err.contains("`base`"), "{err}");
    assert!(err.contains("`app`"), "{err}");
}

#[test]
fn a_dependency_without_a_manifest_is_unusable_where_it_is_declared() {
    let dir = graph();
    std::fs::remove_file(dir.path().join("lib/ply.pkg")).unwrap();
    let out = ply(dir.path()).args(["run", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0135"), "{err}");
    assert!(err.contains("ply.pkg"), "{err}");
}

#[test]
fn a_root_module_may_not_squat_on_a_dependencys_prefix() {
    let dir = graph();
    std::fs::create_dir(dir.path().join("app/lib")).unwrap();
    std::fs::write(
        dir.path().join("app/lib/mine.ply"),
        "fn mine() -> Int = 1\n",
    )
    .unwrap();
    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0133"), "{err}");
}

#[test]
fn a_dependency_cycle_is_refused_naming_the_cycle() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    std::fs::create_dir(root.join("app")).unwrap();
    std::fs::create_dir(root.join("lib")).unwrap();
    std::fs::write(
        root.join("app/ply.pkg"),
        manifest("app", &dep("lib", "../lib")),
    )
    .unwrap();
    std::fs::write(root.join("app/main.ply"), "fn main() -> Int = 1\n").unwrap();
    std::fs::write(
        root.join("lib/ply.pkg"),
        manifest("lib", &dep("app", "../app")),
    )
    .unwrap();
    std::fs::write(root.join("lib/x.ply"), "pub fn x() -> Int = 1\n").unwrap();
    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0134"), "{err}");
}

#[test]
fn a_non_path_dependency_is_told_what_resolves_today() {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(
        dir.path().join("ply.pkg"),
        manifest(
            "app",
            "{name: \"glib\", prefix: None, min: {major: 0, minor: 0, patch: 1}, source: Git(\"https://example.com/glib\", \"abc123\")}",
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join("main.ply"), "fn main() -> Int = 1\n").unwrap();
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0135"), "{err}");
    assert!(err.contains("path dependencies"), "{err}");
}

#[test]
fn a_dependency_may_not_reach_back_into_the_root_package() {
    let dir = graph();
    std::fs::write(dir.path().join("app/extra.ply"), "fn shared() -> Int = 9\n").unwrap();
    std::fs::write(
        dir.path().join("lib/answer.ply"),
        "import extra\npub fn answer() -> Int = extra::shared()\n",
    )
    .unwrap();
    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0132"), "{err}");
    assert!(err.contains("reach back"), "{err}");
}

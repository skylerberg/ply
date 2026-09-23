use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

const GOOD: &str = r#"import std.pkg (Manifest)
fn package() -> Manifest = {name: "app", version: {major: 0, minor: 1, patch: 0}, prefix: None, runtime: {major: 0, minor: 1, patch: 0}, dependencies: [], entry: None}
"#;

fn project(manifest: Option<&str>) -> TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("m.ply"), "fn main() -> Int = 1\n").expect("the module");
    if let Some(text) = manifest {
        std::fs::write(dir.path().join("ply.pkg"), text).expect("the manifest");
    }
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").expect("the binary is built");
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

#[test]
fn a_project_without_a_manifest_is_the_anonymous_package() {
    let dir = project(None);
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_well_formed_manifest_loads_on_both_paths() {
    let dir = project(Some(GOOD));
    for command in ["check", "test"] {
        let out = ply(dir.path()).arg(command).output().unwrap();
        assert!(
            out.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn a_manifest_that_is_not_one_manifest_fn_is_refused_where_it_stands() {
    let dir = project(Some("fn package() -> Int = 1\n"));
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0129"), "{err}");
    assert!(err.contains("ply.pkg"), "{err}");
}

#[test]
fn a_manifest_body_that_runs_is_not_a_value() {
    let dir = project(Some(
        "import std.pkg (Manifest)\nfn package() -> Manifest = {name: greet(\"x\"), version: {major: 0, minor: 0, patch: 1}, prefix: None, runtime: {major: 0, minor: 0, patch: 1}, dependencies: [], entry: None}\n",
    ));
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0130"), "{err}");
}

#[test]
fn a_manifest_field_that_fails_validation_is_named() {
    let dir = project(Some(
        "import std.pkg (Manifest)\nfn package() -> Manifest = {name: \"\", version: {major: 0, minor: 0, patch: 1}, prefix: None, runtime: {major: 0, minor: 0, patch: 1}, dependencies: [], entry: None}\n",
    ));
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0131"), "{err}");
    assert!(err.contains("`name` must not be empty"), "{err}");
}

#[test]
fn a_manifest_with_a_syntax_error_reports_the_parse() {
    let dir = project(Some("fn package() -> Manifest = {name: \"app\"\n"));
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("E0129"), "{err}");
    assert!(err.contains("E0001") || err.contains("E0002"), "{err}");
}

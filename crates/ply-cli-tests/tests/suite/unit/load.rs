use ply_cli::load::*;
use ply_span::{Symbol, codes};
use ply_ty::ModuleName;
use std::fs;
use std::path::{Path, PathBuf};

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn names(loaded: &Loaded) -> Vec<String> {
    loaded
        .modules()
        .iter()
        .map(|m| m.name.to_string())
        .collect()
}

#[test]
fn a_directory_becomes_one_module_per_file_named_after_its_path() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "b.ply", "pub fn b() -> Int = 2\n");
    write(dir.path(), "a.ply", "pub fn a() -> Int = 1\n");
    write(dir.path(), "store/orders.ply", "fn c() -> Int = 3\n");
    write(dir.path(), "notes.txt", "ignored");

    let loaded = load(dir.path()).unwrap();
    assert_eq!(names(&loaded), ["a", "b", "store.orders"]);
    assert_eq!(loaded.module_count(), 3);
    assert!(
        loaded
            .check
            .defs
            .contains_key(&Symbol::new("store.orders.c"))
    );
}

#[test]
fn a_name_in_one_file_is_invisible_in_another_until_it_is_imported() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.ply", "pub fn a() -> Int = 1\n");
    write(dir.path(), "b.ply", "fn b() -> Int = a()\n");

    let err = load(dir.path()).unwrap_err();
    assert!(
        err.diagnostics
            .iter()
            .any(|d| d.code == codes::UNKNOWN_NAME),
        "a directory must no longer be concatenated: {:?}",
        err.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );

    write(dir.path(), "b.ply", "import a\nfn b() -> Int = a::a()\n");
    let loaded = load(dir.path()).unwrap();
    assert!(loaded.check.defs.contains_key(&Symbol::new("b.b")));
}

#[test]
fn a_file_argument_roots_the_project_at_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/one.ply", "fn one() -> Int = 1\n");
    write(dir.path(), "src/two.ply", "fn two() -> Int = 2\n");

    let file = dir.path().join("src/one.ply");
    let loaded = load(&file).unwrap();
    assert_eq!(loaded.root, dir.path().join("src"));
    assert_eq!(names(&loaded), ["one"]);
    assert_eq!(loaded.check.defs.len(), 1);
}

#[test]
fn hidden_directories_are_not_part_of_the_program() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "ok.ply", "fn ok() -> Int = 1\n");
    write(
        dir.path(),
        ".ply-cache/stale.ply",
        "this is not even valid ply\n",
    );
    write(dir.path(), ".git/x.ply", "nor is this\n");

    let loaded = load(dir.path()).unwrap();
    assert_eq!(names(&loaded), ["ok"]);
}

#[test]
fn a_path_that_cannot_name_a_module_is_e0111_against_the_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "my-notes.ply", "fn f() -> Int = 1\n");

    let err = load(dir.path()).unwrap_err();
    assert_eq!(err.diagnostics.len(), 1);
    assert_eq!(err.diagnostics[0].code, codes::INVALID_MODULE_PATH);
    let span = err.diagnostics[0].primary_span().unwrap();
    assert!(!span.is_dummy(), "E0111 must point at the file it is about");
    assert!(err.sources.get(span.source).is_some());
}

#[test]
fn a_directory_segment_that_is_not_an_identifier_is_also_e0111() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "not-a-module/f.ply", "fn f() -> Int = 1\n");
    let err = load(dir.path()).unwrap_err();
    assert_eq!(err.diagnostics[0].code, codes::INVALID_MODULE_PATH);
    assert!(err.diagnostics[0].message.contains("not-a-module"));
}

#[test]
fn a_missing_path_is_a_diagnostic_rather_than_a_panic() {
    let err = load(Path::new("definitely/not/here.ply")).unwrap_err();
    assert_eq!(err.diagnostics.len(), 1);
    assert_eq!(err.diagnostics[0].code, codes::RUNTIME_ERROR);
    assert!(
        err.diagnostics[0]
            .message
            .contains("definitely/not/here.ply")
    );
}

#[test]
fn an_empty_directory_says_what_to_do_about_it() {
    let dir = tempfile::tempdir().unwrap();
    let err = load(dir.path()).unwrap_err();
    assert!(err.diagnostics[0].message.contains("no `.ply` files"));
    assert!(!err.diagnostics[0].notes.is_empty());
}

#[test]
fn a_syntax_error_still_hands_back_the_sources_its_spans_point_into() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "bad.ply", "fn broken( = 1\n");
    let err = load(dir.path()).unwrap_err();
    assert!(!err.diagnostics.is_empty());
    let span = err.diagnostics[0].primary_span().unwrap();
    assert!(err.sources.get(span.source).is_some());
}

#[test]
fn a_type_error_is_reported_after_a_clean_parse() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "bad.ply", "fn f() -> Int = 1 + true\n");
    let err = load(dir.path()).unwrap_err();
    assert!(
        err.diagnostics
            .iter()
            .any(|d| d.code == codes::TYPE_MISMATCH)
    );
}

#[test]
fn a_module_cycle_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "a.ply",
        "import b\npub fn a() -> Int = b::b()\n",
    );
    write(
        dir.path(),
        "b.ply",
        "import a\npub fn b() -> Int = a::a()\n",
    );
    let err = load(dir.path()).unwrap_err();
    assert!(
        err.diagnostics
            .iter()
            .any(|d| d.code == codes::MODULE_CYCLE)
    );
}

#[test]
fn entry_points_finds_main_in_whatever_module_declares_it() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lib.ply", "pub fn one() -> Int = 1\n");
    write(
        dir.path(),
        "app.ply",
        "import lib\nfn main() -> Int = lib::one()\n",
    );

    let loaded = load(dir.path()).unwrap();
    let mains = loaded.entry_points();
    assert_eq!(mains.len(), 1);
    assert_eq!(mains[0].name.as_str(), "app.main");
    assert_eq!(mains[0].module.as_str(), "app");
}

#[test]
fn two_modules_may_each_declare_main() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "one.ply", "fn main() -> Int = 1\n");
    write(dir.path(), "two.ply", "fn main() -> Int = 2\n");

    let loaded = load(dir.path()).unwrap();
    let mains: Vec<&str> = loaded
        .entry_points()
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(mains, ["one.main", "two.main"]);
}

#[test]
fn defs_and_tests_can_be_read_back_per_module() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "a.ply",
        "fn a() -> Int = 1\ntest \"a\" { assert_eq(a(), 1) }\n",
    );
    write(dir.path(), "b.ply", "fn b() -> Int = 2\n");

    let loaded = load(dir.path()).unwrap();
    let a = ModuleName::from_dotted("a");
    assert_eq!(loaded.defs_of(&a).len(), 1);
    assert_eq!(loaded.tests_of(&a).len(), 1);
    assert_eq!(loaded.tests_of(&ModuleName::from_dotted("b")).len(), 0);
}

#[test]
fn a_leading_dot_slash_never_reaches_a_rendered_span() {
    assert_eq!(tidy(Path::new("./src/a.ply")), PathBuf::from("src/a.ply"));
    assert_eq!(tidy(Path::new("src/a.ply")), PathBuf::from("src/a.ply"));
}

/// An empty path names no directory, and a `--fs` root bound to one resolves against nothing.
#[test]
fn the_working_directory_tidies_to_itself_rather_than_to_nothing() {
    assert_eq!(tidy(Path::new(".")), PathBuf::from("."));
    assert_eq!(tidy(Path::new("./")), PathBuf::from("."));
    assert_eq!(project_root(Path::new(".")), PathBuf::from("."));
}

#[test]
fn the_texts_are_every_module_the_port_answered_the_shipped_ones_included() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "a.ply",
        "import std.json\npub fn a() -> Int = 1\n",
    );
    write(dir.path(), "b.ply", "import a\nfn b() -> Int = a::a()\n");

    let loaded = load(dir.path()).unwrap();
    let texts = loaded.texts();
    assert_eq!(texts.len(), loaded.module_count());
    assert!(texts.iter().any(|(name, _)| name == "std.json"));
    assert!(texts.contains(&(
        "b".to_string(),
        "import a\nfn b() -> Int = a::a()\n".to_string()
    )));
}

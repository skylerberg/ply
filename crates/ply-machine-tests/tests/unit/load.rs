use crate::sketch::{Sketch, loaded};
use ply_eval::ModuleName;
use ply_machine::load::*;
use std::path::{Path, PathBuf};

#[test]
fn defs_and_tests_can_be_read_back_per_module() {
    let loaded = loaded(
        Sketch::default()
            .module(
                "a",
                "a.ply",
                "fn a() -> Int = 1\ntest \"a\" { assert_eq(a(), 1) }\n",
            )
            .module("b", "b.ply", "fn b() -> Int = 2\n")
            .def("a", "a", Vec::new())
            .def("b", "b", Vec::new())
            .test("a", "a"),
    );
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
fn the_texts_are_every_module_the_front_end_answered_in_the_order_of_their_sources() {
    let loaded = loaded(
        Sketch::default()
            .module("a", "a.ply", "pub fn a() -> Int = 1\n")
            .module("std.json", "std/json.ply", "pub type Json = | Null\n")
            .module("b", "b.ply", "import a\nfn b() -> Int = a::a()\n"),
    );
    let texts = loaded.texts();
    assert_eq!(texts.len(), loaded.module_count());
    assert_eq!(
        texts
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["a", "std.json", "b"]
    );
    assert!(texts.contains(&(
        "b".to_string(),
        "import a\nfn b() -> Int = a::a()\n".to_string()
    )));
}

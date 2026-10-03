use ply_eval::{ModuleName, codes};
use ply_machine::load::*;
use std::path::{Path, PathBuf};

/// The program of `files`, `(path, text)` below one root, as the machine holds it over the
/// builder's answer for it.
fn loaded(files: &[(&str, &str)]) -> Loaded {
    let files: Vec<(String, String)> = files
        .iter()
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .collect();
    let bytes = ply_machine::builds::answered(&files)
        .unwrap_or_else(|d| panic!("the builder answers: {}", d.message));
    let answer = ply_machine::runnable::decode(&bytes)
        .unwrap_or_else(|why| panic!("the answer reads: {why}"));
    ply_machine::driver::load_over_analysis_taken(PathBuf::from("."), answer.front)
        .unwrap_or_else(|e| panic!("it loads: {:?}", e.diagnostics))
}

#[test]
fn entry_points_finds_main_in_whatever_module_declares_it() {
    let loaded = loaded(&[
        ("lib.ply", "pub fn one() -> Int = 1\n"),
        ("app.ply", "import lib\nfn main() -> Int = lib::one()\n"),
    ]);
    let mains = loaded.entry_points();
    assert_eq!(mains.len(), 1);
    assert_eq!(mains[0].name.as_str(), "app.main");
    assert_eq!(mains[0].module.as_str(), "app");
}

#[test]
fn two_modules_may_each_declare_main() {
    let loaded = loaded(&[
        ("one.ply", "fn main() -> Int = 1\n"),
        ("two.ply", "fn main() -> Int = 2\n"),
    ]);
    let mains: Vec<&str> = loaded
        .entry_points()
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(mains, ["one.main", "two.main"]);
}

#[test]
fn defs_and_tests_can_be_read_back_per_module() {
    let loaded = loaded(&[
        (
            "a.ply",
            "fn a() -> Int = 1\ntest \"a\" { assert_eq(a(), 1) }\n",
        ),
        ("b.ply", "fn b() -> Int = 2\n"),
    ]);
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
fn the_texts_are_every_module_the_front_end_answered_the_shipped_ones_included() {
    let loaded = loaded(&[
        ("a.ply", "import std.json\npub fn a() -> Int = 1\n"),
        ("b.ply", "import a\nfn b() -> Int = a::a()\n"),
    ]);
    let texts = loaded.texts();
    assert_eq!(texts.len(), loaded.module_count());
    assert!(texts.iter().any(|(name, _)| name == "std.json"));
    assert!(texts.contains(&(
        "b".to_string(),
        "import a\nfn b() -> Int = a::a()\n".to_string()
    )));
}

#[test]
fn a_definition_no_root_reaches_is_warned_once_at_its_name() {
    let loaded = loaded(&[(
        "m.ply",
        "pub fn api() -> Int = shared()\n\
         fn shared() -> Int = 1\n\
         fn dead() -> Int = deader()\n\
         fn deader() -> Int = 2\n\
         fn tested() -> Int = 3\n\
         fn lawful(x: Int) -> Int = x\n\
         fn positive(x: Int) -> Bool = x > 0\n\
         pub fn checked(x: Int) -> Int requires positive(x) = x\n\
         type Pair = { a: Int }\n\
         pub fn make() -> Int = {\n\
         \x20 let f = |x: Int| -> Pair { { a: x } };\n\
         \x20 f(1).a\n\
         }\n\
         fn _kept() -> Int = 4\n\
         fn main() -> Int = 5\n\
         type Unused = | Nothing\n\
         test \"calls it\" { assert_eq(tested(), 3) }\n\
         law \"it is the identity\" forall (x: Int) { lawful(x) == x }\n",
    )]);
    let named: Vec<(&str, String)> = loaded
        .frontend
        .warnings
        .iter()
        .filter(|d| d.code == codes::UNUSED_DEFINITION)
        .inspect(|d| assert_eq!(d.severity, ply_eval::Severity::Warning))
        .map(|d| {
            let at = d.primary_span().expect("the name is labelled");
            (d.message.as_str(), loaded.sources.snippet(at).into_owned())
        })
        .collect();
    let unused: Vec<(&str, &str)> = named.iter().map(|(m, s)| (*m, s.as_str())).collect();
    assert_eq!(
        unused,
        [
            ("fn `m.dead` is never used", "dead"),
            ("fn `m.deader` is never used", "deader"),
            ("type `m.Unused` is never used", "Unused"),
        ]
    );
}

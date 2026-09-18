use ply_cli::EXIT_COMPILE_ERROR;
use ply_cli::commands::check::*;
use ply_cli::commands::common::report_load_error;
use ply_cli::load::{Loaded, load};
use ply_cli::style::Style;
use serde_json::{Value, json};
use std::path::Path;

fn fixture(text: &str) -> (tempfile::TempDir, Loaded) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), text).unwrap();
    let loaded = load(dir.path()).unwrap();
    (dir, loaded)
}

/// The report also carries the prelude's effects, so an effect is found by name, not position.
fn effect_named<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["effects"]
        .as_array()
        .expect("effects is a list")
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("no effect named {name} in {report:#}"))
}

#[test]
fn the_json_report_publishes_signatures_and_footprints() {
    let (_dir, loaded) = fixture(
        "effect db {\n  read all[t]() -> List<Int>\n}\n\
         fn total() -> Int / {db.read[users]} = len(db.all[users]())\n\
         test \"total counts\" {\n  handle { assert_eq(total(), 0) } with { db.all[users]() -> [] }\n}\n",
    );
    let v = report_json(&loaded, &[]);
    assert_eq!(v["command"], "check");
    assert_eq!(v["ok"], true);
    assert_eq!(v["definitions"][0]["name"], "m.total");
    assert_eq!(v["definitions"][0]["module"], "m");
    assert_eq!(v["definitions"][0]["simple_name"], "total");
    assert_eq!(v["definitions"][0]["footprint"], "{m.db.read[users]}");
    let db = effect_named(&v, "m.db");
    assert_eq!(db["simple_name"], "db");
    assert_eq!(db["nondet"], false);
    assert_eq!(v["tests"][0]["name"], "total counts");
    assert_eq!(v["tests"][0]["key"], "m.total counts");
    assert_eq!(v["tests"][0]["footprint"], "{}");
}

#[test]
fn a_pure_definition_reports_an_empty_footprint() {
    let (_dir, loaded) = fixture("fn double(x: Int) -> Int = x * 2\n");
    let v = report_json(&loaded, &[]);
    assert_eq!(v["definitions"][0]["type"], "(Int) -> Int");
    assert_eq!(v["definitions"][0]["footprint"], "{}");
    assert!(v["definitions"][0]["atoms"].as_array().unwrap().is_empty());
}

#[test]
fn a_nondet_effect_is_flagged_in_the_report() {
    let (_dir, loaded) =
        fixture("nondet effect wall {\n  read now() -> Int\n}\nfn f() -> Int = 1\n");
    let v = report_json(&loaded, &[]);
    let wall = effect_named(&v, "m.wall");
    assert_eq!(wall["simple_name"], "wall");
    assert_eq!(wall["nondet"], true);
}

#[test]
fn every_module_is_reported_with_its_file_and_its_imports() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("store")).unwrap();
    std::fs::write(
        dir.path().join("store/orders.ply"),
        "pub fn place() -> Int = 1\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("app.ply"),
        "import store.orders\nfn run() -> Int = orders::place()\n",
    )
    .unwrap();

    let loaded = load(dir.path()).unwrap();
    let v = report_json(&loaded, &[]);
    let modules = v["modules"].as_array().unwrap();
    assert_eq!(modules.len(), 2);
    assert_eq!(modules[0]["name"], "app");
    assert_eq!(modules[0]["imports"], json!(["store.orders"]));
    assert_eq!(modules[1]["name"], "store.orders");
    assert!(
        modules[1]["file"]
            .as_str()
            .unwrap()
            .ends_with("store/orders.ply")
    );

    // Definitions follow the run's files and each file's source order.
    let names: Vec<&str> = v["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["app.run", "store.orders.place"]);
}

#[test]
fn two_modules_may_reuse_a_name_without_colliding() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.ply"), "fn total() -> Int = 1\n").unwrap();
    std::fs::write(dir.path().join("b.ply"), "fn total() -> Int = 2\n").unwrap();

    let loaded = load(dir.path()).unwrap();
    let v = report_json(&loaded, &[]);
    let names: Vec<&str> = v["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["a.total", "b.total"]);
}

#[test]
fn a_reuse_fn_is_refused_only_for_a_copy_its_own_body_causes() {
    // Kept: the append is the last use of a parameter, whatever the caller does with it.
    let (_dir, loaded) = fixture(
        "reuse fn grow(xs: List<Int>, n: Int) -> List<Int> = push(xs, n)\n\
         fn keep(xs: List<Int>) -> Int = len(grow(xs, 1)) + len(xs)\n",
    );
    assert!(loaded.promised);
    assert!(ply_cli::costs::promises(&loaded.program, &loaded.resolved).is_empty());

    // Broken: the binding is read again after the append, inside the promised body.
    let (_dir, loaded) = fixture(
        "reuse fn grow(xs: List<Int>, n: Int) -> List<Int> = {\n\
         \x20 let ys = push(xs, n);\n\
         \x20 if len(xs) < 0 { xs } else { ys }\n\
         }\n",
    );
    let broken = ply_cli::costs::promises(&loaded.program, &loaded.resolved);
    assert_eq!(broken.len(), 1, "{broken:#?}");
    assert_eq!(broken[0].code, ply_span::codes::REUSE_BROKEN);
    assert!(broken[0].message.contains("`grow` is a `reuse fn`"));
    assert!(broken[0].notes.iter().any(|n| n.contains("last use")));
    let err = ply_cli::load::LoadError {
        sources: loaded.sources.clone(),
        diagnostics: broken,
    };
    assert_eq!(
        report_load_error("check", &err, true, Style::plain()),
        EXIT_COMPILE_ERROR
    );

    // The same body without the marker is what `--costs` reports, not an error.
    let (_dir, loaded) = fixture(
        "fn grow(xs: List<Int>, n: Int) -> List<Int> = {\n\
         \x20 let ys = push(xs, n);\n\
         \x20 if len(xs) < 0 { xs } else { ys }\n\
         }\n",
    );
    assert!(!loaded.promised);
    assert!(ply_cli::costs::promises(&loaded.program, &loaded.resolved).is_empty());
}

#[test]
fn a_broken_module_never_reaches_the_report() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), "fn f() -> Int = true\n").unwrap();
    let err = load(dir.path()).unwrap_err();
    let code = report_load_error("check", &err, true, Style::plain());
    assert_eq!(code, EXIT_COMPILE_ERROR);
    assert!(!Path::new(&dir.path().join(".ply-cache")).exists());
}

#[test]
fn a_definition_no_root_reaches_is_warned_once_at_its_name() {
    let (_dir, loaded) = fixture(
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
    );
    let unused: Vec<(&str, &str)> = loaded
        .frontend
        .warnings
        .iter()
        .filter(|d| d.code == ply_span::codes::UNUSED_DEFINITION)
        .inspect(|d| assert_eq!(d.severity, ply_span::Severity::Warning))
        .map(|d| {
            let at = d.primary_span().expect("the name is labelled");
            (d.message.as_str(), loaded.sources.snippet(at))
        })
        .collect();
    assert_eq!(
        unused,
        [
            ("fn `m.dead` is never used", "dead"),
            ("fn `m.deader` is never used", "deader"),
            ("type `m.Unused` is never used", "Unused"),
        ]
    );
}

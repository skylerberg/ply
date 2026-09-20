use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;

const SOURCE: &str = "\
fn unrelated() -> Int = 0

// The bound a fresh server starts with.
// It is positive.
pub fn default_limit() -> Int = 1024

// Widens the bound alone.
fn widened(limit: Int, by: Int) -> Int = limit + by

fn undocumented(xs: List<Int>) -> Int = len(xs)
";

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), SOURCE).unwrap();
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn json_of(out: &std::process::Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn a_definition_is_documented_by_its_signature_the_comment_above_it_and_its_place() {
    let dir = project();
    let out = ply(dir.path())
        .args(["doc", "widened", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["exit_code"], 0, "{v}");
    assert_eq!(v["kind"], "definition");
    assert_eq!(v["name"], "m.widened");
    assert_eq!(v["signature"], "fn widened(limit: Int, by: Int) -> Int");
    assert_eq!(v["doc"], "Widens the bound alone.");
    assert_eq!(v["params"][1]["name"], "by");
    assert_eq!(v["params"][1]["type"], "Int");
    assert_eq!(v["footprint"], "{}");
    assert_eq!(v["hash"].as_str().unwrap().len(), 64);
    assert!(
        v["location"].as_str().unwrap().ends_with("m.ply:8:1"),
        "{v}"
    );

    let out = ply(dir.path())
        .args(["doc", "m.default_limit"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("pub fn default_limit() -> Int"), "{text}");
    assert!(
        text.contains("The bound a fresh server starts with.\n")
            && text.contains("It is positive."),
        "{text}"
    );

    let out = ply(dir.path())
        .args(["doc", "undocumented", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["doc"], "");
    assert_eq!(v["signature"], "fn undocumented(xs: List<Int>) -> Int");
}

#[test]
fn a_builtin_is_documented_from_the_compilers_table_with_names_and_a_note() {
    let dir = project();
    let out = ply(dir.path())
        .args(["doc", "map", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["exit_code"], 0, "{v}");
    assert_eq!(v["kind"], "builtin");
    assert_eq!(
        v["signature"],
        "map<a, b | e>(xs: List<a>, f: (a) -> b / e) -> List<b> / e"
    );
    assert_eq!(v["params"][0]["name"], "xs");

    let out = ply(dir.path()).args(["doc", "map_merge"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("map_merge<a, b>(a: Map<a, b>, b: Map<a, b>) -> Map<a, b>")
            && text.contains("`b` wins"),
        "{text}"
    );

    // Outside any project, a builtin still answers.
    let empty = tempfile::tempdir().unwrap();
    let out = ply(empty.path())
        .args(["doc", "len", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["signature"], "len<a>(xs: List<a>) -> Int");
}

#[test]
fn a_name_that_is_neither_exits_two_with_the_unknown_name_code() {
    let dir = project();
    let out = ply(dir.path())
        .args(["doc", "nothing_here", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v = json_of(&out);
    assert_eq!(v["diagnostics"][0]["code"], "E0101", "{v}");
}

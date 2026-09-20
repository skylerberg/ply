use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

/// Ints, bools, containers, strings and a self-handled effect, so the seam carries each kind.
const CORPUS: &str = r#"
effect tally {
  read  base[log]() -> Int
  write note[log](what: Int) -> Unit
}

fn double(x: Int) -> Int = x * 2

fn even(x: Int) -> Bool = x % 2 == 0

fn triple(x: Int) -> Int = x * 3

fn pair(x: Int) -> List<Int> = [x, x]

fn label(x: Int) -> String = "n"

fn grade(x: Int) -> Float = 1.5

fn refused(x: Int) -> Int = if 1.5 > 0.5 { x + 1 } else { x }

fn measured(n: Int) -> Int / {tally.read[log], tally.write[log]} = {
  let b = tally.base[log]();
  tally.note[log](n + 1);
  b + n
}

pub fn handled(n: Int) -> Int =
  with_cell[log](0) { c -> {
    let out = handle {
      measured(n)
    } with {
      tally.base[log]() -> 7,
      tally.note[log](what) -> cell_set(c, cell_get(c) + what),
    };
    out + cell_get(c)
  } }

test "double doubles" { assert_eq(double(4), 8) }
test "even is even" { assert(even(4)) }
test "triple triples" { assert_eq(triple(5), 15) }
test "a pair has two" { assert_eq(len(pair(7)), 2) }
test "a pair holds its number" { assert_eq(pair(7), [7, 7]) }
test "a refused body adds one" { assert_eq(refused(2), 3) }
test "a label is a word" { assert_eq(label(7), "n") }
test "a grade is a float" { assert(grade(7) == 1.5) }
test "a self handled effect still answers" { assert_eq(handled(1), 10) }
"#;

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), source).unwrap();
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn run(dir: &Path, backend: Option<&str>) -> Value {
    let mut cmd = ply(dir);
    cmd.arg("test").arg("-j").arg("1").arg("--json");
    if let Some(backend) = backend {
        cmd.arg("--backend").arg(backend);
    }
    let out = cmd.output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout was not one JSON object: {e}\n---\n{text}\n---"))
}

fn u64_at(report: &Value, path: &[&str]) -> u64 {
    let mut node = report;
    for key in path {
        node = node
            .get(key)
            .unwrap_or_else(|| panic!("the artifact has no `{}`", path.join(".")));
    }
    node.as_u64()
        .unwrap_or_else(|| panic!("`{}` is not a number: {node}", path.join(".")))
}

#[test]
fn the_honest_code_generator_agrees_over_the_corpus_and_enters_it() {
    let dir = project(CORPUS);
    let report = run(dir.path(), Some("c"));

    assert_eq!(report["ok"], Value::Bool(true), "{report}");
    assert_eq!(u64_at(&report, &["summary", "failed"]), 0, "{report}");
    assert_eq!(report["backend"]["name"], "c", "{report}");
    assert!(
        u64_at(&report, &["backend", "entered"]) > 0,
        "the code generator entered nothing, so the seam was never reached: {}",
        report["backend"]
    );
    // The C tier carries the whole language, so it declines nothing.
    assert_eq!(
        u64_at(&report, &["backend", "declined"]),
        0,
        "{}",
        report["backend"]
    );
    assert!(
        u64_at(&report, &["backend", "fragment"]) > 0,
        "{}",
        report["backend"]
    );
    // Only `Int`s go in, which are immediates and build nothing; a list and a string are read back out.
    assert_eq!(
        u64_at(&report, &["backend", "converted_in"]),
        0,
        "an immediate argument built an object at the seam: {}",
        report["backend"]
    );
    assert!(
        u64_at(&report, &["backend", "converted_out"]) > 0,
        "the seam read nothing back out of a corpus that answers lists and strings, so the \
         census is not counting: {}",
        report["backend"]
    );
    // A code generator compiled something, and the report says how much it cost.
    assert!(
        u64_at(&report, &["backend", "units"]) > 0,
        "no unit was compiled, so `c` installed something that is not a code generator: {}",
        report["backend"]
    );
}

#[test]
fn run_attaches_a_backend_to_main_and_refuses_a_spec_it_cannot_parse() {
    let dir = project(
        "fn double(x: Int) -> Int = x * 2\nfn main() -> Int = fold(range(0, 10), 0, |acc: Int, i: Int| acc + double(i))\n",
    );
    let out = ply(dir.path())
        .arg("run")
        .arg("--json")
        .arg("--backend")
        .arg("c")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["value"], Value::String("90".into()), "{report}");
    assert!(out.status.success(), "{report}");

    let out = ply(dir.path())
        .arg("run")
        .arg("--json")
        .arg("--backend")
        .arg("nonsense")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(ply_span::codes::BACKEND_UNAVAILABLE),
        "{text}"
    );
}

#[test]
fn a_backed_run_that_selects_nothing_compiles_nothing() {
    let dir = project(CORPUS);
    let report = run(dir.path(), Some("c"));
    assert!(
        u64_at(&report, &["backend", "fragment"]) > 0,
        "the control did not compile a fragment, so the next assertion proves nothing: {}",
        report["backend"]
    );

    let out = ply(dir.path())
        .arg("test")
        .arg("--backend")
        .arg("c")
        .arg("--filter")
        .arg("nothing-matches-this")
        .arg("--json")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["ok"], Value::Bool(true), "{report}");
    assert_eq!(
        u64_at(&report, &["backend", "fragment"]),
        0,
        "a run that selected no test compiled a fragment anyway: {}",
        report["backend"]
    );
    assert!(
        report["diagnostics"]
            .as_array()
            .is_some_and(|d| d.is_empty()),
        "a run that built no backend reported a disagreement about which engine it was: {report}"
    );
}

#[test]
fn a_backend_name_that_is_not_a_spelling_of_anything_is_refused() {
    let dir = project(CORPUS);
    for spec in ["c:reference", "clif", "wrong:off-by-one"] {
        let out = ply(dir.path())
            .arg("test")
            .arg("--backend")
            .arg(spec)
            .arg("--json")
            .output()
            .unwrap();
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["ok"], Value::Bool(false), "`{spec}`: {report}");
        assert_eq!(
            report["diagnostics"][0]["code"], "E0450",
            "`{spec}`: {report}"
        );
    }
}

#[test]
fn a_test_body_is_entered_whole_and_a_failing_one_still_fails() {
    let dir = project(
        r#"
fn double(x: Int) -> Int = x * 2

test "doubles" { assert_eq(double(21), 42) }

test "wrong" { assert_eq(double(21), 41) }
"#,
    );
    let report = run(dir.path(), Some("c"));
    assert_eq!(u64_at(&report, &["summary", "failed"]), 1, "{report}");
    assert_eq!(
        u64_at(&report, &["backend", "entered"]),
        2,
        "both bodies are entered, and the failing one raises: {}",
        report["backend"]
    );
    assert_eq!(
        u64_at(&report, &["backend", "declined"]),
        0,
        "a raised failure is a verdict, not a decline: {}",
        report["backend"]
    );
}

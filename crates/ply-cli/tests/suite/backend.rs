//! The deliberately wrong backends, caught under tier-only (ADR 0048) by the corpus's own tests
//! going red — a corrupt backend declines every test body it is handed and, with no machine behind
//! the decline, the whole corpus fails.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

/// Five definitions and five tests, chosen so that each corruption has something to bite.
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

// A carried argument and an answer the seam does not carry: offered to every backend and declined
// by the reference one's registry, which is the registry-miss path `wrong:unoffered` corrupts.
fn grade(x: Int) -> Float = 1.5

// Outside the fragment — a `Float` literal has no path in it — so its name is one the registry
// lacks however wide the registry is, which is what `wrong:unoffered` answers for.
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

/// One definition whose recursion outruns the machine's own bound, so that `budget` is a number the
/// backend has to honour rather than a hint.
const DEEP: &str = r#"
fn ladder(n: Int) -> Int = if n <= 0 { 0 } else { 1 + ladder(n - 1) }

test "a ladder past the machine's bound" { assert_eq(ladder(20000), 20000) }
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

/// One `ply test --backend .. -j 1 --json` run. No `--audit-backend`: under tier-only (ADR 0048)
/// its oracle arm is an evaluator-less machine that disagrees with every honest answer, so a
/// corruption is caught by the corpus's own tests going red instead.
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

/// Every corruption is exercised on the C tier: the corpus performs effects the `reference`
/// fragment declines, and under tier-only there is no machine behind a decline, so a bare
/// `wrong:...` — which names `reference` — could not run the corpus at all.
fn on_c_tier(spec: &str) -> String {
    if spec.starts_with("wrong:") {
        format!("c:{spec}")
    } else {
        spec.to_string()
    }
}

/// The keys of every test the run failed. A corrupt backend declines every test body it is handed
/// and no machine picks the body up, so the whole corpus goes red; a specific corruption is caught
/// by its test being among these.
fn caught(report: &Value) -> Vec<String> {
    report["failures"]
        .as_array()
        .expect("the artifact carries a failure list")
        .iter()
        .map(|f| f["key"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[track_caller]
fn fires_and_is_caught(dir: &Path, backend: &str) -> Vec<String> {
    let report = run(dir, Some(&on_c_tier(backend)));
    let failed = u64_at(&report, &["summary", "failed"]);
    assert!(
        failed > 0,
        "`{backend}` left the corpus green, so this run says nothing about a corruption it did \
         not catch: {report}"
    );
    caught(&report)
}

// --- The eight --------------------------------------------------------------

#[test]
fn an_off_by_one_in_a_compiled_answer_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:off-by-one");
    assert!(
        caught.contains(&"m.double doubles".to_string()),
        "{caught:?}"
    );
}

#[test]
fn an_inverted_compiled_comparison_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:inverted");
    assert!(caught.contains(&"m.even is even".to_string()), "{caught:?}");
}

#[test]
fn a_stale_compiled_answer_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    fires_and_is_caught(dir.path(), "wrong:stale");
}

#[test]
fn a_bool_where_an_int_belongs_crosses_the_seam_and_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:wrong-type");
    assert!(
        caught.contains(&"m.double doubles".to_string()),
        "{caught:?}"
    );
    // A `String` crosses the seam too, so an `Int` in its place is a caught wrong kind.
    assert!(
        caught.contains(&"m.a label is a word".to_string()),
        "{caught:?}"
    );
}

/// `grade` is offered — its argument is carried — and declined by the reference registry, whose
/// members are the carried *signatures*; an answer for it is an answer for a name the backend has
/// no body for.
#[test]
fn an_answer_for_a_definition_the_backend_has_no_body_for_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:unoffered");
    assert!(
        caught.contains(&"m.a grade is a float".to_string()),
        "{caught:?}"
    );
}

#[test]
fn a_forged_handle_inside_a_container_answer_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:handle");
    assert!(
        caught.contains(&"m.a pair holds its number".to_string()),
        "{caught:?}"
    );
}

#[test]
fn a_backend_that_runs_past_its_budget_is_caught_by_ply_test() {
    let dir = project(DEEP);
    // The corpus outruns the recursion bound on its own, so a red control here is the stage the
    // budget mutation needs — not a backend being blamed for it.
    let control = run(dir.path(), None);
    assert_eq!(u64_at(&control, &["summary", "failed"]), 1, "{control}");
    assert!(
        control["failures"][0]["diagnostic"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("recursion limit")),
        "the corpus stopped outrunning the recursion bound, so there is nothing for a budget \
         mutation to run past: {control}"
    );

    let caught = fires_and_is_caught(dir.path(), "wrong:exceeds-budget=4");
    assert_eq!(caught, vec!["m.a ladder past the machine's bound"]);
}

#[test]
fn a_backend_that_ignores_its_budget_is_caught_where_the_body_terminates() {
    let dir = project(DEEP);
    let caught = fires_and_is_caught(dir.path(), "wrong:exceeds-budget");
    assert_eq!(caught, vec!["m.a ladder past the machine's bound"]);
}

/// The `answers=` mutation forges an `Int` for a named definition. Its old subject — that the
/// machine never *offers* a self-handled definition to a backend — was a two-tier seam distinction
/// tier-only removes (ADR 0048); the corruption is now caught the way the others are, by the run
/// going red with the test that reaches `handled` among the failures.
#[test]
fn a_forged_answer_for_a_self_handled_definition_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:answers=99@m.handled");
    assert!(
        caught.contains(&"m.a self handled effect still answers".to_string()),
        "{caught:?}"
    );
}

// --- The flag itself --------------------------------------------------------

/// `--audit-backend` is accepted only alongside `--backend`; on its own it is a flag that would do
/// nothing, which `CONTRIBUTING.md` §"The one rule" calls a defect, so the CLI refuses it.
#[test]
fn auditing_a_backend_that_was_not_asked_for_is_refused() {
    let dir = project(CORPUS);
    let out = ply(dir.path())
        .arg("test")
        .arg("--audit-backend")
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stderr).unwrap();
    assert!(text.contains("--backend"), "{text}");
}

#[test]
fn an_unknown_backend_is_refused_rather_than_ignored() {
    let dir = project(CORPUS);
    let out = ply(dir.path())
        .arg("test")
        .arg("--backend")
        .arg("wrong:off-by-two")
        .arg("--json")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["ok"], Value::Bool(false), "{report}");
    assert_eq!(report["diagnostics"][0]["code"], "E0450", "{report}");
}

// --- The eight, over the code generator -------------------------------------

/// The control for everything below: the honest code generator is green, changes no answer, and
/// **enters bodies**.
#[test]
fn the_honest_code_generator_agrees_over_the_corpus_and_enters_it() {
    let dir = project(CORPUS);
    let report = run(dir.path(), Some("c"));

    assert_eq!(report["ok"], Value::Bool(true), "{report}");
    assert_eq!(u64_at(&report, &["summary", "failed"]), 0, "{report}");
    assert_eq!(report["backend"]["name"], "c", "{report}");
    assert_eq!(u64_at(&report, &["backend", "fired"]), 0, "{report}");
    assert!(
        u64_at(&report, &["backend", "entered"]) > 0,
        "the code generator entered nothing, so the seam was never reached: {}",
        report["backend"]
    );
    // Under tier-only the C tier carries the whole language, so it declines nothing — the
    // registry-miss path the two-tier world exercised here is gone (ADR 0048).
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
    // The seam's census counted what the entries converted: this corpus hands in only `Int`s,
    // which are immediates and build nothing, and answers a list and a string, which are read
    // back out.
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
    let plain = run(dir.path(), Some("reference"));
    assert!(
        plain["backend"]["units"].is_null(),
        "`reference` reported a compilation, and it compiles nothing: {}",
        plain["backend"]
    );
}

#[test]
fn an_off_by_one_in_compiled_code_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "c:wrong:off-by-one");
    assert!(
        caught.contains(&"m.double doubles".to_string()),
        "{caught:?}"
    );
}

#[test]
fn an_inverted_comparison_in_compiled_code_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "c:wrong:inverted");
    assert!(caught.contains(&"m.even is even".to_string()), "{caught:?}");
}

#[test]
fn a_stale_answer_from_compiled_code_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    fires_and_is_caught(dir.path(), "c:wrong:stale");
}

#[test]
fn a_wrong_kind_from_compiled_code_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "c:wrong:wrong-type");
    assert!(
        caught.contains(&"m.double doubles".to_string()),
        "{caught:?}"
    );
    assert!(
        caught.contains(&"m.a label is a word".to_string()),
        "{caught:?}"
    );
}

/// The registry-miss path: every compiled definition is registered, so the name the mutation
/// answers for is one the fragment refused.
#[test]
fn an_answer_from_compiled_code_for_a_body_it_lacks_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "c:wrong:unoffered");
    assert!(
        caught.contains(&"m.a refused body adds one".to_string()),
        "{caught:?}"
    );
}

/// The fuel prologue is four instructions in every compiled body — load, subtract, branch, store —
/// and this is what says they are load-bearing.
#[test]
fn compiled_code_that_runs_past_its_budget_is_caught_by_ply_test() {
    let dir = project(DEEP);
    let control = run(dir.path(), Some("c"));
    assert_eq!(u64_at(&control, &["summary", "failed"]), 1, "{control}");
    assert!(
        control["failures"][0]["diagnostic"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("recursion limit")),
        "the honest code generator honours the recursion bound and declines with it, so a red \
         control here is the corpus outrunning the bound rather than the generator misbehaving: \
         {control}"
    );

    let caught = fires_and_is_caught(dir.path(), "c:wrong:exceeds-budget=4");
    assert_eq!(caught, vec!["m.a ladder past the machine's bound"]);
}

#[test]
fn compiled_code_that_ignores_its_budget_is_caught_where_the_body_terminates() {
    let dir = project(DEEP);
    let caught = fires_and_is_caught(dir.path(), "c:wrong:exceeds-budget");
    assert_eq!(caught, vec!["m.a ladder past the machine's bound"]);
}

/// The compiled counterpart of the forged-answer mutation; its old offer-protection subject is a
/// two-tier distinction tier-only removes (ADR 0048), so it too is caught by the run going red.
#[test]
fn compiled_code_forging_an_answer_for_a_self_handled_definition_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "c:wrong:answers=99@m.handled");
    assert!(
        caught.contains(&"m.a self handled effect still answers".to_string()),
        "{caught:?}"
    );
}

/// `ply run --backend` attaches the backend to the program's `main` as `ply test` does to a test,
/// and a spec the grammar refuses is the same diagnostic there.
#[test]
fn run_attaches_a_backend_to_main_and_refuses_a_spec_it_cannot_parse() {
    let dir = project(
        "fn double(x: Int) -> Int = x * 2\nfn main() -> Int = fold(range(0, 10), 0, |acc: Int, i: Int| acc + double(i))\n",
    );
    // Only the C tier is offered here: under tier-only `reference` is a fragment with no machine
    // behind it, so it declines `main` and cannot run the program at all (ADR 0048).
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

// --- The compile laziness ---------------------------------------------------

/// A unit compiled to enter nothing is the whole project's compile spent on an empty selection,
/// and `benches/marginal-change/` prices that at about half of a backed run. A run that selected no
/// test builds none.
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

// --- The grammar -------------------------------------------------------------

/// A corruption may name the backend it wraps, and a bare `wrong:` still means `reference`.
#[test]
fn a_bare_wrong_prefix_still_names_the_reference_backend() {
    let dir = project(CORPUS);
    let bare = run(dir.path(), Some("wrong:off-by-one"));
    assert_eq!(bare["backend"]["name"], "reference", "{bare}");
    let named = run(dir.path(), Some("reference:wrong:off-by-one"));
    assert_eq!(named["backend"]["name"], "reference", "{named}");
    let generated = run(dir.path(), Some("c:wrong:off-by-one"));
    assert_eq!(generated["backend"]["name"], "c", "{generated}");
}

/// A misspelled backend is refused rather than falling back to one that works.
#[test]
fn a_backend_name_that_is_not_a_spelling_of_anything_is_refused() {
    let dir = project(CORPUS);
    for spec in ["c:reference", "clif", "c:wrong:off-by-two"] {
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

/// A test body is a root the code generator enters whole, and a failing one is still a failure:
/// the tier raises the diagnostic, which is an entry and not a decline.
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

// --- Removed under tier-only (ADR 0048) -------------------------------------
//
// The following tests are deleted because their premise is the interpreter-vs-backend separation
// that tier-only removes:
//
// * `the_honest_backend_agrees_over_the_corpus_and_enters_it` — `reference` is a fragment with no
//   machine behind it, so it declines the corpus's effects and runs it red rather than green; the
//   honest-`c` control above is now the only honest backend that runs the corpus.
// * `the_corpus_is_green_with_no_backend` — the C tier is always the evaluator, so there is no
//   "no backend" run and `report["backend"]` is never null.
// * `a_backend_run_reads_no_pass_the_evaluator_earned`,
//   `a_backend_run_writes_no_pass_the_evaluator_will_read`,
//   `a_code_generator_run_reads_no_pass_the_evaluator_earned`,
//   `a_code_generator_run_writes_no_pass` — the result cache is no longer namespaced by engine;
//   the backend IS the evaluator, so there is no evaluator-earned pass a backend run must refuse.
// * `the_unbounded_runaway_is_stopped_under_a_code_generator_and_hangs_under_a_tree_walker` — there
//   is no tree-walker arm to contrast, and a corrupt backend now declines the test body outright
//   (no body) rather than running past its budget over native frames, so the stack-floor vs
//   heap-frame contrast no longer exists.

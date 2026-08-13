//! An adversarial audit of the defect/program-error split, through the binary.
//!
//! Two directions cost an agent its time, and they cost it differently. A
//! legitimate program error reported as a defect in Ply sends the agent to file
//! a bug about its own broken code and — worse — suppresses the bisection that
//! would have named the edit. A defect in Ply reported as an ordinary red test
//! sends the agent hunting through source that is correct, with a suspect set
//! that invented a culprit for something no change caused.
//!
//! `ply-test`'s unit tests pin the classifier on a synthetic executor. These
//! run the real evaluator over real source, because the classifier's input is
//! whatever code an evaluator happens to attach to a failure, and that is a
//! property of nine files rather than of the one `match`.
//!
//! Where a case shows the system getting it wrong the test is named
//! `documents_` and its doc comment says what the right answer is. Those pin
//! present behaviour so a fix is visible as a diff; they are not endorsements.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), source).unwrap();
    dir
}

fn write(dir: &TempDir, source: &str) {
    std::fs::write(dir.path().join("m.ply"), source).unwrap();
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn json_of(output: &std::process::Output) -> Value {
    let text = stdout_of(output);
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout was not one JSON object: {e}\n---\n{text}\n---"))
}

/// One `--json` run, and the single failure it must have produced.
fn sole_failure(dir: &TempDir) -> Value {
    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let v = json_of(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a run with one red test exits 1: {v}"
    );
    let failures = v["failures"].as_array().expect("failures is an array");
    assert_eq!(failures.len(), 1, "exactly one failure: {v}");
    let mut failure = failures[0].clone();
    failure["__status"] = v["results"][0]["status"].clone();
    failure
}

/// The whole claim, in one place: the program is at fault, the run says so, and
/// nothing about the failure took bisection off the table.
fn assert_program_error(failure: &Value, what: &str) {
    assert_eq!(
        failure["defect"], false,
        "{what} is the program's behaviour, not a defect in Ply: {failure}"
    );
    assert_ne!(
        failure["diagnostic"]["code"], "E0505",
        "{what} must not carry the internal-error code: {failure}"
    );
    assert_eq!(
        failure["__status"], "failed",
        "{what} is a red test, not a panic: {failure}"
    );
    assert_ne!(
        failure["culprit"]["skipped"], "panicked",
        "{what} must not have bisection suppressed as a Ply defect: {failure}"
    );
}

// --- every language-defined runtime failure is the program's ----------------

/// The table the fix is really about. Each of these is a failure the language
/// *defines*: an edit can introduce it, an edit can remove it, and it bisects
/// like an assertion. Reading `panicked` off the runtime-error code would put
/// every row here on the wrong side at once, so they are asserted together
/// rather than one test per row.
#[test]
fn every_language_defined_runtime_failure_is_a_program_error() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "a runaway recursion",
            "fn spin(n: Int) -> Int = 1 + spin(n + 1)\n\
             test \"spins\" { assert_eq(spin(0), 0) }\n",
            "recursion limit",
        ),
        (
            "a tail-recursive runaway",
            "fn spin(n: Int) -> Int = spin(n + 1)\n\
             test \"spins in tail position\" { assert_eq(spin(0), 0) }\n",
            "recursion limit",
        ),
        (
            "an integer overflow",
            "fn grow(n: Int) -> Int = n * 4611686018427387904\n\
             test \"grows\" { assert_eq(grow(21), 42) }\n",
            "integer overflow",
        ),
        (
            "a division by zero",
            "fn div(a: Int, b: Int) -> Int = a / b\n\
             test \"divides\" { assert_eq(div(1, 0), 0) }\n",
            "division by zero",
        ),
        (
            "a remainder by zero",
            "fn rem(a: Int, b: Int) -> Int = a % b\n\
             test \"remainders\" { assert_eq(rem(1, 0), 0) }\n",
            "by zero",
        ),
        (
            "an explicit panic",
            "fn boom(n: Int) -> Int = if n > 0 { panic(\"no\") } else { 0 }\n\
             test \"booms\" { assert_eq(boom(1), 0) }\n",
            "panic: no",
        ),
        (
            "a runaway range",
            "fn big() -> List<Int> = range(0, 99999999)\n\
             test \"ranges\" { assert_eq(len(big()), 1) }\n",
            "exceeds the limit",
        ),
        (
            "a refutable `let` that did not match",
            "type Shape = Circle(Int) | Tri(Int)\n\
             fn radius(s: Shape) -> Int = { let Circle(r) = s; r }\n\
             test \"unwraps\" { assert_eq(radius(Tri(2)), 2) }\n",
            "did not match",
        ),
        (
            "a failing assert_eq",
            "fn one() -> Int = 1\n\
             test \"equals\" { assert_eq(one(), 2) }\n",
            "assertion failed",
        ),
        (
            // One argument, not two: `Builtin::Assert` accepts an optional
            // message at run time but the checker's signature does not, so the
            // two-argument form is a type error rather than a red test.
            "a failing assert",
            "fn no() -> Bool = false\n\
             test \"holds\" { assert(no()) }\n",
            "assertion failed",
        ),
    ];

    for (what, source, needle) in cases {
        let dir = project(source);
        let failure = sole_failure(&dir);
        assert_program_error(&failure, what);
        let message = failure["diagnostic"]["message"]
            .as_str()
            .unwrap_or_default();
        assert!(
            message.contains(needle),
            "{what}: expected a message containing {needle:?}, got {message:?}"
        );
    }
}

/// A clause body is ordinary code that happens to run under a `handle`. A
/// failure raised there is the program's for exactly the same reasons, and the
/// handler machinery must not launder it into something the runner reads as an
/// evaluator fault.
#[test]
fn a_failure_inside_a_handler_clause_body_is_still_the_programs() {
    let cases: &[(&str, &str)] = &[
        ("panic: seeded wrong", "panic(\"seeded wrong\")"),
        ("division by zero", "len(range(0, 4 / 0))"),
        ("recursion limit", "spin(1)"),
        ("integer overflow", "4611686018427387904 * 4"),
    ];

    for (needle, clause) in cases {
        let dir = project(&format!(
            "effect db {{ read all[t]() -> List<Int> }}\n\
             fn spin(n: Int) -> Int = spin(n + 1)\n\
             fn rows() -> List<Int> = db.all[users]()\n\
             test \"counts\" {{\n\
             \x20 handle {{ assert_eq(len(rows()), 2) }} with {{\n\
             \x20   db.all[users]() -> range(0, {clause}),\n\
             \x20 }}\n\
             }}\n"
        ));
        let failure = sole_failure(&dir);
        assert_program_error(&failure, needle);
        let message = failure["diagnostic"]["message"]
            .as_str()
            .unwrap_or_default();
        assert!(
            message.contains(needle),
            "expected {needle:?} from the clause body, got {message:?}"
        );
    }
}

/// Both engines and the audit that runs them together classify a resource
/// limit the same way. A divergence here would be `E0503`, which the classifier
/// reads as an ordinary red test — a divergence is Ply's fault by definition, so
/// that is the one code that ought to set `defect` and does not.
#[test]
fn the_recursion_limit_is_one_program_error_on_every_engine() {
    const RUNAWAY: &str = "fn spin(n: Int) -> Int = spin(n + 1)\n\
                           test \"spins\" { assert_eq(spin(0), 0) }\n";
    for engine in ["treewalk", "machine", "both"] {
        let dir = project(RUNAWAY);
        let out = ply(dir.path())
            .args(["test", "--json", "--engine", engine])
            .output()
            .unwrap();
        let v = json_of(&out);
        let failure = &v["failures"][0];
        assert_eq!(failure["defect"], false, "engine {engine}: {failure}");
        assert_eq!(
            failure["diagnostic"]["code"], "E0502",
            "engine {engine}: {failure}"
        );
        assert_ne!(
            failure["diagnostic"]["code"], "E0503",
            "engine {engine} disagreed with the other one: {failure}"
        );
        assert!(
            failure["diagnostic"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("recursion limit of 10000 nested calls exceeded"),
            "engine {engine}: {failure}"
        );
    }
}

// --- the consequence: these failures are actually bisected ------------------

/// Two independent edits, one benign; only *running* a mixture can say which
/// one caused the failure. Every fixture below shares this shape so that a
/// verdict of `sole` — which needs no evaluator — cannot be mistaken for the
/// search having worked.
fn assert_bisected_to(dir: &TempDir, culprit: &str, innocent: &str) {
    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    let failure = &v["failures"][0];
    let found = &failure["culprit"];
    assert_eq!(failure["defect"], false, "{failure}");
    assert_eq!(found["verdict"], "bisected", "{found}");
    assert_eq!(found["skipped"], Value::Null, "{found}");
    assert_eq!(
        found["definitions"],
        serde_json::json!([culprit]),
        "{found}"
    );
    assert!(
        found["search"]["evaluated"].as_u64().unwrap_or_default() > 0,
        "a mixture was actually run: {found}"
    );
    // A consumer that reads only `suspects[0]` has to get the best guess.
    assert_eq!(failure["suspects"][0]["name"], culprit, "{failure}");
    assert_eq!(failure["suspects"][0]["culprit"], true, "{failure}");
    assert!(
        failure["suspects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == innocent && s["culprit"] == false),
        "the benign edit must be present and not named: {failure}"
    );
}

const RECURSION: &str = "\
fn step(n: Int) -> Int = if n <= 0 { 0 } else { step(n - 1) }

fn scale(a: Int, b: Int) -> Int = (a * b) + 0

fn total(a: Int, b: Int) -> Int = scale(a, b) + step(3)

test \"total is scaled\" { assert_eq(total(2, 3), 6) }
";

/// The failure the fix exists for, end to end. Before it, this run printed
/// "this is a defect in Ply" over a base case somebody deleted and refused to
/// bisect — so the one artifact that names the edit was withheld from the one
/// failure whose cause is hardest to eyeball.
#[test]
fn runaway_recursion_introduced_by_an_edit_is_bisected_to_the_culprit() {
    let dir = project(RECURSION);
    ply(dir.path()).arg("test").assert().success();
    write(
        &dir,
        &RECURSION
            .replace("step(n - 1)", "step(n + 1)")
            .replace("(a * b) + 0", "0 + (a * b)"),
    );
    assert_bisected_to(&dir, "m.step", "m.scale");
}

const OVERFLOW: &str = "\
fn grow(n: Int) -> Int = n * 2

fn shift(n: Int) -> Int = (n + 0) * 1

fn total(n: Int) -> Int = shift(grow(n))

test \"total doubles\" { assert_eq(total(21), 42) }
";

#[test]
fn an_overflow_introduced_by_an_edit_is_bisected_to_the_culprit() {
    let dir = project(OVERFLOW);
    ply(dir.path()).arg("test").assert().success();
    write(
        &dir,
        &OVERFLOW
            .replace("n * 2", "n * 4611686018427387904")
            .replace("(n + 0) * 1", "1 * (n + 0)"),
    );
    assert_bisected_to(&dir, "m.grow", "m.shift");
}

const MATCHING: &str = "\
type Shape = Circle(Int) | Square(Int) | Tri(Int)

fn area(s: Shape) -> Int = { let Circle(r) = s; r * 3 }

fn pick(n: Int) -> Shape = if n <= 0 { Circle(1) } else { Tri(n) }

fn shift(n: Int) -> Int = n + 0

fn report(n: Int) -> Int = area(pick(shift(n)))

test \"the picked shape has an area\" { assert_eq(report(0), 3) }
";

/// A `match` that stops covering a case is a *compile* error — see
/// `a_match_that_stops_being_exhaustive_never_reaches_the_runner`. The runtime
/// half of `E0205` is a refutable `let` whose scrutinee changed shape, which is
/// the pattern-match failure an edit can actually introduce.
#[test]
fn a_pattern_that_stops_matching_is_bisected_to_the_culprit() {
    let dir = project(MATCHING);
    ply(dir.path()).arg("test").assert().success();
    write(
        &dir,
        &MATCHING
            .replace("if n <= 0 { Circle(1) }", "if n < 0 { Circle(1) }")
            .replace(
                "fn shift(n: Int) -> Int = n + 0",
                "fn shift(n: Int) -> Int = 0 + n",
            ),
    );
    assert_bisected_to(&dir, "m.pick", "m.shift");
}

/// Exhaustiveness is decided statically, so an edit that drops an arm never
/// produces a red test to bisect: `ply test` exits 2 with `E0205` and runs
/// nothing. Asserted because the alternative — a runtime `E0205` that reaches
/// the classifier — is what the pattern-match row of the audit would otherwise
/// be about, and a reader has to know which of the two this build does.
#[test]
fn a_match_that_stops_being_exhaustive_never_reaches_the_runner() {
    const EXHAUSTIVE: &str = "\
type Shape = Circle(Int) | Square(Int) | Tri(Int)

fn area(s: Shape) -> Int = match s { Circle(r) -> r * 3, Square(a) -> a * a, Tri(b) -> b }

fn pick(n: Int) -> Shape = if n <= 0 { Circle(1) } else { Tri(n) }

test \"the picked shape has an area\" { assert_eq(area(pick(1)), 1) }
";
    let dir = project(EXHAUSTIVE);
    ply(dir.path()).arg("test").assert().success();
    write(&dir, &EXHAUSTIVE.replace(", Tri(b) -> b", ""));

    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let v = json_of(&out);
    assert_eq!(out.status.code(), Some(2), "{v}");
    assert_eq!(v["diagnostics"][0]["code"], "E0205", "{v}");
    assert_eq!(
        v["failures"],
        Value::Null,
        "nothing ran, so there is nothing to classify: {v}"
    );
}

/// A recursion limit used to set `defect`, and `defect` is per failure — but a
/// reader who has only ever seen it in a one-test project cannot tell a
/// per-failure flag from a per-run one. Two red tests in one run, one of each
/// kind, and the ordinary regression must still be bisected.
#[test]
fn a_runaway_recursion_costs_a_sibling_failure_nothing() {
    const BOTH: &str = "\
fn step(n: Int) -> Int = if n <= 0 { 0 } else { step(n - 1) }

fn scale(a: Int, b: Int) -> Int = (a * b) + 0

fn total(a: Int, b: Int) -> Int = scale(a, b) + step(3)

test \"total is scaled\" { assert_eq(total(2, 3), 6) }

test \"step bottoms out\" { assert_eq(step(3), 0) }
";
    let dir = project(BOTH);
    ply(dir.path()).arg("test").assert().success();
    write(
        &dir,
        &BOTH
            .replace("step(n - 1)", "step(n + 1)")
            .replace("(a * b) + 0", "0 + (a * b)"),
    );

    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    let failures = v["failures"].as_array().expect("two failures");
    assert_eq!(failures.len(), 2, "{v}");
    for failure in failures {
        assert_eq!(failure["defect"], false, "{failure}");
        assert_eq!(
            failure["culprit"]["definitions"],
            serde_json::json!(["m.step"]),
            "both failures have the same cause: {failure}"
        );
    }
}

/// `test/nondet` is the one skip that outranks having a perfectly good
/// baseline: its outcome is not a function of the definition set, so a
/// mixture's answer would be evidence about nothing. A runtime limit inside one
/// must land on `nondet` and not on `panicked`.
#[test]
fn a_nondet_test_that_hits_a_runtime_limit_is_skipped_as_nondet() {
    let dir = project(
        "fn spin(n: Int) -> Int = spin(n + 1)\n\
         test/nondet \"spins\" { assert_eq(spin(0), 0) }\n",
    );
    let failure = sole_failure(&dir);
    assert_program_error(&failure, "a runaway recursion in a nondet test");
    assert_eq!(failure["culprit"]["verdict"], "not_attempted", "{failure}");
    assert_eq!(failure["culprit"]["skipped"], "nondet", "{failure}");
}

/// `--bisect never` is the only skip that says nothing about the failure, so it
/// has to win over every other reason — including a runtime limit, which no
/// longer has a reason of its own.
#[test]
fn bisect_never_outranks_the_reason_a_runtime_limit_would_have_given() {
    let dir = project(RECURSION);
    ply(dir.path()).arg("test").assert().success();
    write(&dir, &RECURSION.replace("step(n - 1)", "step(n + 1)"));

    let v = json_of(
        &ply(dir.path())
            .args(["test", "--json", "--bisect", "never"])
            .output()
            .unwrap(),
    );
    let culprit = &v["failures"][0]["culprit"];
    assert_eq!(culprit["verdict"], "not_attempted", "{culprit}");
    assert_eq!(culprit["skipped"], "not_requested", "{culprit}");
    assert_eq!(culprit["search"]["evaluated"], 0, "{culprit}");
    assert_eq!(v["failures"][0]["defect"], false, "{}", v["failures"][0]);
}

/// A first-ever red test has no earlier definition set, whatever it failed
/// with. `never_passed` and not `panicked`, and no definition named.
#[test]
fn a_first_ever_runtime_limit_is_skipped_as_never_passed() {
    let dir = project(
        "fn spin(n: Int) -> Int = spin(n + 1)\n\
         test \"spins\" { assert_eq(spin(0), 0) }\n",
    );
    let failure = sole_failure(&dir);
    assert_eq!(failure["culprit"]["skipped"], "never_passed", "{failure}");
    assert_eq!(
        failure["culprit"]["definitions"],
        serde_json::json!([]),
        "nothing was bisected, so no definition may be named: {failure}"
    );
}

/// `no_bodies` is "go and stop pruning" and `no_hybrids` is "this build cannot
/// do it at all" — a consumer acts on them differently, so a cache with its
/// body store removed must produce the first and not the second. A runtime
/// limit is used as the failure so that the two skips being distinguished is
/// tested on the class the fix moved into scope.
#[test]
fn a_pruned_body_store_says_no_bodies_and_not_no_hybrids() {
    let dir = project(RECURSION);
    ply(dir.path()).arg("test").assert().success();

    let cache = dir.path().join(".ply-cache");
    std::fs::remove_file(cache.join("frontend.idx")).expect("the body index exists");
    std::fs::remove_file(cache.join("frontend.dat")).expect("the body data exists");

    write(
        &dir,
        &RECURSION
            .replace("step(n - 1)", "step(n + 1)")
            .replace("(a * b) + 0", "0 + (a * b)"),
    );
    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    let culprit = &v["failures"][0]["culprit"];
    assert_eq!(v["failures"][0]["defect"], false, "{v}");
    assert_eq!(culprit["verdict"], "not_attempted", "{culprit}");
    assert_eq!(culprit["skipped"], "no_bodies", "{culprit}");
    assert!(
        culprit["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("does not hold the definition bodies"),
        "{culprit}"
    );
}

// --- a user program may never abort the process ------------------------------

/// **A legal program used to kill the run.** The recursion limit is 10,000
/// nested calls and this program stays under it: it builds a 9,800-deep value,
/// which the evaluator constructs and traverses happily. `assert_eq` then
/// compared the two with `ply_eval::value::values_equal`, which recursed on the
/// host stack with no bound at all, and a rayon worker's 2 MiB stack did not
/// survive it.
///
/// The process aborted. Not a panic — `catch_unwind` never saw it — so there was
/// no diagnostic, no `defect` flag, no bisection, and every other test's result
/// in that run went with it. The classifier was not wrong there so much as never
/// consulted, which is the third and worst answer.
///
/// So the sibling test is the other half of the assertion: whatever this one
/// does, the run has to still be a run.
#[test]
fn a_value_the_call_limit_permits_is_compared_rather_than_aborting_the_run() {
    let dir = project(
        "type Chain = Nil | Link(Chain)\n\
         fn build(n: Int) -> Chain = if n <= 0 { Nil } else { Link(build(n - 1)) }\n\
         test \"chains compare equal\" { assert_eq(build(9800), build(9800)) }\n\
         test \"a sibling still reports\" { assert_eq(1, 1) }\n",
    );
    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let v = json_of(&out);
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert_eq!(v["summary"]["failed"], 0, "{v}");
    assert_eq!(v["summary"]["passed"], 2, "{v}");
}

/// Past the bound the answer is a diagnostic, not an abort — and the bound is
/// only reachable by *iteration*, since a value built by recursion is at most as
/// deep as the recursion that built it. It classifies as the program error it is:
/// bisectable, not a defect in Ply, and identical on both engines because both
/// share the walk that raises it.
#[test]
fn a_value_deeper_than_the_bound_is_an_ordinary_program_error() {
    const DEEP: &str = "\
type Chain = Nil | Link(Chain)

fn stack(n: Int) -> Chain = fold(range(0, n), Nil, |acc, x| Link(acc))

test \"deep values compare\" { assert_eq(stack(20000), stack(20000)) }
";
    for engine in ["treewalk", "machine", "both"] {
        let dir = project(DEEP);
        let out = ply(dir.path())
            .args(["test", "--json", "--engine", engine])
            .output()
            .unwrap();
        let v = json_of(&out);
        assert_eq!(out.status.code(), Some(1), "engine {engine}: {v}");
        let mut failure = v["failures"][0].clone();
        failure["__status"] = v["results"][0]["status"].clone();
        assert_program_error(&failure, "a value deeper than the bound");
        assert_eq!(
            failure["diagnostic"]["code"], "E0502",
            "engine {engine}: {failure}"
        );
        assert!(
            failure["diagnostic"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("recursion limit of 10000 nested values exceeded"),
            "engine {engine}: {failure}"
        );
    }
}

/// The same hole reached without any recursion in the *program*: 3,000 terms of
/// `+` is a depth the front end checks and hashes without complaint, and `ply
/// run` evaluates on the main thread. Only `ply test` died, because its workers
/// have a 2 MiB stack and nothing on the evaluator's expression path is bounded
/// by the call limit — the limit counts calls, and this program makes none.
///
/// Answered by growing rather than by a bound: the parser, inference and
/// normalization already accept any depth, so refusing here would reject on the
/// machine a program `ply check` and `ply run` accept — an `E0503` divergence on
/// every corpus with a long operator chain in it.
#[test]
fn a_deeply_nested_expression_runs_rather_than_aborting_the_run() {
    let mut source = String::from("fn deep() -> Int = 1");
    for _ in 0..3000 {
        source.push_str(" + 1");
    }
    source.push_str("\ntest \"deep\" { assert_eq(deep(), 3001) }\n");
    let dir = project(&source);

    let checked = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(
        checked.status.code(),
        Some(0),
        "the front end handles this depth: {}",
        stdout_of(&checked)
    );

    for engine in ["treewalk", "machine", "both"] {
        let out = ply(dir.path())
            .args(["test", "--json", "--no-cache", "--engine", engine])
            .output()
            .unwrap();
        let v = json_of(&out);
        assert_eq!(out.status.code(), Some(0), "engine {engine}: {v}");
        assert_eq!(v["summary"]["passed"], 1, "engine {engine}: {v}");
    }
}

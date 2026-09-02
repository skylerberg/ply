//! The eight deliberately wrong backends, run through a command a user can run.

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

/// One `ply test --backend .. --audit-backend -j 1 --json` run.
fn run(dir: &Path, backend: Option<&str>) -> Value {
    let mut cmd = ply(dir);
    cmd.arg("test").arg("-j").arg("1").arg("--json");
    if let Some(backend) = backend {
        cmd.arg("--backend").arg(backend).arg("--audit-backend");
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

/// Every test whose failure is the backend disagreeing with the machine that offered it the call,
/// by the message a user reads.
fn caught(report: &Value) -> Vec<String> {
    report["failures"]
        .as_array()
        .expect("the artifact carries a failure list")
        .iter()
        .filter(|f| {
            f["diagnostic"]["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("the compiled backend and `machine` disagree"))
        })
        .map(|f| f["key"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[track_caller]
fn fires_and_is_caught(dir: &Path, backend: &str) -> Vec<String> {
    let report = run(dir, Some(backend));
    let fired = u64_at(&report, &["backend", "fired"]);
    assert!(
        fired > 0,
        "`{backend}` never changed an answer, so this run says nothing about the corpus that \
         did not catch it: {}",
        report["backend"]
    );
    let caught = caught(&report);
    assert!(
        !caught.is_empty(),
        "`{backend}` changed {fired} answers and `ply test` reported none of them: {}",
        report["backend"]
    );
    caught
}

// --- The control ------------------------------------------------------------

/// Without this, a red result below could be the backend's *presence* rather than the corruption.
#[test]
fn the_honest_backend_agrees_over_the_corpus_and_enters_it() {
    let dir = project(CORPUS);
    let report = run(dir.path(), Some("reference"));

    assert_eq!(report["ok"], Value::Bool(true), "{report}");
    assert_eq!(u64_at(&report, &["summary", "failed"]), 0, "{report}");
    assert_eq!(u64_at(&report, &["backend", "fired"]), 0, "{report}");
    assert!(
        u64_at(&report, &["backend", "entered"]) > 0,
        "the honest backend entered nothing, so the seam was never reached: {}",
        report["backend"]
    );
    assert!(
        u64_at(&report, &["backend", "declined"]) > 0,
        "the honest backend declined nothing, so the registry-miss path — which is what \
         `wrong:unoffered` corrupts — is unexercised: {}",
        report["backend"]
    );
    assert!(
        u64_at(&report, &["backend", "fragment"]) > 0,
        "{}",
        report["backend"]
    );
}

/// And the other control: the same corpus with no backend at all is green, so nothing below is a
/// corpus that was already broken.
#[test]
fn the_corpus_is_green_with_no_backend() {
    let dir = project(CORPUS);
    let report = run(dir.path(), None);
    assert_eq!(report["ok"], Value::Bool(true), "{report}");
    assert!(report["backend"].is_null(), "{report}");
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
}

#[test]
fn an_answer_for_a_definition_the_backend_has_no_body_for_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:unoffered");
    assert!(
        caught.contains(&"m.a label is a word".to_string()),
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
    // The control first: this corpus is red on its own, and no backend is blamed for it.
    let control = run(dir.path(), None);
    assert_eq!(u64_at(&control, &["summary", "failed"]), 1, "{control}");
    assert!(
        caught(&control).is_empty(),
        "a run with no backend reported a backend divergence: {control}"
    );
    assert!(
        control["failures"][0]["diagnostic"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("recursion limit")),
        "the corpus stopped outrunning the machine's bound, so there is nothing for a backend \
         to run past: {control}"
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

#[test]
fn a_backend_is_never_offered_a_definition_that_performs() {
    let dir = project(CORPUS);
    let report = run(dir.path(), Some("wrong:answers=99@m.handled"));

    assert_eq!(
        u64_at(&report, &["backend", "offered_target"]),
        0,
        "a definition that discharges its own effects was offered to a backend: {}",
        report["backend"]
    );
    assert!(
        u64_at(&report, &["backend", "offered"]) > 0,
        "the seam was never reached at all, so the count above proves nothing: {}",
        report["backend"]
    );
    assert_eq!(u64_at(&report, &["backend", "fired"]), 0, "{report}");
    assert_eq!(report["ok"], Value::Bool(true), "{report}");
}

// --- The result-cache rule --------------------------------------------------

/// The cache rule, both stages, on the path where neither is an accident.
#[test]
fn a_backend_run_reads_no_cached_pass() {
    let dir = project(CORPUS);
    // Warm the cache with an ordinary run, so there is something to believe.
    let warm = ply(dir.path()).arg("test").arg("--json").output().unwrap();
    let warm: Value = serde_json::from_slice(&warm.stdout).unwrap();
    assert_eq!(warm["ok"], Value::Bool(true), "{warm}");

    let again = ply(dir.path()).arg("test").arg("--json").output().unwrap();
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert!(
        u64_at(&again, &["summary", "cached"]) > 0,
        "the cache never warmed, so this test cannot tell a backend run that ignored it from one \
         that had nothing to ignore: {again}"
    );

    let out = ply(dir.path())
        .arg("test")
        .arg("--backend")
        .arg("reference")
        .arg("-j")
        .arg("1")
        .arg("--json")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["no_cache"], Value::Bool(true), "{report}");
    assert_eq!(
        u64_at(&report, &["summary", "cached"]),
        0,
        "a backend run believed a pass an unbacked run earned: {report}"
    );
    assert!(
        u64_at(&report, &["backend", "entered"]) > 0,
        "a backend run that entered nothing proves nothing about whether it read the cache: {}",
        report["backend"]
    );
}

#[test]
fn a_backend_run_writes_no_pass() {
    let dir = project(CORPUS);
    let out = ply(dir.path())
        .arg("test")
        .arg("--backend")
        .arg("reference")
        .arg("-j")
        .arg("1")
        .arg("--json")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["ok"], Value::Bool(true), "{report}");
    assert!(
        u64_at(&report, &["backend", "entered"]) > 0,
        "nothing entered native code, so no rule about entering it was exercised: {}",
        report["backend"]
    );
    assert!(
        report["diagnostics"]
            .as_array()
            .is_some_and(|d| d.is_empty()),
        "{report}"
    );

    // And the fact the diagnostic is a check *on*: a later run with no backend has to run every
    // test again, because the backend run left nothing behind.
    let plain = ply(dir.path()).arg("test").arg("--json").output().unwrap();
    let plain: Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert_eq!(
        u64_at(&plain, &["summary", "cached"]),
        0,
        "a run with no backend believed a pass a backend run recorded: {plain}"
    );
    // Seven: `CORPUS` gained `label` and a second test on `pair` on 2026-08-31 (see [`CORPUS`]'s
    // note).
    assert_eq!(u64_at(&plain, &["summary", "passed"]), 8, "{plain}");
}

// --- The flag itself --------------------------------------------------------

/// A flag that is accepted and does nothing is `CONTRIBUTING.md` §"The one rule"'s defect shape, so
/// the engine that cannot host a backend refuses it rather than ignoring it.
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
    let report = run(dir.path(), Some("cranelift"));

    assert_eq!(report["ok"], Value::Bool(true), "{report}");
    assert_eq!(u64_at(&report, &["summary", "failed"]), 0, "{report}");
    assert_eq!(report["backend"]["name"], "cranelift", "{report}");
    assert_eq!(u64_at(&report, &["backend", "fired"]), 0, "{report}");
    assert!(
        u64_at(&report, &["backend", "entered"]) > 0,
        "the code generator entered nothing, so the seam was never reached: {}",
        report["backend"]
    );
    assert!(
        u64_at(&report, &["backend", "declined"]) > 0,
        "the code generator declined nothing, so the registry-miss path — which is what \
         `wrong:unoffered` corrupts — is unexercised: {}",
        report["backend"]
    );
    assert!(
        u64_at(&report, &["backend", "fragment"]) > 0,
        "{}",
        report["backend"]
    );
    // A code generator compiled something, and the report says how much it cost.
    assert!(
        u64_at(&report, &["backend", "units"]) > 0,
        "no unit was compiled, so `cranelift` installed something that is not a code generator: {}",
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
    let caught = fires_and_is_caught(dir.path(), "cranelift:wrong:off-by-one");
    assert!(
        caught.contains(&"m.double doubles".to_string()),
        "{caught:?}"
    );
}

#[test]
fn an_inverted_comparison_in_compiled_code_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "cranelift:wrong:inverted");
    assert!(caught.contains(&"m.even is even".to_string()), "{caught:?}");
}

#[test]
fn a_stale_answer_from_compiled_code_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    fires_and_is_caught(dir.path(), "cranelift:wrong:stale");
}

#[test]
fn a_wrong_kind_from_compiled_code_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "cranelift:wrong:wrong-type");
    assert!(
        caught.contains(&"m.double doubles".to_string()),
        "{caught:?}"
    );
}

/// The registry-miss path: every compiled definition is registered, so the name the mutation
/// answers for is one the fragment refused.
#[test]
fn an_answer_from_compiled_code_for_a_body_it_lacks_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "cranelift:wrong:unoffered");
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
    let control = run(dir.path(), Some("cranelift"));
    assert_eq!(u64_at(&control, &["summary", "failed"]), 1, "{control}");
    assert!(
        caught(&control).is_empty(),
        "the honest code generator was blamed for a corpus that is red on its own: {control}"
    );
    assert!(
        u64_at(&control, &["backend", "declined"]) > 0,
        "the honest code generator never declined, so the budget it is about to ignore was never \
         honoured either: {}",
        control["backend"]
    );

    let caught = fires_and_is_caught(dir.path(), "cranelift:wrong:exceeds-budget=4");
    assert_eq!(caught, vec!["m.a ladder past the machine's bound"]);
}

#[test]
fn compiled_code_that_ignores_its_budget_is_caught_where_the_body_terminates() {
    let dir = project(DEEP);
    let caught = fires_and_is_caught(dir.path(), "cranelift:wrong:exceeds-budget");
    assert_eq!(caught, vec!["m.a ladder past the machine's bound"]);
}

/// Accepting a call the machine must never offer, over the code generator.
#[test]
fn compiled_code_is_never_offered_a_definition_that_performs() {
    let dir = project(CORPUS);
    let report = run(dir.path(), Some("cranelift:wrong:answers=99@m.handled"));

    assert_eq!(
        u64_at(&report, &["backend", "offered_target"]),
        0,
        "a definition that discharges its own effects was offered to a backend: {}",
        report["backend"]
    );
    assert!(
        u64_at(&report, &["backend", "offered"]) > 0,
        "the seam was never reached at all, so the count above proves nothing: {}",
        report["backend"]
    );
    assert_eq!(u64_at(&report, &["backend", "fired"]), 0, "{report}");
    assert_eq!(report["ok"], Value::Bool(true), "{report}");
}

/// A recursion with no base case, under a backend that ignores its budget **entirely**.
#[test]
fn the_unbounded_runaway_dies_under_a_code_generator_and_hangs_under_a_tree_walker() {
    use std::process::{Command as Raw, Stdio};
    use std::time::{Duration, Instant};

    const SPIN: &str = r#"
fn spin(n: Int) -> Int = 1 + spin(n + 1)

test "a runaway" { assert_eq(spin(0), 0) }
"#;
    let dir = project(SPIN);

    /// Runs one arm as a child and reports whether it ended, and how.
    fn arm(dir: &Path, backend: &str, limit: Duration) -> Option<std::process::ExitStatus> {
        let mut child = Raw::new(assert_cmd::cargo::cargo_bin("ply"))
            .args(["test", "-j", "1", "--color", "never", "--audit-backend"])
            .arg("--backend")
            .arg(backend)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the `ply` binary runs");
        let deadline = Instant::now() + limit;
        loop {
            if let Some(status) = child.try_wait().expect("the child is waitable") {
                return Some(status);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    // The control: the honest code generator over the same corpus comes back, and comes back red
    // with the machine's own bound.
    let honest = arm(dir.path(), "cranelift", Duration::from_secs(60))
        .expect("the honest code generator finished");
    assert!(
        !honest.success(),
        "a recursion with no base case passed: {honest:?}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            honest.signal(),
            None,
            "the honest code generator died by signal on a corpus it should refuse politely"
        );
    }

    // The corruption, over native frames: it dies, and quickly.
    let corrupted = arm(
        dir.path(),
        "cranelift:wrong:exceeds-budget",
        Duration::from_secs(60),
    )
    .expect(
        "a backend that ignores its budget over a recursion with no base case did not come back \
         within 60s — under native frames it is supposed to die, and a hang here means the fuel \
         prologue is being honoured by something that claims not to",
    );
    assert!(
        !corrupted.success(),
        "a backend that ignored its budget entirely reported success: {corrupted:?}"
    );
    // **Died, not merely failed**, and the distinction is what stops this passing vacuously.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert!(
            corrupted.signal().is_some(),
            "the corrupted run ended with an ordinary exit status ({corrupted:?}). Under native \
             frames ignoring the budget is supposed to take the process down; an orderly exit \
             means the corruption bit nothing — check that the fragment is not empty"
        );
    }

    // And the other arm, so that the contrast is asserted rather than recalled: the same corruption
    // over heap-grown frames does NOT come back.
    assert!(
        arm(dir.path(), "wrong:exceeds-budget", Duration::from_secs(10)).is_none(),
        "`reference`'s unbounded runaway now terminates. That is a change in `Reference` and \
         it makes the backend authorisation's account of why this configuration lived nowhere obsolete — \
         update that section rather than this assertion"
    );
}

// --- The result-cache rule, over the code generator --------------------------

/// The cache rule again, on the backend that arrived after it was written.
#[test]
fn a_code_generator_run_reads_no_cached_pass() {
    let dir = project(CORPUS);
    let warm = ply(dir.path()).arg("test").arg("--json").output().unwrap();
    let warm: Value = serde_json::from_slice(&warm.stdout).unwrap();
    assert_eq!(warm["ok"], Value::Bool(true), "{warm}");

    let again = ply(dir.path()).arg("test").arg("--json").output().unwrap();
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert!(
        u64_at(&again, &["summary", "cached"]) > 0,
        "the cache never warmed, so this test cannot tell a backend run that ignored it from one \
         that had nothing to ignore: {again}"
    );

    // No `--audit-backend`, deliberately, so `--backend` is the only thing that could bypass the
    // installed on that path would be cache-safe for a reason that has nothing to do with backends.
    let out = ply(dir.path())
        .arg("test")
        .arg("--backend")
        .arg("cranelift")
        .arg("-j")
        .arg("1")
        .arg("--json")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["no_cache"], Value::Bool(true), "{report}");
    assert_eq!(
        u64_at(&report, &["summary", "cached"]),
        0,
        "a code generator run believed a pass an unbacked run earned: {report}"
    );
    assert!(
        u64_at(&report, &["backend", "entered"]) > 0,
        "nothing entered native code, so no rule about entering it was exercised: {}",
        report["backend"]
    );
    assert!(
        report["diagnostics"]
            .as_array()
            .is_some_and(|d| d.is_empty()),
        "{report}"
    );
}

/// The write half, in a project of its own **because the read half warms the cache**.
#[test]
fn a_code_generator_run_writes_no_pass() {
    let dir = project(CORPUS);
    let out = ply(dir.path())
        .arg("test")
        .arg("--backend")
        .arg("cranelift")
        .arg("-j")
        .arg("1")
        .arg("--json")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["ok"], Value::Bool(true), "{report}");
    assert!(
        u64_at(&report, &["backend", "entered"]) > 0,
        "nothing entered native code, so no rule about entering it was exercised: {}",
        report["backend"]
    );

    // A later run with no backend has to run every test again, because the code generator run left
    // nothing behind.
    let plain = ply(dir.path()).arg("test").arg("--json").output().unwrap();
    let plain: Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert_eq!(
        u64_at(&plain, &["summary", "cached"]),
        0,
        "a run with no backend believed a pass a code generator run recorded: {plain}"
    );
    // Derived from the corpus rather than written down: this read `5` and went stale the moment two
    // tests were added to `CORPUS`, failing a test whose subject — the cache rule above — was still
    // holding.
    let in_corpus = CORPUS.matches("test \"").count() as u64;
    assert_eq!(u64_at(&plain, &["summary", "passed"]), in_corpus, "{plain}");
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
    let generated = run(dir.path(), Some("cranelift:wrong:off-by-one"));
    assert_eq!(generated["backend"]["name"], "cranelift", "{generated}");
}

/// A misspelled backend is refused rather than falling back to one that works.
#[test]
fn a_backend_name_that_is_not_a_spelling_of_anything_is_refused() {
    let dir = project(CORPUS);
    for spec in ["cranelift:reference", "clif", "cranelift:wrong:off-by-two"] {
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

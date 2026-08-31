//! The eight deliberately wrong backends, run through a command a user can run.
//!
//! `CONTRIBUTING.md` §"Things known to be broken" item 13's first bullet read,
//! until this file existed: *"`ply test --engine both` cannot install a backend
//! at all … So the shipping CLI catches **zero** of the eight deliberately wrong
//! backends, and the rule that a backend run must not populate the result cache
//! is **unenforced because it is unreachable**."* ADR 0026 §4.1 decided that a
//! backend is reachable and §4.5 made catching these eight the condition on any
//! backend shipping at all. This is that condition, checked rather than argued.
//!
//! Every test here follows `crates/ply-codegen-spike/tests/mutations.rs`'s three
//! steps, and the middle one is the one that is usually missing:
//!
//!   1. corrupt the backend in one specific way (`--backend wrong:<mutation>`),
//!   2. assert the corruption actually **fired** — `backend.fired` in the
//!      artifact — because a mutation that never changed an answer proves
//!      nothing about the harness that did not catch it,
//!   3. assert `ply test` reported it, by the diagnostic a user would read.
//!
//! # What is caught, and what escapes
//!
//! **Two backends, and the table is per backend rather than per seam.** ADR
//! 0026 §4.5 is a condition on *any* backend — what a corruption can bite
//! depends on which definitions the backend has a body for — so `reference` and
//! `cranelift` are run through the whole table separately and the counts are
//! reported separately. The `cranelift` half is §"The eight, over the code
//! generator" below.
//!
//! Measured on this corpus, 2026-08-28, one run each. **Seven of the eight
//! configurations are caught by `reference`. One escapes**, and saying which is
//! the point of counting rather than claiming eight:
//!
//! | `--backend` | fired | caught by |
//! | --- | ---: | --- |
//! | `wrong:off-by-one` | 2 | two tests, on the value axis |
//! | `wrong:inverted` | 1 | one test, on the value axis |
//! | `wrong:stale` | 2 | two tests — needs `-j 1`, see below |
//! | `wrong:wrong-type` | 3 | three tests, one of them a type error in the caller |
//! | `wrong:unoffered` | 1 | one test: the backend answered for a `List<Int>` body it has none for |
//! | `wrong:exceeds-budget=4` | 1 | one test, on the **verdict** axis: the machine raises and the backend answers |
//! | `wrong:exceeds-budget` over a *terminating* recursion | 1 | the same, on the same axis |
//! | `wrong:answers=<int>@<effectful>` | 0 | **not offered at all** — `offered_target` is 0, which is the gate |
//! | `wrong:exceeds-budget` over a *non-terminating* recursion | — | **nothing. It escapes.** |
//!
//! The last row is the honest answer **about `reference`**, and it is the same
//! finding the spike recorded one layer down. An unbounded runaway on a
//! tree-walker is not a wrong answer: the process never comes back, and every
//! candidate reporter is inside it. Measured rather than reasoned —
//! `ply test --backend wrong:exceeds-budget` over
//! `fn spin(n: Int) -> Int = 1 + spin(n + 1)` produced **no output and did not
//! exit within 45 seconds**, against 0.03s for the run that reports.
//!
//! > **That row moved on 2026-08-31, and it moved because the backend changed
//! > rather than because the harness did.** The paragraph above continued:
//! > *"The reporter has to be outside the process, which is what
//! > `ply_codegen_spike::wrong::run_guarded` is; `ply test` is the process.
//! > There is no standing test for it here, deliberately: the only shape one
//! > could take is a wall clock and a child that grows the heap until it is
//! > killed."*
//! >
//! > The first sentence is still true and is the reason the new test works:
//! > every test in this file already runs `ply` as a **child**, so the reporter
//! > is outside the process by construction. What was missing was a backend
//! > whose runaway *dies* instead of hanging. `cranelift` is native code on a
//! > fixed stack: the same corruption over the same corpus aborts with
//! > `fatal runtime error: stack overflow` and exit **134**, in **0.02 s**
//! > (two runs, release binary), against `reference`'s no-output-in-45-seconds.
//! > A dead child is a fact a parent can assert; a hang is not.
//! > [`the_unbounded_runaway_dies_under_a_code_generator_and_hangs_under_a_tree_walker`]
//! > is that test, and it is a wall clock only in the direction that cannot
//! > produce a false green.
//!
//! `wrong:answers=` is the seventh corruption and it is **not** caught, in the
//! sense that nothing goes red — and that is the finding rather than a gap,
//! exactly as `mutations.rs` records. `Gate::PublishedRow` and
//! `Gate::InternalEffects` mean a definition that performs is never offered, so
//! the mutant stands ready to answer one and is never asked. What stands is the
//! offer count, and it is what
//! [`a_backend_is_never_offered_a_definition_that_performs`] asserts.
//!
//! > **Re-taken whole, 2026-08-31, when `Machine::compiled_answer` began
//! > deciding an answer from the definition's declared RETURN type.** The
//! > corpus changed — `label` was added and `pair` moved inside the fragment,
//! > see [`CORPUS`] — so every row is a different measurement and none is
//! > carried. A ninth backend joins them, `wrong:handle`, which is **not** one
//! > of ADR 0026 §4.5's eight: it attacks the thing that widening gave up,
//! > which is that no `Value::Cell` could come back at all.
//! >
//! > One run each through `target/release/ply`, `-j 1 --no-cache --json`, the
//! > binary checked `current` by `.github/binary-is-current.sh` first. Every row
//! > reads `offered 6 · entered 5 · declined 1` except `unoffered`, which
//! > answers the one it should have declined:
//! >
//! > | `--backend` | fired | tests that reported it |
//! > | --- | ---: | --- |
//! > | `wrong:off-by-one` | 2 | `double doubles`, `triple triples` |
//! > | `wrong:inverted` | 1 | `even is even` |
//! > | `wrong:stale` | 3 | `even is even`, `triple triples`, `a pair has two` |
//! > | `wrong:wrong-type` | 3 | `double doubles`, `even is even`, `triple triples` |
//! > | `wrong:unoffered` | 1 | `a label is a word` — `E0501 expected "n", found 7` |
//! > | `wrong:handle` | 2 | **1 of the 2** — `a pair holds its number`, and that is the row to read twice |
//! > | `reference` | 0 | 0 failed, which is the control |
//! >
//! > `wrong:handle` fires twice and one test notices. The other firing is
//! > `a pair has two`, whose assertion is `len(pair(7)) == 2`: a list with a
//! > forged cell in its head is still two long. That is not a weakness in the
//! > corruption, it is the measure of what a corpus has to *look at* to catch
//! > one — and it is why `a pair holds its number` was added beside it rather
//! > than instead of it.
//!
//! # Why `-j 1`
//!
//! A backend is built per worker, so `wrong:stale` — whose whole corruption is
//! that this call gets the *previous* call's answer — has nothing to be stale
//! about when every test lands on a different worker. Measured: at the default
//! ten workers it fires **0** times over this corpus and nothing goes red; at
//! `-j 1` it fires twice and two tests report it. That is a real weakness in the
//! oracle and it is pinned here rather than smoothed over.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

/// Five definitions and five tests, chosen so that each corruption has
/// something to bite.
///
/// - `double`, `triple` — `Int -> Int`, inside the fragment, for `off-by-one`,
///   `wrong-type` and `stale`.
/// - `even` — `Int -> Bool`, for `inverted`.
/// - `pair` — `Int -> List<Int>`, and `label` — `Int -> String`. The machine
///   **offers** both, because `compiled::admit` gates on the arguments; `label`
///   is the one the backend has no body for. That gap is where `unoffered`
///   lives; without a definition in it the corruption has nothing to invent an
///   answer for.
///
///   > **`pair` moved sides on 2026-08-31 and `label` is its replacement.** The
///   > bullet read, verbatim: *"`pair` — `Int -> List<Int>`. The machine
///   > **offers** it, because `compiled::admit` gates on the shape of the
///   > arguments and never on the return type, and the backend has no body for
///   > it because its signature is not scalar. That gap is where `unoffered`
///   > lives; without a definition in it the corruption has nothing to invent an
///   > answer for."*
///   >
///   > `Machine::compiled_answer` now decides an answer from the definition's
///   > declared **return** type, so `List<Int>` is carried and `pair` is inside
///   > the fragment. Both tests that read this gap failed rather than passing
///   > quietly, which is what they are for:
///   > `an_answer_for_a_definition_the_backend_has_no_body_for_is_caught_by_ply_test`
///   > with `"fired":0 … "declined":0`, and
///   > [`the_honest_backend_agrees_over_the_corpus_and_enters_it`] with *"the
///   > honest backend declined nothing, so the registry-miss path … is
///   > unexercised"*. `label` returns a `String`, which is one of ADR 0019 §5
///   > item 4's three kinds and is deliberately outside the fragment in both
///   > directions, so the gap is a leaf-set fact rather than a container fact
///   > and the next container widening will not close it too.
///   >
///   > `pair` is **kept**, and its second test added, because a definition that
///   > answers a container through the shipping command is exactly what the
///   > widening added and what `wrong:handle` needs to bite on.
/// - `handled` — performs two operations and discharges both under its own
///   `handle`, so it publishes an **empty** row and is refused by
///   `Gate::InternalEffects` rather than by the row gate. It is the target
///   `answers=` stands ready to answer and is never asked about. Copied from
///   `tests/fixtures/self_handled_effect.ply`, which is the only corpus in the
///   tree that reaches those gates.
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
test "a label is a word" { assert_eq(label(7), "n") }
test "a self handled effect still answers" { assert_eq(handled(1), 10) }
"#;

/// One definition whose recursion outruns the machine's own bound, so that
/// `budget` is a number the backend has to honour rather than a hint.
///
/// The test **fails** with or without a backend: 20,000 nested calls is past
/// `DEFAULT_MAX_CALLS`, both engines raise, and they agree about it. That is the
/// control for this corpus — a red run with no divergence reported — because a
/// backend that honours its budget cannot make this test pass.
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

/// One `ply test --engine both -j 1 --json` run with `backend` installed.
///
/// `--engine both` because the oracle is the point: the backend is a third
/// engine and the disagreement is reported against the machine that offered it
/// the call. `-j 1` because a backend is per worker; see this file's header.
fn run(dir: &Path, backend: Option<&str>) -> Value {
    let mut cmd = ply(dir);
    cmd.arg("test")
        .arg("--engine")
        .arg("both")
        .arg("-j")
        .arg("1")
        .arg("--json");
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

/// Every test whose failure is the backend disagreeing with the machine that
/// offered it the call, by the message a user reads.
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

/// The three steps, applied. Asserts the corruption fired before it asserts
/// anything was caught, which is the step `mutations.rs` says is usually
/// missing.
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

/// Without this, a red result below could be the backend's *presence* rather
/// than the corruption.
///
/// Three claims, and the third is the one that makes the others worth anything:
/// the run is green, the honest backend changed no answer, and it **entered
/// bodies**. A backend that entered nothing is a null result — ADR 0018 §0.5
/// records R4 reporting a 0.998x speedup over exactly that — and every test
/// below would be mutating a seam nobody reaches.
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

/// And the other control: the same corpus with no backend at all is green, so
/// nothing below is a corpus that was already broken.
#[test]
fn the_corpus_is_green_with_no_backend() {
    let dir = project(CORPUS);
    let report = run(dir.path(), None);
    assert_eq!(report["ok"], Value::Bool(true), "{report}");
    assert!(report["backend"].is_null(), "{report}");
}

// --- The eight --------------------------------------------------------------

/// 1. Off by one on a compiled arithmetic result.
#[test]
fn an_off_by_one_in_a_compiled_answer_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:off-by-one");
    assert!(
        caught.contains(&"m.double doubles".to_string()),
        "{caught:?}"
    );
}

/// 2. An inverted compiled comparison.
#[test]
fn an_inverted_compiled_comparison_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:inverted");
    assert!(caught.contains(&"m.even is even".to_string()), "{caught:?}");
}

/// 3. A stale answer: this call gets the previous call's result.
///
/// The one corruption that is invisible to a single call — every answer it gives
/// was a correct answer to *some* call — so what catches it is a corpus that
/// varies its arguments across one worker's backend. See this file's header for
/// why `-j 1` is load-bearing here and nowhere else.
#[test]
fn a_stale_compiled_answer_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    fires_and_is_caught(dir.path(), "wrong:stale");
}

/// 4. The right information in the wrong kind.
///
/// The seam checks a *kind* and carries `Bool` and `Int` both, so it does not
/// refuse this and the wrong-kinded value reaches the program. That is the
/// boundary behaving as `compiled.rs` documents, and it is why the check has to
/// be downstream of it.
#[test]
fn a_bool_where_an_int_belongs_crosses_the_seam_and_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:wrong-type");
    assert!(
        caught.contains(&"m.double doubles".to_string()),
        "{caught:?}"
    );
}

/// 5. An answer for a definition the backend has no body for.
///
/// The machine offers every pure call whose arguments cross, so most of what a
/// backend sees are names it has nothing to say about. Declining is the whole of
/// its contract there, and `label` is the definition in this corpus that is
/// offered and outside the fragment.
///
/// > **The target moved on 2026-08-31 and the sentence above is corrected in
/// > place.** It read: *"and `pair` is the definition in this corpus that is
/// > offered and outside the fragment"*, and the assertion below read
/// > `caught.contains(&"m.a pair has two".to_string())`. `pair` answers a
/// > `List<Int>` and a container return is now inside the fragment, so this test
/// > failed with `"fired":0` — correctly, and that is the whole value of step 2.
#[test]
fn an_answer_for_a_definition_the_backend_has_no_body_for_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:unoffered");
    assert!(
        caught.contains(&"m.a label is a word".to_string()),
        "{caught:?}"
    );
}

/// 9. A handle into this run's world, inside an otherwise honest container answer.
///
/// Not one of ADR 0026 §4.5's eight. It exists because the 2026-08-31 widening
/// of `Machine::compiled_answer` gave up a structural claim — while the answer
/// test read the answer's discriminant, no `Value::Cell` could come back at all
/// — and replaced it with the declared return type plus the answer's top-level
/// kind. `pair` answers a `List<Int>`; this corruption returns a list of the
/// right kind with a `Value::Cell` in its head, and the seam believes it.
///
/// What catches it is the program, downstream, exactly as for a wrong `Int`:
/// `assert_eq(pair(7), [7, 7])` compares a cell against a number. `len(pair(7))`
/// does **not** catch it, which is why the second test on `pair` was added with
/// this one — a corpus that only measures a container's length cannot see what
/// is in it.
#[test]
fn a_forged_handle_inside_a_container_answer_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "wrong:handle");
    assert!(
        caught.contains(&"m.a pair holds its number".to_string()),
        "{caught:?}"
    );
}

/// 6. Running past the call budget instead of declining.
///
/// `budget` is the machine's remaining nested calls, and a body that would
/// outrun it must answer `None` so the machine can raise the bound both engines
/// raise. This is caught on the **verdict** axis and only there: the machine
/// raises `recursion limit of 10000 nested calls exceeded` and the backend
/// answers a number.
///
/// It is also the case that found a real hole in the comparison while this was
/// being built. Compared only where the two engines had already agreed on a
/// *pass*, this run reported **nothing at all** — a backend that turns a red
/// test green was the one thing the third pair could not see, which is `(Err,
/// Err)` scored as agreement wearing a third engine's clothes. See
/// `ply_test::InterpExecutor::execute_directly`.
#[test]
fn a_backend_that_runs_past_its_budget_is_caught_by_ply_test() {
    let dir = project(DEEP);
    // The control first: this corpus is red on its own, and no backend is
    // blamed for it.
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

/// 6b. Ignoring the budget *entirely*, over a recursion that terminates.
///
/// Unlimited fuel is caught the same way as four times the budget when the body
/// it is spent on comes back. What it is **not** caught by — and cannot be — is
/// anything at all when the body does not: see this file's header.
#[test]
fn a_backend_that_ignores_its_budget_is_caught_where_the_body_terminates() {
    let dir = project(DEEP);
    let caught = fires_and_is_caught(dir.path(), "wrong:exceeds-budget");
    assert_eq!(caught, vec!["m.a ladder past the machine's bound"]);
}

/// 7. Accepting a call the machine must never offer.
///
/// **This one cannot be done from a backend**, and that is the finding rather
/// than a gap — `crates/ply-codegen-spike/tests/mutations.rs` records the same
/// thing about the same corruption. The mutant stands ready to answer
/// `m.handled` with a number and is never asked, because `handled` performs two
/// operations under a `handle` of its own and `Gate::InternalEffects` refuses
/// it. What stands is the offer count, which is the fact the gate makes true.
///
/// The second assertion is what stops this being vacuous: the seam was reached
/// **four** times on this corpus while the target was offered zero times, so the
/// zero is a gate rather than a backend nobody consulted.
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

/// ADR 0026 §4.6, both stages, on the path where neither is an accident.
///
/// `--engine both` already bypasses the cache, so a backend installed on *that*
/// path would be cache-safe for a reason that has nothing to do with backends.
/// The default engine is where the rule has to hold on its own, so that is what
/// this runs on.
///
/// Seen to fail, twice, and by two different corruptions — which is the point of
/// arming it in two stages:
///
/// - Deleting `args.backend.is_some()` from `cache_bypassed` (stage one, the
///   flag half): with a warm cache the backend run reports `selected 0 of 5 (5
///   cached)` and `0 of 0 offers entered`. A green run over a backend that never
///   ran, which is exactly the defect shape `CONTRIBUTING.md` §"The one rule"
///   names. `a_backend_run_reads_no_cached_pass` is what goes red.
/// - Deleting the `Record::Backend` arm from `ply_test::run_with` (stage two,
///   the diagnostic): the run writes three passes and `backend_escapes` reports
///   `E0505 \`double doubles\` entered compiled code, and its pass was written to
///   the result cache` for each, exit 1. `a_backend_run_writes_no_pass` is what
///   goes red.
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
        "a backend run believed a pass the authoritative engine earned: {report}"
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

    // And the fact the diagnostic is a check *on*: a later run with no backend
    // has to run every test again, because the backend run left nothing behind.
    let plain = ply(dir.path()).arg("test").arg("--json").output().unwrap();
    let plain: Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert_eq!(
        u64_at(&plain, &["summary", "cached"]),
        0,
        "a run with no backend believed a pass a backend run recorded: {plain}"
    );
    // Seven: `CORPUS` gained `label` and a second test on `pair` on 2026-08-31
    // (see [`CORPUS`]'s note). This assertion read `5` and failed with
    // `left: 7 / right: 5`, which is a corpus-size fact and not a cache fact.
    assert_eq!(u64_at(&plain, &["summary", "passed"]), 7, "{plain}");
}

// --- The flag itself --------------------------------------------------------

/// A flag that is accepted and does nothing is `CONTRIBUTING.md` §"The one
/// rule"'s defect shape, so the engine that cannot host a backend refuses it
/// rather than ignoring it.
#[test]
fn the_backend_flag_is_refused_by_an_engine_with_no_compiled_path() {
    let dir = project(CORPUS);
    let out = ply(dir.path())
        .arg("test")
        .arg("--engine")
        .arg("treewalk")
        .arg("--backend")
        .arg("reference")
        .arg("--json")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["ok"], Value::Bool(false), "{report}");
    assert_eq!(report["diagnostics"][0]["code"], "E0450", "{report}");
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
//
// ADR 0026 §4.5's condition — *"No backend may ship until `ply test --engine
// both` can attach one and catch the eight"* — is a condition on **a** backend,
// and until 2026-08-31 there was one. `ply_eval::backend::Mutant` wrapped a
// concrete `Rc<Reference>`, so a second implementation of `Compiled` could not
// be corrupted even if it could be installed. `ply_eval::Policed` is the trait
// that fixed it and `ply_codegen::Cranelift` is the second implementation.
//
// **Measured, 2026-08-31, one run each, release binary, on this file's `CORPUS`
// and `DEEP` unless a row says otherwise.** The `fragment` column is the point
// of running the table twice: a corruption can only bite a definition the
// backend has a body for, and the two backends hold different sets.
//
// | `--backend` | fired | caught |
// | --- | ---: | --- |
// | `cranelift` (the control) | 0 | — · 3 of 4 offers entered, fragment 3 |
// | `cranelift:wrong:off-by-one` | 2 | `double doubles`, `triple triples` |
// | `cranelift:wrong:inverted` | 1 | `even is even` |
// | `cranelift:wrong:stale` | 2 | `even is even`, `triple triples` |
// | `cranelift:wrong:wrong-type` | 3 | all three scalar tests |
// | `cranelift:wrong:unoffered` | 1 | `a pair has two` |
// | `cranelift:wrong:exceeds-budget=4` | 1 | the ladder, on the verdict axis |
// | `cranelift:wrong:exceeds-budget`, terminating | 1 | the same |
// | `cranelift:wrong:answers=<int>@<effectful>` | 0 | **not offered** — the gate |
// | `cranelift:wrong:exceeds-budget`, NON-terminating | — | **the child dies, exit 134** |
//
// So **eight of the eight configurations are accounted for under `cranelift`**,
// against seven under `reference`, and the one that moved is the last row.
// `answers=` is still not "caught" in the sense of something going red, and
// that is still the finding rather than a gap: what stands is an offer count of
// zero, which is a claim about `compiled::admit`'s gates and not about a
// backend.

/// The control for everything below: the honest code generator is green, changes
/// no answer, and **enters bodies**.
///
/// The third clause is the one that makes the others worth anything. ADR 0018
/// §0.5 records R4 reporting a 0.998x speedup over a backend that entered
/// nothing, and a corruption of a backend nobody reaches is a mutation of
/// nothing.
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
    // A code generator compiled something, and the report says how much it
    // cost. `reference` reports no compilation at all, which is the difference
    // `ply_eval::Provider::compilation` returns `Option` to express.
    assert!(
        u64_at(&report, &["backend", "units"]) > 0,
        "no unit was compiled, so `cranelift` installed something that is not a code generator: {}",
        report["backend"]
    );
    let plain = run(dir.path(), Some("reference"));
    assert!(
        plain["backend"]["units"].is_null(),
        "the tree-walker reported a compilation: {}",
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

/// The registry-miss path, over a backend whose registry is much smaller than
/// the tree-walker's.
///
/// `pair` returns a `List<Int>`, so `compiled::admit` offers it — it gates on
/// the shape of the *arguments* and never on the return type — and the code
/// generator has no body for it. That gap is where this corruption lives, and
/// it is **wider** here than under `reference`: this fragment is `Int`/`Bool`
/// only, so `Bytes` signatures fall in it too.
#[test]
fn an_answer_from_compiled_code_for_a_body_it_lacks_is_caught_by_ply_test() {
    let dir = project(CORPUS);
    let caught = fires_and_is_caught(dir.path(), "cranelift:wrong:unoffered");
    assert!(
        caught.contains(&"m.a pair has two".to_string()),
        "{caught:?}"
    );
}

/// The fuel prologue is four instructions in every compiled body — load,
/// subtract, branch, store — and this is what says they are load-bearing.
///
/// The control is the same one the tree-walker's test uses: the corpus is red
/// on its own, with both engines raising the same bound and no backend blamed.
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
///
/// Unchanged from the tree-walker's answer, and it should be: the gate is
/// `compiled::admit`'s and not a backend's. `handled` performs two operations
/// under a `handle` of its own, `Gate::InternalEffects` refuses it, and the
/// mutant stands ready to answer a call it is never asked.
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

/// A recursion with no base case, under a backend that ignores its budget
/// **entirely**. The eighth configuration, and the one ADR 0026 §4.7 recorded as
/// living nowhere in this workspace.
///
/// That record read, of the tree-walker: *"`Reference` is a tree-walker whose
/// frames grow on the heap through `stacker`, so the same corruption does not
/// crash — it **hangs**, measured at no output and no exit in 45 seconds against
/// 0.03s for the run that reports. A harness can run it as a child; what it
/// cannot do is tell a hang from work, and a wall clock is not a
/// disagreement."*
///
/// Every word of that is still true of `reference`, and this test asserts it
/// alongside the other arm rather than taking it on trust. What changed is the
/// backend: native frames sit on a fixed stack, so ignoring the budget
/// **kills the process** instead of hanging it, and a dead child is a fact its
/// parent can read. Both halves are here because the contrast is the finding:
/// one arm must die, the other must not, and a run in which both die would mean
/// the corpus rather than the backend is doing it.
///
/// # Why this is not a wall-clock test in the direction that matters
///
/// The timeout can only turn a **hang into a failure**, never a failure into a
/// pass. The cranelift arm asserts the child died; if a future change made it
/// hang instead, the timeout fires and this goes red. The tree-walker arm
/// asserts the child was *still running* at the deadline; a machine so slow
/// that a correct run had not finished in ten seconds would be the only false
/// red, and the run being timed produces its first output in 0.03 s.
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
            .args(["test", "--engine", "both", "-j", "1", "--color", "never"])
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

    // The control: the honest code generator over the same corpus comes back,
    // and comes back red with the machine's own bound. Without this, a death
    // below could be the corpus rather than the corruption.
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
    // **Died, not merely failed**, and the distinction is what stops this
    // passing vacuously. Watched: with the code generator's fragment forced
    // empty, the corruption has no body to run past its budget, the run comes
    // back red with the machine's own `recursion limit` — and `!success()`
    // alone still held. Nine other tests in this file went red on that
    // corruption and this one did not, which is why the signal is asserted.
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

    // And the other arm, so that the contrast is asserted rather than recalled:
    // the same corruption over heap-grown frames does NOT come back.
    assert!(
        arm(dir.path(), "wrong:exceeds-budget", Duration::from_secs(10)).is_none(),
        "the tree-walker's unbounded runaway now terminates. That is a change in `Reference` and \
         it makes ADR 0026 §4.7's account of why this configuration lived nowhere obsolete — \
         update that section rather than this assertion"
    );
}

// --- The result-cache rule, over the code generator --------------------------

/// ADR 0026 §4.6 again, on the backend that arrived after it was written.
///
/// Both stages are backend-agnostic by construction — `cache_bypassed` reads
/// `args.backend`, which is a string, and `backend_escapes` reads the entry
/// count the *machine* recorded — so this is a check that the arming did not
/// quietly depend on which backend was installed. It is run rather than
/// reasoned because "it must still hold" is the sentence this project's
/// §"The one rule" is about.
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

    // The default engine, deliberately: `--engine both` already bypasses the
    // cache, so a backend installed on that path would be cache-safe for a
    // reason that has nothing to do with backends.
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
        "a code generator run believed a pass the authoritative engine earned: {report}"
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

/// The write half, in a project of its own **because the read half warms the
/// cache**.
///
/// Sharing a directory between the two would make this assertion vacuous in the
/// worst way: the plain run at the end would find five cached passes left by the
/// warming runs and the test would be reporting the warming rather than the
/// backend. Watched: written as one test over one directory, it fails with
/// `left: 5, right: 0` on a tree where the rule holds perfectly.
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

    // A later run with no backend has to run every test again, because the code
    // generator run left nothing behind.
    let plain = ply(dir.path()).arg("test").arg("--json").output().unwrap();
    let plain: Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert_eq!(
        u64_at(&plain, &["summary", "cached"]),
        0,
        "a run with no backend believed a pass a code generator run recorded: {plain}"
    );
    assert_eq!(u64_at(&plain, &["summary", "passed"]), 5, "{plain}");
}

// --- The grammar -------------------------------------------------------------

/// A corruption may name the backend it wraps, and a bare `wrong:` still means
/// the tree-walker.
///
/// The default is a compatibility choice and it is the conservative one: every
/// `--backend wrong:` invocation recorded in this repository was taken against
/// `reference`, and silently re-pointing them at a code generator would change
/// what a re-run of a logged experiment measures. Asserted on the `name` field
/// of the artifact, which is read off the provider that was actually installed
/// rather than off the flag.
#[test]
fn a_bare_wrong_prefix_still_names_the_tree_walker() {
    let dir = project(CORPUS);
    let bare = run(dir.path(), Some("wrong:off-by-one"));
    assert_eq!(bare["backend"]["name"], "reference", "{bare}");
    let named = run(dir.path(), Some("reference:wrong:off-by-one"));
    assert_eq!(named["backend"]["name"], "reference", "{named}");
    let generated = run(dir.path(), Some("cranelift:wrong:off-by-one"));
    assert_eq!(generated["backend"]["name"], "cranelift", "{generated}");
}

/// A misspelled backend is refused rather than falling back to one that works.
///
/// Two spellings, and the second is the one worth pinning: `cranelift:reference`
/// parses a backend prefix and then finds no corruption, which must be a refusal
/// and not a silent installation of either half.
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

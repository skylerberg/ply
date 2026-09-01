//! Drives the real binary.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

const GREEN: &str = "\
fn double(x: Int) -> Int = x * 2

fn main() -> Int = double(21)

test \"double doubles\" { assert_eq(double(4), 8) }

test \"double of zero is zero\" { assert_eq(double(0), 0) }
";

const RED: &str = "\
fn balance() -> Int = 0 - 5

fn main() -> Int = balance()

test \"balance never goes negative\" { assert_eq(balance(), 0) }

test \"this one is fine\" { assert_eq(1 + 1, 2) }
";

const BROKEN: &str = "fn f() -> Int = true\n";

const UNPARSEABLE: &str = "fn f(( = )\n";

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

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn json_of(output: &std::process::Output) -> Value {
    let text = stdout_of(output);
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout was not one JSON object: {e}\n---\n{text}\n---"))
}

// --- check ------------------------------------------------------------------

#[test]
fn check_accepts_a_good_module() {
    let dir = project(GREEN);
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout_of(&out).contains("checked 1 module, 2 definitions, 2 tests"));
}

#[test]
fn check_types_prints_signatures_and_footprints() {
    let dir = project(
        "effect db {\n  read all[t]() -> List<Int>\n}\n\
         fn rows() -> List<Int> / {db.read[users]} = db.all[users]()\n",
    );
    let out = ply(dir.path()).args(["check", "--types"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    // W3 moved the row onto its own line under the type, because at a hundred endpoints a row run
    // onto the end of a signature is a row nobody reads.
    assert!(
        text.contains("     rows : () -> List<Int>\n            / {m.db.read[users]}\n"),
        "got:\n{text}"
    );
    assert!(text.contains("effect db"));
}

/// `--costs` — ADR 0025 §Decision 2a, built at ADR 0032 §11 S2.
///
/// The two definitions differ by **argument order alone** and compute the same
/// list, which is the whole point of the flag: nothing else in the output of
/// `ply check` distinguishes them, and the difference is asymptotic. Pinned
/// here rather than in `ply-eval` because the columns are what a reader diffs,
/// and because until this test existed `ply_eval::costs` had no caller outside
/// its own tests.
///
/// It cannot pass by the checker answering one thing everywhere: `grows_last`
/// must read `reuses` and `grows_first` must read `COPIES`, so a constant
/// verdict in either direction reddens one of the two assertions.
#[test]
fn check_costs_separates_two_spellings_of_one_computation() {
    let dir = project(
        "fn grows_last(n: Int, xs: List<Int>) -> List<Int> =\n\
         \x20 if n == 0 { xs } else { grows_last(n - 1, push(xs, n)) }\n\
         fn grows_first(xs: List<Int>, n: Int) -> List<Int> =\n\
         \x20 if n == 0 { xs } else { grows_first(push(xs, n), n - 1) }\n",
    );
    let out = ply(dir.path()).args(["check", "--costs"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);

    assert!(
        text.contains("m.grows_last  1 reuses"),
        "the last-position spelling must read `reuses`:\n{text}"
    );
    assert!(
        text.contains("m.grows_first  1 COPIES"),
        "the non-final spelling must read `COPIES`:\n{text}"
    );
    // The fix is named, and `copy` is not what is offered — ADR 0025
    // §Decision 4 puts the reordering first and never recommends the
    // pessimization.
    assert!(
        text.contains("fix: move the append into last position"),
        "a copy must name its edit:\n{text}"
    );
    assert!(
        !text.contains("copy("),
        "`copy` is never the recommended fix:\n{text}"
    );
    assert!(
        text.contains("2 appends: 1 reuse, 1 copy, 0 undecided"),
        "the whole-program tally:\n{text}"
    );
}

/// A program with no append says so, rather than printing nothing and leaving a
/// reader unable to tell a clean program from a flag that did not run.
#[test]
fn check_costs_says_so_when_there_is_nothing_to_cost() {
    let dir = project(GREEN);
    let out = ply(dir.path()).args(["check", "--costs"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout_of(&out).contains("no appends: nothing to cost"),
        "got:\n{}",
        stdout_of(&out)
    );
}

#[test]
fn check_exits_two_on_a_type_error_and_says_nothing_on_stdout() {
    let dir = project(BROKEN);
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stdout_of(&out), "");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("E0201"),
        "expected a type mismatch, got:\n{stderr}"
    );
}

#[test]
fn check_exits_two_on_a_syntax_error() {
    let dir = project(UNPARSEABLE);
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8(out.stderr).unwrap().contains("E0001"));
}

#[test]
fn check_json_is_a_single_object_even_when_the_module_is_broken() {
    let dir = project(BROKEN);
    let out = ply(dir.path()).args(["check", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v = json_of(&out);
    assert_eq!(v["command"], "check");
    assert_eq!(v["ok"], false);
    assert_eq!(v["diagnostics"][0]["code"], "E0201");
    assert_eq!(v["diagnostics"][0]["labels"][0]["file"], "m.ply");
    assert_eq!(v["diagnostics"][0]["labels"][0]["start"]["line"], 1);
}

// --- test -------------------------------------------------------------------

#[test]
fn test_leads_with_the_selection_line() {
    let dir = project(GREEN);
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    let first = text.lines().next().unwrap();
    assert_eq!(first.trim(), "selected 2 of 2 (0 cached)");
    assert!(text.contains("workers"));
    assert!(text.contains("0 failed, 2 passed, 0 cached"));
}

#[test]
fn a_second_run_selects_nothing_because_the_cache_is_exact() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("selected 0 of 2 (2 cached)"), "got:\n{text}");
    assert!(text.contains("0 failed, 0 passed, 2 cached"));
}

#[test]
fn renaming_a_definition_re_runs_nothing() {
    let dir = project("fn width() -> Int = 3\ntest \"width is three\" { assert_eq(width(), 3) }\n");
    ply(dir.path()).arg("test").assert().success();

    std::fs::write(
        dir.path().join("m.ply"),
        "fn breadth() -> Int = 3\ntest \"width is three\" { assert_eq(breadth(), 3) }\n",
    )
    .unwrap();

    let out = ply(dir.path()).arg("test").output().unwrap();
    let text = stdout_of(&out);
    assert!(
        text.contains("selected 0 of 1 (1 cached)"),
        "a rename rebuilt something:\n{text}"
    );
}

#[test]
fn editing_a_body_re_runs_exactly_the_tests_that_reach_it() {
    let dir = project(
        "fn a() -> Int = 1\n\
         fn b() -> Int = 2\n\
         test \"a is one\" { assert_eq(a(), 1) }\n\
         test \"b is two\" { assert_eq(b(), 2) }\n",
    );
    ply(dir.path()).arg("test").assert().success();

    std::fs::write(
        dir.path().join("m.ply"),
        "fn a() -> Int = 0 + 1\n\
         fn b() -> Int = 2\n\
         test \"a is one\" { assert_eq(a(), 1) }\n\
         test \"b is two\" { assert_eq(b(), 2) }\n",
    )
    .unwrap();

    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let v = json_of(&out);
    assert_eq!(v["selection"]["selected"], 1);
    assert_eq!(v["selection"]["cached"], 1);
    let ran: Vec<&str> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(ran, ["a is one"]);
}

/// `ply test` loads twice: once to select, and once more to parse the modules a selected test needs
/// a body from.
#[test]
fn a_nondet_test_elsewhere_does_not_cost_an_edited_module_its_body() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("clock.ply"),
        "nondet effect wall { read now() -> Int }\n\
         fn tick() -> Int / {wall.read} = wall.now()\n\
         test/nondet \"the clock ticks\" { assert_eq(handle tick() with { wall.now() -> 1, }, 1) }\n",
    )
    .unwrap();
    let counted = |n: &str| {
        format!("fn size() -> Int = {n}\ntest \"size is known\" {{ assert_eq(size(), {n}) }}\n")
    };
    std::fs::write(dir.path().join("shape.ply"), counted("3")).unwrap();
    ply(dir.path()).arg("test").assert().success();

    std::fs::write(dir.path().join("shape.ply"), counted("4")).unwrap();
    let out = ply(dir.path()).arg("test").output().unwrap();
    let text = stdout_of(&out);
    assert_eq!(out.status.code(), Some(0), "got:\n{text}");
    assert!(
        text.contains("size is known"),
        "the edited module's test did not run:\n{text}"
    );
    assert!(
        !text.contains("cache clear"),
        "a body went missing:\n{text}"
    );
}

#[test]
fn test_exits_one_on_a_failure_and_names_the_suspects() {
    let dir = project(RED);
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = stdout_of(&out);
    assert!(text.contains("1 failed, 1 passed"), "got:\n{text}");
    assert!(text.contains("balance never goes negative"));
    assert!(
        text.contains("assertion failed: expected 0, found -5"),
        "got:\n{text}"
    );
    assert!(text.contains("at m.ply:"), "got:\n{text}");
    assert!(text.contains("suspects: m.balance"), "got:\n{text}");
}

#[test]
fn a_red_test_re_runs_until_it_goes_green() {
    let dir = project(RED);
    ply(dir.path()).arg("test").assert().code(1);

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = stdout_of(&out);
    assert!(
        text.contains("selected 1 of 2 (1 cached)"),
        "a failure was cached:\n{text}"
    );
}

#[test]
fn test_exits_two_on_a_compile_error() {
    let dir = project(BROKEN);
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stdout_of(&out), "");
}

#[test]
fn test_json_is_exactly_one_object_on_stdout() {
    let dir = project(RED);
    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stderr, b"", "--json must not write to stderr either");

    let v = json_of(&out);
    assert_eq!(v["command"], "test");
    assert_eq!(v["ok"], false);
    assert_eq!(v["exit_code"], 1);
    assert_eq!(v["summary"]["failed"], 1);
    assert_eq!(v["summary"]["passed"], 1);

    let failure = &v["failures"][0];
    assert!(
        failure["name"]
            .as_str()
            .unwrap()
            .contains("balance never goes negative"),
        "got: {}",
        failure["name"]
    );
    assert_eq!(failure["diagnostic"]["code"], "E0501");
    assert_eq!(failure["diagnostic"]["labels"][0]["file"], "m.ply");
    assert!(failure["diagnostic"]["labels"][0]["start"]["line"].is_number());
    // Schema v2: a ranked object per suspect, so that a consumer reading only `suspects[0]` gets
    // the best guess rather than the alphabetically first.
    assert_eq!(failure["suspects"][0]["name"], "m.balance");
    assert_eq!(failure["suspects"].as_array().unwrap().len(), 1);

    let selected: Vec<bool> = v["selection"]["tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["selected"].as_bool().unwrap())
        .collect();
    assert_eq!(selected, [true, true]);
    assert!(v["selection"]["groups"][0]["footprint"].is_string());
}

#[test]
fn json_output_carries_no_ansi_escapes() {
    let dir = project(RED);
    let out = ply(dir.path())
        .args(["test", "--json", "--color", "always"])
        .output()
        .unwrap();
    let text = stdout_of(&out);
    assert!(
        !text.contains('\x1b'),
        "--json must stay machine-readable under --color always"
    );
    let _: Value = serde_json::from_str(&text).unwrap();
}

#[test]
fn a_pipe_gets_ascii_marks_and_a_terminal_would_get_glyphs() {
    let dir = project(GREEN);

    let piped = stdout_of(&ply(dir.path()).arg("test").output().unwrap());
    assert!(piped.contains("ok    double doubles"), "got:\n{piped}");
    assert!(!piped.contains('✓'));
    assert!(!piped.contains('\x1b'));

    let dir = project(GREEN);
    let forced = stdout_of(
        &Command::cargo_bin("ply")
            .unwrap()
            .current_dir(dir.path())
            .args(["test", "--color", "always"])
            .output()
            .unwrap(),
    );
    assert!(forced.contains('✓'), "got:\n{forced}");
    assert!(forced.contains('\x1b'));
}

#[test]
fn explain_reports_a_reason_per_test_and_a_footprint_per_group() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();

    let out = ply(dir.path())
        .args(["test", "--explain"])
        .output()
        .unwrap();
    let text = stdout_of(&out);
    assert!(
        text.contains("skip"),
        "cached tests must be shown as skipped:\n{text}"
    );
    assert!(text.contains("cached"));
    assert!(
        text.contains("this exact hash already passed"),
        "got:\n{text}"
    );
    assert!(text.contains("concurrency groups") || text.contains("selected 0 of 2"));

    let cold = project(GREEN);
    let text = stdout_of(
        &ply(cold.path())
            .args(["test", "--explain"])
            .output()
            .unwrap(),
    );
    assert!(text.contains("run "), "got:\n{text}");
    assert!(text.contains("concurrency groups"), "got:\n{text}");
    assert!(text.contains("group 0 ·"), "got:\n{text}");
    assert!(
        text.contains("· {}"),
        "the group's defining footprint is missing:\n{text}"
    );
    assert!(text.contains("isolation: region"), "got:\n{text}");
    assert!(text.contains("region-isolated and free"), "got:\n{text}");
}

/// ADR 0017 §6 through ADR 0008 §6: a report that still said `world` after the world was gone would
/// over-claim by exactly the tests that moved, so `--explain` names the contention *and* what kind
/// it is.
#[test]
fn explain_separates_a_region_label_contention_from_a_real_one() {
    // The `cell` atoms reach the footprint through a written row: ADR 0017 §2 closed every route
    // that carried a cell out of its region, so an annotation is the only way one gets there.
    let dir = project(
        "fn touches(n: Int) -> Int / {cell.read[table], cell.write[table]} = n\n\
         test \"a\" {\n  \
           let seen = with_cell[table](1) { c -> { cell_set(c, 2); cell_get(c) } };\n  \
           assert_eq(touches(seen), 2)\n}\n\
         test \"b\" {\n  \
           let seen = with_cell[table](3) { c -> { cell_set(c, 4); cell_get(c) } };\n  \
           assert_eq(touches(seen), 4)\n}\n\
         test \"pure\" { assert_eq(1, 1) }\n",
    );
    let text = stdout_of(
        &ply(dir.path())
            .args(["test", "--explain"])
            .output()
            .unwrap(),
    );

    assert!(
        text.contains("isolation: shared {cell.read[table], cell.write[table]} (region labels)"),
        "a label contention must say it is one:\n{text}"
    );
    assert!(text.contains("isolation: region"), "got:\n{text}");
    assert!(
        text.contains("1 of 3 region-isolated and free"),
        "two writers of one label are not isolated any more:\n{text}"
    );
    assert!(
        text.contains("2 of them only over a region label"),
        "the cost of losing the fork is a number on every run:\n{text}"
    );
    assert!(
        text.contains("group 1 ·"),
        "two writers of one label may not share a group:\n{text}"
    );

    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    assert_eq!(v["selection"]["parallelism"]["region_contended"], 2);
    assert_eq!(v["selection"]["parallelism"]["isolated"], 1);
}

/// The milestone's claim has to be a number a project can watch, on every run and not only under
/// `--explain`.
#[test]
fn a_run_reports_how_many_tests_cannot_disturb_another() {
    let dir = project(GREEN);
    let text = stdout_of(&ply(dir.path()).arg("test").output().unwrap());
    assert!(text.contains("isolated 2 of 2"), "got:\n{text}");

    // The writing test cannot pass — an atom that escapes the test is an operation nothing handles
    // — but the schedule is still an artifact about it, which is the half being asserted here.
    let effectful = project(
        "effect db {\n  write put[t](v: Int) -> Unit\n}\n\
         fn store(v: Int) -> Int / {db.write[users]} = { db.put[users](v); v }\n\
         test \"writes\" { assert_eq(store(1), 1) }\n\
         test \"pure\" { assert_eq(1, 1) }\n",
    );
    let out = ply(effectful.path())
        .args(["test", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["selection"]["isolated"], 1);
    assert_eq!(v["selection"]["parallelism"]["total"], 2);
    assert_eq!(v["selection"]["parallelism"]["shared"], 1);

    let tests = v["selection"]["tests"].as_array().unwrap();
    let writes = tests
        .iter()
        .find(|t| t["name"] == "writes")
        .expect("the writing test");
    assert_eq!(writes["isolation"], "shared");
    assert_eq!(writes["shared_atoms"][0], "m.db.write[users]");
    let pure = tests
        .iter()
        .find(|t| t["name"] == "pure")
        .expect("the pure test");
    assert_eq!(pure["isolation"], "region");
    assert_eq!(pure["shared_atoms"].as_array().unwrap().len(), 0);
}

#[test]
fn no_cache_runs_everything_and_records_nothing() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();

    let text = stdout_of(
        &ply(dir.path())
            .args(["test", "--no-cache"])
            .output()
            .unwrap(),
    );
    assert!(text.contains("selected 2 of 2 (0 cached)"), "got:\n{text}");
    assert!(text.contains("neither read nor recorded"));

    let after = stdout_of(&ply(dir.path()).arg("test").output().unwrap());
    assert!(
        after.contains("selected 0 of 2 (2 cached)"),
        "--no-cache disturbed the real cache:\n{after}"
    );
}

#[test]
fn filter_narrows_both_the_run_and_the_denominator() {
    let dir = project(GREEN);
    let out = ply(dir.path())
        .args(["test", "--filter", "zero"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("selected 1 of 1 (0 cached)"), "got:\n{text}");
    assert!(text.contains("--filter hid 1 test"));
    assert!(text.contains("double of zero is zero"));
    assert!(
        !text.contains("double doubles"),
        "the filtered-out test still ran:\n{text}"
    );
}

#[test]
fn a_filter_matching_nothing_says_so_rather_than_claiming_success_quietly() {
    let dir = project(GREEN);
    let out = ply(dir.path())
        .args(["test", "--filter", "nonexistent"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("selected 0 of 0"), "got:\n{text}");
    assert!(
        text.contains("no test key contains that substring"),
        "got:\n{text}"
    );
}

#[test]
fn jobs_is_honoured_and_reported() {
    let dir = project(GREEN);
    let out = ply(dir.path())
        .args(["test", "--jobs", "1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout_of(&out).contains("1 worker"),
        "got:\n{}",
        stdout_of(&out)
    );

    let dir = project(GREEN);
    let v = json_of(
        &ply(dir.path())
            .args(["test", "--json", "-j", "3"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["workers"], 3);
}

#[test]
fn a_module_with_no_tests_is_a_clean_run_that_says_so() {
    let dir = project("fn f() -> Int = 1\n");
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("selected 0 of 0 (0 cached)"));
    assert!(text.contains("no `test` items"));
}

#[test]
fn a_nondet_test_in_a_det_test_is_a_compile_error() {
    let dir = project(
        "nondet effect wall {\n  read now() -> Int\n}\n\
         test \"reads the clock\" { assert(wall.now() > 0) }\n",
    );
    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v = json_of(&out);
    assert_eq!(v["diagnostics"][0]["code"], "E0412");
}

// --- directories ------------------------------------------------------------

fn multi_module() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.ply"), "pub fn a() -> Int = 1\n").unwrap();
    std::fs::write(
        dir.path().join("src/z.ply"),
        "import src.a\npub fn z() -> Int = a::a() + 1\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/t.ply"),
        "import src.z (z)\ntest \"z is two\" { assert_eq(z(), 2) }\n",
    )
    .unwrap();
    dir
}

#[test]
fn every_file_under_a_directory_is_its_own_module() {
    let dir = multi_module();

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout_of(&out).contains("selected 1 of 1"));

    let v = json_of(&ply(dir.path()).args(["check", "--json"]).output().unwrap());
    assert_eq!(
        v["files"],
        serde_json::json!(["src/a.ply", "src/t.ply", "src/z.ply"])
    );
    let names: Vec<&str> = v["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["src.a", "src.t", "src.z"]);
    assert_eq!(v["definitions"][0]["name"], "src.a.a");
    assert_eq!(v["tests"][0]["key"], "src.t.z is two");
}

#[test]
fn a_directory_is_no_longer_concatenated_into_one_module() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.ply"), "pub fn a() -> Int = 1\n").unwrap();
    std::fs::write(dir.path().join("b.ply"), "fn b() -> Int = a()\n").unwrap();

    let out = ply(dir.path()).args(["check", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json_of(&out)["diagnostics"][0]["code"], "E0101");
}

#[test]
fn a_private_name_is_reported_against_its_declaration() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.ply"), "fn secret() -> Int = 1\n").unwrap();
    std::fs::write(
        dir.path().join("b.ply"),
        "import a\nfn b() -> Int = a::secret()\n",
    )
    .unwrap();

    let out = ply(dir.path()).args(["check", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json_of(&out)["diagnostics"][0]["code"], "E0107");
}

#[test]
fn a_module_cycle_is_rejected_with_exit_two() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.ply"),
        "import b\npub fn a() -> Int = b::b()\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b.ply"),
        "import a\npub fn b() -> Int = a::a()\n",
    )
    .unwrap();

    let out = ply(dir.path()).args(["check", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json_of(&out)["diagnostics"][0]["code"], "E0109");
}

#[test]
fn a_file_that_cannot_name_a_module_is_e0111() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("not-a-module.ply"), "fn f() -> Int = 1\n").unwrap();

    let out = ply(dir.path()).args(["check", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v = json_of(&out);
    assert_eq!(v["diagnostics"][0]["code"], "E0111");
    assert_eq!(v["diagnostics"][0]["labels"][0]["file"], "not-a-module.ply");
}

#[test]
fn two_modules_may_label_a_test_identically_and_the_output_says_which_is_which() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("alpha.ply"),
        "test \"shared\" { assert_eq(1, 1) }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("beta.ply"),
        "test \"shared\" { assert_eq(2, 3) }\n",
    )
    .unwrap();

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = stdout_of(&out);
    assert!(text.contains("alpha.shared"), "got:\n{text}");
    assert!(text.contains("beta.shared"), "got:\n{text}");
}

#[test]
fn filter_accepts_a_module_prefix() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("alpha.ply"),
        "test \"shared\" { assert_eq(1, 1) }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("beta.ply"),
        "test \"shared\" { assert_eq(2, 2) }\n",
    )
    .unwrap();

    let v = json_of(
        &ply(dir.path())
            .args(["test", "--json", "--filter", "beta."])
            .output()
            .unwrap(),
    );
    assert_eq!(v["selection"]["total"], 1);
    assert_eq!(v["selection"]["tests"][0]["key"], "beta.shared");
}

#[test]
fn the_cache_directory_is_never_parsed_as_source() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();
    assert!(dir.path().join(".ply-cache").is_dir());
    std::fs::write(dir.path().join(".ply-cache/decoy.ply"), "!!! not ply !!!\n").unwrap();
    ply(dir.path()).arg("test").assert().success();
}

#[test]
fn a_missing_path_is_a_diagnostic_with_exit_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = ply(dir.path())
        .args(["check", "nowhere.ply"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("nowhere.ply")
    );
}

// --- run --------------------------------------------------------------------

#[test]
fn run_evaluates_main() {
    let dir = project(GREEN);
    let out = ply(dir.path()).arg("run").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out).trim(), "42");
}

#[test]
fn run_json_reports_the_value() {
    let dir = project(GREEN);
    let v = json_of(&ply(dir.path()).args(["run", "--json"]).output().unwrap());
    assert_eq!(v["command"], "run");
    assert_eq!(v["ok"], true);
    assert_eq!(v["value"], "42");
}

#[test]
fn run_without_a_main_explains_what_to_add() {
    let dir = project("fn f() -> Int = 1\n");
    let out = ply(dir.path()).arg("run").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("no `main` to run"), "got:\n{stderr}");
    assert!(stderr.contains("fn main"));
}

#[test]
fn a_raising_main_exits_one() {
    let dir = project("fn main() -> Unit = panic(\"nope\")\n");
    let out = ply(dir.path()).arg("run").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8(out.stderr).unwrap().contains("nope"));
}

#[test]
fn run_finds_the_one_main_wherever_it_lives() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.ply"), "pub fn answer() -> Int = 42\n").unwrap();
    std::fs::write(
        dir.path().join("app.ply"),
        "import lib\nfn main() -> Int = lib::answer()\n",
    )
    .unwrap();

    let v = json_of(&ply(dir.path()).args(["run", "--json"]).output().unwrap());
    assert_eq!(v["ok"], true);
    assert_eq!(v["value"], "42");
    assert_eq!(v["entry"], "app.main");
    assert_eq!(v["module"], "app");
}

#[test]
fn two_mains_are_reported_rather_than_resolved_by_load_order() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("one.ply"), "fn main() -> Int = 1\n").unwrap();
    std::fs::write(dir.path().join("two.ply"), "fn main() -> Int = 2\n").unwrap();

    let out = ply(dir.path()).args(["run", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v = json_of(&out);
    assert_eq!(v["diagnostics"][0]["code"], "E0112");
    let notes = v["diagnostics"][0]["notes"].to_string();
    assert!(
        notes.contains("one.ply") && notes.contains("two.ply"),
        "got: {notes}"
    );

    // Naming the file picks the module, which is the fix the notes suggest.
    let v = json_of(
        &ply(dir.path())
            .args(["run", "two.ply", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["value"], "2");
    assert_eq!(v["entry"], "two.main");
}

// --- hosts ------------------------------------------------------------------

#[test]
fn hosts_defaults_to_hermetic_and_still_says_what_would_bind() {
    let dir = project(GREEN);
    let out = ply(dir.path()).arg("hosts").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("hermetic"), "got:\n{text}");
    assert!(
        text.contains("no host handler is bound"),
        "an empty listing is indistinguishable from a registry that failed to load:\n{text}"
    );
    // Never a bare "nothing here": the reader has to be able to tell an empty trusted computing
    // base from a binding that was simply not asked for.
    assert!(text.lines().filter(|l| !l.trim().is_empty()).count() >= 2);
}

#[test]
fn hosts_is_byte_identical_across_runs() {
    let dir = project(GREEN);
    let once = ply(dir.path()).args(["hosts", "--host"]).output().unwrap();
    let twice = ply(dir.path()).args(["hosts", "--host"]).output().unwrap();
    assert_eq!(once.status.code(), Some(0));
    assert_eq!(stdout_of(&once), stdout_of(&twice));
    assert!(stdout_of(&once).contains("trusted computing base"));
    assert!(stdout_of(&once).contains("digest: b3:"));
}

/// The one line a CI check pins.
#[test]
fn hosts_digest_is_one_line_and_does_not_depend_on_the_flag() {
    let dir = project(GREEN);
    let bare = ply(dir.path())
        .args(["hosts", "--digest"])
        .output()
        .unwrap();
    let bound = ply(dir.path())
        .args(["hosts", "--host", "--digest"])
        .output()
        .unwrap();
    assert_eq!(bare.status.code(), Some(0));
    let digest = stdout_of(&bare);
    assert_eq!(digest.lines().count(), 1, "got:\n{digest}");
    assert!(digest.starts_with("b3:"), "got: {digest}");
    assert_eq!(digest, stdout_of(&bound));

    let v = json_of(&ply(dir.path()).args(["hosts", "--json"]).output().unwrap());
    assert_eq!(v["digest"], digest.trim());
}

#[test]
fn hosts_json_is_one_object_carrying_the_binding_and_every_row() {
    let dir = project(GREEN);
    let v = json_of(&ply(dir.path()).args(["hosts", "--json"]).output().unwrap());
    assert_eq!(v["command"], "hosts");
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["ok"], true);
    assert_eq!(v["exit_code"], 0);
    assert_eq!(v["binding"], "hermetic");
    assert!(v["hosts"].is_array());
    assert_eq!(
        v["operations"].as_u64().unwrap() as usize,
        v["hosts"].as_array().unwrap().len(),
        "the count and the rows are one claim"
    );

    let bound = json_of(
        &ply(dir.path())
            .args(["hosts", "--host", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(bound["binding"], "host");
    // Only the binding moves: the trusted computing base is the same either way.
    assert_eq!(bound["hosts"], v["hosts"]);
    assert_eq!(bound["digest"], v["digest"]);
}

#[test]
fn hosts_on_a_broken_program_is_a_compile_error_with_a_clean_stdout() {
    let dir = project(BROKEN);
    let out = ply(dir.path()).args(["hosts", "--host"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stdout_of(&out), "");
    assert!(String::from_utf8(out.stderr).unwrap().contains("E0201"));
}

// --- `--host` on test and run -----------------------------------------------

#[test]
fn test_is_hermetic_without_the_flag_and_says_which_binding_it_used() {
    let dir = project(GREEN);
    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    assert_eq!(v["binding"], "hermetic");
    assert_eq!(v["hosts"]["operations"], 0);
    assert!(v["hosts"]["digest"].as_str().unwrap().starts_with("b3:"));
    // Nothing is bound, so no test can reach a host handler, and every test is classified exactly
    // as it was before W1.
    assert_eq!(v["selection"]["host"], 0);
    for test in v["selection"]["tests"].as_array().unwrap() {
        assert_eq!(test["host"], false);
        assert_ne!(test["isolation"], "host");
    }
}

#[test]
fn host_changes_the_binding_a_run_reports() {
    let dir = project(GREEN);
    let v = json_of(
        &ply(dir.path())
            .args(["test", "--host", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["ok"], true);
    assert_eq!(v["binding"], "host");
    assert_eq!(
        v["selection"]["isolated"].as_u64().unwrap()
            + v["selection"]["shared"].as_u64().unwrap()
            + v["selection"]["host"].as_u64().unwrap(),
        v["selection"]["total"].as_u64().unwrap(),
        "the three buckets have to partition the corpus"
    );
}

/// Visible without `--json`: a person reading the terminal has to be able to see that this run
/// reached outside itself.
#[test]
fn a_host_run_says_so_in_the_summary_a_person_reads() {
    let dir = project(GREEN);
    let bound = ply(dir.path()).args(["test", "--host"]).output().unwrap();
    let text = stdout_of(&bound);
    assert_eq!(bound.status.code(), Some(0));
    assert!(text.contains("not cached"), "got:\n{text}");
    assert!(text.contains("binding host"), "got:\n{text}");

    // And silent when nothing was asked for, so the ordinary run reads exactly as it did before W1.
    let hermetic = stdout_of(&ply(dir.path()).args(["test"]).output().unwrap());
    assert!(!hermetic.contains("binding host"), "got:\n{hermetic}");
}

/// The trivially-parallel count is a claim, and a claim that grew when a socket was bound would be
/// the over-claim ADR 0008 §6 exists to prevent.
#[test]
fn explain_never_reports_more_isolation_under_host_than_without_it() {
    let dir = project(GREEN);
    let counts = |extra: &[&str]| {
        let mut args = vec!["test", "--json", "--explain", "--no-cache"];
        args.extend_from_slice(extra);
        let v = json_of(&ply(dir.path()).args(args).output().unwrap());
        (
            v["selection"]["isolated"].as_u64().unwrap(),
            v["selection"]["host"].as_u64().unwrap(),
        )
    };
    let (hermetic, _) = counts(&[]);
    let (bound, host) = counts(&["--host"]);
    assert!(
        bound + host <= hermetic + host && bound <= hermetic,
        "isolated grew under --host: {hermetic} -> {bound}"
    );
}

#[test]
fn run_is_hermetic_by_default_and_reports_its_binding() {
    let dir = project(GREEN);
    let v = json_of(&ply(dir.path()).args(["run", "--json"]).output().unwrap());
    assert_eq!(v["binding"], "hermetic");
    assert_eq!(v["value"], "42");

    let v = json_of(
        &ply(dir.path())
            .args(["run", "--host", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["binding"], "host");
    assert_eq!(
        v["value"], "42",
        "a binding may not change what a program computes"
    );
}

/// Corollary 1 of ADR 0011, checked end to end: if a binding moved a hash, a row or an E0412
/// verdict, `ply check` would answer differently under `--host` and every cache in the system would
/// split on a flag.
#[test]
fn a_binding_changes_nothing_the_front_end_computes() {
    let dir = project(GREEN);
    let hashes = |args: &[&str]| stdout_of(&ply(dir.path()).args(args).output().unwrap());
    assert_eq!(hashes(&["hash", "--json"]), hashes(&["hash", "--json"]));

    let hermetic = json_of(
        &ply(dir.path())
            .args(["test", "--json", "--no-cache"])
            .output()
            .unwrap(),
    );
    let bound = json_of(
        &ply(dir.path())
            .args(["test", "--json", "--no-cache", "--host"])
            .output()
            .unwrap(),
    );
    let hashes_of = |v: &Value| {
        v["selection"]["tests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["hash"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(hashes_of(&hermetic), hashes_of(&bound));
    assert_eq!(hermetic["selection"]["total"], bound["selection"]["total"]);
}

// --- hash -------------------------------------------------------------------

#[test]
fn hash_prints_a_short_hash_per_definition() {
    let dir = project(GREEN);
    let out = ply(dir.path()).arg("hash").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("double"));
    assert!(
        text.contains("2 definitions · 2 tests · 1 module"),
        "got:\n{text}"
    );
}

#[test]
fn hash_deps_shows_the_graph() {
    let dir = project(GREEN);
    let text = stdout_of(&ply(dir.path()).args(["hash", "--deps"]).output().unwrap());
    assert!(text.contains("deps: m.double"), "got:\n{text}");
    assert!(text.contains("closure:"));
}

#[test]
fn hash_json_gives_full_hex_and_the_closure() {
    let dir = project(GREEN);
    let v = json_of(
        &ply(dir.path())
            .args(["hash", "--json", "--deps"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["command"], "hash");
    assert_eq!(v["definitions"][0]["hash"].as_str().unwrap().len(), 64);
    assert!(v["definitions"][0]["closure"].is_array());
    assert_eq!(v["tests"][0]["hash"].as_str().unwrap().len(), 64);
}

#[test]
fn hash_groups_by_module_and_says_the_module_is_not_hashed() {
    let dir = multi_module();
    let text = stdout_of(&ply(dir.path()).arg("hash").output().unwrap());
    assert!(text.contains("src.a"), "got:\n{text}");
    assert!(text.contains("src.z"), "got:\n{text}");
    assert!(
        text.contains("2 definitions · 1 test · 3 modules"),
        "got:\n{text}"
    );
    assert!(text.contains("changes no hash"), "got:\n{text}");

    let v = json_of(&ply(dir.path()).args(["hash", "--json"]).output().unwrap());
    assert_eq!(v["module_is_hashed"], false);
    assert_eq!(v["definitions"][0]["module"], "src.a");
    assert_eq!(v["definitions"][0]["name"], "src.a.a");
    assert_eq!(v["definitions"][0]["simple_name"], "a");
}

#[test]
fn moving_a_definition_between_modules_re_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("app.ply"),
        "fn one() -> Int = 1\n\
         fn two() -> Int = one() + one()\n\
         test \"two is two\" { assert_eq(two(), 2) }\n",
    )
    .unwrap();
    ply(dir.path()).arg("test").assert().success();

    std::fs::write(dir.path().join("lib.ply"), "pub fn one() -> Int = 1\n").unwrap();
    std::fs::write(
        dir.path().join("app.ply"),
        "import lib\n\
         fn two() -> Int = lib::one() + lib::one()\n\
         test \"two is two\" { assert_eq(two(), 2) }\n",
    )
    .unwrap();

    let text = stdout_of(&ply(dir.path()).arg("test").output().unwrap());
    assert!(
        text.contains("selected 0 of 1 (1 cached)"),
        "moving a definition rebuilt something:\n{text}"
    );
}

/// A module is loaded in path order but checked in dependency order, and test indices are shared
/// between the two.
#[test]
fn an_edit_re_runs_the_test_that_reaches_it_when_load_order_is_not_dependency_order() {
    let dir = tempfile::tempdir().unwrap();
    let importer = |body: &str| {
        format!("import b\nfn one() -> Int = {body}\ntest \"a one\" {{ assert_eq(one(), 1) }}\n")
    };
    std::fs::write(dir.path().join("a.ply"), importer("b::zero() + 1")).unwrap();
    std::fs::write(
        dir.path().join("b.ply"),
        "pub fn zero() -> Int = 0\ntest \"b zero\" { assert_eq(zero(), 0) }\n",
    )
    .unwrap();

    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    assert_eq!(v["summary"]["passed"], 2, "{v}");

    std::fs::write(dir.path().join("a.ply"), importer("1 + b::zero()")).unwrap();
    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    assert_eq!(v["selection"]["selected"], 1);
    let ran: Vec<&str> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["key"].as_str().unwrap())
        .collect();
    assert_eq!(ran, ["a.a one"], "the wrong test was re-run");
}

// --- cache ------------------------------------------------------------------

#[test]
fn cache_stats_counts_what_a_run_recorded() {
    let dir = project(GREEN);
    let text = stdout_of(&ply(dir.path()).args(["cache", "stats"]).output().unwrap());
    assert!(text.contains("0 cached results"), "got:\n{text}");

    ply(dir.path()).arg("test").assert().success();
    let v = json_of(
        &ply(dir.path())
            .args(["cache", "stats", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["command"], "cache");
    assert_eq!(v["action"], "stats");
    // Two tests plus the definition each vouched for.
    assert!(v["entries"].as_u64().unwrap() >= 2);
    assert_eq!(v["runtime_version"], ply_store::RUNTIME_VERSION);
}

#[test]
fn cache_clear_makes_the_next_run_re_prove_everything() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();
    assert!(stdout_of(&ply(dir.path()).arg("test").output().unwrap()).contains("selected 0 of 2"));

    let out = ply(dir.path()).args(["cache", "clear"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout_of(&out).contains("cleared"));

    let text = stdout_of(&ply(dir.path()).arg("test").output().unwrap());
    assert!(text.contains("selected 2 of 2 (0 cached)"), "got:\n{text}");
}

#[test]
fn a_corrupt_cache_degrades_to_an_empty_one_rather_than_crashing() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();
    std::fs::write(dir.path().join(".ply-cache/results.json"), "{ truncated").unwrap();

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("selected 2 of 2 (0 cached)"), "got:\n{text}");
    assert!(
        text.contains("warning"),
        "the degradation must be reported:\n{text}"
    );
}

/// Attribution re-normalizes against the loaded AST, and gate 1 leaves no AST for a file it
/// skipped.
#[test]
fn a_skipped_earlier_module_does_not_lose_the_attribution() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.ply"),
        "fn one() -> Int = 1\n\ntest \"a is fine\" { assert_eq(one(), 1) }\n",
    )
    .unwrap();
    let b = dir.path().join("b.ply");
    std::fs::write(
        &b,
        "fn two() -> Int = 2\n\ntest \"b holds\" { assert_eq(two(), 2) }\n",
    )
    .unwrap();
    ply(dir.path()).arg("test").assert().success();

    std::fs::write(
        &b,
        "fn two() -> Int = 2\n\ntest \"b holds\" { assert_eq(two(), 3) }\n",
    )
    .unwrap();
    let explain = stdout_of(
        &ply(dir.path())
            .args(["test", "--explain"])
            .output()
            .unwrap(),
    );
    assert!(
        explain.contains("skipped") && explain.contains("a.ply"),
        "the earlier module has to be the skipped one, or this proves nothing:\n{explain}"
    );

    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    let culprit = &v["failures"][0]["culprit"];
    assert_eq!(
        culprit["verdict"], "test_changed",
        "only the test body moved: {culprit}"
    );
    assert_eq!(culprit["definitions"][0], "b.b holds");
}

/// The baseline is read during the *diagnosis* of a failure, which happens after every other point
/// the run collects warnings from the store.
#[test]
fn a_corrupt_baseline_is_reported_rather_than_read_as_never_passed() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();
    std::fs::write(dir.path().join(".ply-cache/passes.json"), "{ truncated").unwrap();
    std::fs::write(dir.path().join("m.ply"), GREEN.replace("x * 2", "x * 3")).unwrap();

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "the edit has to fail a test");
    let text = stdout_of(&out);
    assert!(
        text.contains("pass records") && text.contains("corrupt"),
        "the unreadable baseline must be named:\n{text}"
    );

    std::fs::write(dir.path().join(".ply-cache/passes.json"), "{ truncated").unwrap();
    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    let codes: Vec<&str> = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].as_str().unwrap())
        .collect();
    assert!(
        codes.contains(&"W0602"),
        "the machine artifact carries the same warning: {codes:?}"
    );
}

#[test]
fn cache_stats_reports_open_time_and_the_front_end_files() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();

    let text = stdout_of(&ply(dir.path()).args(["cache", "stats"]).output().unwrap());
    assert!(
        text.contains("opened in"),
        "the open cost is the claim:\n{text}"
    );
    assert!(
        text.contains("index"),
        "the front-end files must be sized:\n{text}"
    );
    assert!(text.contains("reclaimable"), "got:\n{text}");

    let v = json_of(
        &ply(dir.path())
            .args(["cache", "stats", "--json"])
            .output()
            .unwrap(),
    );
    assert!(v["open_ms"].is_number());
    assert!(v["frontend"]["index_bytes"].as_u64().unwrap() > 0);
    assert!(v["frontend"]["data_bytes"].is_number());
    assert!(v["frontend"]["sources"].as_u64().unwrap() >= 1);
    assert!(v["frontend"]["bodies"].is_number());
    assert_eq!(v["frontend"]["compact_suggested"], false);
    assert!(v["results_bytes"].as_u64().unwrap() > 0);
}

#[test]
fn cache_compact_drops_a_deleted_file_and_keeps_the_rest() {
    let dir = project(GREEN);
    std::fs::write(dir.path().join("gone.ply"), "fn gone() -> Int = 9\n").unwrap();
    ply(dir.path()).arg("test").assert().success();
    assert_eq!(
        json_of(
            &ply(dir.path())
                .args(["cache", "stats", "--json"])
                .output()
                .unwrap()
        )["frontend"]["sources"],
        2
    );

    std::fs::remove_file(dir.path().join("gone.ply")).unwrap();
    let out = ply(dir.path()).args(["cache", "compact"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("compacted"), "got:\n{text}");
    assert!(text.contains("dropped 1 file"), "got:\n{text}");

    let v = json_of(
        &ply(dir.path())
            .args(["cache", "stats", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["frontend"]["sources"], 1);
    // Compaction is a front-end garbage collection.
    assert!(v["entries"].as_u64().unwrap() >= 2);

    let text = stdout_of(&ply(dir.path()).arg("test").output().unwrap());
    assert!(
        text.contains("selected 0 of 2 (2 cached)"),
        "compaction must not cost a test:\n{text}"
    );
}

#[test]
fn cache_compact_json_is_one_object() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();
    let v = json_of(
        &ply(dir.path())
            .args(["cache", "compact", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["command"], "cache");
    assert_eq!(v["action"], "compact");
    assert_eq!(v["ok"], true);
    assert_eq!(v["exit_code"], 0);
    assert_eq!(v["files_kept"], 1);
    assert!(v["dropped"]["definitions"].is_number());
    assert!(v["bytes_before"].is_number());
    assert!(v["reclaimed_bytes"].is_number());
}

#[test]
fn cache_inspect_prints_a_resolved_type_rather_than_a_serialization() {
    let dir = project(
        "effect db {\n  read all[t]() -> List<Int>\n}\n\
         fn active(n: Int) -> List<Int> / {db.read[users]} = db.all[users]()\n",
    );
    ply(dir.path()).arg("check").assert().success();

    let out = ply(dir.path())
        .args(["cache", "inspect", "active"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("m.active"), "got:\n{text}");
    assert!(
        text.contains("(Int) -> List<Int> / {m.db.read[users]}"),
        "the type must read as a signature, not as JSON:\n{text}"
    );
    assert!(
        text.contains("footprint  {m.db.read[users]}"),
        "got:\n{text}"
    );
    assert!(
        text.contains("m.ply:4:"),
        "the declaring position must be shown:\n{text}"
    );
    assert!(text.contains("witness"), "got:\n{text}");
    assert!(text.contains("body"), "got:\n{text}");
    assert!(text.contains("result"), "got:\n{text}");
}

#[test]
fn cache_inspect_accepts_a_hash_prefix_and_emits_json() {
    let dir = project(GREEN);
    ply(dir.path()).arg("check").assert().success();

    let v = json_of(
        &ply(dir.path())
            .args(["cache", "inspect", "double", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["command"], "cache");
    assert_eq!(v["action"], "inspect");
    assert_eq!(v["matches"].as_array().unwrap().len(), 1);
    assert_eq!(v["matches"][0]["name"], "m.double");
    assert_eq!(v["matches"][0]["kind"], "fn");
    assert_eq!(v["matches"][0]["file"], "m.ply");
    assert_eq!(v["matches"][0]["stale"], false);
    assert_eq!(v["matches"][0]["interface"]["type"], "(Int) -> Int");

    let hash = v["matches"][0]["hash"].as_str().unwrap().to_string();
    let by_prefix = json_of(
        &ply(dir.path())
            .args(["cache", "inspect", &hash[..6], "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(by_prefix["matches"][0]["name"], "m.double");
}

#[test]
fn cache_inspect_of_an_unknown_name_is_e0101_and_exits_two() {
    let dir = project(GREEN);
    ply(dir.path()).arg("check").assert().success();

    let out = ply(dir.path())
        .args(["cache", "inspect", "no_such_thing"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8(out.stderr).unwrap().contains("E0101"));

    let out = ply(dir.path())
        .args(["cache", "inspect", "no_such_thing", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v = json_of(&out);
    assert_eq!(v["ok"], false);
    assert_eq!(v["diagnostics"][0]["code"], "E0101");
}

#[test]
fn cache_inspect_reports_a_test_and_whether_it_is_proven() {
    let dir = project(GREEN);
    ply(dir.path()).arg("check").assert().success();
    let v = json_of(
        &ply(dir.path())
            .args(["cache", "inspect", "double doubles", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["matches"][0]["kind"], "test");
    assert!(
        v["matches"][0]["result"]
            .as_str()
            .unwrap()
            .contains("not proven")
    );

    ply(dir.path()).arg("test").assert().success();
    let v = json_of(
        &ply(dir.path())
            .args(["cache", "inspect", "double doubles", "--json"])
            .output()
            .unwrap(),
    );
    assert!(
        v["matches"][0]["result"]
            .as_str()
            .unwrap()
            .contains("passed")
    );
    assert_eq!(v["matches"][0]["interface"]["nondet"], false);
}

/// The whole point of telling the user: a front-end cache this build cannot read costs a recompile
/// and *no* test re-runs, and only saying the first half reads as though the results went too.
#[test]
fn an_unreadable_front_end_cache_says_the_results_survived_it() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();

    let index = dir.path().join(".ply-cache/frontend.idx");
    assert!(index.is_file(), "the front-end index should exist by now");
    std::fs::write(&index, b"PLYFEIDX not really an index").unwrap();

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(
        text.contains("selected 0 of 2 (2 cached)"),
        "the result cache is versioned apart and must survive:\n{text}"
    );
    assert!(
        text.contains("no test re-runs"),
        "the user has to be told what survived:\n{text}"
    );
    assert!(
        text.contains("recomputes types and hashes"),
        "the user has to be told what it cost:\n{text}"
    );
}

/// The migration proper: a project whose cache directory still holds the JSON front-end cache and
/// none of the binary one.
#[test]
fn a_leftover_json_front_end_cache_is_explained_and_then_removed() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();

    let cache = dir.path().join(".ply-cache");
    std::fs::remove_file(cache.join("frontend.idx")).unwrap();
    std::fs::remove_file(cache.join("frontend.dat")).unwrap();
    std::fs::write(cache.join("frontend.json"), "{\"format\":2,\"defs\":{}}").unwrap();

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("format changed"), "got:\n{text}");
    assert!(text.contains("no test re-runs"), "got:\n{text}");
    assert!(
        text.contains("selected 0 of 2 (2 cached)"),
        "the result cache must survive the migration:\n{text}"
    );

    assert!(
        !cache.join("frontend.json").exists(),
        "the unreadable file must not be left behind forever"
    );
    let text = stdout_of(&ply(dir.path()).arg("test").output().unwrap());
    assert!(
        !text.contains("format changed"),
        "the migration is reported once, not every run:\n{text}"
    );
}

#[test]
fn cache_stats_reports_a_discarded_front_end_cache_too() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();
    std::fs::write(
        dir.path().join(".ply-cache/frontend.idx"),
        b"PLYFEIDX not really an index",
    )
    .unwrap();

    let v = json_of(
        &ply(dir.path())
            .args(["cache", "stats", "--json"])
            .output()
            .unwrap(),
    );
    let codes: Vec<&str> = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"W0603"), "got {codes:?}");
}

// --- the failure artifact ---------------------------------------------------

#[test]
fn test_json_carries_schema_version_four_and_a_ranked_suspect_object() {
    let dir = project(RED);
    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = json_of(&out);

    assert_eq!(v["schema_version"], 4);
    let f = &v["failures"][0];
    assert_eq!(f["key"], "m.balance never goes negative");
    assert_eq!(f["name"], "balance never goes negative");
    assert_eq!(f["module"], "m");
    assert_eq!(f["status"], "failed");
    assert_eq!(f["nondet"], false);
    assert_eq!(f["test_hash"].as_str().unwrap().len(), 64);
    assert_eq!(f["location"]["file"], "m.ply");
    assert_eq!(f["location"]["line"], 5);
    assert!(f["location"]["end_column"].is_number());
    assert_eq!(f["diagnostic"]["code"], "E0501");

    assert!(f["culprit"]["verdict"].is_string());
    assert!(f["culprit"]["definitions"].is_array());
    assert_eq!(f["culprit"]["search"]["evaluated"], 0);

    let suspects = f["suspects"].as_array().unwrap();
    assert_eq!(suspects[0]["name"], "m.balance");
    assert_eq!(suspects[0]["culprit"], false);
    assert!(suspects[0]["hash"].as_str().unwrap().len() == 64);
    assert!(
        suspects[0]["change"].is_null(),
        "nothing compared the two eras"
    );

    assert_eq!(f["footprint"]["declared"], serde_json::json!([]));
    assert!(f["footprint"]["observed"].is_null());
    assert!(f["assertion"].is_null());
    assert!(f["causal_slice"].is_null());
}

/// A first-ever red test and a regression are different situations, and this is where the
/// difference shows: a regression leads with a name, a test that has never passed leads with why
/// there is no name to lead with.
#[test]
fn a_test_that_has_never_passed_says_why_it_has_no_culprit() {
    let dir = project(RED);
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = stdout_of(&out);
    assert!(text.contains("balance never goes negative"));
    assert!(text.contains("assertion failed"), "got:\n{text}");
    assert!(
        text.contains("no culprit: this test has never passed"),
        "got:\n{text}"
    );
    assert!(
        !text.contains("culprit: m."),
        "nothing was bisected, so no definition may be named:\n{text}"
    );
    assert!(text.contains("suspects: m.balance"), "got:\n{text}");
}

#[test]
fn bisect_never_reports_not_requested_and_is_still_one_json_object() {
    let dir = project(RED);
    let out = ply(dir.path())
        .args(["test", "--json", "--bisect", "never"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = json_of(&out);
    assert_eq!(v["options"]["bisect"], "never");
    assert_eq!(v["options"]["bisect_budget"], 64);
    assert_eq!(v["options"]["trace"], "auto");
    assert_eq!(v["failures"][0]["culprit"]["verdict"], "not_attempted");
    assert_eq!(v["failures"][0]["culprit"]["skipped"], "not_requested");
    assert_eq!(v["failures"][0]["culprit"]["confidence"], "none");
    assert_eq!(v["failures"][0]["culprit"]["search"]["evaluated"], 0);
}

/// Two runs over one failure must produce the same bytes, or an agent cannot diff today's artifact
/// against yesterday's.
#[test]
fn two_runs_over_one_failure_emit_the_same_artifact() {
    let dir = project(RED);
    let once = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    let twice = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    assert_eq!(
        serde_json::to_string(&once["failures"]).unwrap(),
        serde_json::to_string(&twice["failures"]).unwrap()
    );
}

// --- shape ------------------------------------------------------------------

#[test]
fn every_subcommand_answers_help_and_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    for sub in ["check", "test", "run", "hash", "cache"] {
        let out = ply(dir.path()).args([sub, "--help"]).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "`{sub} --help` failed");
        assert!(!stdout_of(&out).is_empty());
    }
}

#[test]
fn an_unknown_subcommand_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let out = ply(dir.path()).arg("frobnicate").output().unwrap();
    assert_ne!(out.status.code(), Some(0));
}

#[test]
fn check_explain_names_the_reason_a_file_was_parsed() {
    let dir = project("fn f() -> Int = 1\n");
    std::fs::write(dir.path().join("g.ply"), "fn g() -> Int = 2\n").unwrap();
    ply(dir.path()).arg("check").assert().success();

    let out = ply(dir.path())
        .args(["check", "--explain"])
        .output()
        .unwrap();
    let text = stdout_of(&out);
    assert!(
        text.contains("skipped"),
        "a warm run must skip something:\n{text}"
    );
    assert!(
        text.contains("unchanged"),
        "a skip must say why it was allowed:\n{text}"
    );

    std::fs::write(dir.path().join("m.ply"), "fn f() -> Int = 3\n").unwrap();
    let out = ply(dir.path())
        .args(["check", "--explain"])
        .output()
        .unwrap();
    let text = stdout_of(&out);
    assert!(
        text.contains("content changed"),
        "a refusal must say why:\n{text}"
    );
}

/// `--no-incremental` has to be observable, or nobody can use it to decide whether a wrong answer
/// came from the cache.
#[test]
fn no_incremental_parses_everything_and_leaves_the_cache_alone() {
    let dir = project("fn f() -> Int = 1\n");
    ply(dir.path()).arg("check").assert().success();

    let out = ply(dir.path())
        .args(["check", "--explain", "--no-incremental"])
        .output()
        .unwrap();
    let text = stdout_of(&out);
    assert!(
        !text.contains("skipped"),
        "--no-incremental must parse every file:\n{text}"
    );
    assert!(
        text.contains("--no-incremental"),
        "the reason must name the flag:\n{text}"
    );

    let out = ply(dir.path())
        .args(["check", "--explain"])
        .output()
        .unwrap();
    assert!(
        stdout_of(&out).contains("skipped"),
        "a --no-incremental run must not have discarded the cache"
    );
}

#[test]
fn test_no_incremental_still_selects_the_same_tests() {
    let dir = project(
        "fn f() -> Int = 1\ntest \"f is one\" { assert_eq(f(), 1) }\n\
         test \"f is not two\" { assert(f() != 2) }\n",
    );
    ply(dir.path()).arg("test").assert().success();

    let out = ply(dir.path())
        .args(["test", "--no-incremental"])
        .output()
        .unwrap();
    let text = stdout_of(&out);
    assert!(
        text.contains("2 cached"),
        "the result cache survives --no-incremental:\n{text}"
    );
}

/// A warm run has to be observably cheaper, not just observably skipping, so the phase breakdown is
/// part of the reported interface.
#[test]
fn the_front_end_reports_where_its_time_went() {
    let dir = project("fn f() -> Int = 1\ntest \"f is one\" { assert_eq(f(), 1) }\n");
    ply(dir.path()).arg("test").assert().success();

    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let phases = json_of(&out)["front_end"]["phases"].clone();
    for name in [
        "read",
        "parse",
        "resolve",
        "hash",
        "check",
        "restore",
        "write_back",
        "total",
    ] {
        assert!(
            phases[name].is_number(),
            "`{name}` is missing from {phases}"
        );
    }
    let total = phases["total"].as_f64().unwrap();
    let sum: f64 = [
        "read",
        "parse",
        "resolve",
        "hash",
        "check",
        "restore",
        "write_back",
    ]
    .iter()
    .map(|n| phases[n].as_f64().unwrap())
    .sum();
    assert!(
        (total - sum).abs() < 0.5,
        "the total must account for the parts: {phases}"
    );

    let out = ply(dir.path())
        .args(["check", "--explain"])
        .output()
        .unwrap();
    assert!(
        stdout_of(&out).contains("front-end time"),
        "--explain must show the breakdown"
    );
}

/// A selected test needs a body, so its module and everything it imports have to be parsed.
#[test]
fn one_selected_test_reparses_its_module_and_not_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
    write("leaf.ply", "pub fn one() -> Int = 1\n");
    write(
        "used.ply",
        "import leaf\npub fn two() -> Int = leaf::one() + 1\n\
         test \"two is two\" { assert_eq(two(), 2) }\n",
    );
    for i in 0..4 {
        write(
            &format!("far{i}.ply"),
            &format!("fn f{i}() -> Int = {i}\ntest \"far {i}\" {{ assert_eq(f{i}(), {i}) }}\n"),
        );
    }
    ply(dir.path()).arg("test").assert().success();

    write(
        "used.ply",
        "import leaf\npub fn two() -> Int = leaf::one() + 1 + 0\n\
         test \"two is two\" { assert_eq(two(), 2) }\n",
    );
    let out = ply(dir.path())
        .args(["test", "--explain"])
        .output()
        .unwrap();
    let text = stdout_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("selected 1 of 5"),
        "one test must be selected:\n{text}"
    );
    for i in 0..4 {
        let line = text
            .lines()
            .find(|l| l.contains(&format!("far{i}.ply")))
            .unwrap_or_else(|| panic!("far{i}.ply is missing from the report:\n{text}"));
        assert!(line.trim_start().starts_with("skipped"), "{line}");
    }
}

#[test]
fn test_explain_reports_the_front_end_before_the_selection() {
    let dir = project("fn f() -> Int = 1\ntest \"f is one\" { assert_eq(f(), 1) }\n");
    ply(dir.path()).arg("test").assert().success();
    let out = ply(dir.path())
        .args(["test", "--explain"])
        .output()
        .unwrap();
    let text = stdout_of(&out);
    let front = text
        .find("m.ply")
        .expect("the front-end block names each file");
    let selection = text
        .find("f is one")
        .expect("the selection block explains each test");
    assert!(
        front < selection,
        "the front-end block comes first:\n{text}"
    );
}

const LEDGER: &str = "\
fn normal_sign(n: Int) -> Int = if n < 0 { 0 - 1 } else { 1 }

fn balance(a: Int, b: Int, c: Int) -> Int = (a + b) + c

fn presented(a: Int, b: Int, c: Int) -> Int = balance(a, b, c) * normal_sign(a)

test \"balance never goes negative\" { assert_eq(presented(1, 2, 3), 6) }
";

#[test]
fn two_candidate_edits_are_narrowed_to_the_culprit_by_running_the_mixture() {
    let dir = project(LEDGER);
    ply(dir.path()).arg("test").assert().success();

    std::fs::write(
        dir.path().join("m.ply"),
        LEDGER
            .replace(
                "if n < 0 { 0 - 1 } else { 1 }",
                "if n < 0 { 0 - 1 } else { 0 - 1 }",
            )
            .replace("(a + b) + c", "a + (b + c)"),
    )
    .unwrap();

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = stdout_of(&out);
    assert!(
        text.contains("culprit: m.normal_sign"),
        "the terminal block must lead with the culprit:\n{text}"
    );

    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    let failure = &v["failures"][0];
    let culprit = &failure["culprit"];
    assert_eq!(culprit["verdict"], "bisected");
    assert_eq!(culprit["confidence"], "minimal");
    assert_eq!(culprit["definitions"], serde_json::json!(["m.normal_sign"]));
    assert_eq!(culprit["skipped"], serde_json::Value::Null);
    assert!(
        culprit["search"]["evaluated"].as_u64().unwrap() > 0,
        "a mixture was actually run: {culprit}"
    );
    // A consumer that reads only `suspects[0]` has to get the best guess.
    assert_eq!(failure["suspects"][0]["name"], "m.normal_sign");
    assert_eq!(failure["suspects"][0]["culprit"], true);
    assert_eq!(failure["suspects"][1]["name"], "m.balance");
    assert_eq!(failure["suspects"][1]["culprit"], false);
}

/// The same narrowing, in a project where the incremental front end skips a file.
#[test]
fn a_skipped_file_does_not_cost_the_failure_its_culprit() {
    let dir = project(LEDGER);
    // Sorts before `m.ply`, so its tests take the indices the fresh body set would otherwise line
    // `m`'s up against.
    std::fs::write(
        dir.path().join("a.ply"),
        "pub fn untouched(x: Int) -> Int = x\n\
         test \"untouched is the identity\" { assert_eq(untouched(1), 1) }\n\
         test \"untouched keeps zero\" { assert_eq(untouched(0), 0) }\n",
    )
    .unwrap();
    ply(dir.path()).arg("test").assert().success();

    std::fs::write(
        dir.path().join("m.ply"),
        LEDGER
            .replace(
                "if n < 0 { 0 - 1 } else { 1 }",
                "if n < 0 { 0 - 1 } else { 0 - 1 }",
            )
            .replace("(a + b) + c", "a + (b + c)"),
    )
    .unwrap();

    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    assert_eq!(
        v["front_end"]["skipped"], 1,
        "the point of the fixture is that a file was skipped: {}",
        v["front_end"]
    );
    let culprit = &v["failures"][0]["culprit"];
    assert_eq!(culprit["verdict"], "bisected", "{culprit}");
    assert_eq!(culprit["definitions"], serde_json::json!(["m.normal_sign"]));
    assert!(culprit["search"]["evaluated"].as_u64().unwrap() > 0);
}

/// `--bisect never` must still cost nothing, now that there is an engine it could have driven.
#[test]
fn bisect_never_runs_no_mixture_even_when_one_could_be_built() {
    let dir = project(LEDGER);
    ply(dir.path()).arg("test").assert().success();
    std::fs::write(
        dir.path().join("m.ply"),
        LEDGER.replace(
            "if n < 0 { 0 - 1 } else { 1 }",
            "if n < 0 { 0 - 1 } else { 0 - 1 }",
        ),
    )
    .unwrap();

    let v = json_of(
        &ply(dir.path())
            .args(["test", "--json", "--bisect", "never"])
            .output()
            .unwrap(),
    );
    let culprit = &v["failures"][0]["culprit"];
    assert_eq!(culprit["verdict"], "not_attempted");
    assert_eq!(culprit["skipped"], "not_requested");
    assert_eq!(culprit["search"]["evaluated"], 0);
}

/// Bisection proves things about *mixtures*, never about the program the user wrote.
#[test]
fn a_bisected_failure_leaves_the_next_runs_suspects_unchanged() {
    let dir = project(LEDGER);
    ply(dir.path()).arg("test").assert().success();
    std::fs::write(
        dir.path().join("m.ply"),
        LEDGER
            .replace(
                "if n < 0 { 0 - 1 } else { 1 }",
                "if n < 0 { 0 - 1 } else { 0 - 1 }",
            )
            .replace("(a + b) + c", "a + (b + c)"),
    )
    .unwrap();

    let names = |v: &Value| -> Vec<String> {
        v["failures"][0]["suspects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    };
    let first = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    let second = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    assert!(!names(&first).is_empty());
    assert_eq!(names(&first), names(&second));
    assert_eq!(
        first["failures"][0]["culprit"]["definitions"],
        second["failures"][0]["culprit"]["definitions"]
    );
}

const PARITY: &str = "\
fn even(n: Int) -> Bool = if n == 0 { true } else { odd(n - 1) }

fn odd(n: Int) -> Bool = if n == 0 { false } else { even(n - 1) }

test \"parity\" { assert(even(4)) }
";

/// Members of one strongly connected component share a component hash and one stored body, so no
/// hybrid can flip one without the other.
#[test]
fn a_recursive_pair_is_reported_as_one_fused_group_that_says_why() {
    let dir = project(PARITY);
    ply(dir.path()).arg("test").assert().success();
    std::fs::write(
        dir.path().join("m.ply"),
        PARITY.replace(
            "fn even(n: Int) -> Bool = if n == 0 { true }",
            "fn even(n: Int) -> Bool = if n == 0 { false }",
        ),
    )
    .unwrap();

    let v = json_of(&ply(dir.path()).args(["test", "--json"]).output().unwrap());
    let culprit = &v["failures"][0]["culprit"];
    assert_eq!(culprit["confidence"], "fused");
    assert_eq!(culprit["groups"], serde_json::json!([["m.even", "m.odd"]]));
    assert!(
        culprit["reason"]
            .as_str()
            .unwrap()
            .contains("mutually recursive"),
        "the artifact has to say why they are inseparable: {culprit}"
    );
}

// --- multi-shot resumption --------------------------------------------------

/// A clause that binds a continuation.
const MULTI_SHOT: &str = "\
effect amb {
  read flip[coin]() -> Bool
}

test \"both branches\" {
  with_cell[trace](0) { c -> {
    let total = handle {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { 10 } else { 20 }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    };
    assert_eq(total, 30);
    assert_eq(cell_get(c), 2)
  } }
}
";

#[test]
fn a_multi_shot_program_runs_and_caches_with_no_flags_at_all() {
    let dir = project(MULTI_SHOT);
    let cold = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(cold.status.code(), Some(0), "{}", stdout_of(&cold));
    assert!(
        stdout_of(&cold).contains("1 passed"),
        "{}",
        stdout_of(&cold)
    );

    let warm = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(warm.status.code(), Some(0), "{}", stdout_of(&warm));
    assert!(
        stdout_of(&warm).contains("1 cached"),
        "{}",
        stdout_of(&warm)
    );
}

/// Gate 1 skips a file whose bytes did not change, and a second `ply check` over
/// untouched source must not start parsing everything again.
#[test]
fn an_unchanged_file_is_skipped_on_the_second_check() {
    let dir = project(GREEN);
    ply(dir.path()).arg("check").assert().success();
    let out = ply(dir.path())
        .args(["check", "--explain"])
        .output()
        .unwrap();
    let text = stdout_of(&out);
    assert!(text.contains("skipped   m.ply"), "{text}");
}

/// One unreadable file is found by more than one read — a lazy consult and a flush that re-reads to
/// merge — and each reports it.
#[test]
fn an_unreadable_cache_file_is_reported_once_not_once_per_read() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();
    for entry in std::fs::read_dir(dir.path().join(".ply-cache")).unwrap() {
        std::fs::write(entry.unwrap().path(), "garbage").unwrap();
    }

    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let v = json_of(&out);
    let messages: Vec<&str> = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["message"].as_str().unwrap())
        .collect();
    let passes: Vec<&&str> = messages
        .iter()
        .filter(|m| m.contains("passes.json"))
        .collect();
    assert_eq!(passes.len(), 1, "{messages:#?}");

    let mut unique = messages.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), messages.len(), "{messages:#?}");
    assert_eq!(out.status.code(), Some(0));
}

// --- the search plan --------------------------------------------------------

#[test]
fn the_search_plan_is_published_so_two_runs_can_be_compared() {
    let dir = project(GREEN);
    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let sim = json_of(&out)["options"]["sim"].clone();
    assert_eq!(sim["mode"], "dpor");
    assert_eq!(sim["seeds"], 1);
    assert!(sim["seed"].is_null());
    assert_eq!(sim["measure_reduction"], false);
    assert!(sim["budget"].is_number());
    assert!(sim["steps"].is_number());

    let out = ply(dir.path())
        .args(["test", "--json", "--sim", "random", "--seeds", "8"])
        .output()
        .unwrap();
    let sim = json_of(&out)["options"]["sim"].clone();
    assert_eq!(sim["mode"], "random");
    assert_eq!(sim["seeds"], 8);
    assert_eq!(sim["budget"], 1);
}

#[test]
fn a_replay_names_one_interleaving_and_says_so() {
    let dir = project(GREEN);
    let out = ply(dir.path())
        .args(["test", "--json", "--seed", "7:3.0.2"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let sim = json_of(&out)["options"]["sim"].clone();
    assert_eq!(sim["mode"], "once");
    assert_eq!(sim["seed"], "7:3.0.2");
    assert_eq!(sim["seeds"], 1);
}

/// A seed that parses loosely replays something other than what failed.
#[test]
fn a_seed_that_is_not_a_seed_is_refused_before_anything_runs() {
    let dir = project(GREEN);
    for bad in ["7:", "seven", "7.3", "0x", "1_000"] {
        let out = ply(dir.path())
            .args(["test", "--seed", bad])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "`{bad}` should not parse");
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(stderr.contains("is not a seed"), "{stderr}");
    }
}

/// A flag that cannot mean anything is refused rather than ignored: silently dropped, it reads as a
/// search that was widened and was not.
#[test]
fn a_flag_with_nothing_to_mean_is_refused() {
    let dir = project(GREEN);

    // `--seed` names one interleaving, so nothing may widen the search beside it.
    for widening in [
        vec!["--sim", "dpor"],
        vec!["--seeds", "4"],
        vec!["--sim-budget", "4"],
    ] {
        let mut args = vec!["test", "--seed", "7"];
        args.extend(widening.iter().copied());
        let out = ply(dir.path()).args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?} should conflict");
    }

    // `random` is one interleaving per seed, so it has no budget to spend.
    let out = ply(dir.path())
        .args(["test", "--sim", "random", "--sim-budget", "4"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("--sim-budget"), "{stderr}");
}

/// Every field of the plan is in a seeded test's cache key, so changing one has to be visible.
#[test]
fn widening_the_search_does_not_disturb_a_corpus_that_never_simulates() {
    let dir = project(GREEN);
    let first = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    assert_eq!(json_of(&first)["summary"]["passed"], 2);

    let wider = ply(dir.path())
        .args(["test", "--json", "--sim-budget", "1024"])
        .output()
        .unwrap();
    let v = json_of(&wider);
    assert_eq!(v["selection"]["selected"], 0, "nothing here reads a seed");
    assert_eq!(v["options"]["sim"]["budget"], 1024);
    assert_eq!(v["simulation"]["simulated"], 0);
    assert_eq!(v["simulation"]["interleavings"], 0);
}

/// `ply run` chooses which interleaving rather than how many: exploration is a test-time activity.
#[test]
fn run_accepts_a_seed_and_takes_only_that_interleaving() {
    let dir = project(GREEN);
    let out = ply(dir.path())
        .args(["run", "--json", "--seed", "3:1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_of(&out)["value"], "42");

    let out = ply(dir.path())
        .args(["run", "--seed", "3:"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

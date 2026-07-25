//! Drives the real binary. Everything here is a claim about the *interface* —
//! exit codes, what lands on stdout, what `--json` promises — which is exactly
//! the part a unit test cannot reach.

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
    assert!(stdout_of(&out).contains("checked 1 file, 2 definitions, 2 tests"));
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
    assert!(text.contains("rows : () -> List<Int> / {db.read[users]}"), "got:\n{text}");
    assert!(text.contains("effect db"));
}

#[test]
fn check_exits_two_on_a_type_error_and_says_nothing_on_stdout() {
    let dir = project(BROKEN);
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stdout_of(&out), "");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("E0201"), "expected a type mismatch, got:\n{stderr}");
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
    assert!(text.contains("selected 0 of 1 (1 cached)"), "a rename rebuilt something:\n{text}");
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
    let ran: Vec<&str> =
        v["results"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(ran, ["a is one"]);
}

#[test]
fn test_exits_one_on_a_failure_and_names_the_suspects() {
    let dir = project(RED);
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = stdout_of(&out);
    assert!(text.contains("1 failed, 1 passed"), "got:\n{text}");
    assert!(text.contains("balance never goes negative"));
    assert!(text.contains("assertion failed: expected 0, found -5"), "got:\n{text}");
    assert!(text.contains("at m.ply:"), "got:\n{text}");
    assert!(text.contains("suspects: balance"), "got:\n{text}");
}

#[test]
fn a_red_test_re_runs_until_it_goes_green() {
    let dir = project(RED);
    ply(dir.path()).arg("test").assert().code(1);

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = stdout_of(&out);
    assert!(text.contains("selected 1 of 2 (1 cached)"), "a failure was cached:\n{text}");
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
    assert_eq!(failure["name"], "balance never goes negative");
    assert_eq!(failure["diagnostic"]["code"], "E0501");
    assert_eq!(failure["diagnostic"]["labels"][0]["file"], "m.ply");
    assert!(failure["diagnostic"]["labels"][0]["start"]["line"].is_number());
    assert_eq!(failure["suspects"], serde_json::json!(["balance"]));

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
    let out = ply(dir.path()).args(["test", "--json", "--color", "always"]).output().unwrap();
    let text = stdout_of(&out);
    assert!(!text.contains('\x1b'), "--json must stay machine-readable under --color always");
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

    let out = ply(dir.path()).args(["test", "--explain"]).output().unwrap();
    let text = stdout_of(&out);
    assert!(text.contains("skip"), "cached tests must be shown as skipped:\n{text}");
    assert!(text.contains("cached"));
    assert!(text.contains("this exact hash already passed"), "got:\n{text}");
    assert!(text.contains("concurrency groups") || text.contains("selected 0 of 2"));

    let cold = project(GREEN);
    let text = stdout_of(&ply(cold.path()).args(["test", "--explain"]).output().unwrap());
    assert!(text.contains("run "), "got:\n{text}");
    assert!(text.contains("concurrency groups"), "got:\n{text}");
    assert!(text.contains("group 0 ·"), "got:\n{text}");
    assert!(text.contains("· {}"), "the group's defining footprint is missing:\n{text}");
}

#[test]
fn no_cache_runs_everything_and_records_nothing() {
    let dir = project(GREEN);
    ply(dir.path()).arg("test").assert().success();

    let text = stdout_of(&ply(dir.path()).args(["test", "--no-cache"]).output().unwrap());
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
    let out = ply(dir.path()).args(["test", "--filter", "zero"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("selected 1 of 1 (0 cached)"), "got:\n{text}");
    assert!(text.contains("--filter hid 1 test"));
    assert!(text.contains("double of zero is zero"));
    assert!(!text.contains("double doubles"), "the filtered-out test still ran:\n{text}");
}

#[test]
fn a_filter_matching_nothing_says_so_rather_than_claiming_success_quietly() {
    let dir = project(GREEN);
    let out = ply(dir.path()).args(["test", "--filter", "nonexistent"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("selected 0 of 0"), "got:\n{text}");
    assert!(text.contains("no test name contains that substring"), "got:\n{text}");
}

#[test]
fn jobs_is_honoured_and_reported() {
    let dir = project(GREEN);
    let out = ply(dir.path()).args(["test", "--jobs", "1"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout_of(&out).contains("1 worker"), "got:\n{}", stdout_of(&out));

    let dir = project(GREEN);
    let v = json_of(&ply(dir.path()).args(["test", "--json", "-j", "3"]).output().unwrap());
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
        "nondet effect clock {\n  read now() -> Int\n}\n\
         test \"reads the clock\" { assert(clock.now() > 0) }\n",
    );
    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v = json_of(&out);
    assert_eq!(v["diagnostics"][0]["code"], "E0412");
}

// --- directories ------------------------------------------------------------

#[test]
fn a_directory_is_checked_as_one_module_across_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/z.ply"), "fn z() -> Int = a() + 1\n").unwrap();
    std::fs::write(dir.path().join("src/a.ply"), "fn a() -> Int = 1\n").unwrap();
    std::fs::write(
        dir.path().join("src/t.ply"),
        "test \"z is two\" { assert_eq(z(), 2) }\n",
    )
    .unwrap();

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout_of(&out).contains("selected 1 of 1"));

    let v = json_of(&ply(dir.path()).args(["check", "--json"]).output().unwrap());
    assert_eq!(v["files"], serde_json::json!(["src/a.ply", "src/t.ply", "src/z.ply"]));
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
    let out = ply(dir.path()).args(["check", "nowhere.ply"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8(out.stderr).unwrap().contains("nowhere.ply"));
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

// --- hash -------------------------------------------------------------------

#[test]
fn hash_prints_a_short_hash_per_definition() {
    let dir = project(GREEN);
    let out = ply(dir.path()).arg("hash").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("double"));
    assert!(text.contains("2 definitions · 2 tests"), "got:\n{text}");
}

#[test]
fn hash_deps_shows_the_graph() {
    let dir = project(GREEN);
    let text = stdout_of(&ply(dir.path()).args(["hash", "--deps"]).output().unwrap());
    assert!(text.contains("deps: double"), "got:\n{text}");
    assert!(text.contains("closure:"));
}

#[test]
fn hash_json_gives_full_hex_and_the_closure() {
    let dir = project(GREEN);
    let v = json_of(&ply(dir.path()).args(["hash", "--json", "--deps"]).output().unwrap());
    assert_eq!(v["command"], "hash");
    assert_eq!(v["definitions"][0]["hash"].as_str().unwrap().len(), 64);
    assert!(v["definitions"][0]["closure"].is_array());
    assert_eq!(v["tests"][0]["hash"].as_str().unwrap().len(), 64);
}

// --- cache ------------------------------------------------------------------

#[test]
fn cache_stats_counts_what_a_run_recorded() {
    let dir = project(GREEN);
    let text = stdout_of(&ply(dir.path()).args(["cache", "stats"]).output().unwrap());
    assert!(text.contains("0 cached results"), "got:\n{text}");

    ply(dir.path()).arg("test").assert().success();
    let v = json_of(&ply(dir.path()).args(["cache", "stats", "--json"]).output().unwrap());
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
    assert!(text.contains("warning"), "the degradation must be reported:\n{text}");
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

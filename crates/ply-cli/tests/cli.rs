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
    assert!(text.contains("rows : () -> List<Int> / {m.db.read[users]}"), "got:\n{text}");
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

/// `ply test` loads twice: once to select, and once more to parse the modules a
/// selected test needs a body from. Between the two the first load writes its
/// fingerprints back, so a file it parsed because its bytes changed is a file
/// the second load will skip — and the second load is the one that has to
/// produce the body. A `nondet` test is never cached, so it forces the second
/// load on every run and makes this reachable from an ordinary edit.
#[test]
fn a_nondet_test_elsewhere_does_not_cost_an_edited_module_its_body() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("clock.ply"),
        "nondet effect clock { read now() -> Int }\n\
         fn tick() -> Int / {clock.read} = clock.now()\n\
         test/nondet \"the clock ticks\" { assert_eq(handle tick() with { clock.now() -> 1, }, 1) }\n",
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
    assert!(text.contains("size is known"), "the edited module's test did not run:\n{text}");
    assert!(!text.contains("cache clear"), "a body went missing:\n{text}");
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
    assert!(text.contains("suspects: m.balance"), "got:\n{text}");
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
    assert!(
        failure["name"].as_str().unwrap().contains("balance never goes negative"),
        "got: {}",
        failure["name"]
    );
    assert_eq!(failure["diagnostic"]["code"], "E0501");
    assert_eq!(failure["diagnostic"]["labels"][0]["file"], "m.ply");
    assert!(failure["diagnostic"]["labels"][0]["start"]["line"].is_number());
    assert_eq!(failure["suspects"], serde_json::json!(["m.balance"]));

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
    assert!(text.contains("no test key contains that substring"), "got:\n{text}");
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
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout_of(&out).contains("selected 1 of 1"));

    let v = json_of(&ply(dir.path()).args(["check", "--json"]).output().unwrap());
    assert_eq!(v["files"], serde_json::json!(["src/a.ply", "src/t.ply", "src/z.ply"]));
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
    std::fs::write(dir.path().join("b.ply"), "import a\nfn b() -> Int = a::secret()\n").unwrap();

    let out = ply(dir.path()).args(["check", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json_of(&out)["diagnostics"][0]["code"], "E0107");
}

#[test]
fn a_module_cycle_is_rejected_with_exit_two() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.ply"), "import b\npub fn a() -> Int = b::b()\n").unwrap();
    std::fs::write(dir.path().join("b.ply"), "import a\npub fn b() -> Int = a::a()\n").unwrap();

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
    std::fs::write(dir.path().join("alpha.ply"), "test \"shared\" { assert_eq(1, 1) }\n")
        .unwrap();
    std::fs::write(dir.path().join("beta.ply"), "test \"shared\" { assert_eq(2, 3) }\n").unwrap();

    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = stdout_of(&out);
    assert!(text.contains("alpha.shared"), "got:\n{text}");
    assert!(text.contains("beta.shared"), "got:\n{text}");
}

#[test]
fn filter_accepts_a_module_prefix() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.ply"), "test \"shared\" { assert_eq(1, 1) }\n")
        .unwrap();
    std::fs::write(dir.path().join("beta.ply"), "test \"shared\" { assert_eq(2, 2) }\n").unwrap();

    let v = json_of(&ply(dir.path()).args(["test", "--json", "--filter", "beta."]).output().unwrap());
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
    assert!(notes.contains("one.ply") && notes.contains("two.ply"), "got: {notes}");

    // Naming the file picks the module, which is the fix the notes suggest.
    let v = json_of(&ply(dir.path()).args(["run", "two.ply", "--json"]).output().unwrap());
    assert_eq!(v["value"], "2");
    assert_eq!(v["entry"], "two.main");
}

// --- hash -------------------------------------------------------------------

#[test]
fn hash_prints_a_short_hash_per_definition() {
    let dir = project(GREEN);
    let out = ply(dir.path()).arg("hash").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert!(text.contains("double"));
    assert!(text.contains("2 definitions · 2 tests · 1 module"), "got:\n{text}");
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
    let v = json_of(&ply(dir.path()).args(["hash", "--json", "--deps"]).output().unwrap());
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
    assert!(text.contains("2 definitions · 1 test · 3 modules"), "got:\n{text}");
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

/// A module is loaded in path order but checked in dependency order, and test
/// indices are shared between the two. Getting that wrong selects a test on
/// another test's hash, so an edit re-runs the wrong one and the edited one
/// stays green from the cache — the exact failure this asserts against.
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
    let ran: Vec<&str> =
        v["results"].as_array().unwrap().iter().map(|r| r["key"].as_str().unwrap()).collect();
    assert_eq!(ran, ["a.a one"], "the wrong test was re-run");
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

#[test]
fn check_explain_names_the_reason_a_file_was_parsed() {
    let dir = project("fn f() -> Int = 1\n");
    std::fs::write(dir.path().join("g.ply"), "fn g() -> Int = 2\n").unwrap();
    ply(dir.path()).arg("check").assert().success();

    let out = ply(dir.path()).args(["check", "--explain"]).output().unwrap();
    let text = stdout_of(&out);
    assert!(text.contains("skipped"), "a warm run must skip something:\n{text}");
    assert!(text.contains("unchanged"), "a skip must say why it was allowed:\n{text}");

    std::fs::write(dir.path().join("m.ply"), "fn f() -> Int = 3\n").unwrap();
    let out = ply(dir.path()).args(["check", "--explain"]).output().unwrap();
    let text = stdout_of(&out);
    assert!(text.contains("content changed"), "a refusal must say why:\n{text}");
}

/// `--no-incremental` has to be observable, or nobody can use it to decide
/// whether a wrong answer came from the cache.
#[test]
fn no_incremental_parses_everything_and_leaves_the_cache_alone() {
    let dir = project("fn f() -> Int = 1\n");
    ply(dir.path()).arg("check").assert().success();

    let out = ply(dir.path()).args(["check", "--explain", "--no-incremental"]).output().unwrap();
    let text = stdout_of(&out);
    assert!(!text.contains("skipped"), "--no-incremental must parse every file:\n{text}");
    assert!(text.contains("--no-incremental"), "the reason must name the flag:\n{text}");

    let out = ply(dir.path()).args(["check", "--explain"]).output().unwrap();
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

    let out = ply(dir.path()).args(["test", "--no-incremental"]).output().unwrap();
    let text = stdout_of(&out);
    assert!(text.contains("2 cached"), "the result cache survives --no-incremental:\n{text}");
}

/// A warm run has to be observably cheaper, not just observably skipping, so
/// the phase breakdown is part of the reported interface.
#[test]
fn the_front_end_reports_where_its_time_went() {
    let dir = project("fn f() -> Int = 1\ntest \"f is one\" { assert_eq(f(), 1) }\n");
    ply(dir.path()).arg("test").assert().success();

    let out = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    let phases = json_of(&out)["front_end"]["phases"].clone();
    for name in ["read", "parse", "resolve", "hash", "check", "restore", "write_back", "total"] {
        assert!(phases[name].is_number(), "`{name}` is missing from {phases}");
    }
    let total = phases["total"].as_f64().unwrap();
    let sum: f64 = ["read", "parse", "resolve", "hash", "check", "restore", "write_back"]
        .iter()
        .map(|n| phases[n].as_f64().unwrap())
        .sum();
    assert!((total - sum).abs() < 0.5, "the total must account for the parts: {phases}");

    let out = ply(dir.path()).args(["check", "--explain"]).output().unwrap();
    assert!(stdout_of(&out).contains("front-end time"), "--explain must show the breakdown");
}

/// A selected test needs a body, so its module and everything it imports have
/// to be parsed. Every *other* module must stay skipped.
///
/// Without this the front-end cache buys nothing where it matters most: one
/// edited definition is the normal case, and reparsing the whole project the
/// moment one test is selected is exactly the from-scratch run the cache exists
/// to avoid.
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

    write("used.ply", "import leaf\npub fn two() -> Int = leaf::one() + 1 + 0\n\
         test \"two is two\" { assert_eq(two(), 2) }\n");
    let out = ply(dir.path()).args(["test", "--explain"]).output().unwrap();
    let text = stdout_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("selected 1 of 5"), "one test must be selected:\n{text}");
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
    let out = ply(dir.path()).args(["test", "--explain"]).output().unwrap();
    let text = stdout_of(&out);
    let front = text.find("m.ply").expect("the front-end block names each file");
    let selection = text.find("f is one").expect("the selection block explains each test");
    assert!(front < selection, "the front-end block comes first:\n{text}");
}

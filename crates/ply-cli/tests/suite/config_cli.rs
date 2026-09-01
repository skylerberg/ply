//! Configuration as the command line sees it: `--set`, `--config`, the process environment,
//! `--config-schema`, and what each of them refuses.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

/// A program with a schema, one key per shape, and a `main` that reads one.
const DESK: &str = "\
import std.config
import std.config (config)

pub fn spec() -> config::ConfigSpec =
  config::spec([
    config::with_default(\"DESK_REGION\", config::SText, \"us\"),
    config::required(\"DESK_PORT\", config::SInt),
    config::required(\"DESK_API_KEY\", config::SSecret),
  ])

pub fn region() -> Option<String> / {config.read[server]} =
  config.get[server](\"DESK_REGION\")

fn main() -> Option<String> / {config.read[server]} = region()

// A `det` test supplies its own values and reaches no host: the row after the
// handle is empty, so this runs, caches and passes without `--host`.
test \"a handled read is hermetic\" {
  let fixture = config::one_value(\"DESK_REGION\", \"twin\");
  handle {
    assert_eq(region(), Some(\"twin\"))
  } with {
    config.get[server](k) -> config::get_step(fixture, k),
  }
}
";

/// The same program with a `main` that reads the credential, for the tests that search a run's
/// whole output for it.
const SECRET_MAIN: &str = "\
import std.config
import std.config (config)

pub fn spec() -> config::ConfigSpec =
  config::spec([config::required(\"DESK_API_KEY\", config::SSecret)])

fn main() -> Bool / {config.read[credentials]} =
  match config.secret[credentials](\"DESK_API_KEY\") {
    None -> false,
    Some(key) -> secret_verify(key, \"correct-horse\"),
  }
";

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("m.ply"), source).expect("the fixture is written");
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").expect("the binary is built");
    cmd.arg("--color")
        .arg("never")
        .current_dir(dir)
        // The parent's environment is a source, so a test that did not clear it would be a test
        // whose answer depended on the machine it ran on.
        .env_remove("DESK_REGION")
        .env_remove("DESK_PORT")
        .env_remove("DESK_API_KEY");
    cmd
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is utf-8")
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is utf-8")
}

fn json_of(output: &std::process::Output) -> Value {
    serde_json::from_str(&stdout_of(output)).expect("`--json` writes one document on stdout")
}

/// `ply hosts --host --json`, which is the command whose whole output is the resolved
/// configuration, so it is what precedence is read from.
fn hosts(dir: &Path, extra: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = ply(dir);
    cmd.arg("hosts").arg("--host").arg("--json");
    cmd.args(extra);
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.output().expect("the command runs")
}

fn key_of(report: &Value, name: &str) -> Value {
    report["configuration"]["keys"]
        .as_array()
        .expect("the keys are an array")
        .iter()
        .find(|k| k["name"] == name)
        .unwrap_or_else(|| panic!("`{name}` is not in {report:#}"))
        .clone()
}

// --- precedence -------------------------------------------------------------

#[test]
fn precedence_is_set_then_file_then_environment_then_default() {
    let dir = project(DESK);
    std::fs::write(dir.path().join("deploy.env"), "DESK_REGION=file\n").expect("written");
    let schema = ["--config-schema", "m.spec", "--set", "DESK_PORT=8137"];

    let all = hosts(
        dir.path(),
        &[
            &schema[..],
            &["--set", "DESK_REGION=set", "--config", "deploy.env"],
        ]
        .concat(),
        &[("DESK_REGION", "environment"), ("DESK_API_KEY", "k")],
    );
    let region = key_of(&json_of(&all), "DESK_REGION");
    assert_eq!(region["value"], "set");
    assert_eq!(region["source"], "--set");

    let no_set = hosts(
        dir.path(),
        &[&schema[..], &["--config", "deploy.env"]].concat(),
        &[("DESK_REGION", "environment"), ("DESK_API_KEY", "k")],
    );
    let region = key_of(&json_of(&no_set), "DESK_REGION");
    assert_eq!(region["value"], "file");
    assert_eq!(region["source"], "deploy.env");

    let no_files = hosts(
        dir.path(),
        &schema,
        &[("DESK_REGION", "environment"), ("DESK_API_KEY", "k")],
    );
    let region = key_of(&json_of(&no_files), "DESK_REGION");
    assert_eq!(region["value"], "environment");
    assert_eq!(region["source"], "env");

    let nothing = hosts(dir.path(), &schema, &[("DESK_API_KEY", "k")]);
    let region = key_of(&json_of(&nothing), "DESK_REGION");
    assert_eq!(region["value"], "us");
    assert_eq!(region["source"], "default");
}

/// The environment supplies a value and never causes a binding.
#[test]
fn without_host_no_source_is_opened_and_the_flags_are_refused() {
    let dir = project(DESK);
    let output = ply(dir.path())
        .arg("hosts")
        .arg("--json")
        .env("DESK_REGION", "environment")
        .output()
        .expect("the command runs");
    let report = json_of(&output);
    assert_eq!(report["binding"], "hermetic");
    assert!(
        report["configuration"].is_null(),
        "a hermetic run has no configuration block: {report:#}"
    );

    let refused = ply(dir.path())
        .arg("hosts")
        .arg("--set")
        .arg("DESK_REGION=eu")
        .output()
        .expect("the command runs");
    assert!(!refused.status.success());
    assert!(
        stderr_of(&refused).contains("--host"),
        "{}",
        stderr_of(&refused)
    );
}

// --- the file format --------------------------------------------------------

#[test]
fn a_malformed_config_file_is_e0440_naming_the_file_and_line() {
    let dir = project(DESK);
    for (line, contents, expected) in [
        (2, "DESK_REGION=eu\nno equals here\n", "no `=`"),
        (2, "DESK_REGION=eu\n=8137\n", "empty key"),
        (2, "DESK_REGION=eu\nDESK-PORT=8137\n", "contains"),
    ] {
        std::fs::write(dir.path().join("bad.env"), contents).expect("written");
        let output = ply(dir.path())
            .arg("hosts")
            .arg("--host")
            .arg("--config")
            .arg("bad.env")
            .output()
            .expect("the command runs");
        let text = stderr_of(&output);
        assert!(!output.status.success(), "{text}");
        assert!(text.contains("E0440"), "{text}");
        assert!(text.contains("bad.env"), "{text}");
        assert!(text.contains(&format!("line {line}")), "{text}");
        assert!(text.contains(expected), "{text}");
    }
}

#[test]
fn an_unreadable_config_file_is_e0440() {
    let dir = project(DESK);
    let output = ply(dir.path())
        .arg("hosts")
        .arg("--host")
        .arg("--config")
        .arg("absent.env")
        .output()
        .expect("the command runs");
    assert!(!output.status.success());
    let text = stderr_of(&output);
    assert!(text.contains("E0440"), "{text}");
    assert!(text.contains("absent.env"), "{text}");
}

// --- the schema -------------------------------------------------------------

#[test]
fn a_required_key_nothing_supplies_is_e0441_at_startup() {
    let dir = project(DESK);
    let output = ply(dir.path())
        .arg("run")
        .arg("--host")
        .arg("--config-schema")
        .arg("m.spec")
        .output()
        .expect("the command runs");
    assert!(!output.status.success());
    let text = stderr_of(&output);
    assert!(text.contains("E0441"), "{text}");
    assert!(text.contains("DESK_PORT"), "{text}");
    assert!(text.contains("DESK_API_KEY"), "{text}");
    assert!(text.contains("environment variable"), "{text}");
    assert!(
        !text.contains("binding host"),
        "the refusal comes before anything is bound: {text}"
    );
}

#[test]
fn a_value_that_is_not_of_its_shape_is_e0442_naming_the_source() {
    let dir = project(DESK);
    let output = ply(dir.path())
        .arg("run")
        .arg("--host")
        .arg("--config-schema")
        .arg("m.spec")
        .arg("--set")
        .arg("DESK_PORT=eight")
        .env("DESK_API_KEY", "k")
        .output()
        .expect("the command runs");
    assert!(!output.status.success());
    let text = stderr_of(&output);
    assert!(text.contains("E0442"), "{text}");
    assert!(text.contains("DESK_PORT"), "{text}");
    assert!(text.contains("--set"), "{text}");
    assert!(text.contains("eight"), "a plain value is shown: {text}");
}

/// The one refusal that must not quote what it refused.
#[test]
fn a_malformed_secret_is_e0442_and_prints_no_value() {
    let dir = project(DESK);
    let output = ply(dir.path())
        .arg("run")
        .arg("--host")
        .arg("--config-schema")
        .arg("m.spec")
        .arg("--set")
        .arg("DESK_PORT=8137")
        .arg("--set")
        .arg("DESK_API_KEY=")
        .output()
        .expect("the command runs");
    assert!(!output.status.success());
    let text = stderr_of(&output);
    assert!(text.contains("E0442"), "{text}");
    assert!(text.contains("DESK_API_KEY"), "{text}");
    assert!(text.contains("not printed"), "{text}");
}

#[test]
fn an_undeclared_set_warns_and_an_undeclared_environment_key_does_not() {
    let dir = project(DESK);
    let output = ply(dir.path())
        .arg("hosts")
        .arg("--host")
        .arg("--config-schema")
        .arg("m.spec")
        .arg("--set")
        .arg("DESK_PORT=8137")
        .arg("--set")
        .arg("DESK_PROT=8138")
        .env("DESK_API_KEY", "k")
        .env("AWS_PROFILE", "nothing to do with this program")
        .output()
        .expect("the command runs");
    assert!(output.status.success(), "{}", stderr_of(&output));
    let text = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(text.contains("W0607"), "{text}");
    assert!(text.contains("DESK_PROT"), "{text}");
    assert!(
        !text.contains("AWS_PROFILE"),
        "an environment name is not a typo somebody made in this run: {text}"
    );
}

/// A `--config-schema` naming nothing is refused with the candidates, because the fix is a
/// different argument rather than an edit to the program.
#[test]
fn a_config_schema_naming_no_definition_lists_what_the_program_has() {
    let dir = project(DESK);
    let output = ply(dir.path())
        .arg("hosts")
        .arg("--host")
        .arg("--config-schema")
        .arg("m.confg")
        .output()
        .expect("the command runs");
    assert!(!output.status.success());
    let text = stderr_of(&output);
    assert!(text.contains("E0440"), "{text}");
    assert!(text.contains("m.spec"), "{text}");
}

// --- the secret gate --------------------------------------------------------

#[test]
fn get_cannot_read_a_key_the_schema_declares_secret() {
    let source = DESK.replace(
        "config.get[server](\"DESK_REGION\")",
        "config.get[server](\"DESK_API_KEY\")",
    );
    let dir = project(&source);
    let output = ply(dir.path())
        .arg("run")
        .arg("--host")
        .arg("--config-schema")
        .arg("m.spec")
        .arg("--set")
        .arg("DESK_PORT=8137")
        .arg("--set")
        .arg("DESK_API_KEY=correct-horse")
        .output()
        .expect("the command runs");
    let text = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(
        text.contains("None"),
        "`config.get` answered a credential: {text}"
    );
    assert!(
        !text.contains("correct-horse"),
        "the credential reached the run's output: {text}"
    );
}

/// The credential arrives as a `Secret` and is usable through `secret_verify`, and its bytes appear
/// in neither stream.
#[test]
fn a_secret_arrives_as_a_secret_and_appears_in_no_stream() {
    let dir = project(SECRET_MAIN);
    let output = ply(dir.path())
        .arg("run")
        .arg("--host")
        .arg("--config-schema")
        .arg("m.spec")
        .arg("--set")
        .arg("DESK_API_KEY=correct-horse")
        .output()
        .expect("the command runs");
    let out = stdout_of(&output);
    let err = stderr_of(&output);
    assert!(output.status.success(), "{err}");
    assert!(out.contains("true"), "the key verified: {out}");
    for stream in [&out, &err] {
        assert!(
            !stream.contains("correct-horse"),
            "the credential reached a stream: {stream}"
        );
    }
}

/// And the same for `--json`, whose one document on stdout carries the key's name and the source
/// that won it and never its value.
#[test]
fn the_json_report_carries_a_secrets_key_and_source_and_not_its_value() {
    let dir = project(SECRET_MAIN);
    let output = hosts(
        dir.path(),
        &["--config-schema", "m.spec"],
        &[("DESK_API_KEY", "correct-horse")],
    );
    let report = json_of(&output);
    let key = key_of(&report, "DESK_API_KEY");
    assert_eq!(key["value"], "****");
    assert_eq!(key["source"], "env");
    assert_eq!(key["secret"], true);
    assert!(
        !stdout_of(&output).contains("correct-horse"),
        "{}",
        stdout_of(&output)
    );
}

// --- hermetic supply --------------------------------------------------------

#[test]
fn a_test_supplying_configuration_is_hermetic_det_and_cached() {
    let dir = project(DESK);
    let first = ply(dir.path())
        .arg("test")
        .arg("--json")
        .output()
        .expect("the command runs");
    let report = json_of(&first);
    assert_eq!(report["ok"], true, "{report:#}");
    assert_eq!(report["binding"], "hermetic", "{report:#}");

    let second = ply(dir.path())
        .arg("test")
        .arg("--json")
        .output()
        .expect("the command runs");
    let report = json_of(&second);
    assert_eq!(report["ok"], true, "{report:#}");
    assert_eq!(
        report["summary"]["cached"], 1,
        "the second run is a cache hit: {report:#}"
    );
    assert_eq!(report["selection"]["selected"], 0, "{report:#}");
}

/// The other half: an unhandled `config` operation in a `det` test is `E0412` at compile time, with
/// `--host` and without it, because the effect is `nondet`.
#[test]
fn an_unhandled_config_read_in_a_det_test_is_e0412() {
    let source = "\
import std.config
import std.config (config)

pub fn region() -> Option<String> / {config.read[server]} =
  config.get[server](\"DESK_REGION\")

test \"reaches the environment\" {
  assert_eq(region(), None)
}
";
    let dir = project(source);
    for extra in [vec!["test"], vec!["test", "--host"]] {
        let output = ply(dir.path()).args(&extra).output().expect("it runs");
        let text = stderr_of(&output);
        assert!(!output.status.success(), "{extra:?}: {text}");
        assert!(text.contains("E0412"), "{extra:?}: {text}");
    }
}

/// And a run that reaches the boundary with nothing bound is `E0424` naming the handler that
/// *would* have served it — never a silent read of the environment.
#[test]
fn a_hermetic_run_that_reaches_config_is_e0424_naming_the_handler() {
    let dir = project(DESK);
    let output = ply(dir.path())
        .arg("run")
        .env("DESK_REGION", "environment")
        .output()
        .expect("the command runs");
    let text = stderr_of(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("E0424"), "{text}");
    assert!(text.contains("ply_host::config::get"), "{text}");
    assert!(
        !text.contains("environment"),
        "a hermetic run must not have read the environment at all: {text}"
    );
}

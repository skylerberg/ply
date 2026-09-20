use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;

const SOURCE: &str = r#"
import std.process (process)

fn greeting(args: List<String>) -> String =
  fold(args, "hi", |acc: String, a: String| acc ++ " " ++ a)

fn main() -> Unit / {process.read[proc], process.write[proc]} = {
  process.out[proc](greeting(process.args[proc]()));
  process.err[proc]("leaving");
  process.exit[proc](3);
  process.out[proc]("never written")
}
"#;

const OUT_OF_RANGE: &str = r#"
import std.process (process)

fn main() -> Unit / {process.write[proc]} = process.exit[proc](300)
"#;

const IN_A_TEST: &str = r#"
import std.process (process)

pub fn announce() -> Unit / {process.write[proc]} = process.out[proc]("ready")

test/nondet "the announcement reaches the process" {
  announce()
}
"#;

fn project(source: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), source).unwrap();
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

fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn run_with_host_answers_the_arguments_writes_each_stream_and_exits_with_the_code() {
    let dir = project(SOURCE);
    let out = ply(dir.path())
        .args(["run", "m.ply", "--host", "--", "a", "b"])
        .output()
        .unwrap();
    let stdout = text_of(&out.stdout);
    let stderr = text_of(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stdout}\n{stderr}");
    assert_eq!(
        stdout.lines().last(),
        Some("hi a b"),
        "the program's line is the last thing on stdout, and no value follows it:\n{stdout}"
    );
    assert!(!stdout.contains("never written"), "{stdout}");
    assert!(stderr.contains("leaving"), "{stderr}");
    assert!(
        !stderr.contains("E0455") && !stderr.contains("raised at"),
        "an exit the program asked for is not an error:\n{stderr}"
    );
}

#[test]
fn without_host_the_boundary_refuses_and_names_the_handler() {
    let dir = project(SOURCE);
    let out = ply(dir.path()).args(["run", "m.ply"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = text_of(&out.stderr);
    assert!(stderr.contains("E0424"), "{stderr}");
    assert!(stderr.contains("ply_host::process::args"), "{stderr}");
    assert!(!text_of(&out.stdout).contains("hi"), "nothing ran");
}

#[test]
fn under_json_the_object_is_alone_on_stdout_and_the_lines_go_to_stderr() {
    let dir = project(SOURCE);
    let out = ply(dir.path())
        .args(["run", "m.ply", "--host", "--json", "--", "x"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let v = json_of(&out);
    assert_eq!(v["command"], "run");
    assert_eq!(v["ok"], false);
    assert_eq!(v["exit_code"], 3);
    assert_eq!(v["value"], Value::Null);
    assert_eq!(v["binding"], "host");
    let stderr = text_of(&out.stderr);
    assert!(stderr.contains("hi x"), "{stderr}");
    assert!(stderr.contains("leaving"), "{stderr}");
}

#[test]
fn an_exit_code_the_shell_could_not_carry_is_a_runtime_error() {
    let dir = project(OUT_OF_RANGE);
    let out = ply(dir.path())
        .args(["run", "m.ply", "--host"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = text_of(&out.stderr);
    assert!(stderr.contains("E0502"), "{stderr}");
    assert!(stderr.contains("0 to 125"), "{stderr}");
}

#[test]
fn ply_test_withholds_the_process_even_with_host() {
    let dir = project(IN_A_TEST);
    for flags in [vec!["test", "--json"], vec!["test", "--host", "--json"]] {
        let out = ply(dir.path()).args(&flags).output().unwrap();
        let text = format!("{}{}", text_of(&out.stdout), text_of(&out.stderr));
        assert!(
            text.contains("E0424"),
            "`ply {}` did not refuse `process.out` at the boundary\n\n{text}",
            flags.join(" ")
        );
        assert!(
            text.contains("ply_host::process::out"),
            "the refusal names the handler that would have served it\n\n{text}"
        );
    }
}

#[test]
fn a_run_that_never_exits_prints_its_value_as_before() {
    let dir = project(
        r#"
import std.process (process)

fn main() -> Int / {process.read[proc]} = len(process.args[proc]())
"#,
    );
    let out = ply(dir.path())
        .args(["run", "m.ply", "--host", "--json", "--", "one", "two"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["exit_code"], 0);
    assert_eq!(v["value"], "2");
}

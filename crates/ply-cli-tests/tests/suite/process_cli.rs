use crate::harness::{json_of, ply, process, project};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::time::{Duration, Instant};

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

const ONE_LINE: &str = r#"
import std.process (process)

fn main() -> Unit / {process.write[proc]} =
  match process.line[proc]() {
    None -> process.out[proc]("end of input"),
    Some(line) -> process.out[proc]("read " ++ line),
  }
"#;

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
fn run_with_host_reads_a_line_from_standard_input() {
    let dir = project(ONE_LINE);
    let out = ply(dir.path())
        .args(["run", "m.ply", "--host"])
        .write_stdin("hello\nrest\n")
        .output()
        .unwrap();
    let stdout = text_of(&out.stdout);
    let stderr = text_of(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("read hello"), "{stdout}");
    assert!(
        !stdout.contains("rest"),
        "the whole input is not one call's answer:\n{stdout}"
    );
}

#[test]
fn a_line_read_past_the_end_of_input_is_none() {
    let dir = project(ONE_LINE);
    let out = ply(dir.path())
        .args(["run", "m.ply", "--host"])
        .write_stdin("")
        .output()
        .unwrap();
    let stdout = text_of(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}\n{}",
        text_of(&out.stderr)
    );
    assert!(stdout.contains("end of input"), "{stdout}");
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

/// Says, for two labels, whether the run bound a program to each; it starts neither.
const ASKS_WHAT_IS_BOUND: &str = r#"
import std.process (process)

fn said(label: String, bound: Bool) -> String =
  label ++ (if bound { " is bound" } else { " is not bound" })

fn main() -> Unit / {process.bound[sh], process.bound[cc], process.out[proc]} = {
  process.out[proc](said("sh", process.bound[sh]()));
  process.out[proc](said("cc", process.bound[cc]()))
}
"#;

#[test]
fn bound_answers_from_the_exec_table_the_run_was_given() {
    let dir = project(ASKS_WHAT_IS_BOUND);
    for (exec, sh) in [
        (&["--exec", "sh=/bin/sh"][..], "sh is bound"),
        (&[][..], "sh is not bound"),
    ] {
        let out = ply(dir.path())
            .args(["run", "m.ply", "--host"])
            .args(exec)
            .output()
            .unwrap();
        let stdout = text_of(&out.stdout);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{stdout}\n{}",
            text_of(&out.stderr)
        );
        let lines: Vec<&str> = stdout.lines().collect();
        assert!(lines.contains(&sh), "run with {exec:?}:\n{stdout}");
        assert!(
            lines.contains(&"cc is not bound"),
            "run with {exec:?}:\n{stdout}"
        );
    }
}

/// Starts a child that says its pid and sleeps, says that pid, then ends as `ENDING` says.
const LEAVES_A_CHILD: &str = r#"
import std.process
import std.process (process)

fn main() -> Unit / {process.start[sh], process.output_line[sh], process.wait[sh], process.out[proc], process.exit[proc]} = {
  let started = process.start[sh](
    ["-c", "echo $$; exec sleep 600"],
    "",
    [process::var("PATH", "/usr/bin:/bin")],
    { input: false, out: process::Lines, err: process::Discard },
  );
  match started {
    Err(why) -> process.out[proc]("not started: " ++ why),
    Ok(child) -> {
      match process.output_line[sh](child, 10000) {
        process::Said(pid) -> process.out[proc]("pid " ++ pid),
        _ -> process.out[proc]("no pid"),
      };
      ENDING
    },
  }
}
"#;

const WAITS_ON_IT: &str = "let _ = process.wait[sh](child, -1);\n      ()";

fn leaving(ending: &str) -> tempfile::TempDir {
    project(&LEAVES_A_CHILD.replace("ENDING", ending))
}

fn pid_in(line: &str) -> Option<String> {
    line.strip_prefix("pid ").map(|pid| pid.trim().to_string())
}

/// Whether any process has this pid, as `kill -0` asks.
fn alive(pid: &str) -> bool {
    std::process::Command::new("/bin/sh")
        .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
        .status()
        .expect("a shell runs")
        .success()
}

#[test]
fn a_child_still_running_when_the_run_ends_does_not_outlive_it() {
    for (ending, code) in [
        ("()", 0),
        ("process.exit[proc](3)", 3),
        ("panic(\"the program gave up\")", 1),
    ] {
        let dir = leaving(ending);
        let out = ply(dir.path())
            .args(["run", "m.ply", "--host", "--exec", "sh=/bin/sh"])
            .output()
            .unwrap();
        let stdout = text_of(&out.stdout);
        assert_eq!(
            out.status.code(),
            Some(code),
            "ended by `{ending}`\n{stdout}\n{}",
            text_of(&out.stderr)
        );
        let pid = stdout
            .lines()
            .find_map(pid_in)
            .unwrap_or_else(|| panic!("the run never said the child's pid:\n{stdout}"));
        assert!(
            !alive(&pid),
            "the child {pid} outlived a run ended by `{ending}`"
        );
    }
}

/// A run waiting on its child, stopped from outside; the answer is the exit status and the pid.
#[cfg(unix)]
fn stopped(flags: &[&str], signals: usize) -> (Option<i32>, String) {
    let dir = leaving(WAITS_ON_IT);
    let mut run = process(dir.path())
        .args(["run", "m.ply", "--host", "--exec", "sh=/bin/sh"])
        .args(flags)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("`ply run` starts");
    // Read to the end on a thread of its own, so nothing the run writes later meets a closed pipe.
    let stdout = run.stdout.take().expect("piped");
    let (said, heard) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(pid) = pid_in(&line) {
                let _ = said.send(pid);
            }
        }
    });
    let pid = heard
        .recv_timeout(Duration::from_secs(300))
        .expect("the run said the child's pid");
    assert!(alive(&pid), "the child {pid} is running");
    for _ in 0..signals {
        let sent = std::process::Command::new("kill")
            .args(["-TERM", &run.id().to_string()])
            .status()
            .expect("`kill` runs");
        assert!(sent.success(), "`kill -TERM` failed");
        std::thread::sleep(Duration::from_millis(300));
    }
    let until = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = run.try_wait().expect("the run can be waited on") {
            break status;
        }
        if Instant::now() >= until {
            let _ = run.kill();
            panic!("the run was still going a minute after it was stopped");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    reader.join().expect("the reader finished");
    (status.code(), pid)
}

#[cfg(unix)]
#[test]
fn a_second_signal_ends_the_children_before_the_run_exits() {
    let (code, pid) = stopped(&[], 2);
    assert_eq!(code, Some(143), "a second `SIGTERM` exits 143");
    assert!(!alive(&pid), "the child {pid} outlived the second signal");
}

#[cfg(unix)]
#[test]
fn a_drain_that_runs_out_of_time_ends_the_children_with_the_run() {
    let (code, pid) = stopped(&["--drain-ms", "200"], 1);
    assert_eq!(code, Some(3), "an expired drain exits 3");
    assert!(!alive(&pid), "the child {pid} outlived the drain");
}

use ply_core::{Diagnostic, HostRuntime, Span, Symbol, codes};
use ply_host::jobs::{Failure, FallibleJob};
use ply_host::native::{int_of, parts_of};
use ply_host::process::*;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

thread_local! {
    static RUN: Rc<dyn HostRuntime> = ply_host::Host::new().runtime();
}

fn the_run() -> Rc<dyn HostRuntime> {
    RUN.with(Rc::clone)
}

fn host(argv: &[&str]) -> Arc<ProcessHost> {
    Arc::new(ProcessHost::new(
        argv.iter().map(|a| (*a).to_string()).collect(),
        OutputSink::captured(),
    ))
}

const S: Span = Span::DUMMY;

#[test]
fn args_returns_the_vector_it_was_given() {
    let host = host(&["a", "b c"]);
    assert_eq!(host.args(S).expect("a run's own process"), ["a", "b c"]);
}

#[test]
fn a_captured_sink_collects_both_streams_in_order() {
    let host = host(&[]);
    host.line(Stream::Out, "one", S).expect("a write");
    host.line(Stream::Err, "two", S).expect("a write");
    host.line(Stream::Out, "three", S).expect("a write");
    assert_eq!(
        host.captured(),
        [
            (Stream::Out, b"one\n".to_vec()),
            (Stream::Err, b"two\n".to_vec()),
            (Stream::Out, b"three\n".to_vec()),
        ]
    );
    assert_eq!(host.requested_exit(), None);
}

#[test]
fn bytes_are_written_as_they_are_and_a_line_keeps_its_place_among_them() {
    let host = host(&[]);
    let raw = b"\x00\xff no newline";
    assert!(host.write(Stream::Out, raw, S).expect("a write"));
    host.line(Stream::Err, "warn", S).expect("a write");
    assert!(host.write(Stream::Err, raw, S).expect("a write"));
    host.line(Stream::Out, "done", S).expect("a write");
    assert!(host.flush(S).expect("a flush"));
    assert_eq!(
        host.captured(),
        [
            (Stream::Out, raw.to_vec()),
            (Stream::Err, b"warn\n".to_vec()),
            (Stream::Err, raw.to_vec()),
            (Stream::Out, b"done\n".to_vec()),
        ]
    );
}

#[test]
fn a_host_whose_streams_are_captured_leaves_an_unhandled_raise_to_the_run() {
    let host = host(&[]);
    host.unhandled(&Symbol::new(PIPE_EFFECT), &Symbol::new(PIPE_BROKEN));
    host.settle();
}

#[test]
fn exit_records_the_first_code_and_unwinds() {
    let host = host(&[]);
    assert_eq!(host.exit(3, S).code, codes::PROCESS_EXIT);
    assert_eq!(host.requested_exit(), Some(3));
    assert_eq!(host.exit(4, S).code, codes::PROCESS_EXIT);
    assert_eq!(
        host.requested_exit(),
        Some(3),
        "the first code is the one kept"
    );
}

#[test]
fn a_host_that_only_starts_programs_withholds_the_runs_own_process() {
    let host = ProcessHost::spawning(Executables::new());
    let rejections = [
        host.args(S).map(|_| ()).expect_err("no arguments"),
        host.line(Stream::Out, "x", S).expect_err("no stream"),
        host.write(Stream::Err, b"x", S)
            .map(|_| ())
            .expect_err("no stream"),
        host.flush(S).map(|_| ()).expect_err("no stream"),
        host.read_line(S).map(|_| ()).expect_err("no input"),
        host.read(1, S).map(|_| ()).expect_err("no input"),
        host.exit(0, S),
    ];
    for rejected in rejections {
        assert_eq!(rejected.code, codes::HOST_UNBOUND, "{}", rejected.message);
        assert!(
            rejected
                .notes
                .iter()
                .any(|n| n.contains("`ply test` withholds it")),
            "{:?}",
            rejected.notes
        );
    }
    assert!(host.captured().is_empty());
    assert_eq!(host.requested_exit(), None);
}

const SH: &str = "/bin/sh";

const PATH: (&str, &str) = ("PATH", "/usr/bin:/bin");

fn spawning(label: &str, program: &str) -> Arc<ProcessHost> {
    let mut executables = Executables::new();
    executables
        .bind(label, std::path::Path::new(program), S)
        .expect("the program binds");
    Arc::new(ProcessHost::new(Vec::new(), OutputSink::captured()).executing(executables))
}

fn ran(job: FallibleJob) -> Result<Vec<u8>, Diagnostic> {
    job().map_err(|failure| match failure {
        Failure::Rejected(rejection) => rejection,
        Failure::Said(reason) => panic!("the job failed: {reason}"),
    })
}

fn finished(flat: &[u8]) -> Finished {
    let [kind, n, out, err] = parts_of(flat)
        .expect("parts")
        .try_into()
        .expect("four parts");
    let n = int_of(n).expect("an integer");
    Finished {
        ended: if int_of(kind) == Some(0) {
            Ended::Exited(n)
        } else {
            Ended::Signalled(n)
        },
        out: out.to_vec(),
        err: err.to_vec(),
    }
}

fn spawn(
    host: &Arc<ProcessHost>,
    label: &str,
    args: &[&str],
    dir: &str,
    env: &[(&str, &str)],
) -> Result<Finished, Diagnostic> {
    let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
        .collect();
    let job = host.spawn(label, &args, dir, &env, S)?;
    Ok(finished(&ran(job)?))
}

#[test]
fn a_spawn_returns_the_code_and_everything_both_streams_took() {
    let host = spawning("sh", SH);
    let done = spawn(
        &host,
        "sh",
        &["-c", "printf built; printf 'warning' 1>&2; exit 3"],
        "",
        &[PATH],
    )
    .expect("a spawn");
    assert_eq!(done.ended, Ended::Exited(3));
    assert_eq!(done.out, b"built");
    assert_eq!(done.err, b"warning");
}

#[test]
fn a_signal_is_an_ending_of_its_own_rather_than_a_code() {
    let host = spawning("sh", SH);
    let done = spawn(&host, "sh", &["-c", "kill -9 $$"], "", &[PATH]).expect("a spawn");
    assert_eq!(done.ended, Ended::Signalled(9));
}

#[test]
fn ending_the_children_ends_a_spawn_still_running() {
    let host = spawning("sh", SH);
    let args = ["-c".to_string(), "exec sleep 600".to_string()];
    let job = host.spawn("sh", &args, "", &[], S).expect("a spawn");
    host.end_children();
    let ended = Instant::now();
    let done = finished(&ran(job).expect("an ending"));
    assert_eq!(done.ended, Ended::Signalled(9));
    assert!(
        ended.elapsed() < Duration::from_secs(60),
        "the spawn ran on"
    );
}

#[test]
fn a_spawn_sees_only_the_environment_it_was_given() {
    unsafe { std::env::set_var("PLY_SPAWN_WITNESS", "leaked") };
    let host = spawning("sh", SH);
    let done = spawn(
        &host,
        "sh",
        &[
            "-c",
            r#"printf '%s-%s' "${PLY_UNIT-unset}" "${PLY_SPAWN_WITNESS-unset}""#,
        ],
        "",
        &[("PLY_UNIT", "lexer")],
    )
    .expect("a spawn");
    assert_eq!(done.out, b"lexer-unset");
}

#[test]
fn a_spawn_runs_in_the_directory_it_was_given() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(dir.path().join("marker"), b"here\n").unwrap();
    let host = spawning("sh", SH);
    let done = spawn(
        &host,
        "sh",
        &["-c", "read line < marker; printf %s \"$line\""],
        &dir.path().to_string_lossy(),
        &[PATH],
    )
    .expect("a spawn");
    assert_eq!(done.ended, Ended::Exited(0));
    assert_eq!(done.out, b"here");
}

#[test]
fn a_spawn_of_an_unbound_label_names_the_flag_that_would_bind_it() {
    let host = spawning("sh", SH);
    let rejected =
        spawn(&host, "cc", &["-c", "true"], "", &[]).expect_err("`cc` is bound to nothing");
    assert_eq!(rejected.code, codes::PROCESS_EXEC_UNBOUND);
    assert!(
        rejected.notes.iter().any(|n| n.contains("--exec cc=")),
        "the diagnostic should name the flag: {:?}",
        rejected.notes
    );
    assert!(
        rejected
            .notes
            .iter()
            .any(|n| n.contains("process.bound[cc]()")),
        "the diagnostic should name the question a program can ask first: {:?}",
        rejected.notes
    );
}

#[test]
fn bound_returns_whether_the_run_bound_a_program_to_the_label() {
    let shell = spawning("sh", SH);
    for _ in 0..2 {
        assert!(shell.bound("sh"));
        assert!(!shell.bound("cc"));
    }
    assert!(
        !host(&[]).bound("sh"),
        "a run given no `--exec` binds nothing"
    );
}

#[test]
fn an_exec_that_cannot_be_started_does_not_bind() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let plain = dir.path().join("notes.txt");
    std::fs::write(&plain, b"not a program").unwrap();
    let mut executables = Executables::new();

    let missing = executables
        .bind("cc", &dir.path().join("absent"), S)
        .expect_err("a program that is not there");
    assert_eq!(missing.code, codes::PROCESS_EXEC_INVALID);

    let directory = executables
        .bind("cc", dir.path(), S)
        .expect_err("a directory is not a program");
    assert_eq!(directory.code, codes::PROCESS_EXEC_INVALID);

    let unreadable = executables
        .bind("cc", &plain, S)
        .expect_err("a file with no execute bit");
    assert_eq!(unreadable.code, codes::PROCESS_EXEC_INVALID);
    assert!(executables.is_empty());

    executables
        .bind("sh", std::path::Path::new(SH), S)
        .expect("a shell is a program");
    assert_eq!(
        executables
            .listing()
            .map(|(name, _)| name)
            .collect::<Vec<_>>(),
        ["sh"]
    );
}

#[test]
fn an_exec_argument_is_a_label_and_a_path() {
    assert_eq!(
        ExecSpec::parse("cc=/usr/bin/cc").expect("a well-formed pair"),
        ExecSpec {
            name: "cc".to_string(),
            path: std::path::PathBuf::from("/usr/bin/cc"),
        }
    );
    for bad in [
        "cc",
        "=/usr/bin/cc",
        "cc=",
        "1cc=/usr/bin/cc",
        "c-c=/usr/bin/cc",
    ] {
        assert!(ExecSpec::parse(bad).is_err(), "`{bad}` should not parse");
    }
}

fn wired(input: bool, out: Output, err: Output) -> Io {
    Io { input, out, err }
}

fn file(path: &str) -> Output {
    Output::File(path.to_string())
}

fn launched(
    host: &Arc<ProcessHost>,
    label: &str,
    script: &str,
    dir: &str,
    io: Io,
) -> Result<Result<i64, String>, Diagnostic> {
    let env = [(PATH.0.to_string(), PATH.1.to_string())];
    host.start(
        label,
        &["-c".to_string(), script.to_string()],
        dir,
        &env,
        &io,
        S,
    )
}

fn start(host: &Arc<ProcessHost>, script: &str, dir: &str, io: Io) -> i64 {
    launched(host, "sh", script, dir, io)
        .expect("a start returns")
        .unwrap_or_else(|reason| panic!("not started: {reason}"))
}

fn wait_as(
    host: &Arc<ProcessHost>,
    label: &str,
    child: i64,
    timeout_ms: i64,
) -> Result<Option<Finished>, Diagnostic> {
    let found = ran(host.wait(label, child, timeout_ms, S)?)?;
    Ok(match found.split_first() {
        Some((1, flat)) => Some(finished(flat)),
        _ => None,
    })
}

fn wait(
    host: &Arc<ProcessHost>,
    child: i64,
    timeout_ms: i64,
) -> Result<Option<Finished>, Diagnostic> {
    wait_as(host, "sh", child, timeout_ms)
}

fn signal(host: &Arc<ProcessHost>, child: i64, which: Signal) -> Result<bool, Diagnostic> {
    host.signal("sh", child, which, S)
}

fn input(host: &Arc<ProcessHost>, child: i64, bytes: &[u8]) -> Result<bool, Diagnostic> {
    let job = host.input("sh", child, bytes.to_vec(), S)?;
    Ok(job().expect("a write returns") == [1])
}

fn heard(host: &Arc<ProcessHost>, child: i64, timeout_ms: i64) -> Heard {
    let flat = ran(host
        .output_line("sh", child, timeout_ms, S)
        .expect("a usable child"))
    .expect("a line, a silence or an end");
    match flat.split_first() {
        Some((0, line)) => Heard::Said(String::from_utf8(line.to_vec()).expect("text")),
        Some((1, _)) => Heard::Quiet,
        Some((2, _)) => Heard::Closed,
        _ => panic!("not a `Heard`: {flat:?}"),
    }
}

fn alive(pid: &str) -> bool {
    std::process::Command::new(SH)
        .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
        .status()
        .expect("a shell runs")
        .success()
}

fn sleeper(host: &Arc<ProcessHost>) -> (i64, String) {
    let child = start(
        host,
        "echo $$; exec sleep 600",
        "",
        wired(false, Output::Lines, Output::Discard),
    );
    match heard(host, child, 10_000) {
        Heard::Said(pid) => {
            assert!(alive(&pid), "the child {pid} is running");
            (child, pid)
        }
        other => panic!("the child never said its pid: {other:?}"),
    }
}

#[test]
fn a_started_child_runs_beside_the_program_until_a_wait_hands_it_back() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "sleep 1; printf done; exit 4",
        "",
        wired(false, Output::Keep, Output::Keep),
    );
    assert_eq!(
        wait(&host, child, 0).expect("a wait"),
        None,
        "no time looks once"
    );
    assert_eq!(
        wait(&host, child, 50).expect("a wait"),
        None,
        "the timeout expired while it ran"
    );
    let done = wait(&host, child, -1)
        .expect("a wait")
        .expect("a negative timeout waits for the end");
    assert_eq!(done.ended, Ended::Exited(4));
    assert_eq!(done.out, b"done");
    assert!(done.err.is_empty());
}

#[test]
fn a_terminated_child_ends_by_the_signal() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "exec sleep 60",
        "",
        wired(false, Output::Discard, Output::Discard),
    );
    assert!(signal(&host, child, Signal::Terminate).expect("a live child"));
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("a terminated child ends");
    assert_eq!(done.ended, Ended::Signalled(15));
}

#[test]
fn a_signal_to_a_child_that_has_ended_returns_false() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "exit 0",
        "",
        wired(false, Output::Discard, Output::Discard),
    );
    let until = Instant::now() + Duration::from_secs(10);
    while signal(&host, child, Signal::Hangup).expect("an unspent handle") {
        assert!(Instant::now() < until, "the child never ended");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(wait(&host, child, -1).expect("a wait").is_some());
}

#[test]
fn a_child_fed_over_its_input_responds_with_what_it_read() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "exec cat",
        "",
        wired(true, Output::Keep, Output::Keep),
    );
    assert!(input(&host, child, b"hello\n").expect("a write"));
    assert!(input(&host, child, b"world\n").expect("a write"));
    host.end_input("sh", child, S).expect("an end");
    assert!(
        !input(&host, child, b"late\n").expect("a write"),
        "an input once ended takes nothing"
    );
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("`cat` ends at the end of its input");
    assert_eq!(done.ended, Ended::Exited(0));
    assert_eq!(done.out, b"hello\nworld\n");
}

#[test]
fn a_child_started_without_input_reads_nothing_and_takes_nothing() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "exec cat",
        "",
        wired(false, Output::Keep, Output::Keep),
    );
    assert!(!input(&host, child, b"unread").expect("a write"));
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("`cat` of an empty input ends");
    assert_eq!(done.ended, Ended::Exited(0));
    assert!(done.out.is_empty());
}

#[test]
fn lines_arrive_one_at_a_time_and_the_unread_ones_come_back_at_the_wait() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "printf 'one\\ntwo\\r\\nthree\\nfour'",
        "",
        wired(false, Output::Lines, Output::Keep),
    );
    assert_eq!(heard(&host, child, 10_000), Heard::Said("one".to_string()));
    assert_eq!(
        heard(&host, child, 10_000),
        Heard::Said("two".to_string()),
        "a carriage return and a newline are one ending"
    );
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("it ended");
    assert_eq!(
        done.out, b"three\nfour",
        "the lines no one read, as the child wrote them"
    );
}

#[test]
fn a_quiet_child_is_quiet_until_it_speaks_and_closed_once_it_is_done() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "sleep 1; echo late",
        "",
        wired(false, Output::Lines, Output::Discard),
    );
    assert_eq!(heard(&host, child, 50), Heard::Quiet);
    assert_eq!(heard(&host, child, -1), Heard::Said("late".to_string()));
    assert_eq!(heard(&host, child, -1), Heard::Closed);
    assert_eq!(
        heard(&host, child, 0),
        Heard::Closed,
        "the end is not a one-shot result"
    );
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("it ended");
    assert!(done.out.is_empty(), "every line was read");
}

#[test]
fn both_streams_as_lines_arrive_in_one_order_and_go_back_to_their_own_stream() {
    let host = spawning("sh", SH);
    let read = start(
        &host,
        "echo first; sleep 0.3; echo second 1>&2",
        "",
        wired(false, Output::Lines, Output::Lines),
    );
    assert_eq!(heard(&host, read, 10_000), Heard::Said("first".to_string()));
    assert_eq!(
        heard(&host, read, 10_000),
        Heard::Said("second".to_string())
    );
    assert_eq!(heard(&host, read, 10_000), Heard::Closed);

    let unread = start(
        &host,
        "echo first; echo second 1>&2",
        "",
        wired(false, Output::Lines, Output::Lines),
    );
    let done = wait(&host, unread, 10_000)
        .expect("a wait")
        .expect("it ended");
    assert_eq!(done.out, b"first\n");
    assert_eq!(done.err, b"second\n");
}

#[test]
fn a_child_with_no_lines_to_read_is_closed_at_once() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "exec sleep 60",
        "",
        wired(false, Output::Discard, Output::Keep),
    );
    assert_eq!(heard(&host, child, -1), Heard::Closed);
}

#[test]
fn a_file_output_lands_in_the_childs_directory_and_starts_empty() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(
        dir.path().join("log.txt"),
        b"stale and longer than what replaces it\n",
    )
    .unwrap();
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "echo to-out; echo to-err 1>&2",
        &dir.path().to_string_lossy(),
        wired(false, file("log.txt"), file("log.txt")),
    );
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("it ended");
    assert!(done.out.is_empty() && done.err.is_empty());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("log.txt")).unwrap(),
        "to-out\nto-err\n",
        "one file for both streams is shared, as `2>&1` shares it"
    );
}

#[test]
fn kept_output_past_the_bound_is_drained_and_rejected_at_the_wait() {
    let host = spawning("sh", SH);
    let over = MAX_CAPTURE_BYTES + 1;
    let child = start(
        &host,
        &format!("head -c {over} /dev/zero"),
        "",
        wired(false, Output::Keep, Output::Discard),
    );
    let rejected = wait(&host, child, 60_000).expect_err("more than a kept stream holds");
    assert_eq!(rejected.code, codes::PROCESS_OUTPUT_TOO_LARGE);
    assert!(
        rejected.message.contains(&over.to_string()),
        "{}",
        rejected.message
    );
    let again = wait(&host, child, 0).expect_err("the rejection spent the handle");
    assert_eq!(again.code, codes::RUNTIME_ERROR);
}

#[test]
fn a_spent_handle_and_one_never_returned_are_rejections_rather_than_children() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "exit 0",
        "",
        wired(false, Output::Discard, Output::Discard),
    );
    assert!(wait(&host, child, -1).expect("a wait").is_some());
    for rejected in [
        wait(&host, child, 0).map(|_| ()),
        signal(&host, child, Signal::Kill).map(|_| ()),
        input(&host, child, b"x").map(|_| ()),
    ] {
        let rejected = rejected.expect_err("a spent handle names nothing");
        assert_eq!(rejected.code, codes::RUNTIME_ERROR);
        assert!(
            rejected.message.contains("already been waited on"),
            "{}",
            rejected.message
        );
    }
    let unknown = input(&host, 999, b"x").expect_err("no child has this handle");
    assert_eq!(unknown.code, codes::RUNTIME_ERROR);
    assert!(
        unknown.message.contains("no child has that handle"),
        "{}",
        unknown.message
    );
}

#[test]
fn a_child_is_used_under_the_label_it_was_started_as() {
    let mut executables = Executables::new();
    for label in ["sh", "shell"] {
        executables
            .bind(label, std::path::Path::new(SH), S)
            .expect("a shell is a program");
    }
    let host =
        Arc::new(ProcessHost::new(Vec::new(), OutputSink::captured()).executing(executables));
    let child = start(
        &host,
        "exec sleep 60",
        "",
        wired(false, Output::Discard, Output::Discard),
    );
    let rejected = wait_as(&host, "shell", child, 0).expect_err("this child is `[sh]`");
    assert_eq!(rejected.code, codes::RUNTIME_ERROR);
    assert!(
        rejected.message.contains("[sh]") && rejected.message.contains("[shell]"),
        "both labels are named: {}",
        rejected.message
    );
}

#[test]
fn a_start_of_an_unbound_label_names_the_flag_that_would_bind_it() {
    let host = spawning("sh", SH);
    let rejected = launched(
        &host,
        "cc",
        "true",
        "",
        wired(false, Output::Keep, Output::Keep),
    )
    .expect_err("`cc` is bound to nothing");
    assert_eq!(rejected.code, codes::PROCESS_EXEC_UNBOUND);
    assert!(
        rejected.message.contains("`process.start`"),
        "{}",
        rejected.message
    );
    assert!(
        rejected.notes.iter().any(|n| n.contains("--exec cc=")),
        "{:?}",
        rejected.notes
    );
}

#[test]
fn a_child_that_cannot_start_is_a_result_rather_than_a_diagnostic() {
    let host = spawning("sh", SH);
    let nowhere = launched(
        &host,
        "sh",
        "true",
        "/a/directory/that/is/not/there",
        wired(false, Output::Keep, Output::Keep),
    )
    .expect("a start returns")
    .expect_err("there is no such directory to start in");
    assert!(nowhere.contains("could not be started"), "{nowhere}");
    let unopened = launched(
        &host,
        "sh",
        "true",
        "",
        wired(
            false,
            file("/a/directory/that/is/not/there/log"),
            Output::Keep,
        ),
    )
    .expect("a start returns")
    .expect_err("there is no such directory to write in");
    assert!(unopened.contains("could not be opened"), "{unopened}");
}

#[test]
fn an_inheriting_child_writes_where_the_program_does() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "echo from-child; echo complaint 1>&2",
        "",
        wired(false, Output::Inherit, Output::Inherit),
    );
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("it ended");
    assert!(done.out.is_empty(), "an inherited stream is not kept");
    let mut lines = host.captured();
    lines.sort_by_key(|(stream, _)| matches!(stream, Stream::Err));
    assert_eq!(
        lines,
        [
            (Stream::Out, b"from-child\n".to_vec()),
            (Stream::Err, b"complaint\n".to_vec()),
        ]
    );
}

#[test]
fn a_child_left_running_is_killed_and_reaped_when_the_host_ends_its_children() {
    let host = spawning("sh", SH);
    let (child, pid) = sleeper(&host);
    host.end_children();
    assert!(!alive(&pid), "the child {pid} outlived the end");
    assert_eq!(
        wait(&host, child, 0)
            .expect_err("its handle went with it")
            .code,
        codes::RUNTIME_ERROR
    );
}

#[test]
fn dropping_the_host_kills_and_reaps_the_children_it_holds() {
    let host = spawning("sh", SH);
    let (_, pid) = sleeper(&host);
    drop(host);
    assert!(!alive(&pid), "the child {pid} outlived its host");
}

#[test]
fn the_runs_teardown_ends_the_children_it_leaves() {
    let mut executables = Executables::new();
    executables
        .bind("sh", std::path::Path::new(SH), S)
        .expect("a shell is a program");
    let facilities = ply_host::Host::new()
        .with_process(ProcessHost::new(Vec::new(), OutputSink::captured()).executing(executables));
    let host = facilities.process().expect("a process host").clone();
    let (_, pid) = sleeper(&host);
    let _ = facilities.runtime().shutdown();
    assert!(!alive(&pid), "the child {pid} outlived the run's teardown");
}

#[test]
fn a_park_over_a_child_and_a_socket_waits_for_whichever_finishes() {
    let host = spawning("sh", SH);
    let net = ply_host::tcp::TcpHost::new();
    let listener = net.listen("listener", 0, S).expect("a listen");
    let runtime = the_run();
    let run = ply_host::registry::run_of(&*runtime).expect("a host made the run");
    let accepting = ply_host::jobs::submit(
        ply_host::jobs::Work::Waiting,
        run,
        S,
        "test",
        "`net.accept`",
        net.accept("listener", listener, S).expect("an accept"),
    )
    .expect("a thread is had");
    let child = start(
        &host,
        "sleep 0.5",
        "",
        wired(false, Output::Discard, Output::Discard),
    );
    let waiting = ply_host::jobs::refusing(
        ply_host::jobs::Work::Waiting,
        run,
        S,
        "test",
        "`process.wait`",
        host.wait("sh", child, -1, S).expect("a wait"),
    )
    .expect("a thread is had");

    let run = ply_host::registry::run_of(&*runtime).expect("a host made the runtime");
    run.watch(&waiting).expect("the wait is this run's");
    let parked = Instant::now();
    runtime.park().expect("two operations are outstanding");
    assert!(
        parked.elapsed() >= Duration::from_millis(300),
        "the park returned after {:?}, before either operation finished",
        parked.elapsed()
    );
    assert!(
        run.returned().contains(&waiting.token) && run.taken(&waiting).is_ok(),
        "the child's end is what woke the park"
    );

    let peer = std::net::TcpStream::connect(net.local_addr(listener).expect("a bound port"))
        .expect("a peer connects");
    ply_host::registry::job_bytes(&*runtime, &accepting).expect("the accept takes the peer");
    drop(peer);
}

use ply_eval::host::MachineId;
use ply_eval::{
    Determinism, Diagnostic, EffectAtom, Fields, HostAnswer, HostHandler, HostOp, HostRequest,
    HostRuntime, Linearity, Mode, Resource, Span, Symbol, Value, codes,
};
use ply_host::pool::Pooled;
use ply_host::process::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Nothing;

impl HostRuntime for Nothing {
    fn watch(&self, _: &ply_eval::Pending) -> Result<(), Diagnostic> {
        Ok(())
    }
    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }
    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }
    fn block_on(&self, _: ply_eval::Pending) -> Result<Value, Diagnostic> {
        Ok(Value::Unit)
    }
}

fn host(argv: &[&str]) -> Arc<ProcessHost> {
    Arc::new(ProcessHost::new(
        argv.iter().map(|a| (*a).to_string()).collect(),
        OutputSink::captured(),
    ))
}

fn atom(op: Op) -> EffectAtom {
    let mode = if matches!(op, Op::Args | Op::Bound) {
        Mode::Read
    } else {
        Mode::Write
    };
    EffectAtom::new(
        Symbol::new(EFFECT),
        Resource::Named(Symbol::new("proc")),
        mode,
    )
}

fn call(
    handlers: &[(HostOp, Arc<dyn HostHandler>)],
    op: Op,
    args: &[Value],
) -> Result<Value, Diagnostic> {
    let (declaration, handler) = handlers
        .iter()
        .find(|(d, _)| d.op.as_str() == op.name())
        .expect("every operation is registered");
    handler
        .call(
            &Nothing,
            &HostRequest {
                atom: atom(op),
                op: declaration,
                args,
                span: Span::DUMMY,
                machine: MachineId(0),
                task: None,
                declared: None,
            },
        )
        .map(|answer| match answer {
            HostAnswer::Value(v) => v,
            HostAnswer::Pending(_) => panic!("a process operation waits on nothing"),
        })
}

fn value(answer: Result<Value, Diagnostic>) -> Value {
    match answer {
        Ok(v) => v,
        Err(d) => panic!("refused: {} {}", d.code, d.message),
    }
}

fn text(s: &str) -> Value {
    Value::Str(s.into())
}

#[test]
fn the_registrations_declare_what_a_reviewer_relies_on() {
    let host = host(&[]);
    let handlers = registrations(Some(&host));
    assert_eq!(handlers.len(), Op::ALL.len());
    for (op, _) in &handlers {
        assert_eq!(op.effect.as_str(), EFFECT);
        assert_eq!(op.determinism, Determinism::Nondeterministic);
        // Each of these waits outside this process, on another one, on a person or on a pipe a
        // child drains at its own pace, so each goes to the pool rather than parking the machine.
        let waits = matches!(
            op.op.as_str(),
            "spawn" | "line" | "in_bytes" | "wait" | "input" | "output_line"
        );
        assert_eq!(op.blocking, waits, "{op}");
        assert!(!op.secrets, "a line or a code is never a credential");
        assert!(op.path.starts_with("ply_host::process::"));
        // Neither the arguments nor the executables a run was given change while it runs.
        let expected = if matches!(op.op.as_str(), "args" | "bound") {
            Linearity::Repeatable
        } else {
            Linearity::AtMostOnce
        };
        assert_eq!(op.linearity, expected, "{op}");
    }
    let declaration =
        ply_machine::shipped_modules::source(&ply_eval::ModuleName::from_dotted("std.process"))
            .expect("std.process ships");
    assert!(declaration.contains("pub nondet effect process"));
    for op in Op::ALL {
        // The label is the process for its own streams, and the executable for a child or `bound`.
        let executes = matches!(
            op,
            Op::Bound
                | Op::Spawn
                | Op::Start
                | Op::Wait
                | Op::Signal
                | Op::Input
                | Op::EndInput
                | Op::OutputLine
        );
        let label = if executes { "e" } else { "p" };
        assert!(
            declaration.contains(&format!(" {}[{label}]", op.name())),
            "`{}` is not declared in std.process",
            op.name()
        );
    }
}

#[test]
fn args_answers_the_vector_it_was_given() {
    let host = host(&["a", "b c"]);
    let handlers = registrations(Some(&host));
    assert_eq!(
        value(call(&handlers, Op::Args, &[])),
        Value::list(vec![text("a"), text("b c")])
    );
    assert_eq!(
        value(call(&handlers, Op::Args, &[])),
        value(call(&handlers, Op::Args, &[]))
    );
    assert_eq!(host.argv(), ["a", "b c"]);
}

#[test]
fn a_captured_sink_collects_both_streams_in_order() {
    let host = host(&[]);
    let handlers = registrations(Some(&host));
    assert_eq!(value(call(&handlers, Op::Out, &[text("one")])), Value::Unit);
    assert_eq!(value(call(&handlers, Op::Err, &[text("two")])), Value::Unit);
    assert_eq!(
        value(call(&handlers, Op::Out, &[text("three")])),
        Value::Unit
    );
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
    let handlers = registrations(Some(&host));
    let raw = Value::bytes(b"\x00\xff no newline");
    assert_eq!(
        value(call(&handlers, Op::OutBytes, std::slice::from_ref(&raw))),
        Value::Bool(true)
    );
    assert_eq!(
        value(call(&handlers, Op::Err, &[text("warn")])),
        Value::Unit
    );
    assert_eq!(
        value(call(&handlers, Op::ErrBytes, &[raw])),
        Value::Bool(true)
    );
    assert_eq!(
        value(call(&handlers, Op::Out, &[text("done")])),
        Value::Unit
    );
    assert_eq!(value(call(&handlers, Op::Flush, &[])), Value::Bool(true));
    assert_eq!(
        host.captured(),
        [
            (Stream::Out, b"\x00\xff no newline".to_vec()),
            (Stream::Err, b"warn\n".to_vec()),
            (Stream::Err, b"\x00\xff no newline".to_vec()),
            (Stream::Out, b"done\n".to_vec()),
        ]
    );
}

/// Only a host that writes the process's own streams ends the process over them.
#[test]
fn a_host_whose_streams_are_captured_leaves_an_unanswered_raise_to_the_run() {
    let host = host(&[]);
    host.unanswered(&Symbol::new(PIPE_EFFECT), &Symbol::new(PIPE_BROKEN));
    host.settle();
    let declaration =
        ply_machine::shipped_modules::source(&ply_eval::ModuleName::from_dotted("std.process"))
            .expect("std.process ships");
    assert!(declaration.contains("pub effect pipe {"));
    assert!(declaration.contains("  raise broken(stream: Stream)"));
    assert_eq!(PIPE_EFFECT, format!("{MODULE}.pipe"));
}

#[test]
fn a_read_of_a_negative_length_is_refused_before_anything_waits() {
    let host = host(&[]);
    let handlers = registrations(Some(&host));
    let refused = call(&handlers, Op::InBytes, &[Value::Int(-1)]).expect_err("no such read");
    assert_eq!(refused.code, codes::RUNTIME_ERROR);
    assert!(refused.message.contains("-1"), "{}", refused.message);
}

#[test]
fn exit_records_the_first_code_and_unwinds() {
    let host = host(&[]);
    let handlers = registrations(Some(&host));
    let refused = call(&handlers, Op::Exit, &[Value::Int(3)]).expect_err("an exit never resumes");
    assert_eq!(refused.code, codes::PROCESS_EXIT);
    assert_eq!(host.requested_exit(), Some(3));
    let again = call(&handlers, Op::Exit, &[Value::Int(4)]).expect_err("an exit never resumes");
    assert_eq!(again.code, codes::PROCESS_EXIT);
    assert_eq!(
        host.requested_exit(),
        Some(3),
        "the first code is the one kept"
    );
}

#[test]
fn exit_refuses_a_code_the_shell_could_not_carry() {
    let host = host(&[]);
    let handlers = registrations(Some(&host));
    for code in [-1, 126, 256] {
        let refused = call(&handlers, Op::Exit, &[Value::Int(code)]).expect_err("out of range");
        assert_eq!(refused.code, codes::RUNTIME_ERROR, "{code}");
        assert!(refused.message.contains("0 to 125"), "{}", refused.message);
    }
    assert_eq!(host.requested_exit(), None);
    for code in [0, 125] {
        let asked =
            call(&handlers, Op::Exit, &[Value::Int(code)]).expect_err("an exit never resumes");
        assert_eq!(asked.code, codes::PROCESS_EXIT, "{code}");
    }
    assert_eq!(host.requested_exit(), Some(0));
}

#[test]
fn an_argument_of_the_wrong_type_is_the_handlers_refusal() {
    let host = host(&[]);
    let handlers = registrations(Some(&host));
    assert!(call(&handlers, Op::Out, &[Value::Int(1)]).is_err());
    assert!(call(&handlers, Op::Exit, &[text("1")]).is_err());
    assert!(host.captured().is_empty());
    assert_eq!(host.requested_exit(), None);
}

// Arity is inference's, so the wrong count means the module was never checked.
#[test]
fn the_wrong_arity_is_a_dispatch_defect() {
    let host = host(&[]);
    let handlers = registrations(Some(&host));
    let refused = call(&handlers, Op::Args, &[Value::Unit]).expect_err("args takes nothing");
    assert_eq!(refused.code, codes::INTERNAL_ERROR);
    let refused = call(&handlers, Op::Out, &[]).expect_err("out takes a line");
    assert_eq!(refused.code, codes::INTERNAL_ERROR);
}

#[test]
fn a_withheld_registration_refuses_dispatch() {
    let handlers = registrations(None);
    let refused = call(&handlers, Op::Args, &[]).expect_err("nothing answers without a host");
    assert_eq!(refused.code, codes::INTERNAL_ERROR);
}

// --- Spawning ---------------------------------------------------------------

const SH: &str = "/bin/sh";

/// Enough for a shell to find nothing it needs outside its own builtins.
const PATH: (&str, &str) = ("PATH", "/usr/bin:/bin");

fn spawning(label: &str, program: &str) -> Arc<ProcessHost> {
    let mut executables = Executables::new();
    executables
        .bind(label, std::path::Path::new(program), Span::DUMMY)
        .expect("the program binds");
    Arc::new(ProcessHost::new(Vec::new(), OutputSink::captured()).executing(executables))
}

fn variable(name: &str, value: &str) -> Value {
    Value::Record(Arc::new(Fields::from_iter([
        (Symbol::new("name"), text(name)),
        (Symbol::new("value"), text(value)),
    ])))
}

fn spawn(
    host: &Arc<ProcessHost>,
    label: &str,
    args: &[&str],
    dir: &str,
    env: &[(&str, &str)],
) -> Result<Value, Diagnostic> {
    let handlers = registrations(Some(host));
    let (declaration, handler) = handlers
        .iter()
        .find(|(d, _)| d.op.as_str() == "spawn")
        .expect("spawn is registered");
    let arguments = [
        Value::list(args.iter().map(|a| text(a)).collect()),
        text(dir),
        Value::list(env.iter().map(|(n, v)| variable(n, v)).collect()),
    ];
    let answer = handler.call(
        &Nothing,
        &HostRequest {
            atom: EffectAtom::new(
                Symbol::new(EFFECT),
                Resource::Named(Symbol::new(label)),
                Mode::Write,
            ),
            op: declaration,
            args: &arguments,
            span: Span::DUMMY,
            machine: MachineId(0),
            task: None,
            declared: None,
        },
    )?;
    match answer {
        HostAnswer::Pending(pending) => host.pool().block_on(pending),
        // `blocking` is declared, so answering inline would be `E0428` in a real run.
        HostAnswer::Value(v) => panic!("a spawn waits in the pool: {}", v.type_name()),
    }
}

fn field<'a>(record: &'a Value, name: &str) -> &'a Value {
    match record {
        Value::Record(fields) => fields
            .get(&Symbol::new(name))
            .unwrap_or_else(|| panic!("no field `{name}`")),
        other => panic!("not a record: {}", other.type_name()),
    }
}

/// The constructor's program-wide name and the number it carries.
fn ending(record: &Value) -> (String, i64) {
    match field(record, "ended") {
        Value::Ctor { name, args } => (
            name.to_string(),
            args[0].as_int(Span::DUMMY, "a code").expect("an Int"),
        ),
        other => panic!("not a variant: {}", other.type_name()),
    }
}

fn bytes(record: &Value, name: &str) -> Vec<u8> {
    match field(record, name) {
        Value::Bytes(b) => b.to_vec(),
        other => panic!("not Bytes: {}", other.type_name()),
    }
}

#[test]
fn a_spawn_answers_with_the_code_and_everything_both_streams_took() {
    let host = spawning("sh", SH);
    let done = value(spawn(
        &host,
        "sh",
        &["-c", "printf built; printf 'warning' 1>&2; exit 3"],
        "",
        &[PATH],
    ));
    assert_eq!(ending(&done), ("std.process.Exited".to_string(), 3));
    assert_eq!(bytes(&done, "out").as_slice(), b"built");
    assert_eq!(bytes(&done, "err").as_slice(), b"warning");
}

#[test]
fn a_signal_is_an_ending_of_its_own_rather_than_a_code() {
    let host = spawning("sh", SH);
    let done = value(spawn(&host, "sh", &["-c", "kill -9 $$"], "", &[PATH]));
    assert_eq!(ending(&done), ("std.process.Signalled".to_string(), 9));
}

/// The run's own environment is the classic hidden input, so a spawn inherits none of it.
#[test]
fn a_spawn_sees_only_the_environment_it_was_given() {
    // nextest runs one test per process, so this environment is this test's alone.
    unsafe { std::env::set_var("PLY_SPAWN_WITNESS", "leaked") };
    let host = spawning("sh", SH);
    let done = value(spawn(
        &host,
        "sh",
        &[
            "-c",
            r#"printf '%s-%s' "${PLY_UNIT-unset}" "${PLY_SPAWN_WITNESS-unset}""#,
        ],
        "",
        &[("PLY_UNIT", "lexer")],
    ));
    assert_eq!(bytes(&done, "out").as_slice(), b"lexer-unset");
}

#[test]
fn a_spawn_runs_in_the_directory_it_was_given() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(dir.path().join("marker"), b"here\n").unwrap();
    let host = spawning("sh", SH);
    let done = value(spawn(
        &host,
        "sh",
        &["-c", "read line < marker; printf %s \"$line\""],
        &dir.path().to_string_lossy(),
        &[PATH],
    ));
    assert_eq!(ending(&done), ("std.process.Exited".to_string(), 0));
    assert_eq!(bytes(&done, "out").as_slice(), b"here");
}

/// The label is the capability: without one bound, nothing about the call decides what runs.
#[test]
fn a_spawn_of_an_unbound_label_names_the_flag_that_would_bind_it() {
    let host = spawning("sh", SH);
    let refused =
        spawn(&host, "cc", &["-c", "true"], "", &[]).expect_err("`cc` is bound to nothing");
    assert_eq!(refused.code, codes::PROCESS_EXEC_UNBOUND);
    assert!(
        refused.notes.iter().any(|n| n.contains("--exec cc=")),
        "the diagnostic should name the flag: {:?}",
        refused.notes
    );
    assert!(
        refused
            .notes
            .iter()
            .any(|n| n.contains("process.bound[cc]()")),
        "the diagnostic should name the question a program can ask first: {:?}",
        refused.notes
    );
    // And the same refusal is what the shared constructor produces for any label.
    let direct = unbound(Op::Spawn, &Resource::Named(Symbol::new("cc")), Span::DUMMY);
    assert_eq!(direct.code, codes::PROCESS_EXEC_UNBOUND);
}

/// The table the run was given is the whole answer, and asking again reads the same one.
#[test]
fn bound_answers_whether_the_run_bound_a_program_to_the_label() {
    let shell = spawning("sh", SH);
    for _ in 0..2 {
        assert_eq!(
            perform(&shell, Op::Bound, "sh", Vec::new()).expect("an answer"),
            Value::Bool(true)
        );
        assert_eq!(
            perform(&shell, Op::Bound, "cc", Vec::new()).expect("an answer"),
            Value::Bool(false)
        );
    }
    let bare = host(&[]);
    assert_eq!(
        perform(&bare, Op::Bound, "sh", Vec::new()).expect("an answer"),
        Value::Bool(false),
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
        .bind("cc", &dir.path().join("absent"), Span::DUMMY)
        .expect_err("a program that is not there");
    assert_eq!(missing.code, codes::PROCESS_EXEC_INVALID);

    let directory = executables
        .bind("cc", dir.path(), Span::DUMMY)
        .expect_err("a directory is not a program");
    assert_eq!(directory.code, codes::PROCESS_EXEC_INVALID);

    let unreadable = executables
        .bind("cc", &plain, Span::DUMMY)
        .expect_err("a file with no execute bit");
    assert_eq!(unreadable.code, codes::PROCESS_EXEC_INVALID);
    assert!(executables.is_empty());

    executables
        .bind("sh", std::path::Path::new(SH), Span::DUMMY)
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

#[test]
fn a_spawn_with_an_environment_entry_that_is_not_one_is_a_dispatch_defect() {
    let host = spawning("sh", SH);
    let handlers = registrations(Some(&host));
    let (declaration, handler) = handlers
        .iter()
        .find(|(d, _)| d.op.as_str() == "spawn")
        .expect("spawn is registered");
    let arguments = [
        Value::list(vec![text("-c"), text("true")]),
        text(""),
        Value::list(vec![Value::Int(1)]),
    ];
    let answer = handler.call(
        &Nothing,
        &HostRequest {
            atom: EffectAtom::new(
                Symbol::new(EFFECT),
                Resource::Named(Symbol::new("sh")),
                Mode::Write,
            ),
            op: declaration,
            args: &arguments,
            span: Span::DUMMY,
            machine: MachineId(0),
            task: None,
            declared: None,
        },
    );
    match answer {
        Ok(_) => panic!("an Int is not an environment entry"),
        Err(refused) => assert_eq!(refused.code, codes::INTERNAL_ERROR),
    }
}

// --- Children ---------------------------------------------------------------

fn answer(
    host: &Arc<ProcessHost>,
    op: Op,
    label: &str,
    args: &[Value],
) -> Result<HostAnswer, Diagnostic> {
    let handlers = registrations(Some(host));
    let (declaration, handler) = handlers
        .iter()
        .find(|(d, _)| d.op.as_str() == op.name())
        .expect("every operation is registered");
    handler.call(
        &Nothing,
        &HostRequest {
            atom: EffectAtom::new(
                Symbol::new(EFFECT),
                Resource::Named(Symbol::new(label)),
                Mode::Write,
            ),
            op: declaration,
            args,
            span: Span::DUMMY,
            machine: MachineId(0),
            task: None,
            declared: None,
        },
    )
}

/// One perform, answered as the declaration says it is: inline, or by waiting on the pool.
fn perform(
    host: &Arc<ProcessHost>,
    op: Op,
    label: &str,
    args: Vec<Value>,
) -> Result<Value, Diagnostic> {
    let blocking = op.declaration().blocking;
    match answer(host, op, label, &args)? {
        HostAnswer::Pending(pending) => {
            assert!(blocking, "{op:?} waited and is not declared to");
            host.pool().block_on(pending)
        }
        HostAnswer::Value(v) => {
            assert!(!blocking, "{op:?} is declared to wait and did not");
            Ok(v)
        }
    }
}

fn constructor(name: &str, args: Vec<Value>) -> Value {
    Value::ctor(format!("std.process.{name}"), args)
}

fn to(output: &str) -> Value {
    constructor(output, Vec::new())
}

fn file(path: &str) -> Value {
    constructor("File", vec![text(path)])
}

fn wired(input: bool, out: Value, err: Value) -> Value {
    Value::Record(Arc::new(Fields::from_iter([
        (Symbol::new("input"), Value::Bool(input)),
        (Symbol::new("out"), out),
        (Symbol::new("err"), err),
    ])))
}

/// What `start` answered: the handle, or why the child could not start.
fn launched(
    host: &Arc<ProcessHost>,
    label: &str,
    script: &str,
    dir: &str,
    io: Value,
) -> Result<i64, String> {
    let answer = perform(
        host,
        Op::Start,
        label,
        vec![
            Value::list(vec![text("-c"), text(script)]),
            text(dir),
            Value::list(vec![variable(PATH.0, PATH.1)]),
            io,
        ],
    )
    .expect("a start answers");
    match &answer {
        Value::Ctor { name, args } if name.as_str() == "Ok" => {
            Ok(args[0].as_int(Span::DUMMY, "a handle").expect("an Int"))
        }
        Value::Ctor { name, args } if name.as_str() == "Err" => Err(args[0]
            .as_str(Span::DUMMY, "a reason")
            .expect("a String")
            .to_string()),
        other => panic!("not a `Result`: {other:?}"),
    }
}

/// `sh -c script` under `[sh]`, running.
fn start(host: &Arc<ProcessHost>, script: &str, dir: &str, io: Value) -> i64 {
    launched(host, "sh", script, dir, io).unwrap_or_else(|why| panic!("not started: {why}"))
}

fn wait(host: &Arc<ProcessHost>, child: i64, timeout_ms: i64) -> Result<Option<Value>, Diagnostic> {
    perform(
        host,
        Op::Wait,
        "sh",
        vec![Value::Int(child), Value::Int(timeout_ms)],
    )
    .map(|answer| match &answer {
        Value::Ctor { name, args } if name.as_str() == "Some" => Some(args[0].clone()),
        Value::Ctor { name, .. } if name.as_str() == "None" => None,
        other => panic!("not an `Option`: {other:?}"),
    })
}

fn signal(host: &Arc<ProcessHost>, child: i64, which: &str) -> Result<Value, Diagnostic> {
    perform(
        host,
        Op::Signal,
        "sh",
        vec![Value::Int(child), constructor(which, Vec::new())],
    )
}

fn input(host: &Arc<ProcessHost>, child: i64, bytes: &[u8]) -> Result<Value, Diagnostic> {
    perform(
        host,
        Op::Input,
        "sh",
        vec![Value::Int(child), Value::bytes(bytes)],
    )
}

fn heard(host: &Arc<ProcessHost>, child: i64, timeout_ms: i64) -> Heard {
    let answer = perform(
        host,
        Op::OutputLine,
        "sh",
        vec![Value::Int(child), Value::Int(timeout_ms)],
    )
    .expect("a line, a silence or an end");
    match &answer {
        Value::Ctor { name, args } if name.as_str() == "std.process.Said" => Heard::Said(
            args[0]
                .as_str(Span::DUMMY, "a line")
                .expect("a String")
                .to_string(),
        ),
        Value::Ctor { name, .. } if name.as_str() == "std.process.Quiet" => Heard::Quiet,
        Value::Ctor { name, .. } if name.as_str() == "std.process.Closed" => Heard::Closed,
        other => panic!("not a `Heard`: {other:?}"),
    }
}

/// Whether any process has this pid, as `kill -0` asks.
fn alive(pid: &str) -> bool {
    std::process::Command::new(SH)
        .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
        .status()
        .expect("a shell runs")
        .success()
}

/// A child that says its own pid, then sleeps as that pid for longer than any test runs.
fn sleeper(host: &Arc<ProcessHost>) -> (i64, String) {
    let child = start(
        host,
        "echo $$; exec sleep 600",
        "",
        wired(false, to("Lines"), to("Discard")),
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
        wired(false, to("Keep"), to("Keep")),
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
    assert_eq!(ending(&done), ("std.process.Exited".to_string(), 4));
    assert_eq!(bytes(&done, "out").as_slice(), b"done");
    assert!(bytes(&done, "err").is_empty());
}

#[test]
fn a_terminated_child_ends_by_the_signal() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "exec sleep 60",
        "",
        wired(false, to("Discard"), to("Discard")),
    );
    assert_eq!(
        signal(&host, child, "Terminate").expect("a live child"),
        Value::Bool(true)
    );
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("a terminated child ends");
    assert_eq!(ending(&done), ("std.process.Signalled".to_string(), 15));
}

#[test]
fn a_signal_to_a_child_that_has_ended_answers_false() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "exit 0",
        "",
        wired(false, to("Discard"), to("Discard")),
    );
    // A signal that lands before the child is gone still reaches it; the answer turns once it is.
    let until = Instant::now() + Duration::from_secs(10);
    while signal(&host, child, "Hangup").expect("an unspent handle") != Value::Bool(false) {
        assert!(Instant::now() < until, "the child never ended");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(wait(&host, child, -1).expect("a wait").is_some());
}

#[test]
fn a_child_fed_over_its_input_answers_what_it_read() {
    let host = spawning("sh", SH);
    let child = start(&host, "exec cat", "", wired(true, to("Keep"), to("Keep")));
    assert_eq!(
        input(&host, child, b"hello\n").expect("a write"),
        Value::Bool(true)
    );
    assert_eq!(
        input(&host, child, b"world\n").expect("a write"),
        Value::Bool(true)
    );
    assert_eq!(
        perform(&host, Op::EndInput, "sh", vec![Value::Int(child)]).expect("an end"),
        Value::Unit
    );
    assert_eq!(
        input(&host, child, b"late\n").expect("a write"),
        Value::Bool(false),
        "an input once ended takes nothing"
    );
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("`cat` ends at the end of its input");
    assert_eq!(ending(&done), ("std.process.Exited".to_string(), 0));
    assert_eq!(bytes(&done, "out").as_slice(), b"hello\nworld\n");
}

#[test]
fn a_child_started_without_input_reads_nothing_and_takes_nothing() {
    let host = spawning("sh", SH);
    let child = start(&host, "exec cat", "", wired(false, to("Keep"), to("Keep")));
    assert_eq!(
        input(&host, child, b"unread").expect("a write"),
        Value::Bool(false)
    );
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("`cat` of an empty input ends");
    assert_eq!(ending(&done), ("std.process.Exited".to_string(), 0));
    assert!(bytes(&done, "out").is_empty());
}

#[test]
fn lines_arrive_one_at_a_time_and_the_unread_ones_come_back_at_the_wait() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "printf 'one\\ntwo\\r\\nthree\\nfour'",
        "",
        wired(false, to("Lines"), to("Keep")),
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
        bytes(&done, "out").as_slice(),
        b"three\nfour",
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
        wired(false, to("Lines"), to("Discard")),
    );
    assert_eq!(heard(&host, child, 50), Heard::Quiet);
    assert_eq!(heard(&host, child, -1), Heard::Said("late".to_string()));
    assert_eq!(heard(&host, child, -1), Heard::Closed);
    assert_eq!(
        heard(&host, child, 0),
        Heard::Closed,
        "the end is not a one-shot answer"
    );
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("it ended");
    assert!(bytes(&done, "out").is_empty(), "every line was read");
}

#[test]
fn both_streams_as_lines_arrive_in_one_order_and_go_back_to_their_own_stream() {
    let host = spawning("sh", SH);
    let read = start(
        &host,
        "echo first; sleep 0.3; echo second 1>&2",
        "",
        wired(false, to("Lines"), to("Lines")),
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
        wired(false, to("Lines"), to("Lines")),
    );
    let done = wait(&host, unread, 10_000)
        .expect("a wait")
        .expect("it ended");
    assert_eq!(bytes(&done, "out").as_slice(), b"first\n");
    assert_eq!(bytes(&done, "err").as_slice(), b"second\n");
}

#[test]
fn a_child_with_no_lines_to_read_is_closed_at_once() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "exec sleep 60",
        "",
        wired(false, to("Discard"), to("Keep")),
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
    assert!(bytes(&done, "out").is_empty() && bytes(&done, "err").is_empty());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("log.txt")).unwrap(),
        "to-out\nto-err\n",
        "one file for both streams is shared, as `2>&1` shares it"
    );
}

#[test]
fn kept_output_past_the_bound_is_drained_and_refused_at_the_wait() {
    let host = spawning("sh", SH);
    let over = MAX_CAPTURE_BYTES + 1;
    let child = start(
        &host,
        &format!("head -c {over} /dev/zero"),
        "",
        wired(false, to("Keep"), to("Discard")),
    );
    // The child finishes rather than blocking on a full pipe, and the refusal comes at the wait.
    let refused = wait(&host, child, 60_000).expect_err("more than a kept stream holds");
    assert_eq!(refused.code, codes::PROCESS_OUTPUT_TOO_LARGE);
    assert!(
        refused.message.contains(&over.to_string()),
        "{}",
        refused.message
    );
    let again = wait(&host, child, 0).expect_err("the refusal spent the handle");
    assert_eq!(again.code, codes::RUNTIME_ERROR);
}

#[test]
fn a_spent_handle_and_one_never_answered_are_refusals_rather_than_children() {
    let host = spawning("sh", SH);
    let child = start(
        &host,
        "exit 0",
        "",
        wired(false, to("Discard"), to("Discard")),
    );
    assert!(wait(&host, child, -1).expect("a wait").is_some());
    for refused in [
        wait(&host, child, 0).map(|_| Value::Unit),
        signal(&host, child, "Kill"),
        input(&host, child, b"x"),
    ] {
        let refused = refused.expect_err("a spent handle names nothing");
        assert_eq!(refused.code, codes::RUNTIME_ERROR);
        assert!(
            refused.message.contains("already been waited on"),
            "{}",
            refused.message
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
            .bind(label, std::path::Path::new(SH), Span::DUMMY)
            .expect("a shell is a program");
    }
    let host =
        Arc::new(ProcessHost::new(Vec::new(), OutputSink::captured()).executing(executables));
    let child = start(
        &host,
        "exec sleep 60",
        "",
        wired(false, to("Discard"), to("Discard")),
    );
    let refused = perform(
        &host,
        Op::Wait,
        "shell",
        vec![Value::Int(child), Value::Int(0)],
    )
    .expect_err("this child is `[sh]`");
    assert_eq!(refused.code, codes::RUNTIME_ERROR);
    assert!(
        refused.message.contains("[sh]") && refused.message.contains("[shell]"),
        "both labels are named: {}",
        refused.message
    );
}

#[test]
fn a_start_of_an_unbound_label_names_the_flag_that_would_bind_it() {
    let host = spawning("sh", SH);
    let refused = perform(
        &host,
        Op::Start,
        "cc",
        vec![
            Value::list(Vec::new()),
            text(""),
            Value::list(Vec::new()),
            wired(false, to("Keep"), to("Keep")),
        ],
    )
    .expect_err("`cc` is bound to nothing");
    assert_eq!(refused.code, codes::PROCESS_EXEC_UNBOUND);
    assert!(
        refused.message.contains("`process.start`"),
        "{}",
        refused.message
    );
    assert!(
        refused.notes.iter().any(|n| n.contains("--exec cc=")),
        "{:?}",
        refused.notes
    );
}

#[test]
fn a_child_that_cannot_start_is_an_answer_rather_than_a_diagnostic() {
    let host = spawning("sh", SH);
    let nowhere = launched(
        &host,
        "sh",
        "true",
        "/a/directory/that/is/not/there",
        wired(false, to("Keep"), to("Keep")),
    )
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
            to("Keep"),
        ),
    )
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
        wired(false, to("Inherit"), to("Inherit")),
    );
    let done = wait(&host, child, 10_000)
        .expect("a wait")
        .expect("it ended");
    assert!(
        bytes(&done, "out").is_empty(),
        "an inherited stream is not kept"
    );
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
        .bind("sh", std::path::Path::new(SH), Span::DUMMY)
        .expect("a shell is a program");
    let facilities = ply_host::Host::new()
        .with_process(ProcessHost::new(Vec::new(), OutputSink::captured()).executing(executables));
    let host = facilities.process().expect("a process host").clone();
    let (_, pid) = sleeper(&host);
    let _ = facilities.runtime().shutdown();
    assert!(!alive(&pid), "the child {pid} outlived the run's teardown");
}

/// A park over two facilities' pools waits for either to finish something, rather than returning
/// with nothing done and leaving the scheduler to count it as a fruitless park.
#[test]
fn a_park_over_a_child_and_a_socket_waits_for_whichever_finishes() {
    use ply_host::tcp::Net;
    let mut executables = Executables::new();
    executables
        .bind("sh", std::path::Path::new(SH), Span::DUMMY)
        .expect("a shell is a program");
    let facilities = ply_host::Host::new()
        .with_process(ProcessHost::new(Vec::new(), OutputSink::captured()).executing(executables));
    let host = facilities.process().expect("a process host").clone();
    let net = facilities.net();
    let at = Resource::Named(Symbol::new("listener"));
    let listener = match net.listen(&at, 0, Span::DUMMY).expect("a listen") {
        HostAnswer::Value(handle) => handle.as_int(Span::DUMMY, "a handle").expect("an Int"),
        HostAnswer::Pending(_) => panic!("a listen answers at once"),
    };
    let accepting = match net.accept(&at, listener, Span::DUMMY).expect("an accept") {
        HostAnswer::Pending(pending) => pending,
        HostAnswer::Value(_) => panic!("an accept waits for a peer"),
    };
    let child = start(
        &host,
        "sleep 0.5",
        "",
        wired(false, to("Discard"), to("Discard")),
    );
    let waiting = match answer(&host, Op::Wait, "sh", &[Value::Int(child), Value::Int(-1)])
        .expect("a wait")
    {
        HostAnswer::Pending(pending) => pending,
        HostAnswer::Value(_) => panic!("a wait waits in the pool"),
    };

    let runtime = facilities.runtime();
    runtime.watch(&waiting).expect("the wait is this host's");
    let parked = Instant::now();
    runtime.park().expect("two operations are outstanding");
    assert!(
        parked.elapsed() >= Duration::from_millis(300),
        "the park returned after {:?}, before either operation finished",
        parked.elapsed()
    );
    assert!(
        runtime
            .resolved()
            .iter()
            .any(|(token, answer)| *token == waiting.token && answer.is_ok()),
        "the child's end is what woke the park"
    );

    let peer = std::net::TcpStream::connect(net.local_addr(listener).expect("a bound port"))
        .expect("a peer connects");
    runtime
        .block_on(accepting)
        .expect("the accept takes the peer");
    drop(peer);
}

use ply_eval::host::MachineId;
use ply_eval::{
    Determinism, Fields, HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, Linearity,
    Value,
};
use ply_host::process::*;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_ty::{EffectAtom, Mode, Resource};
use std::sync::Arc;

struct Nothing;

impl HostRuntime for Nothing {
    fn poll(&self, _: &ply_eval::Pending) -> Result<Option<Value>, Diagnostic> {
        Ok(None)
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
        Sink::captured(),
    ))
}

fn atom(op: Op) -> EffectAtom {
    let mode = if op == Op::Args {
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
        // Only a spawn waits for something outside this process.
        assert_eq!(op.blocking, op.op.as_str() == "spawn", "{op}");
        assert!(!op.secrets, "a line or a code is never a credential");
        assert!(op.path.starts_with("ply_host::process::"));
        let expected = if op.op.as_str() == "args" {
            Linearity::Repeatable
        } else {
            Linearity::AtMostOnce
        };
        assert_eq!(op.linearity, expected, "{op}");
    }
    assert!(DECLARATION.contains("pub nondet effect process"));
    for op in Op::ALL {
        // The label is the process for its own streams, and the executable for a spawn.
        let label = if op == Op::Spawn { "e" } else { "p" };
        assert!(
            DECLARATION.contains(&format!(" {}[{label}]", op.name())),
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
            (Stream::Out, "one".to_string()),
            (Stream::Err, "two".to_string()),
            (Stream::Out, "three".to_string()),
        ]
    );
    assert_eq!(host.requested_exit(), None);
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
    Arc::new(ProcessHost::new(Vec::new(), Sink::captured()).executing(executables))
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
        HostAnswer::Pending(pending) => host.block_on(pending),
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
    // And the same refusal is what the shared constructor produces for any label.
    let direct = unbound(&Resource::Named(Symbol::new("cc")), Span::DUMMY);
    assert_eq!(direct.code, codes::PROCESS_EXEC_UNBOUND);
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

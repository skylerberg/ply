use ply_eval::host::{HostBinding, MachineId};
use ply_eval::{
    Determinism, Diagnostic, EffectAtom, HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime,
    Linearity, Mode, Resource, Span, Symbol, Value, codes,
};
use ply_host::process::{Executables, OutputSink, ProcessHost};
use ply_host::term::*;
use std::io::IsTerminal;
use std::sync::Arc;

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

fn host() -> Arc<ProcessHost> {
    Arc::new(ProcessHost::new(Vec::new(), OutputSink::captured()))
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
    let mode = if matches!(op, Op::IsTerminal | Op::Size) {
        Mode::Read
    } else {
        Mode::Write
    };
    handler
        .call(
            &Nothing,
            &HostRequest {
                atom: EffectAtom::new(Symbol::new(EFFECT), Resource::Singleton, mode),
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
            HostAnswer::Pending(_) => panic!("this operation waits on nothing"),
        })
}

fn stream(simple: &str) -> Value {
    Value::ctor(format!("std.process.{simple}"), Vec::new())
}

#[test]
fn the_registrations_declare_what_a_reviewer_relies_on() {
    let host = host();
    let handlers = registrations(Some(&host));
    assert_eq!(handlers.len(), Op::ALL.len());
    let declaration =
        ply_machine::shipped_modules::source(&ply_eval::ModuleName::from_dotted("std.term"))
            .expect("std.term ships");
    assert!(declaration.contains("pub nondet effect term"));
    for (op, _) in &handlers {
        assert_eq!(op.effect.as_str(), EFFECT);
        assert_eq!(op.determinism, Determinism::Nondeterministic);
        // Each of these waits on a person, so each goes to the pool.
        let waits = matches!(op.op.as_str(), "input" | "secret_line");
        assert_eq!(op.blocking, waits, "{op}");
        assert!(
            !op.secrets,
            "`secret_line` answers a `Secret`, and nothing here is handed one"
        );
        assert!(op.path.starts_with("ply_host::term::"));
        let reads = matches!(op.op.as_str(), "is_terminal" | "size");
        let expected = if reads {
            Linearity::Repeatable
        } else {
            Linearity::AtMostOnce
        };
        assert_eq!(op.linearity, expected, "{op}");
        let mode = if reads { "read" } else { "write" };
        assert!(
            declaration.contains(&format!("  {mode} {}(", op.op)),
            "`{}` is not declared a `{mode}` in std.term",
            op.op
        );
    }
}

#[test]
fn a_stream_is_a_terminal_when_the_system_says_so() {
    let host = host();
    let handlers = registrations(Some(&host));
    for (name, is) in [
        ("Stdin", std::io::stdin().is_terminal()),
        ("Stdout", std::io::stdout().is_terminal()),
        ("Stderr", std::io::stderr().is_terminal()),
    ] {
        let answered = call(&handlers, Op::IsTerminal, &[stream(name)]).expect("answered");
        assert_eq!(answered, Value::Bool(is), "{name}");
    }
    let any = std::io::stdin().is_terminal()
        || std::io::stdout().is_terminal()
        || std::io::stderr().is_terminal();
    let size = call(&handlers, Op::Size, &[]).expect("answered");
    let Value::Ctor { name, .. } = &size else {
        panic!("a size is an option");
    };
    if !any {
        assert_eq!(name.as_str(), "None", "no stream is a terminal");
    }
}

#[test]
fn an_argument_the_checker_would_have_refused_is_a_dispatch_defect() {
    let host = host();
    let handlers = registrations(Some(&host));
    let refused = call(&handlers, Op::IsTerminal, &[Value::Int(1)]).expect_err("no stream");
    assert_eq!(refused.code, codes::INTERNAL_ERROR);
    let refused = call(&handlers, Op::Mode, &[stream("Stdin")]).expect_err("no mode");
    assert_eq!(refused.code, codes::INTERNAL_ERROR);
    let refused = call(&handlers, Op::Size, &[Value::Unit]).expect_err("size takes nothing");
    assert_eq!(refused.code, codes::INTERNAL_ERROR);
}

#[test]
fn a_read_of_a_negative_length_is_refused_before_anything_waits() {
    let host = host();
    let handlers = registrations(Some(&host));
    let refused =
        call(&handlers, Op::Input, &[Value::Int(-1), Value::Int(0)]).expect_err("no such read");
    assert_eq!(refused.code, codes::RUNTIME_ERROR);
}

#[test]
fn a_withheld_registration_refuses_dispatch() {
    let handlers = registrations(None);
    let refused = call(&handlers, Op::Size, &[]).expect_err("nothing answers without a host");
    assert_eq!(refused.code, codes::INTERNAL_ERROR);
}

/// A test is no process of its own: it has no terminal, as it has no streams.
#[test]
fn a_host_that_only_starts_programs_withholds_the_terminal() {
    let spawning = ply_host::Host::new().with_process(ProcessHost::spawning(Executables::new()));
    let binding = HostBinding::hermetic_with(spawning.registry());
    let whole =
        ply_host::Host::new().with_process(ProcessHost::new(Vec::new(), OutputSink::captured()));
    let bound = HostBinding::hermetic_with(whole.registry());
    for op in Op::ALL {
        let named = Symbol::new(op.name());
        assert!(
            binding
                .withholds(&Symbol::new(EFFECT), &named, None)
                .is_some(),
            "`{}` is withheld from a run that is no process",
            op.name()
        );
        assert!(
            bound
                .withholds(&Symbol::new(EFFECT), &named, None)
                .is_none(),
            "`{}` is served to a run that is one",
            op.name()
        );
    }
}

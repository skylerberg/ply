use ply_eval::host::MachineId;
use ply_eval::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, Linearity, Value,
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
        assert!(!op.blocking);
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
        assert!(
            DECLARATION.contains(&format!(" {}[p]", op.name())),
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

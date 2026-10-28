// `Value`'s shared payloads are `Arc` and deliberately thread-confined.
#![allow(clippy::arc_with_non_send_sync)]

use ply_core::escape::Boundary;
use ply_core::{Closure, ClosureKind, Diagnostic, Span, Symbol, Value, codes};
use std::sync::Arc;

/// Assembled, since only the C backend has a continuation's code.
fn continuation() -> Value {
    Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Continuation {
            code: 0,
            arity: 1,
            captured: Vec::new(),
        },
    }))
}

/// A handler that answers with the boundary's own code has its classification taken back: the run
/// reports a runtime error, and a note names the code it claimed and the handler that claimed it.
#[test]
fn a_handler_may_not_answer_with_the_boundarys_own_code() {
    assert!(ply_core::host::is_reserved_code(
        codes::REGION_ESCAPE_AT_BOUNDARY
    ));
    let d = ply_core::host::attribute(
        Diagnostic::error(
            codes::REGION_ESCAPE_AT_BOUNDARY,
            "a handler claiming the machine's own verdict",
        ),
        "test::forge",
        "ext.ask[socket]",
        Span::DUMMY,
    );
    assert_eq!(
        d.code,
        codes::RUNTIME_ERROR,
        "the classification is taken back from the handler"
    );
    assert_eq!(d.message, "a handler claiming the machine's own verdict");
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains(codes::REGION_ESCAPE_AT_BOUNDARY) && n.contains("test::forge")),
        "the code the handler claimed is not reported: {:?}",
        d.notes
    );
}

#[test]
fn a_handle_in_a_host_operations_argument_is_refused_and_the_position_named() {
    let d = ply_core::escape::check_arguments(
        "ext.ask[socket]",
        "test::forge",
        &[Value::Int(1), continuation()],
        Span::DUMMY,
    )
    .expect_err("argument 2 carries a continuation");

    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
    assert!(d.message.contains("argument 2"), "{}", d.message);
    assert!(
        d.notes.iter().any(|n| n.contains("outlives every region")),
        "{:#?}",
        d.notes
    );
}

/// `std.trace` is a host handler, so a trace field crosses `perform_host` like any argument.
#[test]
fn a_handle_in_a_trace_field_is_the_host_boundary_and_nothing_further() {
    let fields = Value::list(vec![Value::Ctor {
        name: Symbol::new("std.trace.Str"),
        args: Arc::new(vec![Value::str("k"), continuation()]),
    }]);

    let d = ply_core::escape::check(
        &Boundary::HostArgument {
            operation: "trace.event[app]",
            path: "ply_host::trace",
            position: 2,
        },
        &fields,
        Span::DUMMY,
    )
    .expect_err("a field carrying a handle is refused");
    assert!(d.message.contains("item 0"), "{}", d.message);
}

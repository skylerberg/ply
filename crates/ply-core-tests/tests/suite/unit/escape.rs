use ply_core::escape::*;
use ply_core::{Closure, ClosureKind, Span, Symbol, Value, codes};
use std::collections::BTreeMap;
use std::sync::Arc;

fn closure(kind: ClosureKind) -> Value {
    Value::Closure(Arc::new(Closure { name: None, kind }))
}

/// Assembled, since only the C backend has a continuation's code; its captures are numbers, as
/// the backend's are.
fn continuation() -> Value {
    closure(ClosureKind::Continuation {
        code: 0,
        arity: 1,
        captured: vec![Value::Int(3), Value::Int(1)],
    })
}

#[test]
fn data_without_a_handle_crosses() {
    let value = Value::list(vec![
        Value::Int(1),
        Value::str("two"),
        Value::Map(Default::default()),
    ]);
    assert_eq!(carries(&value), None);
}

/// The constructor's field type says nothing of a continuation, so only the value does.
#[test]
fn a_handle_inside_a_constructor_is_found_and_the_route_names_it() {
    let value = Value::Ctor {
        name: Symbol::new("m.Just"),
        args: Arc::new(vec![continuation()]),
    };
    let found = carries(&value).expect("the constructor carries it");
    assert_eq!(found.handle, Handle::Continuation);
    assert_eq!(found.route, vec!["`m.Just`'s argument 1"]);
}

#[test]
fn the_route_reads_outermost_first() {
    let inner = Value::Ctor {
        name: Symbol::new("m.Just"),
        args: Arc::new(vec![continuation()]),
    };
    let mut fields = BTreeMap::new();
    fields.insert(Symbol::new("saved"), inner);
    let value = Value::list(vec![
        Value::Int(0),
        Value::Record(Arc::new(fields.into_iter().collect())),
    ]);

    let found = carries(&value).expect("the record carries it");
    assert_eq!(
        found.route,
        vec!["item 1", "field `saved`", "`m.Just`'s argument 1"]
    );
    assert_eq!(
        found.reached(),
        ", reached through item 1 → field `saved` → `m.Just`'s argument 1"
    );
}

#[test]
fn a_secret_holds_bytes_alone_and_so_no_handle() {
    assert!(carries(&Value::secret_bytes(b"key")).is_none());
    assert!(carries(&Value::secret_text("key")).is_none());
}

#[test]
fn a_continuation_is_a_handle() {
    let found = carries(&continuation()).expect("a continuation is a handle");
    assert_eq!(found.handle, Handle::Continuation);
    assert!(found.route.is_empty());
    assert_eq!(found.reached(), "");
}

/// Only the kind tells the two apart: a native closure over the very numbers a continuation
/// captures is data, and one that captures a continuation carries it.
#[test]
fn a_native_closure_carries_a_continuation_only_by_capturing_one() {
    let native = |captured: Vec<Value>| {
        closure(ClosureKind::Native {
            code: 0,
            arity: 1,
            captured,
        })
    };
    assert_eq!(carries(&native(vec![Value::Int(3), Value::Int(1)])), None);

    let wrapped = native(vec![Value::Int(2), continuation()]);
    let found = carries(&wrapped).expect("the capture is a continuation");
    assert_eq!(found.handle, Handle::Continuation);
}

/// Each boundary refuses a continuation and names it as what it is rather than as a type it does
/// not have.
#[test]
fn every_boundary_refuses_a_continuation_and_names_it() {
    let value = Value::ctor("m.Just", vec![continuation()]);
    let boundaries = [
        Boundary::HostArgument {
            operation: "ext.keep[s]",
            path: "test::keep",
            position: 0,
        },
        Boundary::HostAnswer {
            operation: "ext.keep[s]",
            path: "test::keep",
        },
        Boundary::EntryAnswer { name: "m.parked" },
    ];
    for boundary in boundaries {
        let d = check(&boundary, &value, Span::DUMMY).expect_err("a continuation is refused");
        assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY, "{boundary:?}");
        assert!(
            d.message.contains(" a continuation") && d.message.contains("`m.Just`'s argument 1"),
            "{boundary:?}: {}",
            d.message
        );
    }
}

/// The boundary a handle leaves by rather than enters by: it names the definition that answered,
/// and its remedy is what to answer instead.
#[test]
fn an_entry_points_answer_refuses_a_continuation_and_lets_data_cross() {
    let boundary = Boundary::EntryAnswer { name: "m.parked" };
    let d = check(
        &boundary,
        &Value::ctor("m.Just", vec![continuation()]),
        Span::DUMMY,
    )
    .expect_err("a continuation does not leave the entry that made it");
    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
    assert_eq!(
        d.message,
        "`m.parked` answered a continuation, reached through `m.Just`'s argument 1"
    );
    assert_eq!(d.labels[0].message, "answered here");
    assert!(
        d.notes.iter().any(|n| n.contains("goes to its caller")),
        "{:#?}",
        d.notes
    );
    assert!(
        (d.notes.iter()).any(|n| n.contains("resume the continuation before the entry answers")),
        "{:#?}",
        d.notes
    );

    let data = Value::ctor("m.Just", vec![Value::Int(41)]);
    assert!(check(&boundary, &data, Span::DUMMY).is_ok());
}

#[test]
fn a_builtin_closure_carries_nothing() {
    let value = Value::builtin(ply_core::Builtin::Digest);
    assert_eq!(carries(&value), None);
}

/// The generator never draws a handle, so this value is assembled directly.
#[test]
fn a_generated_function_holding_a_handle_is_found() {
    use ply_core::Synth;

    let closure = Value::Closure(Arc::new(Closure {
        name: Some(Symbol::new("|_| c")),
        kind: ClosureKind::Synth {
            arity: 1,
            rule: Synth::Table {
                entries: vec![(Value::Int(1), continuation())],
                default: Value::Int(0),
            },
        },
    }));

    let found = carries(&closure).expect("the table reaches the continuation");
    assert_eq!(found.handle, Handle::Continuation);
}

#[test]
fn the_diagnostic_names_the_boundary_the_handle_and_the_route() {
    let value = Value::Ctor {
        name: Symbol::new("m.Just"),
        args: Arc::new(vec![continuation()]),
    };
    let boundary = Boundary::HostArgument {
        operation: "net.recv[conn]",
        path: "ply_host::tcp",
        position: 1,
    };
    let d = check(&boundary, &value, Span::DUMMY).expect_err("it is refused");

    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
    assert!(d.message.contains("argument 2"), "{}", d.message);
    assert!(d.message.contains("a continuation"), "{}", d.message);
    assert!(d.message.contains("`m.Just`'s argument 1"), "{}", d.message);
    assert!(
        d.notes.iter().any(|n| n.contains("ply_host::tcp")),
        "the handler is named: {:#?}",
        d.notes
    );
}

#[test]
fn check_arguments_names_the_first_position_that_carries_one() {
    let args = vec![Value::Int(1), Value::Int(2), continuation(), continuation()];
    let d = check_arguments("net.send[s]", "ply_host::tcp", &args, Span::DUMMY)
        .expect_err("argument 3 carries a continuation");
    assert!(d.message.contains("argument 3"), "{}", d.message);
}

#[test]
fn a_host_answer_says_what_outlives_the_region() {
    let answer = check(
        &Boundary::HostAnswer {
            operation: "net.recv[s]",
            path: "ply_host::tcp",
        },
        &continuation(),
        Span::DUMMY,
    )
    .expect_err("a forged handle is refused");
    assert!(
        answer
            .notes
            .iter()
            .any(|n| n.contains("outside the program")),
        "{:#?}",
        answer.notes
    );
}

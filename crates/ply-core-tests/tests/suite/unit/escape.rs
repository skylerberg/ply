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

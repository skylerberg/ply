use ply_eval::arena::{Arena, RegionKind};
use ply_eval::escape::*;
use ply_eval::{ClosureKind, Value};
use ply_span::Symbol;
use ply_span::{Span, codes};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A real slot, because a [`Slot`](ply_eval::arena::Slot) carries a generation only the allocator assigns.
fn cell() -> Value {
    let mut arena = Arena::new();
    arena.open(RegionKind::Shared, Span::DUMMY);
    Value::Cell(arena.alloc(Value::Int(0)).expect("the region is open"))
}

#[test]
fn a_bare_handle_is_found_with_an_empty_route() {
    let found = carries(&cell()).expect("a cell is a handle");
    assert_eq!(found.handle, Handle::Cell);
    assert!(found.route.is_empty());
    assert_eq!(found.reached(), "");
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

/// The constructor's field type mentions no brand, so only the value says so.
#[test]
fn a_handle_inside_a_constructor_is_found_and_the_route_names_it() {
    let value = Value::Ctor {
        name: Symbol::new("m.Just"),
        args: Arc::new(vec![cell()]),
    };
    let found = carries(&value).expect("the constructor carries it");
    assert_eq!(found.handle, Handle::Cell);
    assert_eq!(found.route, vec!["`m.Just`'s argument 1"]);
}

#[test]
fn the_route_reads_outermost_first() {
    let inner = Value::Ctor {
        name: Symbol::new("m.Just"),
        args: Arc::new(vec![cell()]),
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
fn a_secret_is_not_a_place_to_hide_a_handle_and_its_shape_stays_redacted() {
    let value = Value::Secret(Arc::new(Value::Ctor {
        name: Symbol::new("m.Just"),
        args: Arc::new(vec![cell()]),
    }));
    let found = carries(&value).expect("the payload carries it");
    assert_eq!(found.handle, Handle::Cell);
    assert_eq!(found.route, vec!["a `Secret`'s payload"]);
    assert!(
        !found.reached().contains("Just"),
        "the payload's shape stays redacted: {}",
        found.reached()
    );
}

#[test]
fn a_task_and_a_continuation_are_handles_too() {
    assert_eq!(
        carries(&Value::Task(ply_eval::sim::TaskId(0)))
            .expect("a task is a handle")
            .handle,
        Handle::Task
    );
}

#[test]
fn a_builtin_closure_carries_nothing() {
    let value = Value::builtin(ply_eval::Builtin::CellGet);
    assert_eq!(carries(&value), None);
}

/// `E0302` refuses this shape in source, so the environment is assembled directly.
#[test]
fn a_closure_whose_scope_reaches_a_handle_is_found_and_the_binding_named() {
    use ply_eval::Closure;
    use ply_syntax::ast::{Expr, ExprKind, Lit};

    let bindings = vec![
        (Symbol::new("n"), Value::Int(1)),
        (Symbol::new("c"), cell()),
    ];
    let closure = Value::Closure(Arc::new(Closure {
        name: Some(Symbol::new("m.later")),
        kind: ClosureKind::Fn {
            params: Vec::new(),
            body: Arc::new(Expr {
                kind: ExprKind::Lit(Lit::Int(0)),
                span: Span::DUMMY,
            }),
            bindings,
            module: 0,
        },
    }));

    let found = carries(&closure).expect("the scope reaches the cell");
    assert_eq!(found.handle, Handle::Cell);
    assert_eq!(found.route, vec!["`c`, captured by `m.later`"]);
}

#[test]
fn the_diagnostic_names_the_boundary_the_handle_and_the_route_and_no_value() {
    let value = Value::Ctor {
        name: Symbol::new("m.Just"),
        args: Arc::new(vec![cell()]),
    };
    let boundary = Boundary::HostArgument {
        operation: "db.query[users]",
        path: "ply_host::db",
        position: 1,
    };
    let d = check(&boundary, &value, Span::DUMMY).expect_err("it is refused");

    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
    assert!(d.message.contains("argument 2"), "{}", d.message);
    assert!(d.message.contains("`Cell`"), "{}", d.message);
    assert!(d.message.contains("`m.Just`'s argument 1"), "{}", d.message);
    assert!(
        d.notes.iter().any(|n| n.contains("ply_host::db")),
        "the handler is named: {:#?}",
        d.notes
    );
    assert!(
        !d.message.contains('@'),
        "the slot's identity is not printed: {}",
        d.message
    );
}

#[test]
fn check_arguments_names_the_first_position_that_carries_one() {
    let args = vec![Value::Int(1), Value::Int(2), cell(), cell()];
    let d = check_arguments("net.send[s]", "ply_host::tcp", &args, Span::DUMMY)
        .expect_err("argument 3 carries a cell");
    assert!(d.message.contains("argument 3"), "{}", d.message);
}

#[test]
fn an_answer_and_an_entry_point_each_say_what_outlives_the_region() {
    let answer = check(
        &Boundary::HostAnswer {
            operation: "net.recv[s]",
            path: "ply_host::tcp",
        },
        &cell(),
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

    let entry = check(
        &Boundary::EntryPoint {
            name: "m.resume_it",
        },
        &cell(),
        Span::DUMMY,
    )
    .expect_err("a smuggled handle is refused");
    assert!(
        entry
            .notes
            .iter()
            .any(|n| n.contains("resets its region stack to the fixture")),
        "{:#?}",
        entry.notes
    );
}

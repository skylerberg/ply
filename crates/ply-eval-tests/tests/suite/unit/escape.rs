use ply_eval::arena::{Arena, Owner, RegionKind};
use ply_eval::escape::*;
use ply_eval::{Closure, ClosureKind, Span, Symbol, Value, codes};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A real slot, because a [`Slot`](ply_eval::arena::Slot) carries a generation only the allocator assigns.
fn cell() -> Value {
    let mut arena = Arena::new();
    arena.open(Owner::ENTRY, RegionKind::Shared);
    Value::Cell(
        arena
            .alloc(Owner::ENTRY, Value::Int(0))
            .expect("the region is open"),
    )
}

fn task() -> Value {
    Value::Task(ply_eval::TaskHandle::unowned(ply_eval::sim::TaskId(0)))
}

fn closure(kind: ClosureKind) -> Value {
    Value::Closure(Arc::new(Closure { name: None, kind }))
}

/// Assembled, since only the compiled tier has a continuation's code; its captures are numbers, as
/// the tier's are.
fn continuation() -> Value {
    closure(ClosureKind::Continuation {
        code: 0,
        arity: 1,
        captured: vec![Value::Int(3), Value::Int(1)],
    })
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
        carries(&task()).expect("a task is a handle").handle,
        Handle::Task
    );
    let found = carries(&continuation()).expect("a continuation is a handle");
    assert_eq!(found.handle, Handle::Continuation);
    assert!(found.route.is_empty());
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

/// One classification: each boundary refuses a continuation as it refuses a cell, and names it as
/// what it is rather than as a type it does not have.
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
        Boundary::HostToken {
            label: "keep",
            token: 7,
        },
        Boundary::EntryPoint {
            name: "m.resume_it",
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
        assert!(
            !d.notes.iter().any(|n| n.contains("escape brand")),
            "{boundary:?}: a continuation's type is a function's and has no brand: {:#?}",
            d.notes
        );
    }
}

/// The boundary a handle leaves by rather than enters by: it names the definition that answered,
/// and its remedy is what to answer instead.
#[test]
fn an_entry_points_answer_refuses_every_handle_and_lets_data_cross() {
    let boundary = Boundary::EntryAnswer { name: "m.parked" };
    let unreached = "answer with something that does not reach a region";
    let resumed = "resume the continuation before the entry answers";
    for (handle, noun, remedy) in [
        (cell(), "a `Cell`", unreached),
        (task(), "a `Task`", unreached),
        (continuation(), "a continuation", resumed),
    ] {
        let d = check(&boundary, &Value::ctor("m.Just", vec![handle]), Span::DUMMY)
            .expect_err("a handle does not leave the entry that made it");
        assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY, "{noun}");
        assert_eq!(
            d.message,
            format!("`m.parked` answered {noun}, reached through `m.Just`'s argument 1")
        );
        assert_eq!(d.labels[0].message, "answered here", "{noun}");
        assert!(
            d.notes.iter().any(|n| n.contains("goes to its caller")),
            "{noun}: {:#?}",
            d.notes
        );
        assert!(
            d.notes.iter().any(|n| n.contains(remedy)),
            "{noun}: {:#?}",
            d.notes
        );
    }

    let data = Value::ctor("m.Just", vec![Value::Int(41)]);
    assert!(check(&boundary, &data, Span::DUMMY).is_ok());
}

#[test]
fn a_builtin_closure_carries_nothing() {
    let value = Value::builtin(ply_eval::Builtin::CellGet);
    assert_eq!(carries(&value), None);
}

/// The generator never draws a handle, so this value is assembled directly.
#[test]
fn a_generated_function_holding_a_handle_is_found() {
    use ply_eval::Synth;

    let closure = Value::Closure(Arc::new(Closure {
        name: Some(Symbol::new("|_| c")),
        kind: ClosureKind::Synth {
            arity: 1,
            rule: Synth::Table {
                entries: vec![(Value::Int(1), cell())],
                default: Value::Int(0),
            },
        },
    }));

    let found = carries(&closure).expect("the table reaches the cell");
    assert_eq!(found.handle, Handle::Cell);
}

#[test]
fn the_diagnostic_names_the_boundary_the_handle_and_the_route_and_no_value() {
    let value = Value::Ctor {
        name: Symbol::new("m.Just"),
        args: Arc::new(vec![cell()]),
    };
    let boundary = Boundary::HostArgument {
        operation: "net.recv[conn]",
        path: "ply_host::tcp",
        position: 1,
    };
    let d = check(&boundary, &value, Span::DUMMY).expect_err("it is refused");

    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
    assert!(d.message.contains("argument 2"), "{}", d.message);
    assert!(d.message.contains("`Cell`"), "{}", d.message);
    assert!(d.message.contains("`m.Just`'s argument 1"), "{}", d.message);
    assert!(
        d.notes.iter().any(|n| n.contains("ply_host::tcp")),
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

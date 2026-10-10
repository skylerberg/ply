// `Value`'s shared payloads are `Arc` and deliberately thread-confined.
#![allow(clippy::arc_with_non_send_sync)]

use ply_eval::arena::Owner;
use ply_eval::escape::Boundary;
use ply_eval::{Arena, Diagnostic, RegionKind, Span, Symbol, TaskRegions, Value, codes};
use std::sync::Arc;

/// A cell over a still-open region's slot, so what is under test is the boundary and not staleness.
fn live_cell() -> (Arena, Value) {
    let mut arena = Arena::new();
    arena.open(Owner::ENTRY, RegionKind::Shared);
    let slot = arena
        .alloc(Owner::ENTRY, Value::Int(41))
        .expect("the region is open");
    (arena, Value::Cell(slot))
}

/// Why the entry-point check is load-bearing: a reset restores the fixture's generations.
#[test]
fn an_entry_point_reset_leaves_an_earlier_runs_slot_resolvable() {
    let mut regions = TaskRegions::new();
    let slot = regions
        .arena_mut()
        .alloc(Owner::ENTRY, Value::Int(1))
        .expect("the root region is open");
    regions.seal();

    regions.arena_mut().set(slot, Value::Int(2));
    regions.reset();

    assert!(
        regions.arena().contains(slot),
        "a reset restores generations, so nothing downstream reports a smuggled slot"
    );
}

/// A handler that answers with the boundary's own code has its classification taken back: the run
/// reports a runtime error, and a note names the code it claimed and the handler that claimed it.
#[test]
fn a_handler_may_not_answer_with_the_boundarys_own_code() {
    assert!(ply_eval::host::is_reserved_code(
        codes::REGION_ESCAPE_AT_BOUNDARY
    ));
    let d = ply_eval::host::attribute(
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
    let (_arena, cell) = live_cell();
    let d = ply_eval::escape::check_arguments(
        "ext.ask[socket]",
        "test::forge",
        &[Value::Int(1), cell],
        Span::DUMMY,
    )
    .expect_err("argument 2 carries a cell");

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
    let (_arena, cell) = live_cell();
    let fields = Value::list(vec![Value::Ctor {
        name: Symbol::new("std.trace.Str"),
        args: Arc::new(vec![Value::str("k"), cell]),
    }]);

    let d = ply_eval::escape::check(
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

    // A handle is copied out as its slot and never dereferenced.
    let (_a, c) = live_cell();
    let copied = ply_eval::Plain::of(&c);
    assert!(matches!(copied, ply_eval::Plain::Cell { .. }), "{copied:?}");
    assert!(
        !format!("{copied:?}").contains("41"),
        "the slot's contents are not read"
    );
}

#[test]
fn a_stale_slot_reports_rather_than_reading_what_replaced_it() {
    let mut arena = Arena::new();
    let first = arena.open(Owner::ENTRY, RegionKind::Unique);
    let stale = arena
        .alloc(Owner::ENTRY, Value::Int(41))
        .expect("inside a region");
    arena.close(first);

    let second = arena.open(Owner::ENTRY, RegionKind::Unique);
    let fresh = arena
        .alloc(Owner::ENTRY, Value::Int(99))
        .expect("inside a region");

    assert_eq!(
        stale.index(),
        fresh.index(),
        "the bump pointer reused the position, which is the whole hazard"
    );
    assert!(arena.get(stale).is_none(), "the read is refused");
    assert!(!arena.set(stale, Value::Int(0)), "so is the write");
    assert_eq!(arena.get(fresh), Some(&Value::Int(99)));
    arena.close(second);
}

//! Escape enforcement at the runtime boundaries the brand cannot see.

// A `Value`'s shared payloads are `Arc` and are deliberately thread-confined, which is the crate's
// own allow rather than something these fixtures choose.
#![allow(clippy::arc_with_non_send_sync)]

use crate::fixture::Compiled;
use ply_eval::escape::Boundary;
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity,
};
use ply_eval::{Arena, RegionKind, TaskRegions, Value};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::sync::Arc;

/// A `Value::Cell` over a slot from a region that is still open — the shape a legitimate one has,
/// so that what is under test is the boundary and not the slot being stale.
fn live_cell() -> (Arena, Value) {
    let mut arena = Arena::new();
    arena.open(RegionKind::Shared, Span::DUMMY);
    let slot = arena.alloc(Value::Int(41)).expect("the region is open");
    (arena, Value::Cell(slot))
}

fn op(effect: &str, name: &str, linearity: Linearity) -> HostOp {
    HostOp {
        effect: Symbol::new(effect),
        op: Symbol::new(name),
        resource: HostResource::Any,
        determinism: Determinism::Nondeterministic,
        linearity,
        blocking: false,
        secrets: false,
        path: "test::forge",
    }
}

fn bound(compiled: &Compiled, entries: Vec<(HostOp, Arc<dyn HostHandler>)>) -> HostBinding {
    let mut registry = HostRegistry::new();
    for (o, handler) in entries {
        registry.register(o, handler);
    }
    registry.bind(&compiled.check).expect("the registry binds")
}

/// The brand's open route, and the escape case: a continuation parked in an enclosing
/// region's cell and resumed after the region whose cell it reads has returned.
const PARKED: &str = r#"
effect amb { read flip[coin]() -> Bool }

type Saved = Nothing | Just((Bool) -> Int)

fn parked() -> Saved = with_cell[slot](Nothing) { s -> {
  let inner = with_cell[log](41) { c ->
    handle {
      let b = amb.flip[coin]();
      if b { cell_get(c) } else { 0 }
    } with { amb.flip[coin]() resume k -> { cell_set(s, Just(k)); 0 }, return x -> x }
  };
  assert_eq(inner, 0);
  cell_get(s)
} }

fn resume_it(s: Saved) -> Int = match s { Just(k) -> k(true), Nothing -> 0 }

fn identity(n: Int) -> Int = n

test "the parked continuation still reads its region's cell" {
  assert_eq(resume_it(parked()), 41)
}
"#;

/// A constant whose value is a slot in *this run's* arena.
const CONSTANT_OVER_A_CELL: &str = r#"
fn boxed() -> Int = with_cell[log](41) { c -> cell_get(c) }

test "the constant reads this run's cell" {
  assert_eq(boxed(), 41)
}
"#;

/// One host-backed operation with nothing to shadow it, so a handler's answer goes straight back
/// into the program.
const ASKS: &str = r#"
nondet effect ext {
  write ask[s](n: Int) -> Int
}

test/nondet "the answer comes back" {
  assert_eq(ext.ask[socket](1), 1)
}
"#;



/// The obvious extension of the open route — the same constructor erasure carrying a **cell inside
/// a closure** rather than a continuation — and it does not exist.
#[test]
fn the_constructor_erasure_does_not_also_launder_a_cell_inside_a_closure() {
    let diags = Compiled::rejected(
        r#"
type Boxed = Empty | Wrap(() -> Int)
fn boxed() -> Boxed = with_cell[log](41) { c -> Wrap(|| cell_get(c)) }
"#,
    );
    let d = diags
        .iter()
        .find(|d| d.code == codes::EFFECT_NOT_PERMITTED)
        .unwrap_or_else(|| panic!("the constructor's own row check refuses it: {diags:#?}"));
    assert!(
        d.notes
            .iter()
            .chain([&d.message])
            .any(|s| s.contains("log")),
        "the region is named: {d:#?}"
    );
}


/// A bare slot, which is what the check has to catch when no constructor is involved at all.
#[test]
fn a_cell_from_another_arena_is_refused_at_the_entry_point() {
    let compiled = Compiled::new(PARKED);
    let (_arena, cell) = live_cell();

    let d = compiled
        .machine()
        .call("m.identity", vec![cell], Span::DUMMY)
        .expect_err("a cell may not enter a run");

    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
    assert!(d.message.contains("`Cell`"), "{}", d.message);
}

/// Data crosses.
#[test]
fn data_still_crosses_the_entry_point() {
    let compiled = Compiled::new(PARKED);
    assert_eq!(
        compiled
            .machine()
            .call("m.identity", vec![Value::Int(7)], Span::DUMMY)
            .expect("an `Int` is data"),
        Value::Int(7)
    );
}


/// The reason the entry-point check is load-bearing rather than belt-and-braces, stated over the
/// allocator: a reset restores the fixture's generations, so a slot taken out of an earlier run
/// resolves afterwards.
#[test]
fn an_entry_point_reset_leaves_an_earlier_runs_slot_resolvable() {
    let mut regions = TaskRegions::new();
    let slot = regions
        .arena_mut()
        .alloc(Value::Int(1))
        .expect("the root region is open");
    regions.seal();

    regions.arena_mut().set(slot, Value::Int(2));
    regions.reset();

    assert!(
        regions.arena().contains(slot),
        "a reset restores generations, so nothing downstream reports a smuggled slot"
    );
}










/// `E0449` is the machine's verdict about its own memory, so a handler may not mint it: `attribute`
/// rewrites a reserved code to `E0502` and adds the note naming the handler.
struct ClaimsTheCode;

impl HostHandler for ClaimsTheCode {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        Err(Diagnostic::error(
            codes::REGION_ESCAPE_AT_BOUNDARY,
            "a handler claiming the machine's own verdict",
        ))
    }
}

#[test]
fn a_handler_may_not_answer_with_the_boundarys_own_code() {
    assert!(ply_eval::host::is_reserved_code(
        codes::REGION_ESCAPE_AT_BOUNDARY
    ));

    let compiled = Compiled::new(ASKS);
    let binding = bound(
        &compiled,
        vec![(
            op("ext", "ask", Linearity::Repeatable),
            Arc::new(ClaimsTheCode) as Arc<dyn HostHandler>,
        )],
    );
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));

    let d = machine.eval_test(0).expect_err("the handler refused");
    assert_eq!(
        d.code,
        codes::RUNTIME_ERROR,
        "the classification is taken back from the handler"
    );
}

/// The argument half of the same boundary.
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

/// A trace span attribute and a log line are this boundary and not a new one: `std.trace` is served
/// by a host handler, so a value reaching a field crosses `perform_host` exactly as a database
/// parameter does.
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

    // And what a sink writes is text: a handle renders opaquely and is never dereferenced, so the
    // record cannot carry the region into the log.
    let (_a, c) = live_cell();
    let rendered = c.render();
    assert!(rendered.starts_with("<cell "), "{rendered}");
    assert!(!rendered.contains("41"), "the slot's contents are not read");
}

/// `boxed` is nullary and publishes an empty row, which makes it a constant by the memo's rule, and
/// its value is a slot in this run's arena — the exact shape `memo.rs` says must not be remembered.
#[test]
fn a_constant_whose_value_reaches_a_region_is_not_remembered_across_runs() {
    let compiled = Compiled::new(CONSTANT_OVER_A_CELL);
    let mut machine = compiled.machine();

    for run in 0..3 {
        machine
            .eval_test(0)
            .unwrap_or_else(|d| panic!("run {run} must read this run's own cell: {d:#?}"));
    }
}

/// The property that makes every refusal above a bound rather than a hope: when a handle does reach
/// a slot whose region has closed, the read is a diagnostic and never the value that now lives at
/// that position.
#[test]
fn a_stale_slot_reports_rather_than_reading_what_replaced_it() {
    let mut arena = Arena::new();
    let first = arena.open(RegionKind::Unique, Span::DUMMY);
    let stale = arena.alloc(Value::Int(41)).expect("inside a region");
    arena.close(first);

    let second = arena.open(RegionKind::Unique, Span::DUMMY);
    let fresh = arena.alloc(Value::Int(99)).expect("inside a region");

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

/// An M8 counterexample and a shrunk witness are closed at the law, not at the runtime: `E0418`
/// refuses a binder whose type the generator cannot inhabit, so no generated value and no shrunk
/// witness can hold a handle in the first place.
#[test]
fn a_law_cannot_quantify_over_a_record_that_reaches_a_region() {
    let diags = Compiled::rejected(r#"law "no" forall (r: {c: Cell<Int>}) { true }"#);
    assert!(
        diags.iter().any(|d| d.code == codes::UNQUANTIFIABLE_TYPE),
        "a record holding a cell must be refused where the law is written: {diags:#?}"
    );
}


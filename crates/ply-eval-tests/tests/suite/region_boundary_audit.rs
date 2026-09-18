// `Value`'s shared payloads are `Arc` and deliberately thread-confined.
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

/// A cell over a still-open region's slot, so what is under test is the boundary and not staleness.
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

/// A continuation parked in an enclosing region's cell, resumed after the region it reads returns.
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

/// Nothing shadows the host operation, so a handler's answer goes straight to the program.
const ASKS: &str = r#"
nondet effect ext {
  write ask[s](n: Int) -> Int
}

test/nondet "the answer comes back" {
  assert_eq(ext.ask[socket](1), 1)
}
"#;

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

/// Why the entry-point check is load-bearing: a reset restores the fixture's generations.
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

/// Mints `E0449`, which `attribute` must rewrite to `E0502` with a note naming the handler.
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

    // A sink writes text: a handle renders opaquely and is never dereferenced.
    let (_a, c) = live_cell();
    let rendered = c.render();
    assert!(rendered.starts_with("<cell "), "{rendered}");
    assert!(!rendered.contains("41"), "the slot's contents are not read");
}

/// `boxed` is nullary with an empty row, so the memo's rule makes it a constant.
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

/// `E0418` refuses a binder the generator cannot inhabit, so no generated value can hold a handle.
#[test]
fn a_law_cannot_quantify_over_a_record_that_reaches_a_region() {
    let diags = Compiled::rejected(r#"law "no" forall (r: {c: Cell<Int>}) { true }"#);
    assert!(
        diags.iter().any(|d| d.code == codes::UNQUANTIFIABLE_TYPE),
        "a record holding a cell must be refused where the law is written: {diags:#?}"
    );
}

//! Escape enforcement at the runtime boundaries ADR 0017 §2's brand cannot see.
//!
//! §2 makes escape a type error and `brand_in` implements it over resolved
//! types, a function type's effect row included. This file is about the places
//! where no type is left to look at: a host handler, which outlives every
//! region; a value a handler or a runtime answers with; an argument handed to an
//! entry point from Rust.
//!
//! Every attack here is written to *succeed* if it can. The one route ADR 0017
//! §2 leaves deliberately open — a continuation parked in an enclosing region's
//! cell, where a nominal constructor's field type erases the brand — is used as
//! the carrier rather than being worked around, because it is the only carrier
//! the language has and closing the boundaries is what makes its consequences
//! bounded.
//!
//! What each boundary's answer is, and where it lives, is `ply_eval::escape`'s
//! module documentation. This file is the evidence for it.

// A `Value`'s shared payloads are `Arc` and are deliberately thread-confined,
// which is the crate's own allow rather than something these fixtures choose.
#![allow(clippy::arc_with_non_send_sync)]

use ply_core::{CheckOutput, check_program};
use ply_eval::escape::{Boundary, Handle, carries};
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity,
};
use ply_eval::{Arena, Engine, Interp, Machine, RegionKind, TaskRegions, Value};
use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// ------------------------------------------------------------------- harness

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        let inputs = [(SourceId(0), ModuleName::from_dotted("m"), src)];
        let program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
        let resolved =
            resolve(&program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        Compiled {
            program,
            resolved,
            check,
        }
    }

    fn refused(src: &str) -> Vec<Diagnostic> {
        let inputs = [(SourceId(0), ModuleName::from_dotted("m"), src)];
        let program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
        let resolved =
            resolve(&program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        check_program(&program, &resolved).err().unwrap_or_default()
    }

    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    fn interp(&self) -> Interp<'_> {
        Interp::new(&self.program, &self.resolved, &self.check)
    }

    fn index_of(&self, name: &str) -> usize {
        self.check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }
}

/// A `Value::Cell` over a slot from a region that is still open — the shape a
/// legitimate one has, so that what is under test is the boundary and not the
/// slot being stale.
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

// ------------------------------------------------------------------ programs

/// ADR 0017 §2's open route, and ADR 0005 required test 6: a continuation parked
/// in an enclosing region's cell and resumed after the region whose cell it
/// reads has returned. `Just`'s field type is `(Bool) -> Int`, which mentions no
/// brand, so no type after the constructor says `log`.
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

/// A constant whose value is a slot in *this run's* arena. Nullary with an empty
/// published row — `with_cell` discharges its own atoms — so it is a constant by
/// the memo's rule and exactly the shape `memo.rs` says must not be remembered.
const CONSTANT_OVER_A_CELL: &str = r#"
fn boxed() -> Int = with_cell[log](41) { c -> cell_get(c) }

test "the constant reads this run's cell" {
  assert_eq(boxed(), 41)
}
"#;

/// One host-backed operation with nothing to shadow it, so a handler's answer
/// goes straight back into the program.
const ASKS: &str = r#"
nondet effect ext {
  write ask[s](n: Int) -> Int
}

test/nondet "the answer comes back" {
  assert_eq(ext.ask[socket](1), 1)
}
"#;

// ------------------------------------------- 1. the open route stays open

/// ADR 0017 §2 records one route as open and this milestone does not close it.
/// A test that only asserted the refusals would pass just as well if the
/// language had stopped compiling the shape, so this is asserted first and on
/// both engines.
#[test]
fn the_documented_open_route_still_behaves_as_adr_0017_section_2_says() {
    let compiled = Compiled::new(PARKED);
    let index = compiled.index_of("the parked continuation still reads its region's cell");

    compiled
        .machine()
        .eval_test(index)
        .unwrap_or_else(|d| panic!("the open route must still run on the machine: {d:#?}"));

    // The tree-walker refuses every clause that binds a continuation (`E0504`,
    // ADR 0005 required test 3), so its answer here is that refusal and not a
    // different reading of the program. Asserted rather than skipped, because
    // "the other engine agreed" and "the other engine never ran it" are
    // different facts and ADR 0017's §"What must be measured" turns on which.
    let treewalk = compiled.interp().eval_test(index).expect_err("E0504");
    assert_eq!(treewalk.code, codes::MACHINE_ONLY_CLAUSE);
    assert!(ply_eval::is_machine_only(&treewalk));
    let _ = Engine::Machine;
}

/// And the value it carries is found by the walk the boundaries use. If this
/// stopped being true every refusal below would pass vacuously.
#[test]
fn the_value_the_open_route_produces_is_one_the_walk_finds() {
    let compiled = Compiled::new(PARKED);
    let saved = compiled
        .machine()
        .call("m.parked", Vec::new(), Span::DUMMY)
        .expect("the open route produces a value");

    let found = carries(&saved).expect("a continuation is parked inside it");
    assert_eq!(found.handle, Handle::Continuation);
    assert_eq!(
        found.route,
        vec!["`m.Just`'s argument 1"],
        "the constructor whose field type erased the brand is named"
    );
}

/// The obvious extension of the open route — the same constructor erasure
/// carrying a **cell inside a closure** rather than a continuation — and it does
/// not exist.
///
/// A field type is declared once for the whole program, so `Wrap`'s is
/// `() -> Int` with an empty row, and the row on `|| cell_get(c)` has nothing to
/// unify with at the constructor's *application*. That is `E0302` before any
/// region check is consulted, which is why §2's open route is specifically a
/// continuation: `ρ_κ` is a variable the `handle` solves, and it unifies.
///
/// Recorded here because "the erasure route also launders cells" is the
/// plausible-sounding claim a reader would otherwise carry away from §2.
#[test]
fn the_constructor_erasure_does_not_also_launder_a_cell_inside_a_closure() {
    let diags = Compiled::refused(
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

// ------------------------------------------------------ 2. the entry point

/// The boundary that got sharper rather than softer under regions.
///
/// Under the forkable world this was half-closed: a `CellId` the new world did
/// not hold was named by `E0505`, and one it happened to hold was read. A region
/// stack resets by *restoring* the fixture's generations, so a slot carried out
/// of one entry point resolves in the next one — the half that used to be caught
/// is not caught by anything downstream any more.
#[test]
fn a_continuation_from_an_earlier_run_is_refused_at_the_entry_point() {
    let compiled = Compiled::new(PARKED);
    let mut machine = compiled.machine();

    let parked = machine
        .call("m.parked", Vec::new(), Span::DUMMY)
        .expect("a continuation over the region's cell");

    let d = machine
        .call("m.resume_it", vec![parked], Span::DUMMY)
        .expect_err("a continuation may not enter a second run");

    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
    assert!(d.message.contains("m.resume_it"), "{}", d.message);
    assert!(d.message.contains("continuation"), "{}", d.message);
    assert!(
        d.message.contains("`m.Just`'s argument 1"),
        "the route is named: {}",
        d.message
    );
    let notes = d.notes.join(" ");
    assert!(notes.contains("resets its region stack"), "{notes}");
}

/// A bare slot, which is what the check has to catch when no constructor is
/// involved at all.
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

/// Both engines, at the same point and with the same message, or `--engine
/// both` reports the refusal itself as a divergence.
#[test]
fn both_engines_refuse_an_entry_point_argument_identically() {
    let compiled = Compiled::new(PARKED);
    let (_arena, cell) = live_cell();

    let machine = compiled
        .machine()
        .call("m.identity", vec![cell.clone()], Span::DUMMY)
        .expect_err("the machine refuses");
    let treewalk = compiled
        .interp()
        .call("m.identity", vec![cell], Span::DUMMY)
        .expect_err("the tree-walker refuses");

    assert_eq!(machine.code, treewalk.code);
    assert_eq!(machine.message, treewalk.message);
    assert_eq!(machine.notes, treewalk.notes);
}

/// Data crosses. A boundary that refused everything would be a boundary nobody
/// could call an entry point through, and the prover calls one per obligation.
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

/// The refusal happens **before** the reset, so a run that was refused has not
/// also discarded the previous run's arena. Otherwise a caller that recovered
/// from the diagnostic would be standing on a region stack the refusal emptied.
#[test]
fn a_refused_entry_point_leaves_the_previous_runs_state_alone() {
    let compiled = Compiled::new(PARKED);
    let mut machine = compiled.machine();
    // Seeded, because what a run allocates in a region of its own is handed back
    // at that region's close: the state a refusal must not disturb is the
    // fixture, which is what outlives an entry point.
    let fixture = ply_eval::Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(1_000))));
    let (regions, _) = fixture.open();
    machine.set_regions(regions);

    machine
        .call("m.parked", Vec::new(), Span::DUMMY)
        .expect("the first run");
    let live_before = machine.regions().arena().live();
    assert!(live_before > 0, "the fixture is there to be disturbed");

    let (_arena, cell) = live_cell();
    machine
        .call("m.identity", vec![cell], Span::DUMMY)
        .expect_err("refused");

    assert_eq!(
        machine.regions().arena().live(),
        live_before,
        "the refusal ran before the reset"
    );
}

/// The reason the entry-point check is load-bearing rather than belt-and-braces,
/// stated over the allocator: a reset restores the fixture's generations, so a
/// slot taken out of an earlier run resolves afterwards. That is correct — a
/// `Value::Cell` handed to the caller as part of a fixture has to keep working —
/// and it is exactly why the boundary above cannot rely on the slot going stale.
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

// -------------------------------------------------------- 3. the host boundary

/// A handler that answers with a handle it minted. `TaskId` is constructible
/// from outside `ply-eval`, which is what makes this attack writable at all —
/// and what makes the check on the answer necessary rather than theoretical.
struct Forges;

impl HostHandler for Forges {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        Ok(HostAnswer::Value(Value::Task(ply_eval::TaskId(0))))
    }
}

/// Wraps the forged handle in a constructor, so what is under test is the walk
/// rather than a top-level match.
struct ForgesInside;

impl HostHandler for ForgesInside {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        Ok(HostAnswer::Value(Value::Ctor {
            name: Symbol::new("m.Just"),
            args: Arc::new(vec![Value::Task(ply_eval::TaskId(0))]),
        }))
    }
}

#[derive(Default)]
struct Counts {
    calls: AtomicU64,
}

impl HostHandler for Counts {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(HostAnswer::Value(Value::Int(n as i64)))
    }
}

#[test]
fn a_handler_that_answers_with_a_handle_is_refused_and_named() {
    let compiled = Compiled::new(ASKS);
    let binding = bound(
        &compiled,
        vec![(
            op("ext", "ask", Linearity::Repeatable),
            Arc::new(Forges) as Arc<dyn HostHandler>,
        )],
    );
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));

    let d = machine
        .eval_test(0)
        .expect_err("a forged handle is refused");

    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
    assert!(d.message.contains("ext.ask[socket]"), "{}", d.message);
    assert!(d.message.contains("`Task`"), "{}", d.message);
    assert!(
        d.notes.iter().any(|n| n.contains("test::forge")),
        "the handler is named: {:#?}",
        d.notes
    );
}

#[test]
fn a_handle_wrapped_in_a_constructor_by_a_handler_is_refused_too() {
    let compiled = Compiled::new(ASKS);
    let binding = bound(
        &compiled,
        vec![(
            op("ext", "ask", Linearity::Repeatable),
            Arc::new(ForgesInside) as Arc<dyn HostHandler>,
        )],
    );
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));

    let d = machine
        .eval_test(0)
        .expect_err("a wrapped handle is refused");
    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
    assert!(
        d.message.contains("`m.Just`'s argument 1"),
        "the route is named: {}",
        d.message
    );
}

/// The boundary is not a wall. A handler answering with data is the ordinary
/// case and stays untouched — including the linearity accounting beside it,
/// which a refusal placed in the wrong order would have skipped.
#[test]
fn a_handler_answering_with_data_is_untouched() {
    let compiled = Compiled::new(ASKS);
    let counts = Arc::new(Counts::default());
    let binding = bound(
        &compiled,
        vec![(
            op("ext", "ask", Linearity::AtMostOnce),
            counts.clone() as Arc<dyn HostHandler>,
        )],
    );
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(binding));

    machine.eval_test(0).expect("data crosses");
    assert_eq!(counts.calls.load(Ordering::SeqCst), 1);
    assert_eq!(machine.host_ops(), 1);
}

/// `E0449` is the machine's verdict about its own memory, so a handler may not
/// mint it: `attribute` rewrites a reserved code to `E0502` and adds the note
/// naming the handler. Without this, a handler could answer with the code that
/// says "the run handed me a handle into a region" and send the reader looking
/// for a defect in the program.
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

/// The argument half of the same boundary. No source-level route reaches it
/// today — §2's static checks refuse a branded value at every `perform` — so it
/// is driven through the check itself, which is what a backstop's test can
/// honestly assert. The wiring beside it is covered by the answer tests, which
/// share `perform_host`.
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

/// A trace span attribute and a log line are this boundary and not a new one:
/// `std.trace` is served by a host handler, so a value reaching a field crosses
/// `perform_host` exactly as a database parameter does.
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

    // And what a sink writes is text: a handle renders opaquely and is never
    // dereferenced, so the record cannot carry the region into the log.
    let (_a, c) = live_cell();
    let rendered = c.render();
    assert!(rendered.starts_with("<cell "), "{rendered}");
    assert!(!rendered.contains("41"), "the slot's contents are not read");
}

// ------------------------------------------------------- 4. the result cache

/// `boxed` is nullary and publishes an empty row, which makes it a constant by
/// the memo's rule, and its value is a slot in this run's arena — the exact
/// shape `memo.rs` says must not be remembered. Two entry points on one machine
/// is what catches it: the second one runs after a reset, so a remembered value
/// would hand it a slot the first run allocated.
///
/// Driven through the machine rather than through `Memo` directly, because what
/// is under test is that the rule is *applied* and not that it is written down.
#[test]
fn a_constant_whose_value_reaches_a_region_is_not_remembered_across_runs() {
    let compiled = Compiled::new(CONSTANT_OVER_A_CELL);
    let mut machine = compiled.machine();

    for run in 0..3 {
        machine
            .eval_test(0)
            .unwrap_or_else(|d| panic!("run {run} must read this run's own cell: {d:#?}"));
    }

    // And the same on the other engine, or the two disagree about a resource
    // bound and `--engine both` reports it as `E0503`.
    let mut interp = compiled.interp();
    for run in 0..3 {
        interp
            .eval_test(0)
            .unwrap_or_else(|d| panic!("tree-walker run {run}: {d:#?}"));
    }
}

// -------------------------------------------- 5. a stale access, not a value

/// The property that makes every refusal above a bound rather than a hope: when
/// a handle does reach a slot whose region has closed, the read is a diagnostic
/// and never the value that now lives at that position.
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

// ------------------------------------- 6. the boundaries closed elsewhere

/// An M8 counterexample and a shrunk witness are closed at the law, not at the
/// runtime: `E0418` refuses a binder whose type the generator cannot inhabit, so
/// no generated value and no shrunk witness can hold a handle in the first
/// place.
///
/// `ply-core`'s `a_law_cannot_quantify_over_a_type_the_generator_cannot_inhabit`
/// pins the bare and the variant shapes. The record is here because it is the
/// shape a counterexample most plausibly has and because it is the one that
/// needs `ungeneratable` to walk a field rather than an argument.
#[test]
fn a_law_cannot_quantify_over_a_record_that_reaches_a_region() {
    let diags = Compiled::refused(r#"law "no" forall (r: {c: Cell<Int>}) { true }"#);
    assert!(
        diags.iter().any(|d| d.code == codes::UNQUANTIFIABLE_TYPE),
        "a record holding a cell must be refused where the law is written: {diags:#?}"
    );
}

/// A value crossing into a task is deliberately *not* refused — ADR 0017 §2
/// excludes `task.spawn` from a bare `with_cell`'s rule because a cell reaching
/// a task is how tasks share memory — and §3 is what makes that safe: a `task`
/// operation anywhere in a region infers `shared`, and a shared region's slots
/// outlive its close.
#[test]
fn a_cell_reaching_a_task_still_runs_and_its_region_is_shared() {
    let src = r#"
test "two tasks share one cell" {
  let total = simulate {
    with_cell[s](0) { c -> {
      let a = task.spawn(|| cell_set(c, cell_get(c) + 1));
      let b = task.spawn(|| cell_set(c, cell_get(c) + 10));
      task.join(a);
      task.join(b);
      cell_get(c)
    } }
  };
  assert_eq(total, 11)
}
"#;
    let compiled = Compiled::new(src);
    compiled
        .machine()
        .eval_test(0)
        .unwrap_or_else(|d| panic!("the landed shape must still run: {d:#?}"));

    let regions = ply_eval::region_kind::infer(&compiled.program, &compiled.resolved);
    assert!(
        regions.iter().all(|r| r.kind == RegionKind::Shared),
        "a region a task reaches may not be `unique`: {:#?}",
        regions.iter().collect::<Vec<_>>()
    );
}

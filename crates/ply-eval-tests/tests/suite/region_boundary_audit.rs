// `Value`'s shared payloads are `Arc` and deliberately thread-confined.
#![allow(clippy::arc_with_non_send_sync)]

use crate::fixture::Compiled;
use ply_eval::arena::Owner;
use ply_eval::escape::Boundary;
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity,
};
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
    registry
        .bind(&compiled.front.check)
        .expect("the registry binds")
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

pub fn resumed(n: Int) -> Int = n + resume_it(parked())

fn identity(n: Int) -> Int = n

test "the parked continuation still reads its region's cell" {
  assert_eq(resume_it(parked()), 41)
}

test "a later entry resumes the continuation it parked itself" {
  assert_eq(resume_it(parked()), 41)
}
"#;

/// A constant whose body reads a cell of the region it opens.
const CONSTANT_OVER_A_CELL: &str = r#"
fn boxed() -> Int = with_cell[log](41) { c -> cell_get(c) }

test "the constant answers what its cell held" {
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

/// The body reads a cell of the region enclosing its `handle`, and is resumed only once that region
/// has closed: the region's cell outlives its close for as long as the body can run.
#[test]
fn the_parked_continuation_runs_on_the_tier_and_reads_its_regions_cell() {
    let compiled = Compiled::new(PARKED);
    let (mut machine, tier) = compiled.machine_and_tier();
    let test = compiled.index_of("the parked continuation still reads its region's cell");

    machine
        .eval_test(test)
        .into_parts()
        .0
        .expect("the resumed body reads 41 from the closed region's cell");

    assert_eq!(tier.declines().total(), 0, "{:?}", tier.declines());
}

/// `parked` is pure and nullary, so the memo is offered its answer; a `k` it kept would name a
/// body only the first entry held, and the second entry would resume nothing.
#[test]
fn two_entries_on_one_tier_each_resume_the_continuation_they_parked() {
    let compiled = Compiled::new(PARKED);
    let (mut machine, tier) = compiled.machine_and_tier();

    for name in [
        "the parked continuation still reads its region's cell",
        "a later entry resumes the continuation it parked itself",
    ] {
        machine
            .eval_test(compiled.index_of(name))
            .into_parts()
            .0
            .unwrap_or_else(|d| panic!("{name}: {d:#?}"));
    }

    assert_eq!(tier.declines().total(), 0, "{:?}", tier.declines());
}

/// `Saved`'s field is an ordinary function type, so the checker lets `k` reach `parked`'s answer,
/// and the seam refuses it there as the program's error.
#[test]
fn a_continuation_in_an_entrys_answer_is_the_programs_error_and_no_decline() {
    let compiled = Compiled::new(PARKED);
    let (mut machine, tier) = compiled.machine_and_tier();

    let d = machine
        .call("m.parked", vec![], Span::DUMMY)
        .into_parts()
        .0
        .expect_err("a continuation does not leave the entry that captured it");

    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY, "{d:#?}");
    assert!(!codes::is_defect(d.code));
    assert!(
        d.message.contains("`m.parked` answered a continuation"),
        "{}",
        d.message
    );
    assert!(d.message.contains("Just`'s argument 1"), "{}", d.message);
    let at = d
        .labels
        .iter()
        .find(|l| l.primary)
        .expect("the refusal is placed")
        .span;
    assert!(PARKED[at.range()].starts_with("fn parked()"), "{d:#?}");
    assert_eq!(tier.declines().total(), 0, "{:?}", tier.declines());
    assert_eq!(machine.compiled_counts(), (1, 0));
}

/// The seam offers the memo a constant's answer before it refuses one that holds a continuation,
/// and a `k` kept there would answer the next entry past the refusal and reach its every call.
#[test]
fn a_constant_the_seam_refuses_keeps_no_continuation_for_the_next_entry() {
    let compiled = Compiled::new(PARKED);
    let (mut machine, tier) = compiled.machine_and_tier();

    for entry in 0..2 {
        let d = machine
            .call("m.parked", vec![], Span::DUMMY)
            .into_parts()
            .0
            .expect_err("a continuation does not cross out of the tier");
        assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY, "entry {entry}");
    }
    assert_eq!(tier.declines().total(), 0, "{:?}", tier.declines());

    machine
        .eval_test(compiled.index_of("the parked continuation still reads its region's cell"))
        .into_parts()
        .0
        .expect("the test parks and resumes a continuation of its own");
}

/// A callback's row is its caller's to choose, so the `simulate` region that hands it a task cannot
/// see the task stored in an older cell, and `spawned`'s task reaches its answer past the checker.
const HANDED: &str = r#"
fn simulated<| e>(on: (Task<Int>) -> Unit / e) -> Unit / {sim.read | e} =
  simulate { on(task.spawn(|| 1)) }

fn spawned() -> Option<Task<Int>> / {sim.read} = with_cell[slot](None) { kept -> {
  simulated(|t: Task<Int>| cell_set(kept, Some(t)));
  cell_get(kept)
} }
"#;

#[test]
fn a_task_in_an_entrys_answer_is_refused_as_a_continuation_is() {
    let compiled = Compiled::new(HANDED);
    let (mut machine, tier) = compiled.machine_and_tier();

    let d = machine
        .call("m.spawned", vec![], Span::DUMMY)
        .into_parts()
        .0
        .expect_err("a task does not leave the region that spawned it");

    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY, "{d:#?}");
    assert!(
        d.message.contains("`m.spawned` answered a `Task`"),
        "{}",
        d.message
    );
    assert_eq!(tier.declines().total(), 0, "{:?}", tier.declines());
}

/// A type parameter hides the task `spawned` let out from the next region, which numbers a task of
/// its own `@1` as well: joined there by id alone, the handle would answer that task's `2`.
const REJOINED: &str = r#"
fn joined<a>(x: a, wait: (a) -> Int / {task.join}) -> Int / {sim.read} =
  simulate {
    let mine = task.spawn(|| 2);
    wait(x) + task.join(mine)
  }

pub fn rejoined() -> Int / {sim.read} = match spawned() {
  Some(t) -> joined(t, |h: Task<Int>| task.join(h)),
  None -> 0,
}
"#;

#[test]
fn a_task_carried_into_another_region_fails_its_join_rather_than_answering_a_stranger() {
    let compiled = Compiled::new(&format!("{HANDED}{REJOINED}"));
    let (mut machine, tier) = compiled.machine_and_tier();

    let d = machine
        .call("m.rejoined", vec![], Span::DUMMY)
        .into_parts()
        .0
        .expect_err("the handle names a task of the first region");

    assert_eq!(d.code, codes::TASK_ESCAPES_SCOPE, "{d:#?}");
    assert!(d.message.contains("another region"), "{}", d.message);
    assert_eq!(tier.declines().total(), 0, "{:?}", tier.declines());
}

#[test]
fn the_constant_memo_keeps_no_answer_that_holds_a_continuation() {
    let native = Compiled::new(PARKED).native();
    let parked = native
        .constant_index("m.parked")
        .expect("`parked` is pure and nullary, so the memo is offered its answer");
    let entry = native.entry("m.resumed").expect("`resumed` compiled");
    let tables = native.tables().clone();
    let mut ctx = native.context();

    for n in 0..2 {
        ctx.begin(10_000);
        let arg = ctx.heap.to_word(&tables.layouts, &Value::Int(n));
        let answer = unsafe { entry(&mut ctx, [arg].as_ptr()) };
        assert_eq!(ctx.failed, 0, "entry {n}: {:?}", ctx.diagnostic);
        assert_eq!(
            ply_codegen::heap::Heap::to_value(&tables.layouts, answer),
            Value::Int(n + 41),
            "entry {n}"
        );
        ctx.end();
        assert_eq!(
            tables.memoized(parked),
            None,
            "entry {n} left `parked`'s continuation for the next"
        );
    }
}

#[test]
fn a_cell_from_another_arena_is_refused_at_the_entry_point() {
    let compiled = Compiled::new(PARKED);
    let (_arena, cell) = live_cell();

    let d = compiled
        .machine()
        .call("m.identity", vec![cell], Span::DUMMY)
        .into_parts()
        .0
        .expect_err("a cell may not enter a run");

    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
    assert!(d.message.contains("`Cell`"), "{}", d.message);
}

/// `k` converted out of the entry that parked it is refused where a smuggled cell is, before the
/// run begins, rather than resumed into a body that is gone.
#[test]
fn a_continuation_from_another_entry_is_refused_at_the_entry_point() {
    let compiled = Compiled::new(PARKED);
    let native = compiled.native();
    let entry = native.entry("m.parked").expect("`parked` compiled");
    let mut ctx = native.context();
    ctx.begin(10_000);
    let out = unsafe { entry(&mut ctx, std::ptr::null()) };
    assert_eq!(ctx.failed, 0, "`parked`: {:?}", ctx.diagnostic);
    let saved = ply_codegen::heap::Heap::to_value(&native.tables().layouts, out);
    ctx.end();

    let d = compiled
        .machine()
        .call("m.resume_it", vec![saved], Span::DUMMY)
        .into_parts()
        .0
        .expect_err("a continuation may not enter a run");

    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY, "{}", d.message);
    assert!(
        d.message.contains("was called with a continuation"),
        "{}",
        d.message
    );
    assert!(d.message.contains("Just`'s argument 1"), "{}", d.message);
}

#[test]
fn data_still_crosses_the_entry_point() {
    let compiled = Compiled::new(PARKED);
    assert_eq!(
        compiled
            .machine()
            .call("m.identity", vec![Value::Int(7)], Span::DUMMY)
            .into_parts()
            .0
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

    let d = machine
        .eval_test(0)
        .into_parts()
        .0
        .expect_err("the handler refused");
    assert_eq!(
        d.code,
        codes::RUNTIME_ERROR,
        "the classification is taken back from the handler"
    );
    assert_eq!(d.message, "a handler claiming the machine's own verdict");
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains(codes::REGION_ESCAPE_AT_BOUNDARY)),
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

/// `boxed` is nullary with an empty row, so it is a constant: the memo keeps what its cell held.
#[test]
fn a_constant_whose_body_opens_a_region_answers_every_run() {
    let compiled = Compiled::new(CONSTANT_OVER_A_CELL);
    let mut machine = compiled.machine();

    for run in 0..3 {
        machine
            .eval_test(0)
            .into_parts()
            .0
            .unwrap_or_else(|d| panic!("run {run} must answer what the cell held: {d:#?}"));
    }
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

/// `E0418` refuses a binder the generator cannot inhabit, so no generated value can hold a handle.
#[test]
fn a_law_cannot_quantify_over_a_record_that_reaches_a_region() {
    let diags = Compiled::rejected(r#"law "no" forall (r: {c: Cell<Int>}) { true }"#);
    assert!(
        diags.iter().any(|d| d.code == codes::UNQUANTIFIABLE_TYPE),
        "a record holding a cell must be refused where the law is written: {diags:#?}"
    );
}

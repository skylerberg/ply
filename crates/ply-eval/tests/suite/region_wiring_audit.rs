//! The regions a *program* opens, on the evaluation path.

use ply_core::{CheckOutput, check_program};
use ply_eval::{Machine, RegionKind, Value};
use ply_span::SourceId;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        let inputs = [(SourceId(0), ModuleName::from_dotted("m"), src)];
        let mut program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
        let resolved =
            resolve(&mut program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        Compiled {
            program,
            resolved,
            check,
        }
    }

    fn index_of(&self, name: &str) -> usize {
        self.check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }

    fn regions(&self) -> ply_eval::region_kind::Regions {
        ply_eval::region_kind::infer(&self.program, &self.resolved)
    }
}

/// Regions every run opens before the program does: the fixture's, the entry region the stack was
/// built with, and the one the entry point's reset opens in its place.
const SCAFFOLD_REGIONS: u64 = 3;

fn machine_stats(compiled: &Compiled, name: &str) -> ply_eval::arena::Stats {
    let index = compiled.index_of(name);
    let mut machine = Machine::new(&compiled.program, &compiled.resolved, &compiled.check);
    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("{name:?} must run on the machine: {d:#?}"));
    machine.cells().stats()
}

// -------------------------------------------- 1. the analysis is consulted

const PURE: &str = r#"
fn scratch(n: Int) -> Int = with_cell[r](n) { c -> { cell_set(c, cell_get(c) + 1); cell_get(c) } }

test "a region with no capture anywhere near it" {
  assert_eq(scratch(41), 42)
}
"#;

/// The claim R1 could not make about `region_kind`: an engine asks it, and asks it about the span
/// of the `with_cell` expression rather than about the body's.
#[test]
fn a_with_cell_opens_the_scope_the_inference_decided_for_its_span() {
    let compiled = Compiled::new(PURE);
    let regions = compiled.regions();
    assert_eq!(regions.len(), 1, "one `with_cell`, one region");
    assert_eq!(
        regions.iter().next().map(|r| r.kind),
        Some(RegionKind::Unique),
        "nothing in this program captures a continuation"
    );

    {
        let stats = machine_stats(&compiled, "a region with no capture anywhere near it");
        assert_eq!(stats.allocations, 1, "the cell is a bump in the arena");
        assert_eq!(
            stats.regions_opened,
            SCAFFOLD_REGIONS + 1,
            "the `with_cell` opened a scope of its own"
        );
        assert_eq!(
            stats.pins_taken, 0,
            "nothing captured, so nothing was pinned"
        );
    }
}

// ------------------------------------------------ 2. the close is the point

const LOOPED: &str = r#"
fn once(n: Int) -> Int = with_cell[r](n) { c -> cell_get(c) }

fn upto(n: Int) -> Int = if n == 0 { once(0) } else { once(n) + upto(n - 1) }

test "a region per iteration" { assert_eq(upto(99), 4950) }
"#;

/// The shape ADR 0005 §2 charged one retained world entry per iteration for, and which R1's wiring
/// still charged one arena slot per iteration for.
#[test]
fn a_region_in_a_loop_costs_one_slot_rather_than_one_per_iteration() {
    let compiled = Compiled::new(LOOPED);
    let stats = machine_stats(&compiled, "a region per iteration");
    assert_eq!(stats.allocations, 100);
    assert_eq!(
        stats.peak_live, 1,
        "each region closed before the next opened"
    );
    assert_eq!(stats.closes_deferred, 0, "no capture, no deferral");
}

const CAPTURED: &str = r#"
effect amb { read flip[coin]() -> Bool }

test "two resumptions over one cell" {
  with_cell[trace](0) { c -> {
    let answer = handle {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { 10 } else { 20 }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    };
    assert_eq(answer, 30);
    assert_eq(cell_get(c), 2)
  } }
}
"#;

/// The capture path pins.
#[test]
fn a_capture_inside_a_region_pins_it() {
    let compiled = Compiled::new(CAPTURED);
    let machine = machine_stats(&compiled, "two resumptions over one cell");
    assert!(
        machine.pins_taken > 0,
        "the clause bound a continuation and nothing claimed the region"
    );
    assert_eq!(
        machine.allocations, 1,
        "one cell served both resumptions — ADR 0017 §3's two-resumption example"
    );
}

const PARKED: &str = r#"
effect amb { read flip[coin]() -> Bool }

type Saved = Nothing | Just((Bool) -> Int)

test "a continuation outlives the region whose cell it reads" {
  with_cell[slot](Nothing) { s -> {
    let inner = with_cell[log](7) { c ->
      handle {
        let b = amb.flip[coin]();
        if b { cell_get(c) } else { 0 }
      } with {
        amb.flip[coin]() resume k -> { cell_set(s, Just(k)); 0 },
        return x -> x
      }
    };
    assert_eq(inner, 0);
    match cell_get(s) {
      Just(k) -> assert_eq(k(true), 7),
      Nothing -> assert(false)
    }
  } }
}
"#;

/// ADR 0005 required test 6, at the reclamation event R2 added.
#[test]
fn a_close_a_live_continuation_can_reach_is_deferred_rather_than_taken() {
    let compiled = Compiled::new(PARKED);
    let machine = machine_stats(
        &compiled,
        "a continuation outlives the region whose cell it reads",
    );
    assert!(machine.pins_taken > 0);
    assert!(
        machine.closes_deferred > 0,
        "the inner region's close had to be deferred, or `k(true)` read a freed slot"
    );
}

const NESTED_REGION: &str = r#"
fn doubled(n: Int) -> Int = with_region[r] {
  with_cell[r](n) { c -> { cell_set(c, cell_get(c) * 2); cell_get(c) } }
}

test "a with_region around a cell of its own brand" { assert_eq(doubled(21), 42) }
"#;

/// `with_region` lowered to its body until now, so nothing at run time
/// distinguished it from the code inside it.
#[test]
fn a_with_region_opens_one_region_and_the_cell_inside_it_opens_none() {
    let compiled = Compiled::new(NESTED_REGION);
    let regions = compiled.regions();
    assert_eq!(
        regions.len(),
        1,
        "the `with_cell[r]` shares the brand, so there is one region and not two"
    );

    let stats = machine_stats(&compiled, "a with_region around a cell of its own brand");
    assert_eq!(
        stats.regions_opened,
        SCAFFOLD_REGIONS + 1,
        "one region for `with_region` and none for the cell inside it"
    );
    assert_eq!(stats.allocations, 1);
    assert_eq!(
        stats.peak_live, 1,
        "and the cell went back at the region's close"
    );
}

const NO_REGION: &str = r#"
effect ask { read get[q]() -> Int }

fn asked() -> Int = handle { ask.get[q]() + ask.get[q]() } with {
  ask.get[q]() resume k -> k(3),
}

test "a capture outside every region" { assert_eq(asked(), 6) }
"#;

/// A pin is an `Rc` allocation and `handler::perform` is on the request path, so a program with no
/// region of its own must not pay for one.
#[test]
fn a_capture_outside_every_program_region_takes_no_pin() {
    let compiled = Compiled::new(NO_REGION);
    let machine = machine_stats(&compiled, "a capture outside every region");
    assert_eq!(
        machine.regions_opened, SCAFFOLD_REGIONS,
        "the program opened none of its own"
    );
    assert_eq!(
        machine.pins_taken, 0,
        "there is no lexical close for a pin to defer"
    );
}

/// The reset is what replaced `World::fork`, and a pin must not be able to outlive it: a
/// continuation parked in a cell of the region that pins it is a cycle, and honouring it at the
/// entry point's end would leak the run and let the next one read its slots.
#[test]
fn a_parked_continuation_does_not_survive_the_entry_point_that_made_it() {
    let compiled = Compiled::new(PARKED);
    let index = compiled.index_of("a continuation outlives the region whose cell it reads");
    let mut machine = Machine::new(&compiled.program, &compiled.resolved, &compiled.check);

    for _ in 0..3 {
        machine.eval_test(index).expect("the test passes");
        assert_eq!(
            machine.cells().live(),
            0,
            "the entry point ended holding nothing"
        );
        assert_eq!(
            machine.cells().retained_slots(),
            0,
            "and holding nothing on behalf of a continuation either"
        );
    }
    assert_eq!(
        machine.cells().stats().peak_live,
        2,
        "two regions live at once, never six across three runs"
    );
}

/// A fixture cell is not a program region's, so it survives every entry point — the property
/// `TaskRegions::reset` exists for, restated now that a lexical close reclaims.
#[test]
fn the_fixture_survives_what_the_regions_give_back() {
    let compiled = Compiled::new(PURE);
    let index = compiled.index_of("a region with no capture anywhere near it");
    let fixture = ply_eval::Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(1_000))));
    let (regions, handle) = fixture.open();
    let slot = match handle {
        Value::Cell(slot) => slot,
        other => panic!("expected the fixture's handle, found {other:?}"),
    };

    let mut machine = Machine::new(&compiled.program, &compiled.resolved, &compiled.check);
    machine.set_regions(regions);
    for _ in 0..5 {
        machine.eval_test(index).expect("the test passes");
        assert_eq!(
            machine.cells().get(slot).map(Value::render),
            Some("1000".to_string()),
            "the fixture's slot is below every region's mark"
        );
        assert_eq!(machine.cells().live(), 1, "and it is the only thing left");
    }
}

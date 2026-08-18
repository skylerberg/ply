//! One program, one region-kind analysis — however many engines run it.
//!
//! `region_kind_inference.rs` asserts *what* the analysis decides and
//! `region_wiring_audit.rs` asserts that an engine consults it. Neither says
//! anything about **when** it runs, and until R3 the answer was "once per
//! `Machine`": the cell holding the result was a field of the engine, and a
//! `Machine` is built per entry point — per worker per concurrency group under
//! `ply test`, per obligation under `ply prove`, per measured window in the
//! corpus harness. A static property of the program was therefore recomputed at
//! runtime, and the recomputation allocated.
//!
//! The cell is now [`ply_eval::region_kind::Kinds`], a handle an engine can be
//! handed rather than a field it owns. What has to be asserted about that is:
//!
//! - a handed analysis is the *same* analysis, not an equal one — otherwise it
//!   was recomputed and nothing was hoisted;
//! - it answers exactly what a privately-inferred one answers, region by
//!   region, so nothing about the decision moved with it;
//! - meaning did not move: ADR 0017 §3 as amended pins the two-resumption trace
//!   cell at `2`, and it still reads `2` on an engine running off a shared
//!   analysis;
//! - an engine handed nothing still works, because `Machine::for_program` and
//!   every test in this repository builds one that way.
//!
//! What is **not** asserted here, and cannot be, is that a `Kinds` is never
//! shared between two *different* programs. That is a caller's obligation and
//! the type does not enforce it; `ply_eval::region_kind::Kinds`' own
//! documentation states the hazard, and
//! `region_kind_inference.rs::a_capture_in_an_unrelated_module_makes_a_region_shared`
//! is the program that shows what a wrong answer buys — a `unique` where the
//! program captures, which frees an arena a continuation still reaches.

use ply_core::{CheckOutput, check_program};
use ply_eval::region_kind::Kinds;
use ply_eval::{Interp, Machine, RegionKind};
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

/// Both kinds in one program, and a region reached only through a call, so the
/// propagation step has something to do as well as the direct scan.
const BOTH_KINDS: &str = r#"
effect amb { read flip[coin]() -> Bool }

fn scratch(n: Int) -> Int =
  with_cell[pure](n) { c -> { cell_set(c, cell_get(c) + 1); cell_get(c) } }

fn searched() -> Int =
  with_cell[trace](0) { c ->
    handle {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { cell_get(c) } else { cell_get(c) * 10 }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    } }

test "the pure region" { assert_eq(scratch(41), 42) }

test "the shared region" { assert_eq(searched(), 21) }
"#;

/// A handed analysis is *the same allocation*, which is the only observable
/// that separates "shared" from "inferred twice and happened to agree".
#[test]
fn an_engine_handed_an_analysis_does_not_infer_one_of_its_own() {
    let compiled = Compiled::new(BOTH_KINDS);

    let first = compiled.machine();
    let filled = first.region_kinds();
    assert!(filled.len() >= 2, "{filled:?}");

    let mut second = compiled.machine();
    second.share_region_kinds(first.shared_region_kinds());
    assert!(
        std::ptr::eq(filled, second.region_kinds()),
        "the second machine holds a different `Regions`, so it inferred its own"
    );

    // The tree-walker takes the machine's, which is what keeps `--engine both`
    // comparing two engines over one region structure rather than over two.
    let mut treewalk = compiled.interp();
    treewalk.share_region_kinds(first.shared_region_kinds());
    assert!(
        std::ptr::eq(filled, treewalk.region_kinds()),
        "the tree-walker inferred its own"
    );
}

/// An engine handed nothing infers its own, and every engine in this repository
/// that is handed nothing has always done so. Removing that would break
/// `Machine::for_program`, which has no check output to hang an analysis off.
#[test]
fn an_engine_handed_nothing_still_answers() {
    let compiled = Compiled::new(BOTH_KINDS);
    let alone = compiled.machine();
    assert!(alone.region_kinds().len() >= 2);
    assert_eq!(alone.region_kinds().shared(), 1);
}

/// Region by region, on both engines: a shared analysis decides what a private
/// one decides. An equality over the whole set would pass on a program with one
/// region and no capture, which is why the two kinds are both present above and
/// both are checked by span.
#[test]
fn a_shared_analysis_answers_what_a_private_one_answers() {
    let compiled = Compiled::new(BOTH_KINDS);

    let private = ply_eval::region_kind::infer(&compiled.program, &compiled.resolved);
    let shared: Kinds = Kinds::default();

    let mut machine = compiled.machine();
    machine.share_region_kinds(Kinds::clone(&shared));
    let mut treewalk = compiled.interp();
    treewalk.share_region_kinds(Kinds::clone(&shared));

    assert!(
        private.iter().any(|r| r.kind == RegionKind::Unique)
            && private.iter().any(|r| r.kind == RegionKind::Shared),
        "this fixture must carry both kinds or it discriminates nothing: {private:?}"
    );

    for region in private.iter() {
        assert_eq!(
            machine.region_kind(region.span),
            Some(region.kind),
            "the machine's shared analysis disagrees about `{}`",
            region.brand
        );
        assert_eq!(
            treewalk.region_kinds().at(region.span).map(|r| r.kind),
            Some(region.kind),
            "the tree-walker's shared analysis disagrees about `{}`",
            region.brand
        );
    }
    assert_eq!(machine.region_kinds().len(), private.len());
}

/// ADR 0017 §3 as amended, run off a shared analysis.
///
/// The kind decides when the arena goes back, not what a resumption observes,
/// so hoisting where the kind is computed may not move this. One threaded
/// state: the first branch increments the cell to `1`, the second resumption
/// starts from that and reaches `2`, so the total is `1 + 2*10 = 21` and the
/// cell ends at `2`. Snapshot-at-capture answers `11` and `1`.
#[test]
fn the_two_resumption_trace_cell_still_reads_two_under_a_shared_analysis() {
    let compiled = Compiled::new(
        r#"
effect amb { read flip[coin]() -> Bool }

test "the second resumption starts from the first one's writes" {
  with_cell[trace](0) { c -> {
    let total = handle {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { cell_get(c) } else { cell_get(c) * 10 }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    };
    assert_eq(total, 21);
    assert_eq(cell_get(c), 2)
  } }
}
"#,
    );
    let index = compiled.index_of("the second resumption starts from the first one's writes");

    // Filled by a machine that has already run the program, then handed to a
    // second one: the order a runner actually produces, where the worker that
    // fills it is not the worker that reads it.
    let filler = compiled.machine();
    let shared = filler.shared_region_kinds();
    let _ = filler.region_kinds();

    let mut machine = compiled.machine();
    machine.share_region_kinds(shared);
    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("[{}] {}", d.code, d.message));
}

//! A cell is a slot in the region that allocated it.
//!
//! ADR 0017 §1 makes `with_cell[r]` a region and the cell a value allocated in
//! it. This file is the evidence that the arena is actually on the evaluation
//! path — R1 built the allocator and connected nothing, and a benchmark rather
//! than a report is what found that out — and that connecting it moved no
//! program's meaning.
//!
//! What is asserted, in order:
//!
//! - the allocation goes through [`Arena`], counted by the arena's own `Stats`;
//! - reads and writes cross region nesting in both directions, and the region's
//!   lexical close hands the slots back — the reclamation event ADR 0017 §3
//!   describes, which a region that never closed did not have. The values are
//!   asserted from inside the program, where the cell is still live, rather than
//!   from the arena after the run, where it is correctly gone;
//! - a write made inside a resumption is visible after it, and resumption *n*
//!   observes resumption *n−1*'s writes — ADR 0017 §3 as amended, which is ADR
//!   0005 §3. The two-resumption example answers `30` with its trace cell at
//!   `2`, and that integer is the one that distinguishes threading from the
//!   retracted snapshot-at-capture reading;
//! - a `Value::Cell` from a previous entry point reads *nothing* rather than
//!   this run's cell, which is what the generation in a [`Slot`] buys and what
//!   a `CellId` could not do;
//! - the arena is reused across entry points, so a run that has been through
//!   one region takes no chunk from the global allocator for the next.

use ply_core::{CheckOutput, check_program};
use ply_eval::{Interp, Machine};
use ply_span::{SourceId, Span, codes};
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

    fn treewalk(&self) -> Interp<'_> {
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

/// Runs `name` on the machine and answers what the arena did: how many slots it
/// bumped, the high-water mark, and what it still holds afterwards.
///
/// The program's own `assert_eq`s are what check the values, because they run
/// where the cell is live. This is the cost side.
fn arena_after(compiled: &Compiled, name: &str) -> ply_eval::arena::Stats {
    let index = compiled.index_of(name);
    let mut machine = compiled.machine();
    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("{name:?} must run: {d:#?}"));
    assert_eq!(
        machine.cells().live(),
        0,
        "{name:?} closed every region it opened, so nothing is still held"
    );
    machine.cells().stats()
}

/// The same on the tree-walker, so a claim about the cell path is a claim about
/// both engines wherever the tree-walker can express the program.
fn treewalk_arena_after(compiled: &Compiled, name: &str) -> ply_eval::arena::Stats {
    let index = compiled.index_of(name);
    let mut interp = compiled.treewalk();
    interp
        .eval_test(index)
        .unwrap_or_else(|d| panic!("{name:?} must run on the tree-walker: {d:#?}"));
    assert_eq!(
        interp.cells().live(),
        0,
        "{name:?} closed every region it opened on the tree-walker too"
    );
    interp.cells().stats()
}

// ------------------------------------------------- 1. the allocation is a bump

const NESTED: &str = r#"
test "one region one cell" {
  with_cell[a](1) { c -> assert_eq(cell_get(c), 1) }
}

test "an inner region reads and writes the outer one's cell" {
  with_cell[outer](1) { o -> {
    with_cell[inner](10) { i -> {
      cell_set(o, cell_get(o) + cell_get(i));
      cell_set(i, cell_get(o) * 2);
      assert_eq(cell_get(i), 22)
    } };
    assert_eq(cell_get(o), 11)
  } }
}

test "a sibling region does not see its neighbour's cell" {
  with_cell[first](1) { a -> cell_set(a, 7) };
  with_cell[second](2) { b -> assert_eq(cell_get(b), 2) }
}
"#;

/// The claim R1 could not make: the allocation is the arena's, and the arena's
/// own counter says so. The claim R1's wiring could not make either: the slot
/// goes back at the region's lexical close.
#[test]
fn a_with_cell_allocates_a_slot_in_the_arena_and_gives_it_back_at_the_close() {
    let compiled = Compiled::new(NESTED);
    let stats = arena_after(&compiled, "one region one cell");

    assert_eq!(
        stats.allocations, 1,
        "the cell came from the bump pointer rather than from a map"
    );
    assert_eq!(stats.peak_live, 1, "and it was live while the body ran");
    assert_eq!(
        stats.closes_deferred, 0,
        "no continuation was captured, so nothing deferred the close"
    );
}

/// The program asserts the values from inside, where the cells are live: the
/// inner region writes the outer one's cell and reads its own back. What is
/// asserted here is the shape of the arena the two regions leave — two bumps,
/// both live at once, both handed back.
#[test]
fn reads_and_writes_cross_region_nesting_in_both_directions() {
    let compiled = Compiled::new(NESTED);
    let name = "an inner region reads and writes the outer one's cell";
    for stats in [
        arena_after(&compiled, name),
        treewalk_arena_after(&compiled, name),
    ] {
        assert_eq!(stats.allocations, 2);
        assert_eq!(
            stats.peak_live, 2,
            "the inner region nests inside the outer"
        );
    }
}

/// Nesting is not the only shape: two regions in sequence are two bumps, and
/// the second reuses the position the first gave back — one slot live at a time
/// rather than two, which is the whole of what a close buys.
#[test]
fn two_regions_in_sequence_reuse_one_position() {
    let compiled = Compiled::new(NESTED);
    let name = "a sibling region does not see its neighbour's cell";
    for stats in [
        arena_after(&compiled, name),
        treewalk_arena_after(&compiled, name),
    ] {
        assert_eq!(stats.allocations, 2, "two regions, two bumps");
        assert_eq!(
            stats.peak_live, 1,
            "in sequence rather than nested, so the second reuses the first's position"
        );
    }
}

// ------------------------------------------- 2. resumptions, and the integer

const RESUMED: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

effect state {
  read  get[s]() -> Int
  write put[s](v: Int) -> Unit
}

test "a write inside a resumption is there after the handle returns" {
  with_cell[log](0) { c -> {
    let answer = handle {
      let b = amb.flip[coin]();
      if b { 10 } else { 20 }
    } with {
      amb.flip[coin]() resume k -> { cell_set(c, cell_get(c) + 1); k(true) },
      return x -> x
    };
    assert_eq(answer, 10);
    assert_eq(cell_get(c), 1)
  } }
}

test "two resumptions thread one cell" {
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

test "the canonical state handler answers what was put" {
  with_cell[s](0) { c -> {
    let answer = handle {
      state.put[s](5);
      state.get[s]()
    } with {
      state.get[s]() resume k -> k(cell_get(c)),
      state.put[s](v) resume k -> { cell_set(c, v); k(()) },
      return x -> x
    };
    assert_eq(answer, 5)
  } }
}
"#;

/// A cell written by a handler clause before it resumes, read after the whole
/// `handle` returned. Nothing on the cell path saves or restores, so the write
/// is simply there.
#[test]
fn a_cell_written_inside_a_resumption_is_read_after_it() {
    let compiled = Compiled::new(RESUMED);
    let stats = arena_after(
        &compiled,
        "a write inside a resumption is there after the handle returns",
    );
    assert_eq!(stats.allocations, 1);
    assert!(
        stats.pins_taken > 0,
        "the clause captured a continuation, so the capture path must have pinned"
    );
}

/// ADR 0017 §3's two-resumption example, and the integer the whole section
/// turns on: the trace cell reads **2**, because one cell serves both
/// resumptions and `k(false)` observes what `k(true)` wrote. Snapshot-at-capture
/// would answer `1`, and an arena that restored anything at a resumption would
/// too — which is why this test is in the file that wires the arena up.
#[test]
fn the_two_resumption_example_leaves_its_trace_cell_at_two() {
    let compiled = Compiled::new(RESUMED);
    // `assert_eq(cell_get(c), 2)` is written in the program itself, and it runs
    // inside the region where the cell is live. Reaching here at all is the
    // assertion; the arena shape below is what it cost.
    let stats = arena_after(&compiled, "two resumptions thread one cell");
    assert_eq!(stats.allocations, 1, "one cell served both resumptions");
    assert!(stats.pins_taken > 0);
}

/// The reason snapshot-at-capture cannot be taken, run rather than argued: the
/// `put` clause writes the cell and *then* resumes, so restoring at the
/// resumption would discard the write and `put(5); get()` would answer `0`.
#[test]
fn the_canonical_state_handler_is_still_writable() {
    let compiled = Compiled::new(RESUMED);
    let stats = arena_after(
        &compiled,
        "the canonical state handler answers what was put",
    );
    assert_eq!(stats.allocations, 1);
}

/// Nothing on the cell path takes a snapshot. `Arena::snapshot` and
/// `Arena::restore` are a save-and-restore primitive the capture path must not
/// use, and the arena's own counters are what say it did not.
///
/// Nothing in the evaluator calls either, so the counters stay at zero for a
/// whole run: the entry-point reset rewinds the scope stack rather than saving
/// it, so even that path takes no snapshot. An implementation that saved at the
/// capture and restored at each resumption would show two of each here and would
/// answer `1` for the trace cell.
#[test]
fn no_capture_or_resumption_snapshots_the_arena() {
    let compiled = Compiled::new(RESUMED);
    let mut machine = compiled.machine();
    let before = machine.cells().stats();

    let index = compiled.index_of("two resumptions thread one cell");
    machine.eval_test(index).expect("the test passes");

    let stats = machine.cells().stats();
    assert_eq!(
        stats.snapshots - before.snapshots,
        0,
        "a capture must not save the arena"
    );
    assert_eq!(
        stats.slots_copied - before.slots_copied,
        0,
        "and so nothing is copied at one"
    );
    assert_eq!(
        stats.restores - before.restores,
        0,
        "nothing on a run's path restores, least of all a resumption"
    );
}

// ------------------------------------------------ 3. the generation discipline

const ESCAPED: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

type Saved = Nothing | Just((Bool) -> Int)

fn parked() -> Saved = {
  with_cell[slot](Nothing) { s -> {
    with_cell[log](7) { c -> {
      handle {
        let b = amb.flip[coin]();
        if b { cell_get(c) } else { 0 }
      } with {
        amb.flip[coin]() resume k -> { cell_set(s, Just(k)); 0 },
        return x -> x
      }
    } };
    cell_get(s)
  } }
}

fn resume_it(saved: Saved) -> Int = {
  match saved {
    Just(k) -> k(true),
    Nothing -> 0
  }
}
"#;

/// The one route ADR 0017 §2's brand does not close: a continuation parked in a
/// cell, carried out of the run that made it, and resumed in the next one.
///
/// Under `CellId` this was undetectable in the general case — the audit that
/// covered it had to say so out loud, because an id carries no lineage and the
/// second run would have handed the same id to a different cell. Two things now
/// stand in the way and either is a correct answer: the boundary check refuses
/// the value on the way out, and behind it a `Slot` carries a generation the
/// entry-point reset bumps, so the read is a report rather than a plausible
/// number. What must not happen is an answer.
#[test]
fn a_cell_carried_across_two_entry_points_is_named_rather_than_read() {
    let compiled = Compiled::new(ESCAPED);
    let mut machine = compiled.machine();

    let parked = machine
        .call("m.parked", vec![], Span::DUMMY)
        .expect("a continuation over the region's cell");
    let smuggled = machine
        .call("m.resume_it", vec![parked], Span::DUMMY)
        .expect_err("the cell the continuation reads belongs to the previous entry point");

    assert!(
        smuggled.code == codes::REGION_ESCAPE_AT_BOUNDARY || smuggled.code == codes::INTERNAL_ERROR,
        "the smuggled cell must be refused rather than answered: {smuggled:#?}"
    );
}

// ------------------------------------------------------- 4. what the arena buys

/// The point of a bump arena, on the evaluator rather than in the allocator's
/// own unit tests: an entry point that has run once costs the global allocator
/// nothing for the cells of the next one.
///
/// `chunks_allocated` is the only field that counts a call to the global
/// allocator, so it going flat is the claim. Under the persistent world every
/// `with_cell` was a tree insertion and every `cell_set` a path copy, and
/// neither ever went flat.
#[test]
fn the_arena_takes_no_chunk_for_an_entry_point_it_has_already_sized() {
    let mut src = String::from("test \"warm\" {\n  with_cell[r](0) { c -> {\n");
    for i in 0..600 {
        src.push_str(&format!("    cell_set(c, {i});\n"));
    }
    src.push_str("    assert_eq(cell_get(c), 599)\n  } }\n}\n");
    let compiled = Compiled::new(&src);
    let index = compiled.index_of("warm");
    let mut machine = compiled.machine();

    machine.eval_test(index).expect("the test passes");
    let warm = machine.cells().stats().chunks_allocated;

    for _ in 0..50 {
        machine.eval_test(index).expect("the test passes");
    }

    assert_eq!(
        machine.cells().stats().chunks_allocated,
        warm,
        "fifty more entry points of a size the arena has seen took no chunk"
    );
    assert_eq!(
        machine.cells().live(),
        0,
        "and each one gave its cell back at the region's close"
    );
}

/// A `with_cell` in a loop is the shape ADR 0005 §2 charged one retained world
/// entry per iteration for, and which R1's wiring still charged one arena slot
/// per iteration for. Each region now closes at its own lexical end and no
/// continuation outlives any of them, so the high-water mark is **one** however
/// many iterations run.
#[test]
fn a_region_in_a_loop_costs_one_slot_however_many_iterations_run() {
    let compiled = Compiled::new(
        r#"
fn once(n: Int) -> Int = with_cell[r](n) { c -> cell_get(c) }

fn upto(n: Int) -> Int = if n == 0 { once(0) } else { once(n) + upto(n - 1) }

test "a region in a loop" {
  assert_eq(upto(63), 2016)
}
"#,
    );
    let index = compiled.index_of("a region in a loop");
    let mut machine = compiled.machine();

    machine.eval_test(index).expect("the test passes");
    assert_eq!(
        machine.cells().stats().allocations,
        64,
        "sixty-four regions, sixty-four bumps"
    );
    assert_eq!(
        machine.cells().stats().peak_live,
        1,
        "and never two at once, because each region closed before the next opened"
    );
    assert_eq!(machine.cells().live(), 0);

    machine.eval_test(index).expect("the test passes");
    assert_eq!(
        machine.cells().stats().peak_live,
        1,
        "and the next entry point starts from the fixture, not from 64"
    );
}

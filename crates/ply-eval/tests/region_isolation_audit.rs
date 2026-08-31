//! Adversarial audit of the one property the milestone cannot be wrong about:
//! **no two region stacks opened from one fixture observe each other's
//! writes.**
//!
//! Everything else in Ply degrades gracefully when it is wrong — a slower
//! schedule, a wider suspect set. This one does not: two tests that can see each
//! other's state produce a flake that survives the cache, and the cache makes it
//! stick. So the attacks here are written to *succeed* if they can, and the
//! assertions state what must remain true when they cannot.
//!
//! Three layers, because a defect can hide in any of them:
//!
//! 1. [`TaskRegions`] on its own — two stacks off one fixture, an entry point's
//!    slots reclaimed under a slot somebody kept, and the index collision that
//!    makes carrying a value between two stacks unsafe.
//! 2. The machine on real source — a cell smuggled out of its region through
//!    every carrier the language has, and a continuation resumed after the
//!    region that made it returned.
//! 3. The types — an argument from the definitions rather than from a run,
//!    because a test can only sample the executions somebody thought of.

use ply_core::{CheckOutput, Footprint, check_program};
use ply_eval::arena::Slot;
use ply_eval::{Engine, Fixture, Interp, Machine, TaskRegions, Value};
use ply_span::{Diagnostic, SourceId};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::marker::PhantomData;

// ------------------------------------------------------------------ harness

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

    fn rejected(src: &str) -> Vec<Diagnostic> {
        let inputs = [(SourceId(0), ModuleName::from_dotted("m"), src)];
        let mut program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
        let resolved =
            resolve(&mut program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        match check_program(&program, &resolved) {
            Ok(_) => Vec::new(),
            Err(diags) => diags,
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

    fn footprint(&self, name: &str) -> &Footprint {
        &self.check.tests[self.index_of(name)].footprint
    }

    /// An isolation defect that only one engine has is still a defect, so
    /// nothing here is believed until both engines say it.
    fn run_both(&self, name: &str) -> Result<(), Diagnostic> {
        let index = self.index_of(name);
        let machine = self.machine().eval_test(index);
        let treewalk = self.interp().eval_test(index);
        match (&machine, &treewalk) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(m), Err(t)) => {
                assert_eq!(
                    m.message, t.message,
                    "the engines disagree about why {name:?} failed"
                );
                Err(machine.unwrap_err())
            }
            (Ok(()), Err(t)) => panic!("only the tree-walker failed {name:?}: {t:#?}"),
            (Err(m), Ok(())) => panic!("only the machine failed {name:?}: {m:#?}"),
        }
    }
}

fn int_of(regions: &TaskRegions, slot: Slot) -> i64 {
    match regions.get(slot) {
        Some(Value::Int(i)) => *i,
        other => panic!("expected an Int in {slot}, found {other:?}"),
    }
}

fn one_cell() -> Fixture {
    Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(0))))
}

fn cell_of(fixture: &Fixture) -> Slot {
    fixture
        .handle()
        .as_cell(ply_span::Span::DUMMY, "the fixture handle")
        .expect("the handle is a cell")
}

// ----------------------------------------- 1. the region stack on its own

/// The headline property, stated over three stacks rather than two so that a
/// defect that leaks in only one direction cannot hide behind symmetry.
#[test]
fn stacks_opened_from_one_fixture_never_read_each_others_value() {
    let fixture = one_cell();
    let shared = cell_of(&fixture);

    let mut stacks: Vec<TaskRegions> = (0..3).map(|_| fixture.open().0).collect();
    for (i, stack) in stacks.iter_mut().enumerate() {
        assert!(stack.set(shared, Value::Int(i as i64 + 1)));
    }
    // Interleaved a second time: a defect that needs the writes to alternate
    // rather than run in a batch would survive the loop above.
    for i in (0..stacks.len()).rev() {
        assert!(stacks[i].set(shared, Value::Int(i as i64 + 10)));
        for (j, other) in stacks.iter().enumerate() {
            let expected = if j >= i { j as i64 + 10 } else { j as i64 + 1 };
            assert_eq!(
                int_of(other, shared),
                expected,
                "stack {j} after writing {i}"
            );
        }
    }
    assert_eq!(fixture.len(), 1, "the fixture itself is untouched");
    assert_eq!(
        int_of(&fixture.open().0, shared),
        0,
        "and still seeds a zero"
    );
}

/// The direction the forkable world made least obvious was a write to a shared
/// ancestor. A region stack has no ancestor to write to — the fixture is a seed
/// rather than a live parent — so the equivalent attack is to open a stack,
/// mutate it deeply, and check that the seed the next open produces did not
/// move. Twelve stacks, so a defect that needs depth has room to show.
#[test]
fn no_amount_of_writing_to_an_open_stack_moves_what_the_fixture_seeds() {
    const DEPTH: usize = 12;

    let fixture = Fixture::build(|r| {
        Value::list(
            (0..DEPTH)
                .map(|_| Value::Cell(r.alloc_cell(Value::Int(-1))))
                .collect(),
        )
    });
    let cells: Vec<Slot> = match fixture.handle() {
        Value::List(items) => items
            .iter()
            .map(|v| {
                v.as_cell(ply_span::Span::DUMMY, "a handle")
                    .expect("a cell")
            })
            .collect(),
        other => panic!("expected the handle list, found {other:?}"),
    };

    let mut stacks: Vec<TaskRegions> = (0..DEPTH).map(|_| fixture.open().0).collect();
    let mark = |level: usize, i: usize| (level * 100 + i) as i64;
    for level in [0usize, 3, 7] {
        for (i, slot) in cells.iter().enumerate() {
            assert!(stacks[level].set(*slot, Value::Int(mark(level, i))));
        }
        for (other, stack) in stacks.iter().enumerate() {
            for (i, slot) in cells.iter().enumerate() {
                let seen = int_of(stack, *slot);
                let expected = if other == level { mark(level, i) } else { -1 };
                assert_eq!(
                    seen, expected,
                    "stack {other} observed a write made to stack {level}"
                );
            }
        }
        for slot in &cells {
            assert!(stacks[level].set(*slot, Value::Int(-1)));
        }
    }

    for (i, slot) in cells.iter().enumerate() {
        assert_eq!(int_of(&fixture.open().0, *slot), -1, "cell {i}");
    }
}

/// An entry point's reset is the fork's replacement, and it has to mean both
/// halves: the fixture comes back to what it was seeded as, and everything the
/// entry point allocated on top of it is gone.
#[test]
fn a_reset_restores_the_seed_and_discards_what_the_entry_point_allocated() {
    let fixture = one_cell();
    let seeded = cell_of(&fixture);
    let (mut regions, _) = fixture.open();

    assert!(regions.set(seeded, Value::Int(1)));
    let scratch = regions.alloc_cell(Value::Int(2));
    assert_eq!(int_of(&regions, seeded), 1);

    regions.reset();

    assert_eq!(int_of(&regions, seeded), 0);
    assert!(!regions.contains(scratch));
}

/// The hazard that makes every other test here necessary: two stacks opened
/// from one fixture hand out the *same* slot for different cells, and reading a
/// foreign slot succeeds quietly instead of failing. Nothing detects a value
/// that crossed between two stacks — which is why nothing may carry one, and
/// why the machine's isolation has to be structural rather than checked.
///
/// A slot is an index and a generation, and both stacks are fresh, so both
/// generations are zero. The generation closes the *temporal* hole — a slot
/// from a previous entry point, which
/// `a_cell_carried_across_two_runs_of_one_machine_is_named_and_not_read`
/// pins — and not this spatial one.
#[test]
fn a_foreign_slot_is_answered_by_the_reading_stack_and_never_by_its_owner() {
    let fixture = Fixture::empty();
    let (mut a, _) = fixture.open();
    let (mut b, _) = fixture.open();

    let in_a = a.alloc_cell(Value::str("a's secret"));
    let in_b = b.alloc_cell(Value::str("b's secret"));
    assert_eq!(in_a, in_b, "two fresh stacks bump from the same floor");

    assert_eq!(a.get(in_b).map(Value::render).unwrap(), "\"a's secret\"");
    assert_eq!(b.get(in_a).map(Value::render).unwrap(), "\"b's secret\"");

    assert!(a.set(in_b, Value::str("clobbered")));
    assert_eq!(b.get(in_b).map(Value::render).unwrap(), "\"b's secret\"");
}

/// What a slot buys that a `CellId` did not: a slot whose region has been
/// reclaimed reads `None` on every run rather than aliasing whatever was
/// allocated in its place. That is the half of the hazard above that *is*
/// closed, and it is closed by the generation rather than by a check anyone has
/// to remember to write.
#[test]
fn a_slot_from_a_reclaimed_entry_point_reads_nothing_rather_than_its_successor() {
    let (mut regions, _) = Fixture::empty().open();
    let stale = regions.alloc_cell(Value::str("the first run's secret"));

    regions.reset();
    let fresh = regions.alloc_cell(Value::str("the second run's secret"));

    assert_eq!(
        stale.index(),
        fresh.index(),
        "the index was handed out again"
    );
    assert!(
        regions.get(stale).is_none(),
        "and the stale slot reads nothing"
    );
    assert!(
        !regions.set(stale, Value::Int(0)),
        "a write through it is refused"
    );
    assert_eq!(
        regions.get(fresh).map(Value::render).unwrap(),
        "\"the second run's secret\""
    );
}

// ------------------------------------------- 2. the machine on real source

/// The carriers that used to take a cell out of its region, and the one that
/// still can.
///
/// A closure was the last one the region check did not inspect: it looked at the
/// body's *type*, and a function type hid the cell in its effect row. ADR 0017
/// §2 closed it, along with the record-of-closures, operation and store routes —
/// `ply-core/tests/region_escape_audit.rs` is where each is pinned as an error.
/// What is left is a *continuation* parked in an enclosing region's cell: its
/// row is erased where the variant field's function type is declared, so no type
/// downstream of the constructor mentions the region. That is ADR 0005 required
/// test 6, it is a success rather than an error, and it is the carrier this
/// section now attacks with.
const ESCAPED: &str = r#"
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

test "a continuation carries the cell out of its region" {
  assert_eq(resume_it(parked()), 41)
}

test "two regions in one test are two cells" {
  let bumped = with_cell[log](0) { c -> {
    cell_set(c, cell_get(c) + 1);
    cell_set(c, cell_get(c) + 1);
    cell_set(c, cell_get(c) + 1);
    cell_get(c)
  } };
  assert_eq(bumped, 3);
  let fresh = with_cell[log](0) { c -> cell_get(c) };
  assert_eq(fresh, 0)
}
"#;

/// Every carrier ADR 0017 §2 names is refused, including the closure route it
/// lists first among the ways this could go wrong. Each of these ran before the
/// brand looked at a function type's effect row.
#[test]
fn every_closure_shaped_carrier_out_of_a_region_is_refused() {
    for (carrier, src) in [
        (
            "a closure",
            r#"test "smuggle" {
  let read = with_cell[log](41) { c -> || cell_get(c) };
  assert_eq(read(), 41)
}"#,
        ),
        (
            "a record of closures",
            r#"test "smuggle" {
  let ops = with_cell[log](1) { c -> {get: || cell_get(c), put: |v| cell_set(c, v)} };
  let get = ops.get;
  let put = ops.put;
  put(9);
  assert_eq(get(), 9)
}"#,
        ),
        (
            "a closure that only writes",
            r#"test "smuggle" {
  let bump = with_cell[log](0) { c -> || cell_set(c, cell_get(c) + 1) };
  bump()
}"#,
        ),
    ] {
        let diags = Compiled::rejected(src);
        assert!(
            diags.iter().any(|d| d.message.contains("escapes its")),
            "{carrier} must not carry a cell out of its region: {diags:#?}"
        );
    }
}

/// The one carrier left, run. What it must not do is read anything but this
/// run's own world.
///
/// The continuation half is machine-only: the tree-walker refuses a clause that
/// binds a continuation (`E0504`, ADR 0005 required test 3), so `--engine both`
/// has nothing to say about it and this is the oracle instead.
#[test]
fn a_cell_carried_out_through_a_continuation_reads_this_runs_world() {
    let compiled = Compiled::new(ESCAPED);
    let index = compiled.index_of("a continuation carries the cell out of its region");
    compiled
        .machine()
        .eval_test(index)
        .expect("the resumed read answers the region's initial value");
    compiled
        .run_both("two regions in one test are two cells")
        .expect("a second region is a second cell, not the first one again");
}

/// A nullary definition with an empty published row is a constant and the
/// evaluators memoize it. `parked` is one — `with_cell` discharges its own
/// atoms, so its footprint is `{}` — and its *value* is a key into the world
/// that ran it. Remembering it hands the next test a cell its own world never
/// allocated, which is the cross-test interference this whole file exists to
/// rule out.
///
/// `E0505` catches it when the id is absent. It would not catch it when the
/// next run has already allocated that id, which is the same undetectable half
/// `a_cell_carried_across_two_runs_of_one_machine_is_named_and_not_read`
/// records — so the memo has to refuse the value rather than rely on the check.
#[test]
fn a_constant_whose_value_reaches_a_cell_is_not_remembered_across_tests() {
    let compiled = Compiled::new(ESCAPED);
    let mut machine = compiled.machine();
    let index = compiled.index_of("a continuation carries the cell out of its region");
    for run in 0..3 {
        machine
            .eval_test(index)
            .unwrap_or_else(|d| panic!("run {run} must evaluate `parked` afresh: {d:#?}"));
    }

    // The sanity half: a constant whose value reaches no cell is still memoized,
    // so this refuses the value rather than the rule.
    let plain = Compiled::new(
        r#"
fn table() -> List<Int> = [1, 2, 3]

test "a plain constant" { assert_eq(len(table()), 3) }
"#,
    );
    let mut machine = plain.machine();
    let index = plain.index_of("a plain constant");
    machine.eval_test(index).expect("passes");
    machine.eval_test(index).expect("and again");
}

/// A `cell` atom reaching a published footprint is what the scheduler colours
/// on, and with every escape route closed a *written row* is the only way one
/// gets there. ADR 0005 §5 said a `cell` atom in a footprint needed a captured
/// continuation; it now needs an annotation, because the continuation's row is
/// erased where the variant field is declared.
#[test]
fn a_declared_cell_atom_is_what_reaches_a_tests_footprint() {
    let compiled = Compiled::new(
        r#"
fn touches(n: Int) -> Int / {cell.read[log]} = n
fn writes(n: Int) -> Int / {cell.read[log], cell.write[log]} = n

test "a read" {
  let seen = with_cell[log](41) { c -> cell_get(c) };
  assert_eq(touches(seen), 41)
}

test "a read and a write" {
  let seen = with_cell[log](1) { c -> { cell_set(c, 9); cell_get(c) } };
  assert_eq(writes(seen), 9)
}
"#,
    );
    let atoms: Vec<String> = compiled
        .footprint("a read")
        .atoms()
        .map(|a| a.to_string())
        .collect();
    assert_eq!(atoms, vec!["cell.read[log]".to_string()]);

    let mixed: Vec<String> = compiled
        .footprint("a read and a write")
        .atoms()
        .map(|a| a.to_string())
        .collect();
    assert_eq!(
        mixed,
        vec!["cell.read[log]".to_string(), "cell.write[log]".to_string()]
    );

    // And a region discharges its own label: the same atoms performed inside the
    // region never reach the footprint at all.
    let discharged = Compiled::new(
        r#"
test "inside the region" {
  with_cell[log](41) { c -> { cell_set(c, 9); assert_eq(cell_get(c), 9) } }
}
"#,
    );
    assert_eq!(discharged.footprint("inside the region").atoms().count(), 0);
}

/// Two runs of one machine allocate at the *same* indices, because the entry
/// point's reset hands the slots back. That is the collision of
/// `a_foreign_slot_is_answered_by_the_reading_stack_and_never_by_its_owner`,
/// reached through the front door — so the second run must start from the seed
/// and not from whatever the first left behind.
#[test]
fn a_second_run_of_one_machine_reuses_the_indices_and_none_of_the_state() {
    let compiled = Compiled::new(ESCAPED);
    let mut machine = compiled.machine();
    let index = compiled.index_of("two regions in one test are two cells");

    machine.cells_mut().journal();
    machine.eval_test(index).expect("the first run passes");
    let first: Vec<(u32, String)> = machine
        .cells()
        .journalled()
        .iter()
        .map(|(slot, v)| (slot.index(), v.render()))
        .collect();
    // Both regions reclaim index 0: the first hands it back at its close and
    // the second bumps into the position it vacated.
    assert_eq!(first, vec![(0, "3".into()), (0, "0".into())]);

    machine.eval_test(index).expect("the second run passes");
    let second: Vec<(u32, String)> = machine
        .cells()
        .journalled()
        .iter()
        .map(|(slot, v)| (slot.index(), v.render()))
        .collect();
    assert_eq!(
        second, first,
        "the indices repeat and the values start over"
    );
}

/// A cell in a *constructor argument* used to be the one carrier the region
/// check could not see: the variant's field type holds the `Cell`, so the
/// region's result type was `Held` and mentioned no region.
///
/// ADR 0017 §2 closes it, and it is closed at the **declaration** rather than at
/// the escape. A variant's field types are converted once for the whole program,
/// so the region argument a written `Cell<Int>` gets is a single variable that
/// no scheme quantifies — there is no later point at which the brand could be
/// looked at, which is why the hole existed and why refusing the declaration is
/// what closes it.
#[test]
fn a_cell_in_a_constructor_argument_is_refused_where_the_field_is_declared() {
    let diags = Compiled::rejected(
        r#"
type Held = Held(Cell<Int>)

test "a constructor carries the cell out of its region" {
  let h = with_cell[log](1) { c -> Held(c) };
  match h { Held(c) -> { cell_set(c, 2); assert_eq(cell_get(c), 2) } }
}
"#,
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code == ply_span::codes::REGION_ESCAPE),
        "a declared `Cell` field is a brand with nowhere to appear: {diags:#?}"
    );
}

/// The boundary of that hole: the region variable in a declared `Cell<T>` field
/// is fixed by the first region that fills it, so a second region using the same
/// type is a mismatch rather than a silent alias between two regions' cells.
#[test]
fn one_variant_cannot_hold_cells_from_two_regions_at_once() {
    let diags = Compiled::rejected(
        r#"
type Held = Held(Cell<Int>)

test "two regions through one variant" {
  let a = with_cell[log](1) { c -> Held(c) };
  let b = with_cell[audit](2) { c -> Held(c) };
  match a { Held(c) -> match b { Held(d) -> assert_eq(cell_get(c) + cell_get(d), 3) } }
}
"#,
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code == ply_span::codes::TYPE_MISMATCH),
        "a second region must not quietly reuse the first one's tag: {diags:#?}"
    );
}

/// A cell in a list element or a record field *is* caught, because both keep the
/// `Cell` type in the region's result type where `mentions_region` finds it.
#[test]
fn a_cell_in_a_list_or_a_record_field_is_refused_by_the_region_check() {
    for (carrier, src) in [
        (
            "list",
            r#"
test "smuggle" {
  let xs = with_cell[log](1) { c -> [c] };
  assert_eq(len(xs), 1)
}
"#,
        ),
        (
            "record",
            r#"
test "smuggle" {
  let r = with_cell[log](1) { c -> {cell: c} };
  assert_eq(cell_get(r.cell), 1)
}
"#,
        ),
    ] {
        let diags = Compiled::rejected(src);
        assert!(
            diags.iter().any(|d| d.message.contains("escapes its")),
            "a cell in a {carrier} must be refused: {diags:#?}"
        );
    }
}

const RESUMED: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

type Saved = Nothing | Just((Bool) -> Int)

test "two resumptions write one cell in one world" {
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

test "each resumption allocates its own region cell" {
  with_cell[tally](0) { t -> {
    let total = handle {
      let b = amb.flip[coin]();
      with_cell[scratch](0) { s -> {
        cell_set(s, cell_get(s) + 1);
        cell_set(t, cell_get(t) + cell_get(s));
        cell_get(s)
      } }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    };
    assert_eq(total, 2);
    assert_eq(cell_get(t), 2)
  } }
}

test "a continuation resumed after its region returned reads that region's cell" {
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

/// ADR 0005 §3.2's "resumes twice", with the second resumption's write landing
/// on the first one's: one threaded world, not a snapshot per resumption. The
/// first branch sees `1`, the second sees `2`, and `21 = 1 + 2 * 10`.
#[test]
fn two_resumptions_of_one_handler_write_one_cell_in_one_world() {
    let compiled = Compiled::new(RESUMED);
    let index = compiled.index_of("two resumptions write one cell in one world");
    let mut machine = compiled.machine();
    machine.cells_mut().journal();
    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("the threaded-world reading must hold: {d:#?}"));
    assert_eq!(
        machine.cells().journalled().len(),
        1,
        "one region, one cell — reclaimed once, at its close"
    );
}

/// A `with_cell` *inside* a handled body runs once per resumption, and each run
/// has to allocate its own cell: two resumptions sharing one region cell would
/// be the two branches of a search seeing each other's scratch state. The tally
/// discriminates — it is `1 + 1` when each branch starts from zero and `1 + 2`
/// when the second inherits the first's.
#[test]
fn each_resumption_allocates_its_own_region_cell() {
    let compiled = Compiled::new(RESUMED);
    let index = compiled.index_of("each resumption allocates its own region cell");
    let mut machine = compiled.machine();
    machine.cells_mut().journal();
    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("each branch must get its own scratch cell: {d:#?}"));
    assert_eq!(
        machine.cells().stats().allocations,
        3,
        "the tally, and one scratch cell per resumption"
    );
    assert_eq!(
        machine.cells().journalled().len(),
        3,
        "and every one of them went back at the close of the region that made it"
    );
}

/// A continuation parked in an enclosing region's cell and resumed after the
/// region whose cell it reads has returned. Both regions are `shared`, so their
/// slots outlive their lexical close and this is a success rather than a
/// dangling read — and the value it answers with is this run's, never a
/// neighbour's.
#[test]
fn a_continuation_resumed_after_its_region_returned_reads_this_runs_cell() {
    let compiled = Compiled::new(RESUMED);
    let index = compiled
        .index_of("a continuation resumed after its region returned reads that region's cell");

    let mut machine = compiled.machine();
    machine.cells_mut().journal();
    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("resuming outside the region must succeed: {d:#?}"));
    let first: Vec<String> = machine
        .cells()
        .journalled()
        .iter()
        .map(|(_, v)| v.render())
        .collect();

    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("the second run must also succeed: {d:#?}"));
    let second: Vec<String> = machine
        .cells()
        .journalled()
        .iter()
        .map(|(_, v)| v.render())
        .collect();
    assert_eq!(first, second, "the second run started from the seed again");
    assert!(
        !first.is_empty(),
        "or the comparison above is between two empties"
    );
}

/// The one place a value *can* cross two entry points is the host API: `call`
/// resets the region stack and then accepts arguments the caller built during an
/// earlier run. Nothing in the tree does that, and this pins the refusal.
///
/// The carrier is a continuation parked in an enclosing region's cell, because
/// that is the only one ADR 0017 §2's brand does not catch: the row is erased
/// where the variant field's function type is declared.
///
/// Under the forkable world only half of this was detectable — had the second
/// run already allocated `#0`, the resumed continuation would have read *that*
/// cell and answered plausibly, because a `CellId` carried no lineage. Two
/// things close the other half now, and the entry point's boundary check gets
/// there first: it refuses the argument as `E0449` before the run starts. The
/// backstop behind it is the slot's generation, which the reset bumps, and
/// `a_slot_from_a_reclaimed_entry_point_reads_nothing_rather_than_its_successor`
/// is where that is stated on its own.
#[test]
fn a_cell_carried_across_two_runs_of_one_machine_is_named_and_not_read() {
    let compiled = Compiled::new(ESCAPED);
    let mut machine = compiled.machine();

    let parked = machine
        .call("m.parked", vec![], ply_span::Span::DUMMY)
        .expect("a continuation over the region's cell");
    let smuggled = machine
        .call("m.resume_it", vec![parked], ply_span::Span::DUMMY)
        .expect_err("the second run holds no slot the first one allocated");

    assert_eq!(smuggled.code, ply_span::codes::REGION_ESCAPE_AT_BOUNDARY);
}

/// The teeth behind every "they never observed each other" assertion elsewhere:
/// tests that reset one stack all allocate their first cell at *the same index*.
/// Isolation is therefore doing real work rather than being an accident of two
/// tests happening to name different slots.
#[test]
fn separate_tests_write_the_very_same_slot_index_in_their_own_entry_points() {
    let mut src = String::new();
    for i in 0..4 {
        src.push_str(&format!(
            "test \"contender {i}\" {{ with_cell[table]({i}) {{ c -> cell_set(c, {i} * 7) }} }}\n"
        ));
    }
    let compiled = Compiled::new(&src);
    let mut machine = compiled.machine();

    machine.cells_mut().journal();
    for i in 0..4 {
        machine.eval_test(i).expect("the test passes");
        let cells: Vec<(u32, String)> = machine
            .cells()
            .journalled()
            .iter()
            .map(|(slot, v)| (slot.index(), v.render()))
            .collect();
        assert_eq!(
            cells,
            vec![(0, (i * 7).to_string())],
            "every contender owns slot 0 and nobody else's value"
        );
    }
}

/// The fixture is what every entry point resets to. A test that writes must
/// therefore leave it exactly as it found it, or the *next* test inherits the
/// write — the interference this milestone exists to make impossible.
#[test]
fn a_seeded_fixture_survives_every_test_that_opens_it() {
    let compiled = Compiled::new(ESCAPED);
    let fixture = Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(1_000))));
    let seeded = cell_of(&fixture);

    let mut machine = compiled.machine();
    machine.set_regions(fixture.open().0);

    for _ in 0..3 {
        for name in [
            "a continuation carries the cell out of its region",
            "two regions in one test are two cells",
        ] {
            let index = compiled.index_of(name);
            machine.eval_test(index).expect("the test passes");
            assert_eq!(
                int_of(machine.regions(), seeded),
                1_000,
                "{name} disturbed the seed it opened"
            );
        }
    }
    assert_eq!(
        int_of(&fixture.open().0, seeded),
        1_000,
        "the fixture itself is untouched"
    );
}

// --------------------------------------------------- 3. the types themselves

/// A test can only sample the executions somebody thought of. This is the
/// argument from the definitions: a `Value` that could be shared across two
/// worlds would need either interior mutability — a `RefCell`, a `Cell`, an
/// atomic — or a way to reach one world from another thread. It has neither, and
/// `Value: !Send` is what makes the second half unrepresentable rather than
/// merely unwritten.
///
/// A `Value` holds `Rc` for its shared code and continuations, so a region
/// stack that became `Send` would be a data race on a non-atomic refcount, not
/// merely a scheduling surprise. If someone adds an `unsafe impl Send`, this
/// fails.
#[test]
fn a_region_stack_and_the_values_in_it_cannot_cross_a_thread() {
    assert!(
        !is_send!(TaskRegions),
        "a region stack must stay thread-confined"
    );
    assert!(!is_send!(ply_eval::Arena));
    assert!(!is_send!(Value), "Value must stay thread-confined");
    assert!(!is_send!(ply_eval::Continuation));
    assert!(!is_send!(ply_eval::Stack));
    assert!(!is_send!(ply_eval::Env));
    assert!(!is_send!(ply_eval::Fixture));
    assert!(!is_send!(Machine<'static>));
    assert!(!is_send!(Interp<'static>));
    // The sanity half: the probe reports `true` for something that is `Send`.
    assert!(is_send!(Slot));
    assert!(is_send!(Engine));
}

/// Autoref specialization: the inherent method exists only when `T: Send`, and
/// the trait method on `&Probe<T>` needs one more autoref step, so it is chosen
/// exactly when the inherent one does not apply. It has to be a macro rather
/// than a generic function, because resolution inside a generic body happens
/// once, against a `T` that satisfies nothing.
struct Probe<T>(PhantomData<T>);

impl<T: Send> Probe<T> {
    fn probe(&self) -> bool {
        true
    }
}

trait NotSend {
    fn probe(&self) -> bool;
}

impl<T> NotSend for &Probe<T> {
    fn probe(&self) -> bool {
        false
    }
}

macro_rules! is_send {
    ($t:ty) => {
        (&Probe::<$t>(PhantomData)).probe()
    };
}

use is_send;

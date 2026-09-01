//! What a resumption observes, pinned as numbers.

use ply_core::{CheckOutput, check_program};
use ply_eval::{Machine, Plan, Seed, Value, explore};
use ply_span::{Diagnostic, SourceId, codes};
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

    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    fn index_of(&self, name: &str) -> usize {
        self.check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }

    /// Runs one test and requires it to pass; the oracle is the integer each
    /// probe writes down, which [`Compiled::ints_after`] reads.
    fn passes(&self, name: &str) {
        let index = self.index_of(name);
        if let Err(d) = self.machine().eval_test(index) {
            panic!("{name:?} must pass: {d:#?}");
        }
    }

    /// Every cell the test held, at the value it held when its region closed, by ascending cell id.
    fn ints_after(&self, name: &str) -> Vec<i64> {
        self.reclaimed(name)
            .into_iter()
            .map(|v| match v {
                Value::Int(i) => i,
                other => panic!("expected Int cells, found {other:?}"),
            })
            .collect()
    }

    /// The same, for a world holding something other than integers — the parked continuation in the
    /// escape fixture is a value like any other.
    fn renders_after(&self, name: &str) -> Vec<String> {
        self.reclaimed(name).iter().map(Value::render).collect()
    }

    fn reclaimed(&self, name: &str) -> Vec<Value> {
        let index = self.index_of(name);
        let mut machine = self.machine();
        machine.cells_mut().journal();
        machine
            .eval_test(index)
            .unwrap_or_else(|d| panic!("{name:?} must run: {d:#?}"));
        let mut cells: Vec<(ply_eval::arena::Slot, Value)> = machine.cells().journalled().to_vec();
        cells.sort_by_key(|(slot, _)| *slot);
        cells.into_iter().map(|(_, v)| v).collect()
    }
}

const WORKED: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

effect state {
  read  get[s]() -> Int
  write put[s](v: Int) -> Unit
}

// §3, "Zero resumptions". The clause never resumes; the region closes and
// whatever the abandoned computation would have written never happens.
test "zero resumptions" {
  with_cell[log](0) { c -> {
    let out = handle {
      cell_set(c, 1);
      let b = amb.flip[coin]();
      cell_set(c, 2);
      if b { 10 } else { 20 }
    } with {
      amb.flip[coin]() resume k -> cell_get(c),
      return x -> x
    };
    assert_eq(out, 1);
    assert_eq(cell_get(c), 1)
  } }
}

// §3, "One resumption". The clause writes the cell and *then* resumes.
test "one resumption" {
  with_cell[s](0) { c -> {
    let out = handle {
      state.put[s](5);
      state.get[s]()
    } with {
      state.get[s]() resume k -> k(cell_get(c)),
      state.put[s](v) resume k -> { cell_set(c, v); k(()) },
      return x -> x
    };
    assert_eq(out, 5);
    assert_eq(cell_get(c), 5)
  } }
}

// §3, "Two resumptions" — the case ADR 0017 says decides the design.
test "two resumptions" {
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

test "three resumptions" {
  with_cell[trace](0) { c -> {
    let total = handle {
      let n = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if n { cell_get(c) } else { cell_get(c) * 100 }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false) + k(true),
      return x -> x
    };
    assert_eq(total, 204);
    assert_eq(cell_get(c), 3)
  } }
}
"#;

/// §3's zero-resumption example.
#[test]
fn zero_resumptions_keeps_the_clauses_writes_and_none_of_the_abandoned_ones() {
    let compiled = Compiled::new(WORKED);
    compiled.passes("zero resumptions");
    assert_eq!(
        compiled.ints_after("zero resumptions"),
        vec![1],
        "the write before the perform survives; the one after it never ran"
    );
}

/// §3's one-resumption example, written as the canonical cell-backed state handler rather than as
/// `ask.get() resume k -> k(7)`.
#[test]
fn one_resumption_observes_the_write_the_clause_made_before_resuming() {
    let compiled = Compiled::new(WORKED);
    compiled.passes("one resumption");
    assert_eq!(
        compiled.ints_after("one resumption"),
        vec![5],
        "snapshot-at-capture would discard the `put` and leave 0"
    );
}

/// §3's two-resumption example, run as written.
#[test]
fn two_resumptions_thread_one_world_rather_than_snapshotting_per_branch() {
    let compiled = Compiled::new(WORKED);
    compiled.passes("two resumptions");
    assert_eq!(
        compiled.ints_after("two resumptions"),
        vec![2],
        "each resumption incremented the one world; snapshot-at-capture leaves 1"
    );
}

/// Three resumptions, so that a defect which happens to be symmetric between two branches has
/// nowhere to hide.
#[test]
fn three_resumptions_each_see_the_previous_ones_write() {
    let compiled = Compiled::new(WORKED);
    compiled.passes("three resumptions");
    assert_eq!(
        compiled.ints_after("three resumptions"),
        vec![3],
        "three resumptions on one threaded world; snapshot-at-capture leaves 1"
    );
}

const NESTED: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

type Saved = Nothing | Just((Bool) -> Int)

// A capture inside an inner region, answered by a handler in the outer one.
// The continuation crosses the inner region's boundary on every resumption.
test "nested region capture" {
  with_cell[outer](0) { o -> {
    let total = handle {
      with_cell[inner](0) { i -> {
        let b = amb.flip[coin]();
        cell_set(i, cell_get(i) + 1);
        cell_set(o, cell_get(o) + 1);
        if b { cell_get(i) } else { cell_get(o) }
      } }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    };
    assert_eq(total, 3);
    assert_eq(cell_get(o), 2)
  } }
}

// The inner `with_cell` is *inside* the handled body, so it is re-entered per
// resumption and allocates a fresh cell each time. Allocation is already
// per-resumption; mutation of an enclosing cell is not. That asymmetry is the
// whole of the disagreement, and this test holds both halves at once.
test "an inner region is re-entered per resumption" {
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

// A continuation parked in an enclosing cell and resumed *twice* after the
// region it reads has already returned.
test "a continuation resumed twice across a region boundary" {
  with_cell[slot](Nothing) { s -> {
    let inner = with_cell[log](0) { c ->
      handle {
        let b = amb.flip[coin]();
        cell_set(c, cell_get(c) + 1);
        cell_get(c)
      } with {
        amb.flip[coin]() resume k -> { cell_set(s, Just(k)); 0 },
        return x -> x
      }
    };
    assert_eq(inner, 0);
    match cell_get(s) {
      Just(k) -> {
        assert_eq(k(true), 1);
        assert_eq(k(false), 2)
      },
      Nothing -> assert(false)
    }
  } }
}
"#;

/// A capture inside a nested region, answered outside it.
#[test]
fn a_capture_inside_a_nested_region_accumulates_in_the_outer_one() {
    let compiled = Compiled::new(NESTED);
    compiled.passes("nested region capture");
    let ints = compiled.ints_after("nested region capture");
    assert_eq!(
        ints[0], 2,
        "the outer cell saw both resumptions; snapshot-at-capture leaves 1"
    );
}

/// The half of the disagreement that is *already* per-resumption.
#[test]
fn an_inner_region_already_gives_each_resumption_its_own_allocation() {
    let compiled = Compiled::new(NESTED);
    compiled.passes("an inner region is re-entered per resumption");
    let ints = compiled.ints_after("an inner region is re-entered per resumption");
    assert_eq!(
        ints[0], 2,
        "1 + 1: each branch allocated its own scratch cell and started from zero"
    );
    assert_eq!(
        ints.len(),
        3,
        "the tally plus one scratch cell per resumption — the world is monotone"
    );
}

/// A continuation that escapes its region and is resumed twice from outside it.
#[test]
fn two_resumptions_across_a_region_boundary_still_thread_one_world() {
    let compiled = Compiled::new(NESTED);
    compiled.passes("a continuation resumed twice across a region boundary");
    assert_eq!(
        compiled.renders_after("a continuation resumed twice across a region boundary"),
        vec![
            "m.Just(<continuation 1 frames>)".to_string(),
            "2".to_string()
        ],
        "the escaped continuation's second resumption saw the first's write"
    );
}

/// The test that decides whether ADR 0017 §3 can be implemented at all.
#[test]
fn snapshot_at_capture_would_make_the_canonical_state_handler_unwritable() {
    let compiled = Compiled::new(WORKED);
    let index = compiled.index_of("one resumption");
    compiled
        .machine()
        .eval_test(index)
        .expect("`put(5); get()` answers 5 under threading and 0 under snapshot-at-capture");
    assert_eq!(compiled.ints_after("one resumption"), vec![5]);
}

/// ADR 0005 §3.3: a handler that *wants* per-branch state saves and restores around each
/// resumption, in four lines and in the handler where a reader can see it.
#[test]
fn a_handler_can_build_per_branch_state_on_top_of_threading() {
    let compiled = Compiled::new(
        r#"
effect amb {
  read flip[coin]() -> Bool
}

test "per-branch state, built by the handler" {
  with_cell[s](0) { c -> {
    let total = handle {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { cell_get(c) } else { cell_get(c) * 10 }
    } with {
      amb.flip[coin]() resume k -> {
        let before = cell_get(c);
        let a = k(true);
        cell_set(c, before);
        let d = k(false);
        a + d
      },
      return x -> x
    };
    assert_eq(total, 11);
    assert_eq(cell_get(c), 1)
  } }
}
"#,
    );
    compiled.passes("per-branch state, built by the handler");
    assert_eq!(
        compiled.ints_after("per-branch state, built by the handler"),
        vec![1],
        "each branch started from the saved value, so the total is 1 + 1 * 10"
    );
}

const SIMULATED: &str = r#"
effect counter {
  read  get[n]() -> Int
  write put[n](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  let _ = clock.now();
  counter.put[n](seen + 1)
}

test "two tasks bump one counter" {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| bump());
        let b = task.spawn(|| bump());
        task.join(a);
        task.join(b);
        cell_get(c)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
      return x -> assert(x == 1 || x == 2)
    }
  }
}
"#;

/// M7's search does not re-execute a prefix over shared mutable state, and this is the test that
/// says so rather than assuming it.
#[test]
fn the_search_re_runs_each_interleaving_from_the_seed_rather_than_from_the_last_one() {
    let compiled = Compiled::new(SIMULATED);
    let index = compiled.index_of("two tasks bump one counter");

    let explored = explore(&Plan::default(), &mut |seed: &Seed| {
        let mut machine = compiled.machine();
        machine.cells_mut().journal();
        machine.set_seed(seed.clone(), 10_000);
        let outcome = machine.eval_test(index);
        // The world after each interleaving is that interleaving's alone: one cell, holding 1 (an
        // update was lost) or 2 (both landed).
        let ints: Vec<i64> = machine
            .cells()
            .journalled()
            .iter()
            .map(|(_, v)| match v {
                Value::Int(i) => *i,
                other => panic!("expected an Int counter, found {other:?}"),
            })
            .collect();
        assert_eq!(ints.len(), 1, "one region, one cell");
        assert!(
            ints[0] == 1 || ints[0] == 2,
            "interleaving started from {} rather than from the seeded 0",
            ints[0] - 2
        );
        machine
            .simulated()
            .expect("the test reaches a `simulate` region")
            .interleaving(&outcome)
    });

    assert!(
        explored.exploration.explored > 1,
        "the search must actually explore more than one schedule to prove anything"
    );
    assert!(
        explored.passed(),
        "every interleaving started from the seed, so every one is in range: {:#?}",
        explored.diagnostic
    );
}

/// A second resumption *across* a `simulate` delimiter is refused, and this is where ADR 0017's two
/// readings would have visibly parted company even under ADR 0005.
#[test]
fn a_second_resumption_across_a_simulate_delimiter_is_a_diagnostic() {
    let compiled = Compiled::new(
        r#"
effect pick {
  read choose[r]() -> Int
}

test "resumed twice across a region" {
  with_cell[n](0) { c -> {
    handle {
      simulate {
        let a = task.spawn(|| { let v = pick.choose[n](); cell_set(c, cell_get(c) + v) });
        task.join(a)
      }
    } with {
      pick.choose[n]() resume k -> { k(1); k(2) },
    };
    cell_get(c)
  } }
}
"#,
    );
    let mut machine = compiled.machine();
    machine.set_seed(Seed::default(), 10_000);
    let refused: Diagnostic = machine
        .eval_test(compiled.index_of("resumed twice across a region"))
        .expect_err("the region has already ended");
    assert_eq!(refused.code, codes::TASK_ESCAPES_SCOPE);
}

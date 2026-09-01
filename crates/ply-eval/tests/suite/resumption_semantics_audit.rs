//! What a resumption observes, pinned as numbers.
//!
//! ADR 0017 §3 is the sharp part of the region milestone and it is the part
//! two ADRs disagree about. ADR 0005 §3 decided that **the world is threaded**:
//! there is one current world, capture and resumption do not touch it, and
//! resumption *n* observes resumption *n−1*'s writes. ADR 0017 §3 decided that
//! **each resumption observes the region as of capture**, and asserts that this
//! is what ADR 0005 already meant. It is not, and the difference is observable
//! in one integer on every example below.
//!
//! ADR 0017's own governing property is that *program meaning does not change*.
//! So the discriminating observable has to be written down and run before either
//! reading is built on, or the milestone lands a semantics change under a
//! heading that promises there is none.
//!
//! Every test here states both numbers: the one this evaluator produces
//! (ADR 0005 §3, threaded) and the one ADR 0017 §3's snapshot-at-capture rule
//! would produce. Nothing is asserted about which is *right* — that is the
//! ADR's call. What is asserted is that they differ, so no implementation can
//! move from one to the other while believing it changed only representation.
//!
//! Layer 3 is the one that settles it: the canonical cell-backed state handler,
//! which snapshot-at-capture makes unwritable. That is not a backtracking corner
//! case; it is every `handle` in the language that writes a cell before
//! resuming.

use ply_core::{CheckOutput, check_program};
use ply_eval::{Interp, Machine, Plan, Seed, Value, explore};
use ply_span::{Diagnostic, SourceId, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};

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

    /// Runs one test on both engines and requires them to agree, or the
    /// tree-walker to refuse the program by name.
    ///
    /// **The oracle is blind here, and that is a finding.** ADR 0017 offers
    /// `--engine both` as the evidence that meaning did not move. But a clause
    /// that binds a continuation is `E0504`
    /// ([`codes::MACHINE_ONLY_CLAUSE`]) — the tree-walker cannot express an
    /// explicit control stack, so it refuses every `resume k` clause without
    /// running it. `--engine both` therefore audits nothing about multi-shot
    /// resumption, which is precisely the construct this milestone changes.
    ///
    /// What it still audits is that the refusal is *by name*: a tree-walker
    /// that quietly produced a different number instead would be caught. That
    /// is worth pinning and it is all there is.
    fn both(&self, name: &str) {
        let index = self.index_of(name);
        let machine = self.machine().eval_test(index);
        let treewalk = Interp::new(&self.program, &self.resolved, &self.check).eval_test(index);
        match (machine, treewalk) {
            (Ok(()), Ok(())) => {}
            (Ok(()), Err(t)) => assert!(
                ply_eval::is_machine_only(&t),
                "the tree-walker failed {name:?} for a reason other than the missing \
                 control stack: {t:#?}"
            ),
            (Err(m), Err(t)) => assert!(
                ply_eval::is_machine_only(&t),
                "the engines disagree about why {name:?} failed: {m:#?} vs {t:#?}"
            ),
            (Err(m), Ok(())) => panic!("only the machine failed {name:?}: {m:#?}"),
        }
    }

    /// Every cell the test held, at the value it held when its region closed,
    /// by ascending cell id.
    ///
    /// Read from the arena's reclamation journal rather than from what the run
    /// left behind: a region hands its slots back at its lexical close, so the
    /// arena afterwards is empty and an oracle built on it would agree with
    /// every reading of §3 at once. The assertions in the source are half the
    /// evidence; this is the half that distinguishes threading from snapshotting
    /// when a program's own value happens to coincide.
    fn ints_after(&self, name: &str) -> Vec<i64> {
        self.reclaimed(name)
            .into_iter()
            .map(|v| match v {
                Value::Int(i) => i,
                other => panic!("expected Int cells, found {other:?}"),
            })
            .collect()
    }

    /// The same, for a world holding something other than integers — the parked
    /// continuation in the escape fixture is a value like any other.
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

// ------------------------------------------- 1. ADR 0017 §3's three examples

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

/// §3's zero-resumption example. This is the one case the two readings agree
/// on: an unused snapshot and an unused fork are both dropped, so there is
/// nothing to disagree about. It is here so that a change which breaks the
/// other three is visibly *not* a change that breaks everything.
#[test]
fn zero_resumptions_keeps_the_clauses_writes_and_none_of_the_abandoned_ones() {
    let compiled = Compiled::new(WORKED);
    compiled.both("zero resumptions");
    assert_eq!(
        compiled.ints_after("zero resumptions"),
        vec![1],
        "the write before the perform survives; the one after it never ran"
    );
}

/// §3's one-resumption example, written as the canonical cell-backed state
/// handler rather than as `ask.get() resume k -> k(7)`.
///
/// The ADR's own `k(7)` version cannot tell the two readings apart, because its
/// clause writes nothing before resuming. This version can, and it is the
/// version every real handler in the language is shaped like:
///
/// - threaded (this evaluator): `put(5); get()` is **5**, and the cell ends at 5.
/// - snapshot-at-capture (ADR 0017 §3): `k(())` restores the arena as of the
///   capture, which is *before* `cell_set(c, v)`, so `get()` answers **0**.
///
/// ADR 0005 §3.1 states this consequence in as many words. It is why the
/// forkable world threads instead of snapshotting, and it is not confined to
/// `with_cell` — it is the meaning of every `resume` that follows a write.
#[test]
fn one_resumption_observes_the_write_the_clause_made_before_resuming() {
    let compiled = Compiled::new(WORKED);
    compiled.both("one resumption");
    assert_eq!(
        compiled.ints_after("one resumption"),
        vec![5],
        "snapshot-at-capture would discard the `put` and leave 0"
    );
}

/// §3's two-resumption example, run as written.
///
/// The ADR asserts that a write inside `k(true)` is invisible to `k(false)` and
/// that this "is exactly ADR 0005's semantics". It is exactly not:
///
/// - threaded (this evaluator): the branches see 1 then 2, the handle is
///   `1 + 2 * 10 = 21`, and the cell ends at **2**.
/// - snapshot-at-capture (ADR 0017 §3): both branches see 1, the handle is
///   `1 + 1 * 10 = 11`, and the cell ends at **1**.
///
/// ADR 0005 §3.2 pins the 2 explicitly: "under snapshot-at-capture it would be
/// 1 — that number is the observable that pins the semantics, and it is a
/// required test". Both numbers cannot be required at once.
#[test]
fn two_resumptions_thread_one_world_rather_than_snapshotting_per_branch() {
    let compiled = Compiled::new(WORKED);
    compiled.both("two resumptions");
    assert_eq!(
        compiled.ints_after("two resumptions"),
        vec![2],
        "each resumption incremented the one world; snapshot-at-capture leaves 1"
    );
}

/// Three resumptions, so that a defect which happens to be symmetric between
/// two branches has nowhere to hide.
///
/// - threaded: the branches read 1, 2 and 3, the total is `1 + 200 + 3 = 204`,
///   and the cell ends at **3**.
/// - snapshot-at-capture: every branch starts from 0 and writes 1, the total is
///   `1 + 100 + 1 = 102`, and the cell ends at **1**.
#[test]
fn three_resumptions_each_see_the_previous_ones_write() {
    let compiled = Compiled::new(WORKED);
    compiled.both("three resumptions");
    assert_eq!(
        compiled.ints_after("three resumptions"),
        vec![3],
        "three resumptions on one threaded world; snapshot-at-capture leaves 1"
    );
}

// --------------------------------------------------- 2. nesting and escape

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

/// A capture inside a nested region, answered outside it. Both resumptions
/// re-enter the inner region, so the inner cell is fresh each time while the
/// outer one accumulates.
///
/// - threaded: inner reads 1 in both branches, outer reads 1 then 2, total
///   `1 + 2 = 3`, outer ends at **2**.
/// - snapshot-at-capture: the outer write is rolled back per branch, the second
///   branch reads 1, total `1 + 1 = 2`, outer ends at **1**.
#[test]
fn a_capture_inside_a_nested_region_accumulates_in_the_outer_one() {
    let compiled = Compiled::new(NESTED);
    compiled.both("nested region capture");
    let ints = compiled.ints_after("nested region capture");
    assert_eq!(
        ints[0], 2,
        "the outer cell saw both resumptions; snapshot-at-capture leaves 1"
    );
}

/// The half of the disagreement that is *already* per-resumption. A region
/// opened inside the handled body is re-entered on every resumption and
/// allocates a fresh cell, so nothing a branch allocates leaks into its
/// sibling. ADR 0017's "each resumption observes the arena as of capture" is
/// already true of allocation; the tally is 1 + 1 rather than 1 + 2 exactly
/// because of it.
///
/// This is the reading of §3 that would preserve meaning. It is not the reading
/// §3 states, because §3 states the rule in terms of a *write*.
#[test]
fn an_inner_region_already_gives_each_resumption_its_own_allocation() {
    let compiled = Compiled::new(NESTED);
    compiled.both("an inner region is re-entered per resumption");
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
///
/// This is the case ADR 0005 was written to answer and the case ADR 0017 §2
/// proposes to make a type error. Until the brand exists it is reachable, and
/// what it observes has to be pinned: the world is monotone, so both
/// resumptions read the inner region's cell rather than dangling, and the
/// second sees the first's write.
///
/// - threaded: `k(true)` is 1 and `k(false)` is **2**.
/// - snapshot-at-capture: both are 1.
#[test]
fn two_resumptions_across_a_region_boundary_still_thread_one_world() {
    let compiled = Compiled::new(NESTED);
    compiled.both("a continuation resumed twice across a region boundary");
    assert_eq!(
        compiled.renders_after("a continuation resumed twice across a region boundary"),
        vec![
            "m.Just(<continuation 1 frames>)".to_string(),
            "2".to_string()
        ],
        "the escaped continuation's second resumption saw the first's write"
    );
}

// ------------------------------------------------------ 3. what settles it

/// The test that decides whether ADR 0017 §3 can be implemented at all.
///
/// `state.put[s](v) resume k -> { cell_set(c, v); k(()) }` is the canonical
/// state handler and the shape of essentially every stateful handler in this
/// codebase. Under snapshot-at-capture, `k(())` restores the region as of the
/// capture — which is before `cell_set` — so the write is discarded and
/// `put(5); get()` answers 0.
///
/// So ADR 0017 §3's rule is not confined to multi-shot and not confined to
/// backtracking. It retypes the meaning of *one*-shot resumption, which is the
/// overwhelming majority of handlers, and it is therefore incompatible with
/// ADR 0017's own governing property that program meaning does not change.
///
/// The assertion is deliberately the strong one: not "these differ" but "this
/// evaluator answers 5". A future implementation that answers 0 fails here with
/// the reason attached.
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

/// ADR 0005 §3.3: a handler that *wants* per-branch state saves and restores
/// around each resumption, in four lines and in the handler where a reader can
/// see it.
///
/// This is the constructive half of the argument. Threading is the strictly more
/// expressive default because a handler can build snapshot semantics on top of
/// it; the reverse is not true, which is the asymmetry ADR 0005 §3.3 settles the
/// question on. Any resolution of the ADR conflict has to keep this working,
/// because it is the only way a program can ask for per-branch state at all.
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
    compiled.both("per-branch state, built by the handler");
    assert_eq!(
        compiled.ints_after("per-branch state, built by the handler"),
        vec![1],
        "each branch started from the saved value, so the total is 1 + 1 * 10"
    );
}

// ------------------------------------------------- 4. capture under `simulate`

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

/// M7's search does not re-execute a prefix over shared mutable state, and this
/// is the test that says so rather than assuming it.
///
/// `explore` re-runs the *whole* entry point per interleaving against a fresh
/// machine, whose `reset` forks the base world back to the fixture seed. So the
/// counter starts at 0 in every interleaving, and the search's outcomes are a
/// function of the seed alone.
///
/// If the search shared one world across interleavings, the counter would
/// accumulate — the second interleaving would start at 1 or 2 — and the
/// `return` clause's `x == 1 || x == 2` would fail on a later interleaving while
/// the first ones passed. That is the exact shape of a false green: green on the
/// seed a developer runs, red on the seed CI happens to reach.
#[test]
fn the_search_re_runs_each_interleaving_from_the_seed_rather_than_from_the_last_one() {
    let compiled = Compiled::new(SIMULATED);
    let index = compiled.index_of("two tasks bump one counter");

    let explored = explore(&Plan::default(), &mut |seed: &Seed| {
        let mut machine = compiled.machine();
        machine.cells_mut().journal();
        machine.set_seed(seed.clone(), 10_000);
        let outcome = machine.eval_test(index);
        // The world after each interleaving is that interleaving's alone: one
        // cell, holding 1 (an update was lost) or 2 (both landed). Never more.
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

/// A second resumption *across* a `simulate` delimiter is refused, and this is
/// where ADR 0017's two readings would have visibly parted company even under
/// ADR 0005.
///
/// ADR 0006 §1.5: re-entering a region whose scheduler has already ended needs
/// the region forked, and forking a live scheduler needs the world snapshot
/// ADR 0005 refused. So the machine names it rather than dropping the
/// resumption silently.
///
/// This is the honest answer to "a resumption inside a `simulate` region where
/// M7's search re-executes prefixes": the search re-executes nothing — it re-runs
/// whole entry points — and multi-shot across a region delimiter is a
/// diagnostic, not a supported shape. Any region design that makes it
/// *supported* has to say what the scheduler, the virtual clock and the recorded
/// step sequence do on the second pass, and ADR 0017 does not.
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

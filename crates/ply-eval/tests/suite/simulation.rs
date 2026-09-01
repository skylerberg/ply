//! The machine's simulated regions, end to end.

use ply_core::{CheckOutput, check_program};
use ply_eval::explore::{Interleaving, Verdict};
use ply_eval::{Machine, Plan, Seed, SimMode, Value, explore, measure_reduction};
use ply_span::SourceId;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

fn compile(source: &str) -> Compiled {
    let mut program =
        ply_syntax::parse_program(vec![(SourceId(0), ModuleName::from_dotted("t"), source)])
            .expect("parses");
    let resolved = resolve(&mut program).expect("resolves");
    let check = check_program(&program, &resolved).expect("checks");
    Compiled {
        program,
        resolved,
        check,
    }
}

impl Compiled {
    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    /// One interleaving of the first test, at `seed`.
    fn at(&self, seed: &Seed) -> Interleaving {
        let mut machine = self.machine();
        machine.set_seed(seed.clone(), 10_000);
        let outcome = machine.eval_test(0);
        let record = machine
            .simulated()
            .expect("the test reaches a `simulate` region");
        record.interleaving(&outcome)
    }

    /// The whole search of the first test under `plan`.
    fn search(&self, plan: &Plan) -> ply_eval::Explored {
        explore(plan, &mut |seed: &Seed| self.at(seed))
    }
}

fn dpor() -> Plan {
    Plan::default()
}

fn passed(i: &Interleaving) -> bool {
    matches!(i.verdict, Verdict::Passed)
}

const LOST_UPDATE: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  let _ = clock.now();
  counter.put[n](seen + 1)
}

test "two increments" {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| bump());
        let b = task.spawn(|| bump());
        task.join(a);
        task.join(b);
        assert_eq(cell_get(c), 2)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;

/// ADR 0006's first required test.
#[test]
fn the_search_finds_the_lost_update() {
    let compiled = compile(LOST_UPDATE);
    let explored = compiled.search(&dpor());
    assert!(
        explored.exploration.failure.is_some(),
        "the classic lost update must be found, not sampled past"
    );
    let seed = explored
        .exploration
        .failure
        .clone()
        .expect("a failing seed was reported");
    assert!(
        !passed(&compiled.at(&seed)),
        "the reported seed must reproduce the failure exactly"
    );
    assert!(
        explored.exploration.race.is_some(),
        "the search observed the flip, so it knows which two steps had to be reordered"
    );
}

/// The same program under one seed that happens to serialize the two tasks.
#[test]
fn one_interleaving_can_pass_a_program_the_search_fails() {
    let compiled = compile(LOST_UPDATE);
    let explored = compiled.search(&dpor());
    let failing = explored
        .exploration
        .failure
        .clone()
        .expect("a failing seed");
    let others: Vec<&Seed> = explored.seeds.iter().filter(|s| **s != failing).collect();
    assert!(
        others.iter().any(|s| passed(&compiled.at(s))),
        "some interleaving of an unguarded increment must succeed, or the fixture proves nothing"
    );
}

const DISJOINT: &str = r#"
effect shard {
  read  take[r]() -> Int
  write done[r](v: Int) -> Unit
}

fn drain_a() -> Unit / {shard.read[a], shard.write[a], task.write} = {
  let v = shard.take[a]();
  task.yield();
  shard.done[a](v + 1)
}

fn drain_b() -> Unit / {shard.read[b], shard.write[b], task.write} = {
  let v = shard.take[b]();
  task.yield();
  shard.done[b](v + 1)
}

test "two shards that share nothing" {
  with_cell[a](0) { ca ->
  with_cell[b](0) { cb ->
    handle {
      simulate {
        let x = task.spawn(|| drain_a());
        let y = task.spawn(|| drain_b());
        task.join(x);
        task.join(y)
      }
    } with {
      shard.take[a]() -> 10,
      shard.done[a](v) -> cell_set(ca, v),
      shard.take[b]() -> 20,
      shard.done[b](v) -> cell_set(cb, v),
    }
  } }
}
"#;

/// The payoff, and the number the milestone is claimed on: two tasks whose footprints do not
/// conflict commute, so every order of their steps reaches the same world and exactly one of those
/// orders is run.
#[test]
fn tasks_touching_disjoint_resources_explore_one_interleaving() {
    let compiled = compile(DISJOINT);
    let explored = measure_reduction(&dpor(), &mut |seed: &Seed| compiled.at(seed));
    assert!(explored.passed());
    assert!(explored.exploration.exhaustive);
    assert_eq!(
        explored.exploration.explored, 1,
        "disjoint footprints commute, so there is one equivalence class"
    );
    let naive = explored
        .exploration
        .naive
        .expect("`measure_reduction` reports what an unpruned search would have run");
    assert!(
        naive.explored > 1,
        "the naive count must exceed one, or the reduction measures nothing"
    );
}

const CONTENDED: &str = r#"
effect shard {
  read  take[r]() -> Int
  write add[r](v: Int) -> Unit
}

fn push_a() -> Unit / {shard.read[a], shard.write[m], task.write} = {
  let v = shard.take[a]();
  task.yield();
  shard.add[m](v)
}

fn push_b() -> Unit / {shard.read[b], shard.write[m], task.write} = {
  let v = shard.take[b]();
  task.yield();
  shard.add[m](v)
}

test "two writers to one resource" {
  with_cell[m](0) { c ->
    handle {
      simulate {
        let x = task.spawn(|| push_a());
        let y = task.spawn(|| push_b());
        task.join(x);
        task.join(y);
        assert_eq(cell_get(c), 30)
      }
    } with {
      shard.take[a]() -> 10,
      shard.take[b]() -> 20,
      shard.add[m](v) -> cell_set(c, cell_get(c) + v),
    }
  }
}
"#;

/// The other side of the same predicate.
#[test]
fn two_writers_to_one_resource_explore_both_orders() {
    let compiled = compile(CONTENDED);
    let explored = compiled.search(&dpor());
    assert!(explored.passed());
    assert!(
        explored.exploration.explored >= 2,
        "a write/write pair must produce both orders, not one; ran {}",
        explored.exploration.explored
    );
}

/// ADR 0006 §10's first validation: comparing outcomes alone would pass on a run whose interleaving
/// differed and whose assertions happened not to notice.
#[test]
fn the_same_seed_twice_produces_the_same_steps_and_the_same_world() {
    let compiled = compile(CONTENDED);
    let seed = Seed::at(3, vec![1, 0, 1]);

    let mut a = compiled.machine();
    a.set_seed(seed.clone(), 10_000);
    let first = a.eval_test(0);
    let steps_a = a.simulated().expect("a region ran").steps.clone();
    let cells_a: Vec<String> = a.cells().slots().map(|(_, v)| v.render()).collect();

    let mut b = compiled.machine();
    b.set_seed(seed, 10_000);
    let second = b.eval_test(0);
    let steps_b = b.simulated().expect("a region ran").steps.clone();

    assert_eq!(first.is_ok(), second.is_ok());
    assert_eq!(
        steps_a, steps_b,
        "the step sequence is a function of the seed"
    );
    let cells_b: Vec<String> = b.cells().slots().map(|(_, v)| v.render()).collect();
    assert_eq!(
        cells_a, cells_b,
        "the final world is a function of the seed"
    );
}

/// The two streams are independent, so adding a draw to a program must not shift the interleaving
/// at any scheduling point that precedes it.
#[test]
fn a_later_draw_does_not_shift_an_earlier_choice() {
    let without = compile(CONTENDED);
    let with = compile(&CONTENDED.replace(
        "assert_eq(cell_get(c), 30)",
        "assert_eq(cell_get(c) + random.below(4) * 0, 30)",
    ));
    let seed = Seed::root(11);
    let before: Vec<u16> = without.at(&seed).steps.iter().map(|s| s.choice).collect();
    let after: Vec<u16> = with.at(&seed).steps.iter().map(|s| s.choice).collect();
    // The draw is in the root's last step, so every scheduling point that precedes it is shared and
    // must agree choice for choice.
    let shared = before.len().min(after.len()) - 1;
    assert!(shared > 2, "the fixture must have points before the draw");
    assert_eq!(
        before[..shared],
        after[..shared],
        "a `random` draw must not move the `sched` stream"
    );
}

const SLEEPER: &str = r#"
test "a long sleep costs no wall clock" {
  simulate {
    let t = task.spawn(|| {
      clock.sleep(30000000000);
      clock.now()
    });
    let woke = task.join(t);
    assert_eq(woke, 30000000000);
    assert_eq(clock.now(), 30000000000)
  }
}
"#;

/// Thirty simulated seconds is a jump rather than a wait, and the region's virtual duration is a
/// function of its sleeps rather than of the machine.
#[test]
fn a_long_sleep_is_a_jump() {
    let compiled = compile(SLEEPER);
    let started = std::time::Instant::now();
    let run = compiled.at(&Seed::default());
    assert!(passed(&run), "the sleeper should pass: {:?}", run.verdict);
    assert_eq!(run.virtual_time, 30_000_000_000);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "a sleeping test must cost no wall clock"
    );
}

const TIMEOUT: &str = r#"
effect log {
  write note[r](v: Int) -> Unit
}

fn worker() -> Unit / {log.write[l], task.write} = {
  task.yield();
  task.yield();
  log.note[l](1)
}

test "a deadline cannot fire while the worker can still run" {
  with_cell[l](0) { c ->
    handle {
      simulate {
        let w = task.spawn(|| worker());
        let d = task.spawn(|| {
          clock.sleep(5);
          log.note[l](2)
        });
        task.join(w);
        task.join(d);
        assert_eq(cell_get(c), 2)
      }
    } with {
      log.note[l](v) -> cell_set(c, v),
    }
  }
}
"#;

/// Time moves only when nothing is runnable, so a simulated timeout never pre-empts work that could
/// still complete.
#[test]
fn a_timeout_never_fires_while_anything_can_run() {
    let compiled = compile(TIMEOUT);
    let explored = compiled.search(&dpor());
    assert!(
        explored.passed(),
        "the deadline fired early under some interleaving: {:?}",
        explored.diagnostic.map(|d| d.message)
    );
    assert!(explored.exploration.exhaustive);
}

const OUTLIVES: &str = r#"
effect log {
  write note[r](v: Int) -> Unit
}

test "a task still runnable when the body returns is run to completion" {
  with_cell[l](0) { c -> {
    handle {
      simulate {
        task.spawn(|| log.note[l](9));
        7
      }
    } with {
      log.note[l](v) -> cell_set(c, v),
    };
    assert_eq(cell_get(c), 9)
  } }
}
"#;

/// The handler is the scope: a task that outlives every join still finishes inside the region that
/// made it, which is the whole of the structure rule.
#[test]
fn a_task_that_outlives_every_join_still_finishes_inside_the_region() {
    let compiled = compile(OUTLIVES);
    let explored = compiled.search(&dpor());
    assert!(
        explored.passed(),
        "an unjoined task must be drained before the region delivers: {:?}",
        explored.diagnostic.map(|d| d.message)
    );
}

const DEADLOCK: &str = r#"
type Slot =
  | Empty
  | Peer(Task<Int>)

test "two tasks waiting on each other stop the region" {
  simulate {
    with_cell[slot](Empty) { peer -> {
      let first = task.spawn(|| {
        clock.sleep(1);
        match cell_get(peer) {
          Peer(other) -> task.join(other),
          Empty -> 0,
        }
      });
      let second = task.spawn(|| task.join(first));
      cell_set(peer, Peer(second));
      task.join(first)
    } }
  }
}
"#;

/// The region ends when its last task ends, so a wait that nothing can satisfy is a diagnostic
/// rather than a hang.
#[test]
fn a_join_cycle_is_a_diagnostic_and_not_a_hang() {
    let compiled = compile(DEADLOCK);
    let run = compiled.at(&Seed::default());
    let Verdict::Failed(diagnostic) = &run.verdict else {
        panic!("a join cycle must not be reported as a pass");
    };
    assert_eq!(diagnostic.code, ply_span::codes::DEADLOCK);
    assert!(
        diagnostic
            .labels
            .iter()
            .any(|l| l.message.contains("waits here")),
        "the diagnostic names what each blocked task is waiting on: {diagnostic:?}"
    );
}

/// The budget is a search parameter, not a semantics: raising it changes only the thoroughness of a
/// test, never the value a region delivers.
#[test]
fn the_budget_does_not_change_what_a_passing_program_means() {
    let compiled = compile(DISJOINT);
    let value = |budget: u32| {
        let plan = Plan { budget, ..dpor() };
        let explored = compiled.search(&plan);
        assert!(explored.passed());
        explored.exploration.explored
    };
    assert_eq!(value(1), 1);
    assert_eq!(value(256), 1);
}

/// `once` is the replay path: one interleaving, the one the seed names.
#[test]
fn once_runs_exactly_one_interleaving() {
    let compiled = compile(CONTENDED);
    let plan = Plan::once(Seed::at(0, vec![1, 0]));
    let explored = compiled.search(&plan);
    assert_eq!(plan.mode, SimMode::Once);
    assert_eq!(explored.exploration.explored, 1);
    assert!(
        explored.exploration.race.is_none(),
        "there is nothing to observe under `once`, so a race must never be inferred"
    );
}

/// The region's value is the seed's, and a task handle is a key rather than a pointer: two spawns
/// in one region get distinct ids and joining answers the body's value.
#[test]
fn a_region_delivers_the_value_its_body_returned() {
    let compiled = compile(
        r#"
test "join answers the body" {
  simulate {
    let a = task.spawn(|| 40);
    let b = task.spawn(|| 2);
    assert_eq(task.join(a) + task.join(b), 42)
  }
}
"#,
    );
    let mut machine = compiled.machine();
    machine.set_seed(Seed::default(), 10_000);
    machine.eval_test(0).expect("the region delivers 42");
    assert!(machine.simulated().is_some());
}

/// A `simulate` region is machine-only, so the tree-walker refuses it by name rather than running
/// one unnamed interleaving that the cache would then keep.
#[test]
fn the_tree_walker_refuses_a_region() {
    let compiled = compile(OUTLIVES);
    let refusals = ply_eval::machine_only_clauses(&compiled.program);
    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals[0].code, ply_span::codes::MACHINE_ONLY_CLAUSE);

    let mut interp = ply_eval::Interp::new(&compiled.program, &compiled.resolved, &compiled.check);
    let refused = interp.eval_test(0).expect_err("the tree-walker declines");
    assert!(ply_eval::is_machine_only(&refused));
}

/// Values that cross the boundary are ordinary values: the world a region wrote through is the
/// machine's, threaded, and it survives the region.
#[test]
fn the_world_a_region_wrote_survives_it() {
    let compiled = compile(OUTLIVES);
    let mut machine = compiled.machine();
    machine.cells_mut().journal();
    machine.set_seed(Seed::default(), 10_000);
    machine.eval_test(0).expect("passes");
    assert!(
        machine
            .cells()
            .journalled()
            .iter()
            .any(|(_, v)| matches!(v, Value::Int(9))),
        "the task's write is in the cell the region held when it closed"
    );
}

/// Two regions in sequence — only *nesting* is `E0416` — are one choice sequence, so the record the
/// search reads covers both of them.
#[test]
fn two_regions_in_sequence_are_one_record_and_one_choice_sequence() {
    let compiled = compile(
        r#"
test "two regions" {
  with_cell[n](0) { c -> {
    simulate {
      let a = task.spawn(|| { task.yield(); cell_set(c, cell_get(c) + 1) });
      task.join(a)
    };
    simulate {
      let b = task.spawn(|| { task.yield(); cell_set(c, cell_get(c) + 1) });
      task.join(b)
    };
    assert_eq(cell_get(c), 2)
  } }
}
"#,
    );
    let mut machine = compiled.machine();
    machine.set_seed(Seed::default(), 10_000);
    machine.eval_test(0).expect("both regions run");
    let record = machine.simulated().expect("a record");
    let regions: std::collections::BTreeSet<u32> =
        record.steps.iter().map(|s| s.region.0).collect();
    assert_eq!(
        regions,
        [0u32, 1].into_iter().collect(),
        "the record covers one region only: {regions:?}"
    );
    let choices: Vec<u16> = record.steps.iter().map(|s| s.choice).collect();
    assert_eq!(
        compiled.at(&Seed::at(0, choices.clone())).steps.len(),
        choices.len(),
        "the realized choice sequence, pinned, does not replay its own run"
    );
}

/// A `resume k` clause outside the region delivers the region's value onto the stack the resumption
/// spliced it over, not onto the one `simulate` was entered on.
#[test]
fn a_region_resumed_from_a_clause_keeps_the_clauses_pending_work() {
    let compiled = compile(
        r#"
effect pick {
  read choose[r]() -> Int
}

test "the clause has work after the resumption" {
  with_cell[n](0) { c -> {
    handle {
      simulate {
        let a = task.spawn(|| { let v = pick.choose[n](); cell_set(c, cell_get(c) + v) });
        task.join(a)
      }
    } with {
      pick.choose[n]() resume k -> { k(1); cell_set(c, cell_get(c) + 10) },
    };
    assert_eq(cell_get(c), 11)
  } }
}
"#,
    );
    let mut machine = compiled.machine();
    machine.set_seed(Seed::default(), 10_000);
    machine
        .eval_test(0)
        .expect("the work after `k(1)` must still run");
}

#[test]
fn abandoning_a_region_is_a_diagnostic_rather_than_a_truncated_trace() {
    let compiled = compile(
        r#"
effect bail {
  read stop[r]() -> Int
}

test "the handler never resumes" {
  with_cell[n](0) { c -> {
    handle {
      simulate {
        let a = task.spawn(|| { task.yield(); bail.stop[n]() });
        let b = task.spawn(|| { task.yield(); cell_set(c, 1); 5 });
        task.join(a);
        task.join(b)
      }
    } with {
      bail.stop[n]() resume k -> 7,
    };
    cell_get(c)
  } }
}
"#,
    );
    let mut machine = compiled.machine();
    machine.set_seed(Seed::default(), 10_000);
    let refused = machine.eval_test(0).expect_err("the region was abandoned");
    assert_eq!(refused.code, ply_span::codes::TASK_ESCAPES_SCOPE);
    assert!(refused.message.contains("abandoned"), "{}", refused.message);
}

/// ADR 0006 §1.6.
#[test]
fn resuming_a_region_that_already_ended_is_a_diagnostic() {
    let compiled = compile(
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
    let refused = machine.eval_test(0).expect_err("the region has ended");
    assert_eq!(refused.code, ply_span::codes::TASK_ESCAPES_SCOPE);
    assert!(
        refused.message.contains("already ended"),
        "{}",
        refused.message
    );
}

/// A `handle` written inside the region encloses the tasks it lexically contains.
#[test]
fn a_task_runs_under_the_handlers_written_inside_the_region() {
    let compiled = compile(
        r#"
effect counter {
  write bump[r]() -> Unit
}

test "a handler inside the region" {
  with_cell[n](0) { c ->
    simulate {
      handle {
        let a = task.spawn(|| counter.bump[n]());
        let b = task.spawn(|| counter.bump[n]());
        task.join(a);
        task.join(b);
        assert_eq(cell_get(c), 2)
      } with {
        counter.bump[n]() -> cell_set(c, cell_get(c) + 1),
      }
    }
  }
}
"#,
    );
    let mut machine = compiled.machine();
    machine.set_seed(Seed::default(), 10_000);
    machine
        .eval_test(0)
        .expect("the handler inside the region covers its tasks");
}

#[test]
fn a_cell_allocation_is_an_access_of_the_step_that_made_it() {
    let compiled = compile(
        r#"
test "two tasks that each allocate" {
  simulate {
    let a = task.spawn(|| with_cell[p](1) { c -> cell_get(c) });
    let b = task.spawn(|| with_cell[p](2) { c -> cell_get(c) });
    assert_eq(task.join(a) + task.join(b), 3)
  }
}
"#,
    );
    let run = compiled.at(&Seed::default());
    assert!(
        run.steps
            .iter()
            .any(|s| s.accesses.accesses().any(|a| a.to_string() == "cell.alloc")),
        "no step recorded an allocation: {:?}",
        run.steps
            .iter()
            .map(|s| s
                .accesses
                .accesses()
                .map(|a| a.to_string())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
}

/// **A scheduling point inside an `iterate` step.**
#[test]
fn two_iterate_loops_interleave_and_each_keeps_its_own_countdown() {
    let c = compile(ITERATE_TASKS);
    let explored = c.search(&dpor());
    assert!(
        explored.passed(),
        "an interleaving of two `iterate` loops did not finish: {:?}",
        explored.diagnostic.map(|d| d.message)
    );
    assert!(explored.exploration.exhaustive);
    // More than one interleaving, or the search found no scheduling point inside the loop and the
    // case is asserting nothing.
    assert!(
        explored.exploration.explored > 1,
        "one interleaving is not a search: {}",
        explored.exploration.explored
    );
}

const ITERATE_TASKS: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump_n(k: Int) -> Unit / {counter.read[r], counter.write[r]} =
  iterate({ left: k }, k + 1, |s: { left: Int }|
    if s.left <= 0 {
      Stop(())
    } else {
      let seen = counter.get[r]();
      counter.put[r](seen + 1);
      Continue({ left: s.left - 1 })
    })

test "two iterate loops" {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| bump_n(3));
        let b = task.spawn(|| bump_n(3));
        task.join(a);
        task.join(b);
        // Each loop ran its own three rounds however they interleaved, so the
        // counter is at least 3 — a shared countdown would stop one loop short.
        assert(cell_get(c) >= 3 && cell_get(c) <= 6)
      }
    } with {
      counter.get[r]() -> cell_get(c),
      counter.put[r](v) -> cell_set(c, v),
    }
  }
}
"#;

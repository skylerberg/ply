//! An adversarial audit of exploration soundness.

use ply_core::{CheckOutput, check_program};
use ply_eval::explore::Interleaving;
use ply_eval::{Dependence, Machine, Plan, Seed, explore, explore_under};
use ply_span::SourceId;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::cell::RefCell;
use std::collections::BTreeSet;

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

/// What one interleaving is observable as: how the test ended, and the world it left behind.
fn observe(c: &Compiled, seed: &Seed) -> (Interleaving, String) {
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    machine.cells_mut().journal();
    machine.set_seed(seed.clone(), 10_000);
    let outcome = machine.eval_test(0);
    let verdict = match &outcome {
        Ok(()) => "ok".to_string(),
        Err(diagnostic) => format!("{} {}", diagnostic.code, diagnostic.message),
    };
    let world: Vec<String> = machine
        .cells()
        .journalled()
        .iter()
        .map(|(slot, value)| format!("#{}={}", slot.index(), value.render()))
        .collect();
    let record = machine
        .simulated()
        .expect("the fixture reaches a `simulate` region");
    (
        record.interleaving(&outcome),
        format!("{verdict} | {}", world.join(",")),
    )
}

const ENUMERATION_CAP: usize = 3000;

/// Every schedule the recorded enabled sets admit, run exactly once.
fn every_schedule(c: &Compiled) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut frontier: Vec<Vec<u16>> = vec![Vec::new()];
    let mut visited: BTreeSet<Vec<u16>> = BTreeSet::new();
    let mut runs = 0usize;
    while let Some(prefix) = frontier.pop() {
        if !visited.insert(prefix.clone()) {
            continue;
        }
        runs += 1;
        assert!(
            runs <= ENUMERATION_CAP,
            "the fixture is too large to enumerate; shrink it rather than trusting a sample"
        );
        let (run, outcome) = observe(c, &Seed::at(0, prefix.clone()));
        seen.insert(outcome);
        let choices: Vec<u16> = run.steps.iter().map(|s| s.choice).collect();
        for (i, step) in run.steps.iter().enumerate().skip(prefix.len()) {
            for choice in 0..step.enabled.len() as u16 {
                if choice != step.choice {
                    let mut next = choices[..i].to_vec();
                    next.push(choice);
                    frontier.push(next);
                }
            }
        }
    }
    seen
}

struct Searched {
    outcomes: BTreeSet<String>,
    explored: u32,
    exhaustive: bool,
    failed: bool,
}

fn search_with(c: &Compiled, dependence: Dependence) -> Searched {
    let outcomes = RefCell::new(BTreeSet::new());
    let plan = Plan {
        budget: 4096,
        ..Plan::default()
    };
    let explored = explore_under(&plan, dependence, &mut |seed: &Seed| {
        let (run, outcome) = observe(c, seed);
        outcomes.borrow_mut().insert(outcome);
        run
    });
    Searched {
        outcomes: outcomes.into_inner(),
        explored: explored.exploration.explored,
        exhaustive: explored.exploration.exhaustive,
        failed: explored.exploration.failure.is_some(),
    }
}

fn search(c: &Compiled) -> Searched {
    search_with(c, Dependence::Exact)
}

/// The property pruning has to preserve, checked against the independent enumerator rather than
/// argued.
fn assert_reaches_every_outcome(name: &str, source: &str) {
    let compiled = compile(source);
    let searched = search(&compiled);
    let all = every_schedule(&compiled);
    // A search that stopped at a failure is not claiming to have seen the rest, so only a search
    // that reached its frontier owes the whole set.
    if !searched.failed {
        let missing: Vec<&String> = all.difference(&searched.outcomes).collect();
        assert!(
            missing.is_empty(),
            "{name}: the search ran {} interleavings, reported exhaustive={}, and never reached \
             {missing:?} — every schedule reaches {all:?}",
            searched.explored,
            searched.exhaustive
        );
    } else {
        assert!(
            all.iter().any(|o| !o.starts_with("ok ")),
            "{name}: the search reported a failure no schedule produces"
        );
    }
}

/// A test may contain two `simulate` regions in sequence: only *nesting* is `E0416`.
const RACE_IN_THE_FIRST_OF_TWO_REGIONS: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], task.write} = {
  let seen = counter.get[n]();
  task.yield();
  counter.put[n](seen + 1)
}

test "the lost update is in the first of two regions" {
  with_cell[n](0) { c ->
    handle {
      {
        simulate {
          let a = task.spawn(|| bump());
          let b = task.spawn(|| bump());
          task.join(a);
          task.join(b)
        };
        simulate {
          let d = task.spawn(|| 1);
          task.join(d)
        };
        assert_eq(cell_get(c), 2)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;

/// The same race with one region, which is the control: whatever the two-region fixture reports,
/// this one must find the lost update.
const RACE_IN_ONE_REGION: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], task.write} = {
  let seen = counter.get[n]();
  task.yield();
  counter.put[n](seen + 1)
}

test "the lost update with one region" {
  with_cell[n](0) { c ->
    handle {
      {
        simulate {
          let a = task.spawn(|| bump());
          let b = task.spawn(|| bump());
          task.join(a);
          task.join(b)
        };
        assert_eq(cell_get(c), 2)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;

/// The same two regions in the other order, so that the race is in the region the recording covers
/// and the region before it is the one the search perturbs.
const RACE_IN_THE_SECOND_OF_TWO_REGIONS: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], task.write} = {
  let seen = counter.get[n]();
  task.yield();
  counter.put[n](seen + 1)
}

test "the lost update is in the second of two regions" {
  with_cell[n](0) { c ->
    handle {
      {
        simulate {
          let d = task.spawn(|| 1);
          task.join(d)
        };
        simulate {
          let a = task.spawn(|| bump());
          let b = task.spawn(|| bump());
          task.join(a);
          task.join(b)
        };
        assert_eq(cell_get(c), 2)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;

#[test]
fn the_control_finds_the_lost_update() {
    let searched = search(&compile(RACE_IN_ONE_REGION));
    assert!(
        searched.failed,
        "the fixture proves nothing unless the one-region form of it fails"
    );
}

/// A second `simulate` region in the same test hides the first one's races completely, and the run
/// is reported as an exhaustive proof.
#[test]
fn a_race_in_the_first_of_two_regions_is_reachable_and_must_be_found() {
    let compiled = compile(RACE_IN_THE_FIRST_OF_TWO_REGIONS);
    let sampled = (0..64u64)
        .filter(|&root| !observe(&compiled, &Seed::root(root)).1.starts_with("ok "))
        .count();
    assert!(
        sampled > 0,
        "the fixture proves nothing unless some seed fails it"
    );
    let searched = search(&compiled);
    assert!(
        searched.failed,
        "the search ran {} interleavings and reported exhaustive={} over a program {sampled} of \
         64 sampled seeds fail: a `simulate` region other than the recorded one is never explored",
        searched.explored, searched.exhaustive
    );
}

/// The same defect from the other side.
#[test]
fn a_search_over_two_regions_does_not_report_a_simulation_divergence() {
    let searched = search(&compile(RACE_IN_THE_SECOND_OF_TWO_REGIONS));
    let diverged: Vec<&String> = searched
        .outcomes
        .iter()
        .filter(|o| o.contains("E0415"))
        .collect();
    assert!(
        diverged.is_empty(),
        "the search perturbed a region it was not branching in: {diverged:?}"
    );
}

/// A handler installed outside a `simulate` region may discard the continuation it is handed, which
/// abandons the region with tasks still runnable.
const ABANDONED_REGION: &str = r#"
effect bail {
  read stop[r]() -> Int
}

test "a handler outside the region that never resumes" {
  with_cell[n](0) { c -> {
    handle {
      simulate {
        let a = task.spawn(|| { task.yield(); cell_set(c, cell_get(c) + 1); bail.stop[n]() });
        let b = task.spawn(|| { task.yield(); cell_set(c, cell_get(c) + 10); 0 });
        task.join(a);
        task.join(b)
      }
    } with {
      bail.stop[n]() resume k -> 7,
    };
    assert_eq(cell_get(c), 1)
  } }
}
"#;

/// Whether `b` got to write before `a` aborted the region is a real difference with a real
/// assertion behind it, and the enumerator reaches both.
#[test]
fn an_abandoned_region_does_not_hide_the_schedules_it_cut_short() {
    assert_reaches_every_outcome("an abandoned region", ABANDONED_REGION);
}

/// the dependence relation: "Two steps that are not dependent commute: executing them in either order from
/// the same configuration reaches the same world and the same result."
const ALLOCATING_TASKS: &str = r#"
test "two tasks that each allocate a cell" {
  with_cell[o](0) { out -> {
    simulate {
      let a = task.spawn(|| with_cell[p](1) { c -> {
        task.yield();
        cell_set(out, cell_get(out) + cell_get(c))
      } });
      let b = task.spawn(|| with_cell[p](2) { c -> {
        task.yield();
        cell_set(out, cell_get(out) + cell_get(c))
      } });
      task.join(a);
      task.join(b)
    };
    cell_get(out)
  } }
}
"#;

#[test]
fn allocation_order_is_part_of_the_world_and_must_not_be_pruned_away() {
    assert_reaches_every_outcome("two allocating tasks", ALLOCATING_TASKS);
}

/// A conflict on one resource that is *conditional* on a value read from another: `b` writes the
/// shared cell one way or the other depending on whether it saw `a`'s write.
const CONDITIONAL_CONFLICT: &str = r#"
test "a conflict conditional on a value from elsewhere" {
  with_cell[f](0) { flag ->
  with_cell[g](0) { guard -> {
    simulate {
      let a = task.spawn(|| { cell_set(flag, 1); task.yield() });
      let b = task.spawn(|| {
        let seen = cell_get(flag);
        task.yield();
        if seen == 1 { cell_set(guard, 9) } else { cell_set(guard, 1) }
      });
      task.join(a);
      task.join(b)
    };
    cell_get(guard)
  } } }
}
"#;

#[test]
fn a_conditional_conflict_is_explored_in_both_directions() {
    assert_reaches_every_outcome("a conditional conflict", CONDITIONAL_CONFLICT);
}

/// A race whose two orders agree on every value and differ only in the order the effects were
/// performed.
const SAME_VALUE_DIFFERENT_EFFECTS: &str = r#"
effect log {
  write note[r](v: Int) -> Unit
}

fn tag(n: Int) -> Unit / {log.write[l], task.write} = {
  task.yield();
  log.note[l](n)
}

test "two orders, one value, two effect sequences" {
  with_cell[l](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| tag(1));
        let b = task.spawn(|| tag(2));
        task.join(a);
        task.join(b)
      }
    } with {
      log.note[l](v) -> cell_set(c, cell_get(c) * 10 + v),
    }
  }
}
"#;

#[test]
fn a_race_that_differs_only_in_the_effects_performed_is_explored() {
    let compiled = compile(SAME_VALUE_DIFFERENT_EFFECTS);
    assert_reaches_every_outcome(
        "same value, different effects",
        SAME_VALUE_DIFFERENT_EFFECTS,
    );
    let searched = search(&compiled);
    assert!(
        searched.outcomes.len() > 1,
        "the fixture proves nothing unless the two orders are distinguishable"
    );
}

/// A deadlock reachable only through some interleavings: the cycle exists only when `first` reads
/// the slot after the body has filled it.
const CONDITIONAL_DEADLOCK: &str = r#"
type Slot =
  | Empty
  | Peer(Task<Int>)

test "a deadlock only some interleavings reach" {
  simulate {
    with_cell[s](Empty) { slot -> {
      let first = task.spawn(|| {
        task.yield();
        match cell_get(slot) {
          Peer(other) -> task.join(other),
          Empty -> 0,
        }
      });
      let second = task.spawn(|| task.join(first));
      task.yield();
      cell_set(slot, Peer(second));
      task.join(first)
    } }
  }
}
"#;

#[test]
fn a_deadlock_that_only_some_interleavings_reach_is_found() {
    let searched = search(&compile(CONDITIONAL_DEADLOCK));
    assert!(
        searched.failed,
        "a deadlock behind a cell read must be reachable by the search, not only by a lucky seed"
    );
}

/// Two tasks whose only shared channel is the region's `random` stream.
const SHARED_RANDOM_STREAM: &str = r#"
test "two tasks drawing from one stream" {
  with_cell[o](0) { out -> {
    simulate {
      let a = task.spawn(|| { task.yield(); cell_set(out, cell_get(out) * 10 + random.below(2)) });
      let b = task.spawn(|| { task.yield(); cell_set(out, cell_get(out) * 10 + random.below(2)) });
      task.join(a);
      task.join(b)
    };
    cell_get(out)
  } }
}
"#;

#[test]
fn a_shared_random_stream_is_interference_the_search_reaches() {
    assert_reaches_every_outcome("a shared random stream", SHARED_RANDOM_STREAM);
}

/// The handler that discharges a task's effect is itself shared state.
const ONE_HANDLER_TWO_LABELS: &str = r#"
effect shard {
  read  take[r]() -> Int
  write add[r](v: Int) -> Unit
}

test "two labels, one handler, one cell" {
  with_cell[m](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| { let v = shard.take[x](); task.yield(); shard.add[x](v + 1) });
        let b = task.spawn(|| { let v = shard.take[y](); task.yield(); shard.add[y](v + 1) });
        task.join(a);
        task.join(b)
      }
    } with {
      shard.take[x]() -> cell_get(c),
      shard.take[y]() -> cell_get(c),
      shard.add[x](v) -> cell_set(c, v),
      shard.add[y](v) -> cell_set(c, v),
    }
  }
}
"#;

#[test]
fn a_handler_that_shares_state_across_two_labels_is_still_a_race() {
    assert_reaches_every_outcome("one handler, two labels", ONE_HANDLER_TWO_LABELS);
}

/// A handler installed *inside* a `simulate` region, wrapping the `spawn`.
const HANDLER_INSIDE_THE_REGION: &str = r#"
effect counter {
  write bump[r]() -> Unit
}

test "a handler installed inside the region" {
  with_cell[n](0) { c ->
    simulate {
      handle {
        let a = task.spawn(|| counter.bump[n]());
        task.join(a);
        assert_eq(cell_get(c), 1)
      } with {
        counter.bump[n]() -> cell_set(c, cell_get(c) + 1),
      }
    }
  }
}
"#;

#[test]
fn a_handler_inside_the_region_covers_the_tasks_it_encloses() {
    let compiled = compile(HANDLER_INSIDE_THE_REGION);
    let (_, outcome) = observe(&compiled, &Seed::default());
    assert!(
        !outcome.contains("E0303"),
        "a well-typed program found no handler at run time: {outcome}"
    );
}

/// A `resume k` clause outside the region may resume more than once; the machine's continuations
/// are multi-shot and the control fixture below proves it.
const MULTI_SHOT_ACROSS_A_REGION: &str = r#"
effect pick {
  read choose[r]() -> Int
}

test "a multi-shot handler outside a region" {
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
"#;

const MULTI_SHOT_WITHOUT_A_REGION: &str = r#"
effect pick {
  read choose[r]() -> Int
}

test "a multi-shot handler with no region" {
  with_cell[n](0) { c -> {
    handle {
      let v = pick.choose[n]();
      cell_set(c, cell_get(c) + v);
      cell_get(c)
    } with {
      pick.choose[n]() resume k -> { k(1); k(2) },
    };
    cell_get(c)
  } }
}
"#;

#[test]
fn a_region_does_not_silently_swallow_a_second_resumption() {
    let control = compile(MULTI_SHOT_WITHOUT_A_REGION);
    let mut machine = Machine::new(&control.program, &control.resolved, &control.check);
    machine.cells_mut().journal();
    machine.eval_test(0).expect("the control passes");
    let expected: Vec<String> = machine
        .cells()
        .journalled()
        .iter()
        .map(|(_, v)| v.render())
        .collect::<Vec<_>>();
    assert_eq!(
        expected,
        vec!["3".to_string()],
        "the control must resume twice, or it pins nothing"
    );

    let (_, outcome) = observe(&compile(MULTI_SHOT_ACROSS_A_REGION), &Seed::default());
    assert!(
        !outcome.starts_with("ok | #0=1"),
        "a `simulate` region dropped a resumption without saying so: {outcome}"
    );
}

/// The reduction is a ratio, and a denominator produced by the same search as the numerator is a
/// number that cannot disagree with itself.
#[test]
fn the_naive_baseline_enumerates_every_schedule() {
    for (name, source) in [
        ("a conditional conflict", CONDITIONAL_CONFLICT),
        ("two allocating tasks", ALLOCATING_TASKS),
        (
            "same value, different effects",
            SAME_VALUE_DIFFERENT_EFFECTS,
        ),
        ("a shared random stream", SHARED_RANDOM_STREAM),
    ] {
        let compiled = compile(source);
        let naive = search_with(&compiled, Dependence::All);
        assert!(
            !naive.failed,
            "{name}: a baseline that stopped at a failure counted nothing"
        );
        assert_eq!(
            naive.outcomes,
            every_schedule(&compiled),
            "{name}: the naive baseline is not the unpruned space it is measured against"
        );
    }
}

/// The budget is a search parameter and not a semantics: raising it may not change the value or the
/// world a passing program delivers.
#[test]
fn widening_the_budget_does_not_change_what_a_program_means() {
    let compiled = compile(SAME_VALUE_DIFFERENT_EFFECTS);
    let at = |budget: u32| {
        let plan = Plan {
            budget,
            ..Plan::default()
        };
        explore(&plan, &mut |seed: &Seed| observe(&compiled, seed).0)
            .exploration
            .failure
    };
    assert_eq!(at(1), None);
    assert_eq!(at(256), None);
}

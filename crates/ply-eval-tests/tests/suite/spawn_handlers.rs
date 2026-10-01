//! A task performs against the handlers around its `task.spawn` as they stood at the spawn,
//! whatever its spawner does before the task first runs. No access tells those orders apart, so
//! the pruned search runs one of them; these run under the unpruned one `--measure-reduction` adds.

use crate::fixture::Compiled;
use ply_eval::explore::Step;
use ply_eval::sched::ROOT;
use ply_eval::{Plan, Seed, TaskId, measure_reduction};

const TESTS: &str = r#"
effect ask {
  read get() -> Int
}

effect abort {
  write stop(code: Int) -> Int
}

fn abandoned(n: Int) -> Int =
  handle {
    simulate {
      let t = handle { task.spawn(|| ask.get()) } with { ask.get() -> n };
      abort.stop(n) + task.join(t)
    }
  } with {
    abort.stop(code) resume k -> code,
  }

test "the handle around the spawn has ended" {
  let total = simulate {
    let t = handle { task.spawn(|| ask.get()) } with { ask.get() -> 7 };
    task.join(t)
  };
  assert(total == 7)
}

test "a handle is installed after the spawn" {
  let total = simulate {
    let t = handle { task.spawn(|| ask.get()) } with { ask.get() -> 7 };
    handle {
      task.yield();
      task.join(t)
    } with {
      ask.get() -> 100,
    }
  };
  assert(total == 7)
}

test "the region unwinds before the task starts" { assert(abandoned(7) == 7) }
"#;

const SPAWNED: TaskId = TaskId(1);

/// The steps of every run of the test under the unpruned search, each of which passed.
#[track_caller]
fn every_order(name: &str) -> Vec<Vec<Step>> {
    let compiled = Compiled::new(TESTS);
    let index = compiled.index_of(name);
    let mut machine = compiled.machine();
    let plan = Plan::default();
    let mut runs = Vec::new();
    let explored = measure_reduction(&plan, &mut |seed: &Seed| {
        machine.set_seed(seed.clone(), plan.steps);
        let (outcome, _) = machine.eval_test(index).into_parts();
        let record = machine
            .simulated()
            .unwrap_or_else(|| panic!("`{name}` ran no region: {outcome:?}"));
        runs.push(record.steps.clone());
        record.interleaving(&outcome)
    });
    assert!(explored.passed(), "`{name}`: {:#?}", explored.diagnostic);
    let naive = explored.exploration.naive.expect("the unpruned search ran");
    assert!(!naive.bounded, "`{name}`: the unpruned search ran {naive}");
    runs
}

/// Who ran where the spawned task first could: its spawner, whose `handle` then ends, or the task.
fn first_after_the_spawn(steps: &[Step]) -> Option<TaskId> {
    steps
        .iter()
        .find(|s| s.enabled.contains(&SPAWNED))
        .map(|s| s.task)
}

#[track_caller]
fn both_orders_ran(name: &str) {
    let firsts: Vec<Option<TaskId>> = every_order(name)
        .iter()
        .map(|steps| first_after_the_spawn(steps))
        .collect();
    assert!(
        firsts.contains(&Some(ROOT)),
        "`{name}`: no order let the spawner's `handle` end before the task started: {firsts:?}"
    );
    assert!(
        firsts.contains(&Some(SPAWNED)),
        "`{name}`: no order started the task first: {firsts:?}"
    );
}

#[test]
fn a_task_answers_from_the_handle_around_its_spawn_after_that_handle_ended() {
    both_orders_ran("the handle around the spawn has ended");
}

#[test]
fn a_task_does_not_answer_from_a_handle_installed_after_its_spawn() {
    both_orders_ran("a handle is installed after the spawn");
}

/// A task that never starts gives back the handlers it held from its spawn as its region ends.
#[test]
fn a_region_unwinds_before_a_task_holding_its_handlers_starts() {
    let runs = every_order("the region unwinds before the task starts");
    assert!(
        runs.iter()
            .any(|steps| steps.iter().all(|s| s.task != SPAWNED)),
        "no order ended the region before the task ran"
    );
}

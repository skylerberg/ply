use crate::fixture::Compiled;
use ply_eval::explore::{Explored, Interleaving, Step};
use ply_eval::{Diagnostic, Machine, Plain, Plan, Provider, Seed, SimMode, codes, explore, slot};
use std::rc::Rc;

/// Everything one interleaving is allowed to be a function of, rendered.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Transcript {
    verdict: String,
    virtual_time: i64,
    steps: Vec<String>,
    /// The operations the tier reports the run performed, handled ones included.
    performed: String,
}

fn render_step(step: &Step) -> String {
    let enabled: Vec<String> = step.enabled.iter().map(|t| t.to_string()).collect();
    let accesses: Vec<String> = step.accesses.accesses().map(|a| a.to_string()).collect();
    format!(
        "{} choice={} enabled=[{}] accesses=[{}] stamp={:?} in={:?}",
        step.task,
        step.choice,
        enabled.join(","),
        accesses.join(","),
        step.stamp,
        step.definition.as_ref().map(|d| d.to_string()),
    )
}

/// A program and the one tier every run of it goes to in turn, as one worker's runs do.
struct Audited {
    compiled: Compiled,
    tier: Rc<dyn ply_eval::Compiled>,
}

impl Audited {
    fn new(source: &str) -> Audited {
        let compiled = Compiled::named("t", source);
        let tier = compiled.unit().attach();
        Audited { compiled, tier }
    }

    /// Test `index` at `seed`, run by the tier through the `simulate` region every fixture here
    /// opens: a transcript of anything else would compare nothing the program did.
    fn run(&self, index: usize, seed: &Seed) -> (Machine<'_>, Result<(), Diagnostic>) {
        let mut machine = self.compiled.machine_on(Rc::clone(&self.tier));
        machine.set_seed(seed.clone(), 100_000);
        let (outcome, _) = machine.eval_test(index).into_parts();
        assert_eq!(
            machine.compiled_counts(),
            (1, 0),
            "test {index} at seed {seed} was not run by the tier: {outcome:?}"
        );
        if let Err(d) = &outcome {
            assert_ne!(
                d.code,
                codes::INTERNAL_ERROR,
                "test {index} at seed {seed} failed in Ply: {d:?}"
            );
        }
        let steps = machine.simulated().map_or(0, |record| record.steps.len());
        assert!(
            steps > 0,
            "test {index} at seed {seed} recorded no step of its region"
        );
        (machine, outcome)
    }

    fn transcript_of(&self, index: usize, seed: &Seed) -> Transcript {
        let (machine, outcome) = self.run(index, seed);
        let record = machine.simulated().expect("the run recorded its region");
        let trace = machine.trace();
        Transcript {
            verdict: match &outcome {
                Ok(()) => "passed".to_string(),
                // The code and the message, so that two different failures cannot compare equal.
                Err(d) => format!("{}: {}", d.code, d.message),
            },
            virtual_time: record.virtual_time,
            steps: record.steps.iter().map(render_step).collect(),
            performed: format!("{} in {} performs", trace.footprint(), trace.performs()),
        }
    }

    fn transcript(&self, seed: &Seed) -> Transcript {
        self.transcript_of(0, seed)
    }

    fn interleaving_at(&self, index: usize, seed: &Seed) -> Interleaving {
        let (machine, outcome) = self.run(index, seed);
        machine
            .simulated()
            .expect("the run recorded its region")
            .interleaving(&outcome)
    }
}

/// The failure a search stopped at is the fixture's assertion seeing an update lost.
#[track_caller]
fn assert_lost_update(explored: &Explored, total: i64, what: &str) {
    let Some(d) = &explored.diagnostic else {
        panic!(
            "{what}: no interleaving of {} failed",
            explored.exploration.explored
        );
    };
    assert_eq!(d.code, codes::ASSERTION_FAILED, "{what}: {d:?}");
    let lost = format!("assertion failed: expected {}, found ", slot(0));
    assert!(d.message.starts_with(&lost), "{what}: {}", d.message);
    assert_eq!(d.values.first(), Some(&Plain::Int(total)), "{what}: {d:?}");
}

fn dpor(budget: u32) -> Plan {
    Plan {
        budget,
        ..Plan::default()
    }
}

const LOST_UPDATE: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  clock.now();
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
        assert_eq(counter.get[n](), 2)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;

/// Shapes that could plausibly reach something the seed does not name.
const SHAPES: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  clock.now();
  counter.put[n](seen + 1)
}

test "a deep task tree" {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let outer = task.spawn(|| {
          let mid = task.spawn(|| {
            let inner = task.spawn(|| bump());
            task.join(inner);
            bump()
          });
          task.join(mid);
          bump()
        });
        let other = task.spawn(|| bump());
        task.join(outer);
        task.join(other);
        assert(counter.get[n]() >= 1)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}

test "a task that fails part way through an interleaving" {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| bump());
        let b = task.spawn(|| { bump(); assert_eq(counter.get[n](), 99); () });
        task.join(a);
        task.join(b)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}

test "two tasks waking at one deadline" {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| { clock.sleep(50); bump() });
        let b = task.spawn(|| { clock.sleep(50); bump() });
        task.join(a);
        task.join(b);
        assert(counter.get[n]() >= 1)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}

test "two tasks drawing from one stream" {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| counter.put[n](random.below(1000)));
        let b = task.spawn(|| counter.put[n](random.below(1000)));
        task.join(a);
        task.join(b);
        assert(counter.get[n]() >= 0)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;

const SHAPE_NAMES: [&str; 4] = [
    "a deep task tree",
    "a task that fails part way through an interleaving",
    "two tasks waking at one deadline",
    "two tasks drawing from one stream",
];

#[test]
fn every_shape_reproduces_itself_at_every_seed_in_a_range() {
    let audited = Audited::new(SHAPES);
    for (index, name) in SHAPE_NAMES.iter().enumerate() {
        for root in 0..48u64 {
            let seed = Seed::root(root);
            let first = audited.transcript_of(index, &seed);
            let again = audited.transcript_of(index, &seed);
            assert_eq!(again, first, "`{name}` diverged at seed {root}");
        }
    }
}

#[test]
fn an_edit_that_changes_no_hash_changes_no_interleaving() {
    let plain = Audited::new(LOST_UPDATE);
    let edited = Audited::new(
        &LOST_UPDATE
            .replace("let seen =", "// a comment nobody reads\n  let observed =")
            .replace("seen + 1", "observed + 1")
            .replace("let a = task.spawn", "let first  =  task.spawn")
            .replace("let b = task.spawn", "let second = task.spawn")
            .replace("task.join(a)", "task.join(first)")
            .replace("task.join(b)", "task.join(second)"),
    );
    assert_eq!(
        edited.compiled.front.hashes.tests, plain.compiled.front.hashes.tests,
        "the edit must change no hash, or this compares two programs"
    );
    assert_eq!(
        edited.compiled.front.hashes.defs,
        plain.compiled.front.hashes.defs
    );
    for root in 0..24u64 {
        let seed = Seed::root(root);
        assert_eq!(
            edited.transcript(&seed),
            plain.transcript(&seed),
            "seed {root}: renaming locals and adding a comment moved the interleaving"
        );
    }
}

#[test]
fn the_whole_search_is_a_function_of_its_plan() {
    let audited = Audited::new(SHAPES);
    for (index, name) in SHAPE_NAMES.iter().enumerate() {
        let first = explore(&dpor(128), &mut |seed: &Seed| {
            audited.interleaving_at(index, seed)
        });
        for _ in 0..8 {
            let again = explore(&dpor(128), &mut |seed: &Seed| {
                audited.interleaving_at(index, seed)
            });
            assert_eq!(again.seeds, first.seeds, "`{name}`: the search wandered");
            assert_eq!(
                again.exploration, first.exploration,
                "`{name}`: the search reported a different exploration"
            );
        }
    }
}

/// The tasks touch the cell themselves, so the race is reported over the cell's name rather than
/// over an effect's atom.
const CELL_RACE: &str = r#"
test "a lost update on a cell" {
  with_cell[n](0) { c ->
    simulate {
      let a = task.spawn(|| { let seen = cell_get(c); clock.now(); cell_set(c, seen + 1) });
      let b = task.spawn(|| { let seen = cell_get(c); clock.now(); cell_set(c, seen + 1) });
      task.join(a);
      task.join(b);
      assert_eq(cell_get(c), 2)
    }
  }
}
"#;

/// Each interleaving is an entry on the worker's tier, as is every search the worker ran before, so
/// a race names the cell as a fresh tier does or the report depends on what ran first.
#[test]
fn every_search_on_one_tier_names_a_races_cell_alike() {
    let audited = Audited::new(CELL_RACE);
    let search = || {
        explore(&dpor(256), &mut |seed: &Seed| {
            audited.interleaving_at(0, seed)
        })
    };

    let first = search();

    assert_lost_update(&first, 2, "the first search");
    let race = first
        .exploration
        .race
        .clone()
        .expect("the search reached the lost update by reordering two steps");
    for site in [&race.left, &race.right] {
        assert!(
            site.access.starts_with("cell.") && site.access.ends_with("[@0.0]"),
            "the test's one cell, named as a fresh tier names it: {}",
            site.access
        );
    }
    for _ in 0..2 {
        assert_eq!(
            search().exploration.race,
            Some(race.clone()),
            "a later search on the same tier reported the race differently"
        );
    }
}

/// A mixture's run, on a tier of its own, reproduces a failure only if its message matches the one
/// the worker's tier reported.
#[test]
fn every_entry_of_one_tier_names_a_failures_cell_alike() {
    let compiled = Compiled::named(
        "t",
        r#"
test "an update that reads the cell it holds" {
  with_cell[n](1) { c -> cell_update(c, |x: Int| x + cell_get(c)) }
}
"#,
    );
    let tier = compiled.unit().attach();
    for _ in 0..3 {
        let d = compiled
            .machine_on(Rc::clone(&tier))
            .eval_test(0)
            .into_parts()
            .0
            .expect_err("the update's function reads the cell it holds");
        assert_eq!(
            (d.code, d.message.as_str()),
            (
                codes::RUNTIME_ERROR,
                "`cell_get` reached cell @0.0 while a `cell_update` holds its contents"
            )
        );
    }
}

#[test]
fn the_budget_and_the_mode_do_not_change_what_the_seed_names() {
    let audited = Audited::new(SHAPES);
    for (index, name) in SHAPE_NAMES.iter().enumerate() {
        let named = audited.transcript_of(index, &Seed::root(0));
        for plan in [
            Plan {
                mode: SimMode::Once,
                budget: 1,
                ..Plan::default()
            },
            dpor(1),
            dpor(256),
            Plan::random(1),
        ] {
            let mut seen: Option<Transcript> = None;
            let explored = explore(&plan, &mut |seed: &Seed| {
                if seed == &Seed::root(0) && seen.is_none() {
                    seen = Some(audited.transcript_of(index, seed));
                }
                audited.interleaving_at(index, seed)
            });
            assert!(explored.exploration.explored >= 1);
            assert_eq!(
                seen.expect("every plan starts from root 0"),
                named,
                "`{name}`: {:?} changed what the seed's own interleaving delivered",
                plan.mode
            );
        }
    }
}

const TWO_REGIONS: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  clock.now();
  counter.put[n](seen + 1)
}

test "a race in the first region and a quiet second one" {
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
          clock.now();
          ()
        };
        assert_eq(counter.get[n](), 2)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;

const REGION_IN_A_HELPER: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  clock.now();
  counter.put[n](seen + 1)
}

fn race() -> Unit / {counter.read[n], counter.write[n], sim.read} = simulate {
  let a = task.spawn(|| bump());
  let b = task.spawn(|| bump());
  task.join(a);
  task.join(b)
}

test "the same region twice through a call" {
  with_cell[n](0) { c ->
    handle {
      {
        race();
        race();
        assert_eq(counter.get[n](), 4)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;

#[test]
fn a_second_simulate_region_does_not_hide_the_first_regions_race() {
    for (source, name, total) in [
        (TWO_REGIONS, "two regions written out", 2),
        (
            REGION_IN_A_HELPER,
            "one region reached twice through a call",
            4,
        ),
    ] {
        let audited = Audited::new(source);
        let searched = explore(&dpor(1024), &mut |seed: &Seed| {
            audited.interleaving_at(0, seed)
        });

        let sampled = explore(&Plan::random(64), &mut |seed: &Seed| {
            audited.interleaving_at(0, seed)
        });
        assert_lost_update(
            &sampled,
            total,
            &format!(
                "`{name}`: the fixture must contain a reachable lost update, or it proves nothing"
            ),
        );

        assert!(
            searched.exploration.failure.is_some(),
            "`{name}`: the search explored {} interleavings, reported exhaustive={}, and never \
             reached the lost update that a 64-seed sample finds at {}",
            searched.exploration.explored,
            searched.exploration.exhaustive,
            sampled
                .exploration
                .failure
                .as_ref()
                .expect("a sampled failure"),
        );
        assert_lost_update(&searched, total, &format!("`{name}`: the search"));
    }
}

#[test]
fn a_legal_program_is_never_reported_as_a_simulation_divergence() {
    let audited = Audited::new(
        r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
  write note[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.get[n], counter.put[n], clock.now} = {
  let seen = counter.get[n]();
  clock.now();
  counter.put[n](seen + 1)
}

fn noise() -> Unit / {counter.get[n], counter.note[m], clock.now} = {
  let seen = counter.get[n]();
  clock.now();
  counter.note[m](seen)
}

test "the second region's shape depends on what the first raced to" {
  with_cell[n](0) { c -> {
  with_cell[m](0) { d ->
    handle {
      {
        simulate {
          let a = task.spawn(|| bump());
          let b = task.spawn(|| bump());
          task.join(a);
          task.join(b)
        };
        simulate {
          if counter.get[n]() == 2 {
            let a = task.spawn(|| noise());
            let b = task.spawn(|| noise());
            task.join(a);
            task.join(b)
          } else {
            let a = task.spawn(|| noise());
            task.join(a)
          }
        };
        assert(cell_get(d) >= 0)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
      counter.note[m](v) -> cell_set(d, v),
    }
  }
  } }
}
"#,
    );
    let explored = explore(&dpor(256), &mut |seed: &Seed| {
        audited.interleaving_at(0, seed)
    });
    if let Some(diagnostic) = &explored.diagnostic {
        assert_ne!(
            diagnostic.code,
            codes::SIMULATION_DIVERGENCE,
            "a legal program was blamed on Ply's simulation: {}\nnotes: {:?}",
            diagnostic.message,
            diagnostic.notes
        );
        panic!("its assertion holds in every interleaving, and a run failed: {diagnostic:?}");
    }
    // Replay is checked on a branch, so a search that took none compared nothing.
    assert!(
        explored.exploration.explored > 1,
        "{:?}",
        explored.exploration
    );
}

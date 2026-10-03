use crate::fixture::Compiled;
use ply_eval::region::Step;
use ply_eval::{Diagnostic, Machine, Provider, Seed, codes};
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
        let a = task.spawn(|| { clock.sleep(Duration(50)); bump() });
        let b = task.spawn(|| { clock.sleep(Duration(50)); bump() });
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
            let seed = Seed::at(root, Vec::new());
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
        let seed = Seed::at(root, Vec::new());
        assert_eq!(
            edited.transcript(&seed),
            plain.transcript(&seed),
            "seed {root}: renaming locals and adding a comment moved the interleaving"
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

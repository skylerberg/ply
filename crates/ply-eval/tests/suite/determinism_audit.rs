//! An adversarial audit of the one property the milestone sells: **a simulated
//! run is a pure function of its definition set and its seed.**
//!
//! Everything else M7 ships is downstream of it. A seed that does not reproduce
//! makes the repro artifact worthless and makes simulation strictly worse than
//! no simulation, because it promises reproduction and does not deliver. So this
//! file does not test that the feature works — `simulation.rs` does that — it
//! tries to break the promise, and each test states the attack it is making.
//!
//! Two attacks are worth naming because they are the ones that are invisible
//! when they succeed:
//!
//! - **A run whose *outcome* is stable while its *interleaving* is not.** Every
//!   comparison here is over the whole recorded step sequence — the enabled set
//!   at each point, the choice taken, the access set, the vector clock — and the
//!   final world, never over the verdict alone. An assertion that happens not to
//!   notice a reordering would hide exactly the defect being hunted.
//! - **A search that is cheaper than it should be.** Pruning is sound only over
//!   a complete access set, and a step that forgets an access produces a
//!   *smaller* number rather than a failing test. `pruning_hides_no_outcome_...`
//!   below compares the pruned search against the unpruned one on the real
//!   machine, over the set of final worlds each observed, which is the only
//!   check that fails rather than flatters.
//!
//! The converse is audited too: a seed that changes nothing would mean the
//! search is theatre, so `different_seeds_really_do_explore_different_...`
//! fails if the seed stops mattering.

use ply_core::{CheckOutput, check_program};
use ply_eval::explore::{Interleaving, Step};
use ply_eval::{Dependence, Machine, Plan, Seed, SimMode, explore, explore_under};
use ply_span::SourceId;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
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

/// Everything one interleaving is allowed to be a function of, rendered.
///
/// The step sequence rather than the verdict, because a verdict agrees far more
/// often than a schedule does; the world rather than the return value, because
/// two tasks share memory and that is where a lost update lands.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Transcript {
    verdict: String,
    virtual_time: i64,
    steps: Vec<String>,
    world: Vec<String>,
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

impl Compiled {
    /// One interleaving of test `index`, at `seed`, with everything it produced.
    fn transcript_of(&self, index: usize, seed: &Seed) -> Transcript {
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
        machine.set_seed(seed.clone(), 100_000);
        let outcome = machine.eval_test(index);
        let steps = match machine.simulated() {
            Some(record) => record.steps.iter().map(render_step).collect(),
            None => Vec::new(),
        };
        let virtual_time = machine.simulated().map_or(0, |r| r.virtual_time);
        Transcript {
            verdict: match &outcome {
                Ok(()) => "passed".to_string(),
                // The code and the message, so that two different failures
                // cannot compare equal.
                Err(d) => format!("{}: {}", d.code, d.message),
            },
            virtual_time,
            steps,
            world: machine
                .cells()
                .slots()
                .map(|(slot, v)| format!("{}={}", slot.index(), v.render()))
                .collect(),
        }
    }

    fn transcript(&self, seed: &Seed) -> Transcript {
        self.transcript_of(0, seed)
    }

    /// The choice sequence a run realized, which is not its seed's path: beyond
    /// the path the `sched` stream chose.
    fn choices_of(&self, index: usize, seed: &Seed) -> Vec<u16> {
        self.at(index, seed)
            .steps
            .iter()
            .map(|s| s.choice)
            .collect()
    }

    /// The interleaving as [`explore`] consumes it, for the tests that drive a
    /// whole search rather than one run.
    fn at(&self, index: usize, seed: &Seed) -> Interleaving {
        self.run(index, seed).0
    }

    /// One call of a named function, with both of the things a search wants from
    /// it: what it interleaved, and the value it answered.
    ///
    /// The answer rather than the arena, because a region hands its cells back
    /// at its lexical close and there is nothing left to read afterwards. A
    /// program that reports its own outcome is the stronger oracle anyway: it
    /// observes what the program computed rather than what the allocator kept.
    fn answer(&self, name: &str, seed: &Seed) -> (Interleaving, Vec<String>) {
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
        machine.set_seed(seed.clone(), 100_000);
        let outcome = machine.call(name, Vec::new(), ply_span::Span::DUMMY);
        let world = match &outcome {
            Ok(value) => vec![value.render()],
            Err(_) => Vec::new(),
        };
        let outcome = outcome.map(|_| ());
        let interleaving = match machine.simulated() {
            Some(record) => record.interleaving(&outcome),
            None => match outcome {
                Ok(()) => Interleaving::passed(Vec::new()),
                Err(d) => Interleaving::failed(Vec::new(), d),
            },
        };
        (interleaving, world)
    }

    /// One run, with both of the things a search wants from it: what it
    /// interleaved, and the world it left behind.
    fn run(&self, index: usize, seed: &Seed) -> (Interleaving, Vec<String>) {
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
        machine.set_seed(seed.clone(), 100_000);
        let outcome = machine.eval_test(index);
        let world = machine
            .cells()
            .slots()
            .map(|(slot, v)| format!("{}={}", slot.index(), v.render()))
            .collect();
        let interleaving = match machine.simulated() {
            Some(record) => record.interleaving(&outcome),
            None => match outcome {
                Ok(()) => Interleaving::passed(Vec::new()),
                Err(d) => Interleaving::failed(Vec::new(), d),
            },
        };
        (interleaving, world)
    }
}

fn dpor(budget: u32) -> Plan {
    Plan {
        budget,
        ..Plan::default()
    }
}

// --------------------------------------------------------------- the fixtures

/// The lost update. The `clock.now()` between the read and the write is what
/// makes it reachable: ADR 0006 §3.3's steps end at a *scheduler-visible*
/// perform, and `counter.*` is answered by a handler outside the region, so
/// without it `bump` would run as one step and no schedule would separate the
/// two.
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

/// Every shape that could plausibly reach something the seed does not name:
/// a three-deep spawn tree, a task that fails half way through, two tasks woken
/// by one timer, and two tasks drawing from one `random` stream.
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

/// Every test in [`SHAPES`], by index, with a name for the failure message.
const SHAPE_NAMES: [&str; 4] = [
    "a deep task tree",
    "a task that fails part way through an interleaving",
    "two tasks waking at one deadline",
    "two tasks drawing from one stream",
];

// ------------------------------------------------- one seed, one run, always

/// The property everything else rests on, hammered rather than sampled: one
/// seed, two hundred runs in one process, compared over the whole recording.
///
/// Two hundred rather than two because the failure modes being hunted — a hash
/// map's iteration order, an allocation-order dependence, a stale counter — are
/// the kind that agree for a while and then do not.
#[test]
fn one_seed_is_one_interleaving_across_hundreds_of_runs_in_one_process() {
    let compiled = compile(LOST_UPDATE);
    // A bare root, and the same root with its own realized path pinned — the
    // two halves of what a seed is.
    let pinned = Seed::at(3, compiled.choices_of(0, &Seed::root(3)));
    for seed in [Seed::root(0), Seed::root(7), pinned] {
        let first = compiled.transcript(&seed);
        assert!(
            first.steps.len() > 3,
            "the fixture must have several scheduling points to disagree about"
        );
        for run in 1..200 {
            let again = compiled.transcript(&seed);
            assert_eq!(
                again, first,
                "seed {seed} produced a different run at repetition {run}"
            );
        }
    }
}

/// The same, over a wide seed range and every fixture shape, so that a
/// dependence on something the seed does not name has many chances to show.
///
/// A failing task, a timer, a draw and a three-deep spawn tree are each a place
/// where a run could pick up something the seed does not fix, and all four are
/// covered here rather than only the one the happy path exercises.
#[test]
fn every_shape_reproduces_itself_at_every_seed_in_a_range() {
    let compiled = compile(SHAPES);
    for (index, name) in SHAPE_NAMES.iter().enumerate() {
        for root in 0..48u64 {
            let seed = Seed::root(root);
            let first = compiled.transcript_of(index, &seed);
            let again = compiled.transcript_of(index, &seed);
            assert_eq!(again, first, "`{name}` diverged at seed {root}");
        }
    }
}

/// The converse defect, and an equally fatal one: a seed that changes nothing
/// means the search is theatre. Distinct *recordings* rather than distinct
/// outcomes, because a program whose assertions cannot tell two schedules apart
/// still has two schedules.
#[test]
fn different_seeds_really_do_explore_different_interleavings() {
    let compiled = compile(SHAPES);
    for (index, name) in SHAPE_NAMES.iter().enumerate() {
        let seen: BTreeSet<Vec<String>> = (0..48u64)
            .map(|root| compiled.transcript_of(index, &Seed::root(root)).steps)
            .collect();
        assert!(
            seen.len() > 2,
            "`{name}`: 48 seeds produced {} distinct interleavings, so the seed barely decides \
             anything",
            seen.len()
        );
    }
}

/// A path is the other half of a seed, and every claim the search makes rests on
/// it meaning one run.
///
/// Three properties, and the third is the one a reader is likely to assume
/// wrongly:
///
/// - a run's **whole** realized choice sequence, pinned, replays that run
///   exactly — this is what makes `--seed 7:3.0.2` a repro;
/// - a **prefix** pins exactly the points it names, choice for choice, which is
///   what makes a backtrack point a seed rather than a whole schedule;
/// - a prefix does **not** extend the run it was taken from. Pinning a point
///   serves no draw, so a run with `k` choices pinned meets the `sched` stream
///   `k` draws earlier than the run those choices came from, and the two part
///   company after the prefix. `7:0` and `7` are therefore different seeds even
///   when seed `7` chose `0` at point 0. That is consistent and reproducible —
///   which is all the search needs — but it is not what "prefix" suggests, so it
///   is pinned here rather than left to be discovered by someone truncating a
///   seed to get closer to a failure.
#[test]
fn a_path_pins_what_it_names_and_a_whole_path_replays_its_run() {
    let compiled = compile(LOST_UPDATE);
    for root in 0..24u64 {
        let free = compiled.transcript(&Seed::root(root));
        let choices = compiled.choices_of(0, &Seed::root(root));
        assert!(choices.len() > 3, "the fixture must make several choices");

        assert_eq!(
            compiled.transcript(&Seed::at(root, choices.clone())),
            free,
            "seed {root}: pinning the whole realized choice sequence replayed a different run"
        );

        for cut in 1..choices.len() {
            let prefix: Vec<u16> = choices[..cut].to_vec();
            let seed = Seed::at(root, prefix.clone());
            let taken = compiled.choices_of(0, &seed);
            assert!(
                taken.len() >= cut && taken[..cut] == prefix[..],
                "seed {seed}: the path named {prefix:?} and the run took {taken:?}"
            );
            assert_eq!(
                compiled.transcript(&seed),
                compiled.transcript(&seed),
                "seed {seed}: a prefixed run is not a function of its seed"
            );
        }
    }
}

/// A run must be a function of the *definition set*, not of the source text. An
/// edit that changes no hash — a comment, a rename of a local, whitespace —
/// must not move a single scheduling decision, or a seed printed before the edit
/// names a different run after it.
#[test]
fn an_edit_that_changes_no_hash_changes_no_interleaving() {
    let plain = compile(LOST_UPDATE);
    let edited = compile(
        &LOST_UPDATE
            .replace("let seen =", "// a comment nobody reads\n  let observed =")
            .replace("seen + 1", "observed + 1")
            .replace("let a = task.spawn", "let first  =  task.spawn")
            .replace("let b = task.spawn", "let second = task.spawn")
            .replace("task.join(a)", "task.join(first)")
            .replace("task.join(b)", "task.join(second)"),
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

// ------------------------------------------------------------- the search

/// The search is itself part of the run's determinism: which interleaving comes
/// next is as much a function of the seed as which task comes next. Two searches
/// over one plan must visit the same seeds in the same order, or neither the
/// reduction number nor the failure it reports is reproducible.
#[test]
fn the_whole_search_is_a_function_of_its_plan() {
    let compiled = compile(SHAPES);
    for (index, name) in SHAPE_NAMES.iter().enumerate() {
        let first = explore(&dpor(128), &mut |seed: &Seed| compiled.at(index, seed));
        for _ in 0..8 {
            let again = explore(&dpor(128), &mut |seed: &Seed| compiled.at(index, seed));
            assert_eq!(again.seeds, first.seeds, "`{name}`: the search wandered");
            assert_eq!(
                again.exploration, first.exploration,
                "`{name}`: the search reported a different exploration"
            );
        }
    }
}

/// A budget is a search parameter and not a semantics: the value and the world a
/// region delivers are those of the interleaving its seed names, whatever else
/// the search went on to explore.
#[test]
fn the_budget_and_the_mode_do_not_change_what_the_seed_names() {
    let compiled = compile(SHAPES);
    for (index, name) in SHAPE_NAMES.iter().enumerate() {
        let named = compiled.transcript_of(index, &Seed::root(0));
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
                    seen = Some(compiled.transcript_of(index, seed));
                }
                compiled.at(index, seed)
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

/// The shapes the pruned and the unpruned search are compared over.
///
/// Every assertion here is order-insensitive on purpose: a search that stops at
/// a failure has seen a prefix of its space, and comparing two prefixes says
/// nothing. Each test must therefore run to its frontier under *both*
/// relations, which also caps how large the unpruned space may be — a fixture
/// whose naive search spends its budget is a fixture this audit cannot use.
const PRUNING: &str = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  clock.now();
  counter.put[n](seen + 1)
}

pub fn two_tasks_contending() -> Int = {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| bump());
        let b = task.spawn(|| bump());
        task.join(a);
        task.join(b);
        assert(counter.get[n]() >= 1);
        counter.get[n]()
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}

pub fn a_racer_behind_a_barrier() -> Int = {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let late = task.spawn(|| {
          let barrier = task.spawn(|| task.yield());
          task.join(barrier);
          bump()
        });
        bump();
        task.join(late);
        assert(counter.get[n]() >= 1);
        counter.get[n]()
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}

pub fn a_nested_spawn_racing() -> Int = {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let outer = task.spawn(|| {
          let inner = task.spawn(|| bump());
          task.join(inner)
        });
        let other = task.spawn(|| bump());
        task.join(outer);
        task.join(other);
        assert(counter.get[n]() >= 1);
        counter.get[n]()
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}

pub fn two_tasks_one_timer() -> Int = {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| { clock.sleep(50); bump() });
        let b = task.spawn(|| { clock.sleep(50); bump() });
        task.join(a);
        task.join(b);
        assert(counter.get[n]() >= 1);
        counter.get[n]()
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}

pub fn two_tasks_one_stream() -> Int = {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| counter.put[n](random.below(1000)));
        let b = task.spawn(|| counter.put[n](random.below(1000)));
        task.join(a);
        task.join(b);
        assert(counter.get[n]() >= 0);
        counter.get[n]()
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;

/// The five fixtures are **functions** rather than tests, and the outcome a
/// search compares is what one answers rather than what it left in the arena.
/// A region hands its cells back at its lexical close, so a counter read after
/// the run is a read of nothing on every interleaving alike — an oracle that
/// cannot tell two outcomes apart is an audit that passes for the wrong reason.
const PRUNING_NAMES: [&str; 5] = [
    "t.two_tasks_contending",
    "t.a_racer_behind_a_barrier",
    "t.a_nested_spawn_racing",
    "t.two_tasks_one_timer",
    "t.two_tasks_one_stream",
];

/// The audit that fails rather than flatters.
///
/// Pruning is sound only over a *complete* access set, and a step that forgets
/// an access does not produce a failing test — it produces a smaller number and
/// a larger claimed reduction. So the pruned search is compared against the
/// unpruned one over the set of final worlds each observed: an outcome the
/// pruned search cannot reach is a race the milestone would report as proved
/// absent.
///
/// The comparison runs on the real machine rather than on a model scheduler,
/// because the access set is assembled by the machine — the tracer's atoms plus
/// the world's cells — and a model cannot be wrong in the way the machine can.
#[test]
fn pruning_hides_no_outcome_the_unpruned_search_reaches() {
    let compiled = compile(PRUNING);
    for name in PRUNING_NAMES.iter() {
        let plan = dpor(4096);

        let mut pruned: BTreeSet<Vec<String>> = BTreeSet::new();
        let a = explore_under(&plan, Dependence::Exact, &mut |seed: &Seed| {
            let (interleaving, world) = compiled.answer(name, seed);
            pruned.insert(world);
            interleaving
        });

        let mut whole: BTreeSet<Vec<String>> = BTreeSet::new();
        let b = explore_under(&plan, Dependence::All, &mut |seed: &Seed| {
            let (interleaving, world) = compiled.answer(name, seed);
            whole.insert(world);
            interleaving
        });

        // A search that stopped at a failure saw a prefix of its space, and
        // comparing two prefixes says nothing. These fixtures assert only what
        // survives every interleaving, so a failure here is a defect of its own.
        assert!(
            a.exploration.failure.is_none() && b.exploration.failure.is_none(),
            "`{name}`: an order-insensitive fixture failed, so the comparison below would be \
             between two prefixes"
        );
        assert!(
            a.exploration.exhaustive && b.exploration.exhaustive,
            "`{name}`: both searches must empty their frontier for the comparison to mean \
             anything (pruned {} / naive {})",
            a.exploration.explored,
            b.exploration.explored
        );
        assert_eq!(
            pruned, whole,
            "`{name}`: pruning hid an outcome the unpruned search reached — a step's access set \
             is missing something two tasks share ({} interleavings against {})",
            a.exploration.explored, b.exploration.explored
        );
        // A fixture whose outcome does not depend on the order would make the
        // comparison above vacuous, so each one is required to have two.
        assert!(
            whole.len() > 1,
            "`{name}`: every interleaving reached the same world, so this fixture proves nothing \
             about pruning"
        );
        assert!(
            a.exploration.explored < b.exploration.explored,
            "`{name}`: the pruned search ran {} interleavings and the unpruned one {}, so nothing \
             was pruned and the comparison is vacuous",
            a.exploration.explored,
            b.exploration.explored
        );
    }
}

// ------------------------------------------------------- more than one region

/// Two `simulate` regions in one test, in sequence. Nothing refuses the program:
/// nesting is `E0416` and this is not nesting, so it typechecks, runs, and is
/// reported on.
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

/// A `simulate` region reached twice through an ordinary call, which is the same
/// shape without the syntax pointing at it.
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

/// **BLOCKER.** A test with more than one `simulate` region is searched over one
/// of them and reported `exhaustive` about all of them.
///
/// `Machine::record` holds the steps of the last region that *completed*, so the
/// backtrack points the search derives describe that region only. The race in
/// every other region is never explored, the search empties a frontier that
/// covers one region's choice points, and the run is reported green, exhaustive
/// — a proof over every interleaving — and cached under its plan key.
///
/// The same program under `--sim random` fails at root 4 within sixty-four
/// samples, so this is not a race the search decided was unreachable: it is one
/// the search never looked at. An `exhaustive: true` that is wrong is the worst
/// artifact this milestone can produce, because it is the one a project is
/// invited to watch go up.
///
/// `ply_eval::region::Record`'s own documentation says "the first region's
/// record is the one kept", which is neither what happens nor sufficient: no
/// single region's record covers the test.
#[test]
fn a_second_simulate_region_does_not_hide_the_first_regions_race() {
    for (source, name) in [
        (TWO_REGIONS, "two regions written out"),
        (
            REGION_IN_A_HELPER,
            "one region reached twice through a call",
        ),
    ] {
        let compiled = compile(source);
        let searched = explore(&dpor(1024), &mut |seed: &Seed| compiled.at(0, seed));

        // The race is real: a sample finds it.
        let sampled = explore(&Plan::random(64), &mut |seed: &Seed| compiled.at(0, seed));
        assert!(
            sampled.exploration.failure.is_some(),
            "`{name}`: the fixture must contain a reachable lost update, or it proves nothing"
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
    }
}

/// **BLOCKER.** The same defect from the other side: when a later region's shape
/// depends on what an earlier one raced to, replaying a seed does not reproduce
/// the recorded schedule, and the run is reported as `E0415` — *Ply's* fault,
/// `Status::Panicked`, no bisection — on an ordinary program.
///
/// Every region takes its choices from `path[0..]` with a counter of its own, so
/// a backtrack point named against one region's trace re-aims every other
/// region's schedule as well. Here that changes how many tasks the second region
/// spawns, so the enabled set at the point the seed names is a different set,
/// and the replay self-check fires.
#[test]
fn a_legal_program_is_never_reported_as_a_simulation_divergence() {
    let compiled = compile(
        r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
  write note[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  clock.now();
  counter.put[n](seen + 1)
}

fn noise() -> Unit / {counter.read[n], counter.write[m], clock.read} = {
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
    let explored = explore(&dpor(256), &mut |seed: &Seed| compiled.at(0, seed));
    if let Some(diagnostic) = &explored.diagnostic {
        assert_ne!(
            diagnostic.code,
            ply_span::codes::SIMULATION_DIVERGENCE,
            "a legal program was blamed on Ply's simulation: {}\nnotes: {:?}",
            diagnostic.message,
            diagnostic.notes
        );
    }
}

// ------------------------------------------------------------------ hygiene

/// The rule `ply-eval::sim`, `sched`, `explore` and `region` each enforce on
/// themselves, enforced across the seam that binds them instead.
///
/// Each of those four greps its own source; none of them greps the machine, and
/// the machine is where a step's access set is assembled, where the site of a
/// race is found and where a region is entered and left. A hash map iterated on
/// that path would put the host's memory layout into a seeded run's answer just
/// as surely as one in the scheduler, and the four existing greps would all
/// still pass.
#[test]
fn the_machines_simulated_seam_reads_nothing_a_seed_does_not_name() {
    let source = include_str!("../../src/machine.rs");
    let body = source
        .split_once("mod tests")
        .map(|(body, _)| body)
        .unwrap_or(source);
    // The machine legitimately holds hash maps for its name tables, which are
    // looked up by key and never iterated. What it may not do is read a clock,
    // a thread, an address or an entropy source on the path a simulated run
    // takes.
    for banned in [
        "SystemTime",
        "Instant",
        "thread::",
        "rayon",
        "as_ptr",
        "strong_count",
        "rand::",
        "env::var",
    ] {
        assert!(
            !body.contains(banned),
            "`{banned}` appears in ply_eval::machine, which is on the path of every simulated \
             step; a seeded run must be a function of its definitions and its seed"
        );
    }
    for iterated in [".values()", ".keys()", ".iter()"] {
        for (line, text) in body.lines().enumerate() {
            assert!(
                !(text.contains(iterated) && (text.contains("fns") || text.contains("ctors"))),
                "machine.rs:{} iterates a hash-based table: {}",
                line + 1,
                text.trim()
            );
        }
    }
}

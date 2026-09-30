use ply_eval::arena::Slot;
use ply_eval::cont::SimId;
use ply_eval::host::{HostRuntime, MachineId, Pending};
use ply_eval::region::Trail;
use ply_eval::sched::*;
use ply_eval::sim::{Access, Clock, DEFAULT_STEPS, Seed, StepFootprint, TaskId};
use ply_eval::sim::{Answer, Handlers, signature};
use ply_eval::{Diagnostic, EffectAtom, Mode, Resource, Span, Symbol, TaskHandle, Value, codes};

type Sched = Scheduler<usize, Value>;
type Choice = Turn<usize, Value>;

/// No scheduler decision looks inside a continuation, so a bare id stands in for a suspended task.
fn suspended() -> usize {
    0
}

/// What a task does, in the order it does it.
#[derive(Clone)]
enum Act {
    Mark(&'static str),
    Yield,
    /// Spawn the script at this index.
    Spawn(usize),
    Join(u64),
    Sleep(i64),
    /// Serve `rand` without ending the step, so runs with and without draws share a step structure.
    Draw,
    Fail,
}

/// A whole program: script 0 is the region's body and every other script is something spawned.
type Program = Vec<Vec<Act>>;

fn solo(root: u64) -> (Sched, Clock, Trail) {
    (
        Scheduler::new(SimId(0), Span::DUMMY),
        Clock::new(),
        Trail::new(Seed::root(root)),
    )
}

/// A `Turn` has no `Debug`, so an expected refusal is unwrapped here rather than by `expect_err`.
fn refused(turn: Result<Choice, Diagnostic>, why: &str) -> Diagnostic {
    match turn {
        Ok(_) => panic!("the scheduler handed out a task: {why}"),
        Err(diagnostic) => diagnostic,
    }
}

#[derive(Debug)]
struct Run {
    /// `(task, mark)` in the order reached: the observable that tells interleavings apart.
    marks: Vec<(u64, &'static str)>,
    /// Virtual time at the end.
    clock: i64,
    choices: Vec<u16>,
    steps: Vec<(u64, Vec<u64>, u16)>,
    /// `(task, stamp)` per step, which the search reads to decide whether two steps could reorder.
    stamps: Vec<(TaskId, Stamp)>,
}

fn run(program: &Program, seed: Seed) -> Result<Run, Diagnostic> {
    run_with(program, seed, DEFAULT_STEPS)
}

/// Drives the scheduler as the machine's seeded prompt does: a perform ends a step.
fn run_with(program: &Program, seed: Seed, budget: u32) -> Result<Run, Diagnostic> {
    let root = seed.root;
    let mut trail = Trail::new(seed);
    let mut sched = Scheduler::new(SimId(0), Span::DUMMY).with_step_budget(budget);
    let mut handlers = Handlers::new(root);
    let mut marks = Vec::new();
    // Which script each task runs, and how far into it that task has got.
    let mut script: Vec<usize> = vec![0];
    let mut pc: Vec<usize> = vec![0];

    loop {
        match sched.next(handlers.clock_mut(), &mut trail)? {
            Turn::Complete(_) => {
                return Ok(Run {
                    marks,
                    clock: handlers.clock().now(),
                    choices: trail.choices().to_vec(),
                    steps: trail
                        .steps()
                        .iter()
                        .map(|s| {
                            (
                                s.task.0,
                                s.enabled.iter().map(|t| t.0).collect::<Vec<_>>(),
                                s.choice,
                            )
                        })
                        .collect(),
                    stamps: trail
                        .steps()
                        .iter()
                        .map(|s| (s.task, s.stamp.clone()))
                        .collect(),
                });
            }
            Turn::Run { task, resumption } => {
                let at = task.0 as usize;
                if let Resumption::Start { body, .. } = &resumption {
                    let index = match body {
                        Value::Int(i) => *i as usize,
                        other => panic!("a spawned body is a script index, found {other:?}"),
                    };
                    while script.len() <= at {
                        script.push(0);
                        pc.push(0);
                    }
                    script[at] = index;
                }
                // One step: act until something suspends this task.
                loop {
                    let Some(act) = program[script[at]].get(pc[at]).cloned() else {
                        sched.finish(Value::Int(task.0 as i64))?;
                        break;
                    };
                    pc[at] += 1;
                    match act {
                        Act::Mark(m) => {
                            marks.push((task.0, m));
                            continue;
                        }
                        Act::Draw => {
                            handlers.dispatch(
                                signature("random", "next").expect("declared"),
                                task,
                                &[],
                                Span::DUMMY,
                            )?;
                            continue;
                        }
                        Act::Yield => sched.suspend(suspended(), Value::Unit)?,
                        Act::Spawn(index) => {
                            let id = sched.spawn(Value::Int(index as i64), Span::DUMMY).id();
                            while script.len() <= id.0 as usize {
                                script.push(0);
                                pc.push(0);
                            }
                            sched.suspend(suspended(), Value::Int(id.0 as i64))?;
                        }
                        Act::Join(id) => sched.join(suspended(), TaskId(id), Span::DUMMY)?,
                        Act::Sleep(nanos) => {
                            let answer = handlers.dispatch(
                                signature("clock", "sleep").expect("declared"),
                                task,
                                &[Value::Int(nanos)],
                                Span::DUMMY,
                            )?;
                            match answer {
                                Answer::Value(value) => sched.suspend(suspended(), value)?,
                                Answer::Sleeping { deadline } => {
                                    sched.sleep_until(suspended(), deadline, Span::DUMMY)?
                                }
                            }
                        }
                        Act::Fail => {
                            return Err(sched.fail(
                                Diagnostic::error(codes::RUNTIME_ERROR, "the task failed")
                                    .primary(Span::DUMMY, "here"),
                                trail.seed(),
                            ));
                        }
                    }
                    break;
                }
            }
        }
    }
}

/// Two tasks that each mark twice, spawned and joined by the root.
fn two_workers() -> Program {
    vec![
        vec![
            Act::Spawn(1),
            Act::Spawn(2),
            Act::Join(1),
            Act::Join(2),
            Act::Mark("joined"),
        ],
        vec![Act::Mark("a1"), Act::Yield, Act::Mark("a2")],
        vec![Act::Mark("b1"), Act::Yield, Act::Mark("b2")],
    ]
}

#[test]
fn a_region_with_no_tasks_delivers_its_bodys_value() {
    let program = vec![vec![Act::Mark("only")]];
    let run = run(&program, Seed::root(1)).expect("no reason to block");
    assert_eq!(run.marks, vec![(0, "only")]);
    assert_eq!(run.choices, vec![0], "the body's own step is a step");
}

#[test]
fn one_seed_produces_one_interleaving_however_often_it_is_run() {
    let program = two_workers();
    let first = run(&program, Seed::root(7)).expect("completes");
    for _ in 0..64 {
        let again = run(&program, Seed::root(7)).expect("completes");
        assert_eq!(again.marks, first.marks);
        assert_eq!(again.choices, first.choices);
        assert_eq!(again.steps, first.steps);
    }
}

#[test]
fn different_seeds_produce_different_interleavings() {
    let program = two_workers();
    let mut seen: Vec<Vec<(u64, &'static str)>> = Vec::new();
    for root in 0..32 {
        let run = run(&program, Seed::root(root)).expect("completes");
        if !seen.contains(&run.marks) {
            seen.push(run.marks);
        }
    }
    assert!(
        seen.len() > 1,
        "32 seeds explored one interleaving, so the seed decides nothing"
    );
}

#[test]
fn every_interleaving_runs_every_task_in_its_own_order() {
    let program = two_workers();
    for root in 0..64 {
        let run = run(&program, Seed::root(root)).expect("completes");
        let of = |task: u64| -> Vec<&'static str> {
            run.marks
                .iter()
                .filter(|(t, _)| *t == task)
                .map(|(_, m)| *m)
                .collect()
        };
        assert_eq!(of(1), vec!["a1", "a2"], "seed {root}");
        assert_eq!(of(2), vec!["b1", "b2"], "seed {root}");
        assert_eq!(run.marks.last(), Some(&(0, "joined")), "seed {root}");
    }
}

/// The realized choice sequence, not the seed's path (which runs out), names the interleaving.
#[test]
fn the_realized_choice_sequence_replays_the_run_it_came_from() {
    let program = two_workers();
    let free = run(&program, Seed::root(11)).expect("completes");
    let pinned = run(&program, Seed::at(11, free.choices.clone())).expect("completes");
    assert_eq!(pinned.marks, free.marks);
    assert_eq!(pinned.choices, free.choices);
}

#[test]
fn a_path_prefix_pins_only_the_steps_it_names() {
    let program = two_workers();
    let free = run(&program, Seed::root(3)).expect("completes");
    let prefix: Vec<u16> = free.choices.iter().copied().take(3).collect();
    let branched = run(&program, Seed::at(3, prefix.clone())).expect("completes");
    assert_eq!(&branched.choices[..3], &prefix[..]);
}

#[test]
fn a_choice_that_does_not_index_the_enabled_set_is_a_divergence() {
    let program = two_workers();
    let err = run(&program, Seed::at(1, vec![9])).expect_err("point 0 has one enabled task");
    assert_eq!(err.code, codes::SIMULATION_DIVERGENCE);
    assert!(err.notes.iter().any(|n| n.contains("scheduling point 0")));
}

#[test]
fn a_task_may_spawn_tasks_of_its_own() {
    let program: Program = vec![
        vec![Act::Spawn(1), Act::Join(1), Act::Mark("root done")],
        vec![
            Act::Spawn(2),
            Act::Spawn(3),
            Act::Mark("spawned two"),
            Act::Join(2),
            Act::Join(3),
            Act::Mark("children done"),
        ],
        vec![Act::Mark("grandchild a")],
        vec![Act::Mark("grandchild b")],
    ];
    for root in 0..16 {
        let run = run(&program, Seed::root(root)).expect("completes");
        let marks: Vec<&'static str> = run.marks.iter().map(|(_, m)| *m).collect();
        assert!(marks.contains(&"grandchild a"), "seed {root}");
        assert!(marks.contains(&"grandchild b"), "seed {root}");
        assert_eq!(marks.last(), Some(&"root done"), "seed {root}");
        let children = marks
            .iter()
            .position(|m| *m == "children done")
            .expect("the child joined its own children");
        for grandchild in ["grandchild a", "grandchild b"] {
            let at = marks.iter().position(|m| *m == grandchild).expect("ran");
            assert!(
                at < children,
                "seed {root}: {grandchild} ran after the join"
            );
        }
    }
}

#[test]
fn a_task_nobody_joins_still_runs_to_completion() {
    let program: Program = vec![
        vec![Act::Spawn(1), Act::Mark("body done")],
        vec![Act::Yield, Act::Mark("worker done")],
    ];
    for root in 0..16 {
        let run = run(&program, Seed::root(root)).expect("completes");
        assert!(
            run.marks.contains(&(1, "worker done")),
            "seed {root} abandoned an unjoined task"
        );
    }
}

#[test]
fn a_join_orders_the_child_before_the_parent_and_siblings_against_nobody() {
    let program: Program = vec![
        vec![
            Act::Spawn(1),
            Act::Spawn(2),
            Act::Join(1),
            Act::Join(2),
            Act::Mark("after both joins"),
        ],
        vec![Act::Yield, Act::Mark("a")],
        vec![Act::Yield, Act::Mark("b")],
    ];
    for root in 0..8 {
        let run = run(&program, Seed::root(root)).expect("completes");
        let last = |task: u64| {
            run.stamps
                .iter()
                .rposition(|(t, _)| t.0 == task)
                .expect("every task takes a step")
        };
        let (parent, a, b) = (last(0), last(1), last(2));
        for child in [1u64, 2] {
            let (t, stamp) = &run.stamps[last(child)];
            assert!(
                happens_before(stamp, *t, &run.stamps[parent].1),
                "seed {root}: @{child}'s last step is not ordered before the parent's",
            );
        }
        assert!(
            !happens_before(&run.stamps[a].1, TaskId(1), &run.stamps[b].1)
                && !happens_before(&run.stamps[b].1, TaskId(2), &run.stamps[a].1),
            "seed {root}: two siblings were ordered against each other",
        );
    }
}

#[test]
fn joining_a_task_that_already_finished_does_not_block() {
    let program: Program = vec![
        vec![
            Act::Spawn(1),
            Act::Join(1),
            Act::Join(1),
            Act::Mark("joined twice"),
        ],
        vec![Act::Mark("worker")],
    ];
    for root in 0..8 {
        let run = run(&program, Seed::root(root)).expect("completes");
        assert!(run.marks.contains(&(0, "joined twice")), "seed {root}");
    }
}

#[test]
fn a_join_cycle_is_a_deadlock_naming_both_tasks() {
    let program: Program = vec![vec![Act::Spawn(1), Act::Join(1)], vec![Act::Join(0)]];
    for root in 0..8 {
        let err = run(&program, Seed::root(root)).expect_err("nothing can run");
        assert_eq!(err.code, codes::DEADLOCK);
        assert!(
            err.message.contains("2 tasks are blocked"),
            "{}",
            err.message
        );
        let waits: Vec<&str> = err.labels.iter().map(|l| l.message.as_str()).collect();
        assert!(
            waits.iter().any(|m| m.contains("@0 waits here for @1")),
            "{waits:?}"
        );
        assert!(
            waits.iter().any(|m| m.contains("@1 waits here for @0")),
            "{waits:?}"
        );
        assert!(err.notes.iter().any(|n| n.contains("replay with seed")));
    }
}

#[test]
fn a_task_that_joins_itself_deadlocks_rather_than_hanging() {
    let program: Program = vec![vec![Act::Join(0)]];
    let err = run(&program, Seed::root(0)).expect_err("nothing can run");
    assert_eq!(err.code, codes::DEADLOCK);
    assert!(err.message.contains("1 task is blocked"), "{}", err.message);
    assert!(
        err.labels
            .iter()
            .any(|l| l.message.contains("@0 waits here for @0"))
    );
}

/// A livelock shares the deadlock's code, with a different message.
#[test]
fn a_region_that_never_stops_spends_its_step_budget() {
    let mut forever = vec![Act::Yield; 64];
    forever.push(Act::Mark("unreachable"));
    let program: Program = vec![forever];
    let err = run_with(&program, Seed::root(0), 16).expect_err("the budget is spent");
    assert_eq!(err.code, codes::DEADLOCK);
    assert!(
        err.message.contains("16 scheduling steps"),
        "{}",
        err.message
    );
    assert!(err.notes.iter().any(|n| n.contains("step budget")));
}

#[test]
fn a_task_failing_stops_the_region_and_names_the_task() {
    let program: Program = vec![
        vec![Act::Spawn(1), Act::Spawn(2), Act::Join(1), Act::Join(2)],
        vec![Act::Mark("a"), Act::Fail],
        vec![Act::Mark("b"), Act::Yield, Act::Mark("b2")],
    ];
    let err = run(&program, Seed::root(4)).expect_err("a task failed");
    assert_eq!(err.code, codes::RUNTIME_ERROR);
    assert!(
        err.notes.iter().any(|n| n.contains("@1")),
        "the failure does not name the task: {:?}",
        err.notes
    );
    assert!(err.notes.iter().any(|n| n.contains("replay with seed 4")));
}

#[test]
fn a_failed_region_answers_with_its_failure_forever() {
    let (mut sched, mut clock, mut trail) = solo(0);
    let Turn::Run { .. } = sched
        .next(&mut clock, &mut trail)
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    let failure = sched.fail(
        Diagnostic::error(codes::RUNTIME_ERROR, "boom"),
        &Seed::root(0),
    );
    assert_eq!(failure.code, codes::RUNTIME_ERROR);
    for _ in 0..4 {
        let err = refused(sched.next(&mut clock, &mut trail), "the region is over");
        assert_eq!(err.code, codes::RUNTIME_ERROR);
    }
}

#[test]
fn virtual_time_does_not_advance_while_any_task_can_run() {
    let program: Program = vec![
        vec![Act::Spawn(1), Act::Spawn(2), Act::Join(1), Act::Join(2)],
        vec![Act::Sleep(100), Act::Mark("woke")],
        vec![
            Act::Mark("t1"),
            Act::Yield,
            Act::Mark("t2"),
            Act::Yield,
            Act::Mark("t3"),
        ],
    ];
    for root in 0..16 {
        let run = run(&program, Seed::root(root)).expect("completes");
        let woke = run
            .marks
            .iter()
            .position(|m| *m == (1, "woke"))
            .expect("the sleeper woke");
        let last = run
            .marks
            .iter()
            .position(|m| *m == (2, "t3"))
            .expect("the runnable task finished");
        assert!(
            last < woke,
            "seed {root}: a timer fired while work could still run"
        );
        assert_eq!(run.clock, 100, "seed {root}");
    }
}

#[test]
fn tasks_sleeping_to_one_deadline_wake_together_and_their_order_is_explored() {
    let program: Program = vec![
        vec![Act::Spawn(1), Act::Spawn(2), Act::Join(1), Act::Join(2)],
        vec![Act::Sleep(50), Act::Mark("a")],
        vec![Act::Sleep(50), Act::Mark("b")],
    ];
    let mut orders: Vec<Vec<u64>> = Vec::new();
    for root in 0..32 {
        let run = run(&program, Seed::root(root)).expect("completes");
        assert_eq!(run.clock, 50, "seed {root}");
        let order: Vec<u64> = run
            .marks
            .iter()
            .filter(|(t, _)| *t != 0)
            .map(|(t, _)| *t)
            .collect();
        if !orders.contains(&order) {
            orders.push(order);
        }
    }
    assert_eq!(
        orders.len(),
        2,
        "the wake order was never explored: {orders:?}"
    );
}

#[test]
fn a_sleep_of_no_duration_is_a_yield_and_moves_no_clock() {
    let program: Program = vec![vec![Act::Sleep(0), Act::Sleep(-5), Act::Mark("through")]];
    let run = run(&program, Seed::root(0)).expect("completes");
    assert_eq!(run.clock, 0);
    assert_eq!(run.marks, vec![(0, "through")]);
}

#[test]
fn consecutive_sleeps_accumulate_virtual_time() {
    let program: Program = vec![vec![Act::Sleep(30), Act::Sleep(12), Act::Mark("done")]];
    let run = run(&program, Seed::root(0)).expect("completes");
    assert_eq!(run.clock, 42);
}

#[test]
fn joining_a_task_this_region_never_created_is_a_scope_error() {
    let (mut sched, mut clock, mut trail) = solo(0);
    let Turn::Run { .. } = sched
        .next(&mut clock, &mut trail)
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    let err = sched
        .join(suspended(), TaskId(7), Span::DUMMY)
        .expect_err("no such task");
    assert_eq!(err.code, codes::TASK_ESCAPES_SCOPE);
    assert!(err.message.contains("@7"));
}

#[test]
fn every_step_records_the_set_its_choice_indexed() {
    let program = two_workers();
    let run = run(&program, Seed::root(13)).expect("completes");
    assert_eq!(run.choices.len(), run.steps.len());
    for (i, (task, enabled, choice)) in run.steps.iter().enumerate() {
        let mut ascending = enabled.clone();
        ascending.sort_unstable();
        assert_eq!(&ascending, enabled, "step {i} reported an unordered set");
        assert_eq!(
            enabled.get(*choice as usize),
            Some(task),
            "step {i}'s choice does not index its enabled set"
        );
        assert_eq!(run.choices[i], *choice);
    }
}

fn atom(effect: &str, resource: Option<&str>, mode: Mode) -> Access {
    Access::Atom(EffectAtom::new(
        effect,
        resource
            .map(|r| Resource::Named(Symbol::new(r)))
            .unwrap_or(Resource::Singleton),
        mode,
    ))
}

#[test]
fn the_schedulers_own_bookkeeping_is_not_an_access_but_a_draw_is() {
    let (mut sched, mut clock, mut trail) = solo(0);
    let Turn::Run { .. } = sched
        .next(&mut clock, &mut trail)
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    trail.record_access(atom("task", None, Mode::Write));
    trail.record_access(atom("clock", None, Mode::Read));
    trail.record_access(atom("clock", None, Mode::Write));
    assert_eq!(trail.steps()[0].accesses.len(), 0);

    trail.record_access(atom("random", None, Mode::Write));
    trail.record_access(atom("db", Some("orders"), Mode::Write));
    trail.record_access(Access::Cell {
        id: Slot::new(3, 0),
        mode: Mode::Write,
    });
    assert_eq!(trail.steps()[0].accesses.len(), 3);
}

#[test]
fn two_steps_touching_one_cell_are_dependent() {
    let (mut sched, mut clock, mut trail) = solo(0);
    let Turn::Run { .. } = sched
        .next(&mut clock, &mut trail)
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    trail.record_access(Access::Cell {
        id: Slot::new(1, 0),
        mode: Mode::Write,
    });
    sched.suspend(suspended(), Value::Unit).expect("running");
    let Turn::Run { .. } = sched.next(&mut clock, &mut trail).expect("still enabled") else {
        panic!("expected a second step");
    };
    trail.record_access(Access::Cell {
        id: Slot::new(1, 0),
        mode: Mode::Read,
    });
    let steps = trail.steps();
    assert!(steps[0].accesses.conflicts_with(&steps[1].accesses));
    assert!(!steps[0].accesses.conflicts_with(&StepFootprint::new()));
}

#[test]
fn drawing_random_numbers_does_not_disturb_the_schedule() {
    let plain = two_workers();
    let drawing: Program = plain
        .iter()
        .map(|script| {
            let mut with_draws = Vec::new();
            for act in script {
                with_draws.push(Act::Draw);
                with_draws.push(act.clone());
            }
            with_draws
        })
        .collect();
    for root in 0..16 {
        let a = run(&plain, Seed::root(root)).expect("completes");
        let b = run(&drawing, Seed::root(root)).expect("completes");
        assert_eq!(a.choices, b.choices, "seed {root}");
        assert_eq!(a.marks, b.marks, "seed {root}");
    }
}

#[test]
fn the_scheduler_refuses_to_hand_out_a_second_task_while_one_is_running() {
    let (mut sched, mut clock, mut trail) = solo(0);
    let Turn::Run { .. } = sched
        .next(&mut clock, &mut trail)
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    let err = refused(
        sched.next(&mut clock, &mut trail),
        "a task is still running",
    );
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert!(err.message.contains("@0"));
}

#[test]
fn suspending_with_nothing_running_is_refused_rather_than_silently_applied() {
    let (mut sched, _clock, _trail) = solo(0);
    let err = sched
        .suspend(suspended(), Value::Unit)
        .expect_err("no task is running");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
}

#[test]
fn this_module_names_nothing_a_seeded_run_may_not_depend_on() {
    let body = include_str!("../../../../ply-eval/src/sched.rs");
    for banned in [
        "HashMap",
        "HashSet",
        "FxHashMap",
        "FxHashSet",
        "SystemTime",
        "Instant",
        "thread::",
        "rayon",
        "as_ptr",
        "strong_count",
        "rand::",
    ] {
        assert!(
            !body.contains(banned),
            "`{banned}` appears in ply_eval::sched; a scheduling decision must be a \
             function of the seed and nothing else"
        );
    }
}

use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, Linearity,
};
use std::sync::Arc;

/// Enough of a program to bind against; these tests need only that it is bound.
fn binding() -> HostBinding {
    let source = "nondet effect db { read get[r](k: Int) -> Int }\n\
                  fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)";
    let check = crate::fixture::port_check(&[("", source)]);
    let mut registry = HostRegistry::new();
    registry.register(
        HostOp {
            effect: Symbol::new("db"),
            op: Symbol::new("get"),
            resource: HostResource::Only(Resource::Named(Symbol::new("users"))),
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::AtMostOnce,
            blocking: false,
            secrets: false,
            path: "test::handler",
        },
        Arc::new(Never),
    );
    registry.bind(&check).expect("the fixture binds")
}

struct Never;

impl HostHandler for Never {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        Err(
            Diagnostic::error(codes::INTERNAL_ERROR, "the test handler was called")
                .primary(req.span, "here"),
        )
    }
}

/// Owns no token, so it fails loudly if a task ever waits.
struct Idle;

impl HostRuntime for Idle {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the idle runtime was asked to watch `{pending}`"),
        ))
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "the idle runtime was asked to wait",
        ))
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "the idle runtime was asked to wait",
        ))
    }
}

fn production() -> Sched {
    let binding = binding();
    let permit = HostPolicy::of(&binding).expect("a bound binding mints a permit");
    Scheduler::production(SimId(0), Span::DUMMY, permit, MachineId::next())
}

#[test]
fn a_simulated_region_is_seeded_by_construction() {
    let (sched, _clock, _trail) = solo(0);
    assert_eq!(sched.policy(), Policy::Seeded);
    assert!(sched.records_steps());
}

#[test]
fn a_hermetic_binding_mints_no_permit() {
    assert!(HostPolicy::of(&HostBinding::hermetic()).is_none());
    assert!(HostPolicy::of(&HostBinding::default()).is_none());
    assert!(HostPolicy::of(&binding()).is_some());
}

#[test]
fn each_entry_point_refuses_the_other_policys_region() {
    let (mut seeded, mut clock, mut trail) = solo(0);
    let err = refused(
        seeded.next_host(&Idle),
        "a seeded region has no host runtime to schedule against",
    );
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert!(err.message.contains("seeded region"), "{}", err.message);

    let mut host = production();
    let err = refused(
        host.next(&mut clock, &mut trail),
        "a production region has no seed",
    );
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert!(err.message.contains("host region"), "{}", err.message);
}

/// A seeded token would be polled by nothing, and the boundary refuses the operation that would
/// hand one over (`E0425`) long before this: reaching here is Ply's defect, not the program's.
#[test]
fn a_seeded_region_refuses_to_park_a_task_on_a_host_token() {
    let (mut sched, mut clock, mut trail) = solo(0);
    let Turn::Run { .. } = sched
        .next(&mut clock, &mut trail)
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    let err = sched
        .park_on_host(
            suspended(),
            Pending {
                token: 1,
                label: "read",
            },
            Span::DUMMY,
            &Idle,
        )
        .expect_err("a simulated region may not wait on the host");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert!(err.message.contains("seeded region"), "{}", err.message);
    assert!(sched.current().is_some(), "the task was parked anyway");
}

/// There is no virtual clock to reach the deadline, so parking on one is a hang.
#[test]
fn a_production_region_refuses_a_virtual_sleep() {
    let mut sched = production();
    let Turn::Run { .. } = sched.next_host(&Idle).expect("the root is enabled") else {
        panic!("expected the root's step");
    };
    let err = sched
        .sleep_until(suspended(), 100, Span::DUMMY)
        .expect_err("no virtual clock exists here");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
}

#[test]
fn the_production_scheduler_starves_nobody() {
    let mut sched = production();
    let Turn::Run { task, .. } = sched.next_host(&Idle).expect("the root is enabled") else {
        panic!("expected the root's step");
    };
    assert_eq!(task, ROOT);
    sched.spawn(Value::Unit, Span::DUMMY);
    sched.spawn(Value::Unit, Span::DUMMY);
    sched.suspend(suspended(), Value::Unit).expect("running");

    let mut order = Vec::new();
    for _ in 0..9 {
        let Turn::Run { task, .. } = sched.next_host(&Idle).expect("three are ready") else {
            panic!("expected a step");
        };
        order.push(task.0);
        sched.suspend(suspended(), Value::Unit).expect("running");
    }
    assert_eq!(order, vec![1, 2, 0, 1, 2, 0, 1, 2, 0]);
}

/// The `task.*` that opened the region is still unanswered, so the root starts running.
#[test]
fn a_lazily_opened_region_roots_on_the_control_that_opened_it() {
    let mut sched = production().rooted_running().expect("nothing has run yet");
    assert_eq!(sched.current(), Some(ROOT));

    // Answered through the same path every later perform takes, so `spawn` means one thing.
    let child = sched.spawn(Value::Unit, Span::DUMMY);
    let id = child.id();
    sched
        .suspend(suspended(), Value::Task(child))
        .expect("the root is running");

    let Turn::Run { task, resumption } = sched.next_host(&Idle).expect("the root is enabled")
    else {
        panic!("expected a step");
    };
    assert_eq!(task, ROOT);
    assert!(
        matches!(&resumption, Resumption::Resume { value: Value::Task(handle), .. } if handle.id() == id),
        "a lazily-opened root resumes with the answer, never evaluates a body it does not have"
    );

    let err = sched
        .rooted_running()
        .err()
        .expect("the region has already begun");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
}

#[test]
fn a_production_region_records_nothing_in_the_trail() {
    let trail = Trail::new(Seed::root(9));
    let mut sched = production();
    assert!(!sched.records_steps());
    for _ in 0..4 {
        let Turn::Run { .. } = sched.next_host(&Idle).expect("the root is enabled") else {
            panic!("expected a step");
        };
        sched.suspend(suspended(), Value::Unit).expect("running");
    }
    assert!(trail.steps().is_empty());
    assert!(trail.choices().is_empty());
    assert!(!trail.entered());
    assert_eq!(trail.point(), 0);
}
/// A runtime that is stopping and has nothing outstanding.
struct Stopping {
    parks: std::cell::Cell<u32>,
    expire_after: u32,
}

impl HostRuntime for Stopping {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the stopping runtime was asked to watch `{pending}`"),
        ))
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        self.parks.set(self.parks.get() + 1);
        Ok(())
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Err(Diagnostic::error(codes::INTERNAL_ERROR, "not reached"))
    }

    fn stopping(&self) -> bool {
        true
    }

    fn drain_expired(&self) -> Option<Diagnostic> {
        (self.parks.get() >= self.expire_after)
            .then(|| Diagnostic::warning(codes::DRAIN_INCOMPLETE, "the drain deadline expired"))
    }
}

/// Two tasks each waiting on the other, with no host wait and no virtual clock.
fn deadlock(sched: &mut Sched) {
    let other = sched.spawn(Value::Unit, Span::DUMMY).id();
    sched
        .join(suspended(), other, Span::DUMMY)
        .expect("the root is running");
    let Turn::Run { task, .. } = sched.next_host(&Idle).expect("the spawned task is enabled")
    else {
        panic!("expected the spawned task's first step");
    };
    assert_eq!(task, other);
    sched
        .join(suspended(), ROOT, Span::DUMMY)
        .expect("the spawned task is running");
}

#[test]
fn a_stopping_region_with_nothing_outstanding_drains_rather_than_deadlocking() {
    let mut sched = production();
    let Turn::Run { .. } = sched
        .next_host(&Stopping {
            parks: std::cell::Cell::new(0),
            expire_after: 3,
        })
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    // Nothing enabled and nothing waiting on the host: `err_host_deadlock`'s exact condition.
    deadlock(&mut sched);

    let runtime = Stopping {
        parks: std::cell::Cell::new(0),
        expire_after: 3,
    };
    let err = refused(
        sched.next_host(&runtime),
        "the drain deadline ends the region",
    );
    assert_eq!(
        err.code,
        codes::DRAIN_INCOMPLETE,
        "a stopping region ends on its deadline, not on `E0414`: {}",
        err.message
    );
    assert!(
        runtime.parks.get() >= 3,
        "the scheduler parked {} times before the deadline, so it never waited",
        runtime.parks.get()
    );
}

#[test]
fn a_park_that_woke_on_a_stop_is_not_counted_as_fruitless() {
    let mut sched = production();
    let Turn::Run { .. } = sched
        .next_host(&Stopping {
            parks: std::cell::Cell::new(0),
            expire_after: u32::MAX,
        })
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    deadlock(&mut sched);

    let runtime = Stopping {
        parks: std::cell::Cell::new(0),
        expire_after: FRUITLESS_PARKS + 8,
    };
    let err = refused(sched.next_host(&runtime), "the drain deadline ends it");
    assert_eq!(
        err.code,
        codes::DRAIN_INCOMPLETE,
        "{} parks past the fruitless bound reported `{}` instead",
        runtime.parks.get(),
        err.code
    );
    assert!(runtime.parks.get() > FRUITLESS_PARKS);
}

/// So the stopping exemption is not a hole.
#[test]
fn a_region_that_is_not_stopping_still_deadlocks() {
    let mut sched = production();
    let Turn::Run { .. } = sched.next_host(&Idle).expect("the root is enabled") else {
        panic!("expected the root's step");
    };
    deadlock(&mut sched);
    let err = refused(sched.next_host(&Idle), "nothing can make progress");
    assert_eq!(err.code, codes::DEADLOCK);
}

/// Finishes each task the region hands out until it hands out the root, and answers how the root
/// resumes.
fn until_root(sched: &mut Sched, rt: &dyn HostRuntime) -> Resumption<usize, Value> {
    loop {
        let Turn::Run { task, resumption } = sched.next_host(rt).expect("a task is enabled") else {
            panic!("the region ended while its root was still running");
        };
        if task == ROOT {
            return resumption;
        }
        sched
            .finish(Value::Int(task.0 as i64))
            .expect("the task is running");
    }
}

fn root_step(sched: &mut Sched, rt: &dyn HostRuntime) {
    let Turn::Run { task, .. } = sched.next_host(rt).expect("the root is enabled") else {
        panic!("expected the root's step");
    };
    assert_eq!(task, ROOT);
}

#[test]
fn a_production_region_keeps_only_the_tasks_that_can_still_run_or_be_joined() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    for round in 0..200 {
        drop(sched.spawn(Value::Unit, Span::DUMMY));
        let joined = sched.spawn(Value::Unit, Span::DUMMY);
        let target = joined.id();
        sched
            .suspend(suspended(), Value::Unit)
            .expect("the root is running");
        until_root(&mut sched, &Idle);
        sched
            .join(suspended(), target, Span::DUMMY)
            .expect("a task whose handle is held is kept");
        drop(joined);
        let Resumption::Resume { value, .. } = until_root(&mut sched, &Idle) else {
            panic!("the root resumes from its join");
        };
        assert_eq!(value, Value::Int(target.0 as i64));
        assert_eq!(
            sched.tasks(),
            1,
            "round {round} left a finished task behind"
        );
    }
}

/// Host state is keyed by task, so a reused id would hand a new task a retired one's state.
#[test]
fn a_retired_tasks_id_is_never_handed_out_again() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..100 {
        let handle = sched.spawn(Value::Unit, Span::DUMMY);
        assert!(
            seen.insert(handle.id()),
            "{} was handed out twice",
            handle.id()
        );
        drop(handle);
        sched
            .suspend(suspended(), Value::Unit)
            .expect("the root is running");
        until_root(&mut sched, &Idle);
        assert_eq!(sched.tasks(), 1, "the task retired before the next spawn");
    }
}

#[test]
fn a_kept_handle_keeps_its_finished_task_joinable_and_every_join_answers() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let kept: Vec<TaskHandle> = (0..8)
        .map(|_| sched.spawn(Value::Unit, Span::DUMMY))
        .collect();
    for _ in 0..4 {
        sched
            .suspend(suspended(), Value::Unit)
            .expect("the root is running");
        until_root(&mut sched, &Idle);
    }
    assert_eq!(
        sched.tasks(),
        9,
        "a finished task whose handle is held stays"
    );
    for handle in &kept {
        for join in 0..2 {
            sched
                .join(suspended(), handle.id(), Span::DUMMY)
                .expect("the task is kept");
            let Resumption::Resume { value, .. } = until_root(&mut sched, &Idle) else {
                panic!("the root resumes from its join");
            };
            assert_eq!(
                value,
                Value::Int(handle.id().0 as i64),
                "join {join} of {}",
                handle.id()
            );
        }
    }
    drop(kept);
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the root is running");
    until_root(&mut sched, &Idle);
    assert_eq!(
        sched.tasks(),
        1,
        "the last handles going retired their tasks"
    );
}

/// Records each task the region retired, and the machine it ran on.
#[derive(Default)]
struct Ends {
    ended: std::cell::RefCell<Vec<(MachineId, TaskId)>>,
}

impl HostRuntime for Ends {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("nothing here parks, yet `{pending}` was watched"),
        ))
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "nothing here waits",
        ))
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "nothing here waits",
        ))
    }

    fn end_task(&self, machine: MachineId, task: TaskId) {
        self.ended.borrow_mut().push((machine, task));
    }
}

#[test]
fn retiring_a_task_ends_its_host_state_once_and_a_held_task_is_not_retired() {
    let machine = MachineId::next();
    let permit = HostPolicy::of(&binding()).expect("a bound binding mints a permit");
    let mut sched: Sched = Scheduler::production(SimId(0), Span::DUMMY, permit, machine);
    let rt = Ends::default();
    root_step(&mut sched, &rt);
    let forgotten = sched.spawn(Value::Unit, Span::DUMMY).id();
    let kept = sched.spawn(Value::Unit, Span::DUMMY);
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the root is running");
    until_root(&mut sched, &rt);
    assert_eq!(*rt.ended.borrow(), vec![(machine, forgotten)]);

    let held = kept.id();
    drop(kept);
    for _ in 0..2 {
        sched
            .suspend(suspended(), Value::Unit)
            .expect("the root is running");
        until_root(&mut sched, &rt);
    }
    assert_eq!(
        *rt.ended.borrow(),
        vec![(machine, forgotten), (machine, held)]
    );
}

#[test]
fn a_region_hands_back_the_bodies_of_the_tasks_it_never_started() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let started = sched.spawn(Value::Int(1), Span::DUMMY);
    let _never = sched.spawn(Value::Int(2), Span::DUMMY);
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the root is running");
    let Turn::Run {
        task,
        resumption: Resumption::Start { .. },
    } = sched.next_host(&Idle).expect("a spawned task is enabled")
    else {
        panic!("the first spawned task starts");
    };
    assert_eq!(task, started.id());
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the started task is running");
    assert_eq!(sched.unstarted(), vec![Value::Int(2)]);
    assert!(sched.unstarted().is_empty(), "a body is handed back once");
}

/// Resolves every watched token at the next look, newest first, beside a token nothing here
/// parked on, as one a region of this machine that already ended left behind.
#[derive(Default)]
struct Backwards {
    watched: std::cell::RefCell<Vec<u64>>,
}

const STALE: u64 = 999;

impl HostRuntime for Backwards {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        self.watched.borrow_mut().push(pending.token);
        Ok(())
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        let mut resolved: Vec<(u64, Result<Value, Diagnostic>)> = self
            .watched
            .take()
            .into_iter()
            .rev()
            .map(|token| (token, Ok(Value::Int(token as i64 * 10))))
            .collect();
        resolved.push((STALE, Ok(Value::Int(-1))));
        resolved
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Err(Diagnostic::error(codes::INTERNAL_ERROR, "a task parks"))
    }
}

#[test]
fn each_parked_task_wakes_with_what_its_own_token_resolved_to() {
    let rt = Backwards::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let children: Vec<TaskHandle> = (0..3)
        .map(|_| sched.spawn(Value::Unit, Span::DUMMY))
        .collect();
    let parked = |sched: &mut Sched, token: u64| {
        sched
            .park_on_host(
                suspended(),
                Pending {
                    token,
                    label: "test",
                },
                Span::DUMMY,
                &rt,
            )
            .expect("the task is running")
    };
    parked(&mut sched, 100);
    for _ in &children {
        let Turn::Run { task, .. } = sched.next_host(&rt).expect("a child starts") else {
            panic!("expected a child's first step");
        };
        parked(&mut sched, 200 + task.0);
    }

    let mut woke = std::collections::BTreeMap::new();
    loop {
        match sched.next_host(&rt).expect("every task wakes") {
            Turn::Complete(_) => break,
            Turn::Run {
                task,
                resumption: Resumption::Resume { value, .. },
            } => {
                woke.insert(task, value);
                sched.finish(Value::Unit).expect("the task is running");
            }
            Turn::Run { task, .. } => panic!("{task} ran without its token resolving"),
        }
    }
    let mut expected = std::collections::BTreeMap::from([(ROOT, Value::Int(1000))]);
    for child in &children {
        let id = child.id();
        expected.insert(id, Value::Int((200 + id.0 as i64) * 10));
    }
    assert_eq!(woke, expected);
}

#[test]
fn a_task_cannot_park_on_a_token_its_runtime_did_not_mint() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let err = sched
        .park_on_host(
            suspended(),
            Pending {
                token: 7,
                label: "stray",
            },
            Span::DUMMY,
            &Idle,
        )
        .expect_err("the idle runtime mints nothing");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert_eq!(
        sched.current(),
        Some(ROOT),
        "the refused task was parked anyway"
    );
}

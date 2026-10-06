use ply_eval::arena::Slot;
use ply_eval::host::{HostRuntime, MachineId, Pending};
use ply_eval::region::{StepSite, Trail};
use ply_eval::sched::*;
use ply_eval::sim::{Access, ChanId, Clock, DEFAULT_STEPS, Seed, StepFootprint, TaskId};
use ply_eval::sim::{Answer, Handlers, channel_access, ended, liveness, signature};
use ply_eval::{
    Diagnostic, EffectAtom, Mode, Resource, SimId, SourceId, Span, Symbol, TaskHandle, Value, codes,
};

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
    /// Make a channel of this capacity; the region numbers channels from 0.
    Channel(i64),
    Send(u64, i64),
    Recv(u64),
    Close(u64),
    Cancel(u64),
    Await(u64),
    /// Each arm a channel beside what it sends, and whether to wait when none can go.
    Select(Vec<(u64, Option<i64>)>, bool),
    /// Enter a bracket's `acquire` or `release`, without ending the step.
    Enter(Shield),
    /// Leave one; a cancel that was held back lands here.
    Leave(Shield),
}

/// A whole program: script 0 is the region's body and every other script is something spawned.
type Program = Vec<Vec<Act>>;

fn solo(root: u64) -> (Sched, Clock, Trail) {
    (
        Scheduler::new(SimId(0), Span::DUMMY),
        Clock::new(),
        Trail::new(Seed::at(root, Vec::new())),
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
    /// `(task, answer)` for each send, receive, select, cancel and await, in the order the tasks
    /// heard them.
    heard: Vec<(u64, Value)>,
    /// `(task, accesses)` per step, each access as it prints.
    touched: Vec<(u64, Vec<String>)>,
}

fn chan(n: u64) -> ChanHandle {
    ChanHandle {
        region: SimId(0),
        id: ChanId(n),
    }
}

fn run(program: &Program, seed: Seed) -> Result<Run, Diagnostic> {
    run_with(program, seed, DEFAULT_STEPS)
}

/// These scripts have no source, so an access is placed nowhere.
fn nowhere() -> StepSite {
    StepSite {
        definition: None,
        span: Span::DUMMY,
    }
}

/// What a region records of a cancel beside the liveness it writes.
fn record_let_go(trail: &mut Trail, let_go: LetGo) {
    match let_go {
        LetGo::Nothing => {}
        LetGo::Task(on) => trail.record_access(ended(on), nowhere()),
        LetGo::Chans(chans) => {
            for chan in chans {
                trail.record_access(channel_access(chan), nowhere());
            }
        }
    }
}

/// Drives the scheduler as the machine's seeded prompt does: a perform ends a step.
fn run_with(program: &Program, seed: Seed, budget: u32) -> Result<Run, Diagnostic> {
    let root = seed.root;
    let mut trail = Trail::new(seed);
    let mut sched = Scheduler::new(SimId(0), Span::DUMMY).with_step_budget(budget);
    let mut handlers = Handlers::new(root);
    let mut marks = Vec::new();
    let mut heard = Vec::new();
    // Which script each task runs, how far into it that task has got, and whether it waits on an
    // answer from a channel.
    let mut script: Vec<usize> = vec![0];
    let mut pc: Vec<usize> = vec![0];
    let mut asked: Vec<bool> = vec![false];

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
                    heard,
                    touched: trail
                        .steps()
                        .iter()
                        .map(|s| {
                            (
                                s.task.0,
                                s.accesses.accesses().map(|a| a.to_string()).collect(),
                            )
                        })
                        .collect(),
                });
            }
            Turn::Run { task, resumption } => {
                let at = task.0 as usize;
                // A cancelled task only unwinds: nothing of its script runs again.
                if let Resumption::Cancel { .. } = &resumption {
                    marks.push((task.0, "stopped"));
                    trail.record_access(ended(task), nowhere());
                    sched.finish_cancelled()?;
                    continue;
                }
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
                while asked.len() <= at {
                    asked.push(false);
                }
                if let Resumption::Resume { value, .. } = &resumption
                    && asked[at]
                {
                    heard.push((task.0, value.clone()));
                    asked[at] = false;
                }
                // One step: act until something suspends this task.
                loop {
                    let Some(act) = program[script[at]].get(pc[at]).cloned() else {
                        trail.record_access(ended(task), nowhere());
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
                        Act::Join(id) => sched.join(
                            suspended(),
                            &TaskHandle::unowned(SimId(0), TaskId(id)),
                            Span::DUMMY,
                        )?,
                        Act::Sleep(nanos) => {
                            let answer = handlers.dispatch(
                                signature("clock", "sleep").expect("declared"),
                                task,
                                &[Value::ctor("Duration", vec![Value::Int(nanos)])],
                                Span::DUMMY,
                            )?;
                            match answer {
                                Answer::Value(value) => sched.suspend(suspended(), value)?,
                                Answer::Sleeping { deadline } => {
                                    sched.sleep_until(suspended(), deadline, Span::DUMMY)?
                                }
                            }
                        }
                        Act::Channel(capacity) => {
                            sched.channel(suspended(), capacity_of(capacity, Span::DUMMY)?)?
                        }
                        Act::Send(n, x) => {
                            asked[at] = true;
                            sched.send(suspended(), &chan(n), Value::Int(x), Span::DUMMY)?
                        }
                        Act::Recv(n) => {
                            asked[at] = true;
                            sched.recv(suspended(), &chan(n), Span::DUMMY)?
                        }
                        Act::Close(n) => sched.close(suspended(), &chan(n), Span::DUMMY)?,
                        Act::Cancel(id) => {
                            asked[at] = true;
                            let stopped = sched.cancel(
                                suspended(),
                                &TaskHandle::unowned(SimId(0), TaskId(id)),
                                Span::DUMMY,
                                Some(handlers.clock_mut()),
                            )?;
                            if stopped.ended {
                                trail.record_access(ended(TaskId(id)), nowhere());
                            } else {
                                trail.record_access(liveness(TaskId(id), Mode::Write), nowhere());
                                trail.mark_last_step_of(
                                    SimId(0),
                                    TaskId(id),
                                    liveness(TaskId(id), Mode::Read),
                                );
                                record_let_go(&mut trail, stopped.let_go);
                            }
                        }
                        Act::Await(id) => {
                            asked[at] = true;
                            sched.await_task(
                                suspended(),
                                &TaskHandle::unowned(SimId(0), TaskId(id)),
                                Span::DUMMY,
                            )?
                        }
                        Act::Select(arms, wait) => {
                            asked[at] = true;
                            let arms = arms
                                .into_iter()
                                .map(|(n, send)| (chan(n), send.map(Value::Int)))
                                .collect();
                            sched.select(suspended(), arms, wait, Span::DUMMY)?
                        }
                        Act::Enter(shield) => {
                            sched.shield(shield).expect("a task is running");
                            continue;
                        }
                        Act::Leave(shield) => {
                            if sched.unshield(task, shield) {
                                marks.push((task.0, "landed"));
                                trail.record_access(ended(task), nowhere());
                                sched.finish_cancelled()?;
                                break;
                            }
                            continue;
                        }
                        Act::Fail => {
                            return Err(sched.fail(
                                Diagnostic::error(codes::RUNTIME_ERROR, "the task failed")
                                    .primary(Span::DUMMY, "here"),
                                trail.seed(),
                            ));
                        }
                    }
                    let let_go = sched.refuse_wait(task, Some(handlers.clock_mut()));
                    record_let_go(&mut trail, let_go);
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
    let run = run(&program, Seed::at(1, Vec::new())).expect("no reason to block");
    assert_eq!(run.marks, vec![(0, "only")]);
    assert_eq!(run.choices, vec![0], "the body's own step is a step");
}

#[test]
fn one_seed_produces_one_interleaving_however_often_it_is_run() {
    let program = two_workers();
    let first = run(&program, Seed::at(7, Vec::new())).expect("completes");
    for _ in 0..64 {
        let again = run(&program, Seed::at(7, Vec::new())).expect("completes");
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
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
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
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
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
    let free = run(&program, Seed::at(11, Vec::new())).expect("completes");
    let pinned = run(&program, Seed::at(11, free.choices.clone())).expect("completes");
    assert_eq!(pinned.marks, free.marks);
    assert_eq!(pinned.choices, free.choices);
}

#[test]
fn a_path_prefix_pins_only_the_steps_it_names() {
    let program = two_workers();
    let free = run(&program, Seed::at(3, Vec::new())).expect("completes");
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
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
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
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
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
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
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
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
        assert!(run.marks.contains(&(0, "joined twice")), "seed {root}");
    }
}

#[test]
fn a_join_cycle_is_a_deadlock_naming_both_tasks() {
    let program: Program = vec![vec![Act::Spawn(1), Act::Join(1)], vec![Act::Join(0)]];
    for root in 0..8 {
        let err = run(&program, Seed::at(root, Vec::new())).expect_err("nothing can run");
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
    let err = run(&program, Seed::at(0, Vec::new())).expect_err("nothing can run");
    assert_eq!(err.code, codes::DEADLOCK);
    assert!(err.message.contains("1 task is blocked"), "{}", err.message);
    assert!(
        err.labels
            .iter()
            .any(|l| l.message.contains("@0 waits here for @0"))
    );
}

fn received(run: &Run, task: u64) -> Vec<Value> {
    run.heard
        .iter()
        .filter(|(t, _)| *t == task)
        .map(|(_, v)| v.clone())
        .collect()
}

fn some_int(n: i64) -> Value {
    Value::ctor("Some", vec![Value::Int(n)])
}

#[test]
fn a_receiver_takes_every_value_in_the_order_it_was_sent() {
    for capacity in [0, 1, 3] {
        let program: Program = vec![
            vec![
                Act::Channel(capacity),
                Act::Spawn(1),
                Act::Recv(0),
                Act::Recv(0),
                Act::Recv(0),
                Act::Recv(0),
                Act::Join(1),
            ],
            vec![
                Act::Send(0, 1),
                Act::Send(0, 2),
                Act::Send(0, 3),
                Act::Close(0),
            ],
        ];
        for root in 0..8 {
            let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
            assert_eq!(
                received(&run, 0),
                vec![
                    some_int(1),
                    some_int(2),
                    some_int(3),
                    Value::ctor("None", vec![])
                ],
                "capacity {capacity}, seed {root}"
            );
            assert_eq!(
                received(&run, 1),
                vec![Value::Bool(true); 3],
                "capacity {capacity}, seed {root}"
            );
        }
    }
}

#[test]
fn closing_wakes_a_waiting_receiver_with_none_and_a_waiting_sender_with_false() {
    let program: Program = vec![
        vec![
            Act::Channel(0),
            Act::Spawn(1),
            Act::Spawn(2),
            Act::Yield,
            Act::Close(0),
            Act::Join(1),
            Act::Join(2),
        ],
        vec![Act::Recv(0)],
        vec![Act::Send(0, 7)],
    ];
    for root in 0..16 {
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
        let (r, s) = (received(&run, 1), received(&run, 2));
        // Either the two met before the close, or the close reached each of them.
        assert!(
            (r == vec![some_int(7)] && s == vec![Value::Bool(true)])
                || (r == vec![Value::ctor("None", vec![])] && s == vec![Value::Bool(false)]),
            "seed {root}: the receiver heard {r:?} and the sender {s:?}"
        );
    }
}

#[test]
fn a_value_sent_orders_its_send_before_the_receive_that_takes_it() {
    let program: Program = vec![
        vec![Act::Channel(1), Act::Spawn(1), Act::Recv(0), Act::Join(1)],
        vec![Act::Mark("before"), Act::Send(0, 5), Act::Yield],
    ];
    for root in 0..8 {
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
        assert_eq!(received(&run, 0), vec![some_int(5)], "seed {root}");
        let send = run
            .stamps
            .iter()
            .position(|(t, _)| t.0 == 1)
            .expect("the sender takes a step");
        // The root's fourth step is the one its receive answered, before it joins anything.
        let after = run
            .stamps
            .iter()
            .enumerate()
            .filter(|(_, (t, _))| t.0 == 0)
            .nth(3)
            .map(|(i, _)| i)
            .expect("the root steps after its receive");
        assert!(
            happens_before(&run.stamps[send].1, TaskId(1), &run.stamps[after].1),
            "seed {root}: the send is not ordered before the receive"
        );
    }
}

#[test]
fn a_receive_nobody_answers_is_a_deadlock_naming_the_channel() {
    let program: Program = vec![vec![Act::Channel(0), Act::Recv(0)]];
    let err = run(&program, Seed::at(0, Vec::new())).expect_err("nothing sends");
    assert_eq!(err.code, codes::DEADLOCK);
    assert!(
        err.labels.iter().any(|l| l
            .message
            .contains("@0 waits here to receive from channel #0")),
        "{:?}",
        err.labels
    );
    let full: Program = vec![vec![Act::Channel(1), Act::Send(0, 1), Act::Send(0, 2)]];
    let err = run(&full, Seed::at(0, Vec::new())).expect_err("nothing receives");
    assert!(
        err.labels.iter().any(|l| l
            .message
            .contains("@0 waits here to send on channel #0, which is full")),
        "{:?}",
        err.labels
    );
}

#[test]
fn a_negative_capacity_is_refused() {
    let program: Program = vec![vec![Act::Channel(-1)]];
    let err = run(&program, Seed::at(0, Vec::new())).expect_err("no channel holds -1 values");
    assert_eq!(err.code, codes::RUNTIME_ERROR);
}

/// What a select heard: `None` where no arm could go, else the arm that went and what it took or
/// sent, `None` there for a closed channel.
fn selected(heard: &Value) -> Option<Option<(i64, Option<i64>)>> {
    let Value::Ctor { name, args } = heard else {
        return None;
    };
    match (name.as_str(), args.as_slice()) {
        ("None", []) => Some(None),
        ("Some", [Value::Record(fields)]) => {
            let Value::Int(at) = fields.named("_0")? else {
                return None;
            };
            let Value::Ctor { name, args } = fields.named("_1")? else {
                return None;
            };
            let got = match (name.as_str(), args.as_slice()) {
                ("Some", [Value::Int(n)]) => Some(*n),
                _ => None,
            };
            Some(Some((*at, got)))
        }
        _ => None,
    }
}

fn selects(run: &Run, task: u64) -> Vec<Option<(i64, Option<i64>)>> {
    received(run, task).iter().filter_map(selected).collect()
}

#[test]
fn a_select_goes_on_the_first_arm_that_can_in_the_order_its_arms_are_given() {
    let program: Program = vec![vec![
        Act::Channel(1),
        Act::Channel(1),
        Act::Send(0, 10),
        Act::Send(1, 11),
        Act::Select(vec![(1, None), (0, None)], true),
        Act::Select(vec![(1, None), (0, None)], true),
    ]];
    let run = run(&program, Seed::at(0, Vec::new())).expect("completes");
    assert_eq!(
        selects(&run, 0),
        vec![Some((0, Some(11))), Some((1, Some(10)))]
    );
}

#[test]
fn a_select_that_does_not_wait_answers_at_once_whatever_its_arms_can_do() {
    let program: Program = vec![vec![
        Act::Channel(0),
        Act::Channel(1),
        // Nothing to receive, and a rendezvous with no receiver.
        Act::Select(vec![(0, None), (1, None), (0, Some(5))], false),
        // Room for one, then none.
        Act::Select(vec![(1, Some(6))], false),
        Act::Select(vec![(1, Some(7))], false),
        Act::Close(1),
        // A closed channel refuses a send, gives up what it held, and then is at its end.
        Act::Select(vec![(1, Some(8))], false),
        Act::Select(vec![(1, None)], false),
        Act::Select(vec![(1, None)], false),
        Act::Select(vec![], false),
    ]];
    let run = run(&program, Seed::at(0, Vec::new())).expect("completes");
    assert_eq!(
        selects(&run, 0),
        vec![
            None,
            Some((0, Some(6))),
            None,
            Some((0, None)),
            Some((0, Some(6))),
            Some((0, None)),
            None,
        ]
    );
}

#[test]
fn a_waiting_select_goes_on_the_arm_another_task_makes_ready_and_leaves_the_others() {
    let program: Program = vec![
        vec![
            Act::Channel(0),
            Act::Channel(0),
            Act::Spawn(1),
            Act::Select(vec![(0, None), (1, None)], true),
            // Were the select still waiting on channel 0, this send would be handed to it.
            Act::Spawn(2),
            Act::Recv(0),
            Act::Join(1),
            Act::Join(2),
        ],
        vec![Act::Send(1, 9)],
        vec![Act::Send(0, 4)],
    ];
    for root in 0..16 {
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
        assert_eq!(selects(&run, 0), vec![Some((1, Some(9)))], "seed {root}");
        assert_eq!(received(&run, 0).last(), Some(&some_int(4)), "seed {root}");
        assert_eq!(received(&run, 1), vec![Value::Bool(true)], "seed {root}");
    }
}

#[test]
fn a_select_that_waits_to_send_hands_its_value_to_the_receiver_that_comes() {
    let program: Program = vec![
        vec![
            Act::Channel(0),
            Act::Channel(0),
            Act::Spawn(1),
            Act::Select(vec![(1, None), (0, Some(5))], true),
            Act::Join(1),
        ],
        vec![Act::Recv(0)],
    ];
    for root in 0..16 {
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
        assert_eq!(selects(&run, 0), vec![Some((1, Some(5)))], "seed {root}");
        assert_eq!(received(&run, 1), vec![some_int(5)], "seed {root}");
    }
}

/// A select may wait on both sides of one channel, and a close reaches it once.
#[test]
fn a_close_reaches_a_select_waiting_on_both_sides_of_the_channel_once() {
    let program: Program = vec![
        vec![
            Act::Channel(0),
            Act::Spawn(1),
            Act::Sleep(1),
            Act::Close(0),
            Act::Join(1),
        ],
        vec![Act::Select(vec![(0, Some(1)), (0, None)], true)],
    ];
    for root in 0..8 {
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
        assert_eq!(selects(&run, 1), vec![Some((1, None))], "seed {root}");
    }
}

#[test]
fn cancelling_a_waiting_select_takes_it_off_every_channel_it_waited_on() {
    let program: Program = vec![
        vec![
            Act::Channel(0),
            Act::Channel(0),
            Act::Spawn(1),
            Act::Sleep(1),
            Act::Cancel(1),
            Act::Await(1),
            Act::Spawn(2),
            Act::Recv(0),
            Act::Join(2),
        ],
        vec![Act::Select(vec![(0, None), (1, Some(3))], true)],
        vec![Act::Send(0, 7)],
    ];
    for root in 0..8 {
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
        assert_eq!(
            received(&run, 0),
            vec![Value::Bool(true), Value::ctor("None", vec![]), some_int(7)],
            "seed {root}"
        );
        // The cancel is ordered against every other step on either channel.
        let cancel = run
            .touched
            .iter()
            .find(|(task, touched)| *task == 0 && touched.iter().any(|a| a.contains("task.alive")))
            .expect("the root's cancel is a step");
        for chan in ["#0", "#1"] {
            assert!(
                cancel.1.iter().any(|a| a.contains(chan)),
                "seed {root}: the cancel touched {:?}",
                cancel.1
            );
        }
    }
}

/// The search orders two steps only where it has both, so a task cancelled before it ran still
/// takes one, which reads what the cancel wrote and is not ordered after it.
#[test]
fn a_task_cancelled_before_it_starts_takes_one_step_the_cancel_is_ordered_against() {
    let program: Program = vec![
        vec![Act::Spawn(1), Act::Cancel(1), Act::Await(1)],
        vec![Act::Mark("ran")],
    ];
    let run = run(&program, Seed::at(0, vec![0, 0])).expect("completes");
    assert!(run.marks.is_empty(), "{:?}", run.marks);
    assert_eq!(
        received(&run, 0),
        vec![Value::Bool(true), Value::ctor("None", vec![])]
    );
    let dropped = run
        .touched
        .iter()
        .position(|(task, _)| *task == 1)
        .expect("the cancelled task takes a step");
    assert!(
        run.touched[dropped]
            .1
            .iter()
            .any(|a| a.contains("task.alive")),
        "{:?}",
        run.touched[dropped].1
    );
    assert!(
        !happens_before(&run.stamps[1].1, TaskId(0), &run.stamps[dropped].1),
        "a task that could have run before its cancel is ordered after it"
    );
}

/// The script of task 1 here stands in a bracket: `Enter` and `Leave` are where its `acquire` or
/// its `release` begins and returns.
fn bracketed(shield: Shield, inside: Vec<Act>) -> Vec<Act> {
    let mut script = vec![Act::Enter(shield)];
    script.extend(inside);
    script.extend([
        Act::Mark("returned"),
        Act::Leave(shield),
        Act::Mark("after"),
    ]);
    script
}

#[test]
fn a_cancel_takes_no_answer_from_an_acquire_and_lands_when_it_returns() {
    let root = vec![
        Act::Channel(1),
        Act::Send(0, 4),
        Act::Spawn(1),
        Act::Cancel(1),
        Act::Await(1),
    ];
    // The root makes, sends and spawns; the task's receive is answered; then the cancel.
    let seed = || Seed::at(0, vec![0, 0, 0, 1, 0]);
    let held: Program = vec![root.clone(), bracketed(Shield::Acquire, vec![Act::Recv(0)])];
    let run_held = run(&held, seed()).expect("completes");
    assert_eq!(received(&run_held, 1), vec![some_int(4)]);
    assert_eq!(run_held.marks, vec![(1, "returned"), (1, "landed")]);
    assert_eq!(
        received(&run_held, 0),
        vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::ctor("None", vec![])
        ]
    );

    let bare: Program = vec![root, vec![Act::Recv(0), Act::Mark("after")]];
    let run_bare = run(&bare, seed()).expect("completes");
    assert_eq!(received(&run_bare, 1), Vec::<Value>::new());
    assert_eq!(run_bare.marks, vec![(1, "stopped")]);
}

#[test]
fn a_cancel_lets_go_of_a_wait_inside_an_acquire_whenever_the_wait_begins() {
    let root = vec![
        Act::Channel(0),
        Act::Spawn(1),
        Act::Cancel(1),
        Act::Await(1),
    ];
    // Waiting already when the cancel comes.
    let waiting: Program = vec![root.clone(), bracketed(Shield::Acquire, vec![Act::Recv(0)])];
    let run_waiting = run(&waiting, Seed::at(0, vec![0, 0, 1, 0])).expect("completes");
    assert_eq!(run_waiting.marks, vec![(1, "stopped")]);

    // Answered when the cancel comes, and then asking for something it would wait on.
    let later: Program = vec![
        root,
        bracketed(Shield::Acquire, vec![Act::Yield, Act::Recv(0)]),
    ];
    let run_later = run(&later, Seed::at(0, vec![0, 0, 1, 0, 1])).expect("completes");
    assert_eq!(run_later.marks, vec![(1, "stopped")]);
    assert_eq!(received(&run_later, 1), Vec::<Value>::new());
}

#[test]
fn a_cancel_waits_for_a_release_and_a_second_cancel_changes_nothing() {
    let program: Program = vec![
        vec![
            Act::Channel(0),
            Act::Spawn(1),
            Act::Sleep(1),
            Act::Cancel(1),
            Act::Cancel(1),
            Act::Mark("cancelled"),
            Act::Send(0, 1),
            Act::Await(1),
        ],
        bracketed(Shield::Release, vec![Act::Recv(0)]),
    ];
    for root in 0..8 {
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
        assert_eq!(
            run.marks,
            vec![(0, "cancelled"), (1, "returned"), (1, "landed")],
            "seed {root}"
        );
        assert_eq!(received(&run, 1), vec![some_int(1)], "seed {root}");
        assert_eq!(
            received(&run, 0),
            vec![
                Value::Bool(true),
                Value::Bool(false),
                Value::Bool(true),
                Value::ctor("None", vec![])
            ],
            "seed {root}"
        );
    }
}

/// A livelock shares the deadlock's code, with a different message.
#[test]
fn a_region_that_never_stops_spends_its_step_budget() {
    let mut forever = vec![Act::Yield; 64];
    forever.push(Act::Mark("unreachable"));
    let program: Program = vec![forever];
    let err = run_with(&program, Seed::at(0, Vec::new()), 16).expect_err("the budget is spent");
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
    let err = run(&program, Seed::at(4, Vec::new())).expect_err("a task failed");
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
        &Seed::at(0, Vec::new()),
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
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
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
        let run = run(&program, Seed::at(root, Vec::new())).expect("completes");
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
    let run = run(&program, Seed::at(0, Vec::new())).expect("completes");
    assert_eq!(run.clock, 0);
    assert_eq!(run.marks, vec![(0, "through")]);
}

#[test]
fn consecutive_sleeps_accumulate_virtual_time() {
    let program: Program = vec![vec![Act::Sleep(30), Act::Sleep(12), Act::Mark("done")]];
    let run = run(&program, Seed::at(0, Vec::new())).expect("completes");
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
        .join(
            suspended(),
            &TaskHandle::unowned(SimId(0), TaskId(7)),
            Span::DUMMY,
        )
        .expect_err("no such task");
    assert_eq!(err.code, codes::TASK_ESCAPES_SCOPE);
    assert!(err.message.contains("@7"));
}

/// Both regions number a task `@1`, so a join by id alone would answer the other region's task.
#[test]
fn joining_another_regions_task_fails_rather_than_answering_this_regions_namesake() {
    let (mut first, mut clock, mut trail) = solo(0);
    let Turn::Run { .. } = first
        .next(&mut clock, &mut trail)
        .expect("the root is enabled")
    else {
        panic!("expected the first region's root");
    };
    let stranger = first.spawn(Value::Int(1), Span::DUMMY);

    let mut second: Sched = Scheduler::new(SimId(1), Span::DUMMY);
    let (mut clock, mut trail) = (Clock::new(), Trail::new(Seed::at(0, Vec::new())));
    let Turn::Run { .. } = second
        .next(&mut clock, &mut trail)
        .expect("the root is enabled")
    else {
        panic!("expected the second region's root");
    };
    let namesake = second.spawn(Value::Int(2), Span::DUMMY);
    assert_eq!(
        namesake.id(),
        stranger.id(),
        "each region numbers from `@1`"
    );

    let err = second
        .join(suspended(), &stranger, Span::DUMMY)
        .expect_err("the handle names a task of the first region");
    assert_eq!(err.code, codes::TASK_ESCAPES_SCOPE);
    assert!(err.message.contains("another region"), "{}", err.message);
    assert_ne!(
        Value::Task(stranger),
        Value::Task(namesake.clone()),
        "two regions' tasks are not one value"
    );

    second
        .join(suspended(), &namesake, Span::DUMMY)
        .expect("its own task is still joinable");
}

#[test]
fn every_step_records_the_set_its_choice_indexed() {
    let program = two_workers();
    let run = run(&program, Seed::at(13, Vec::new())).expect("completes");
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

/// The site `at` bytes into source 0, in the definition `definition`.
fn site(definition: &str, at: u32) -> StepSite {
    StepSite {
        definition: Some(Symbol::new(definition)),
        span: Span::new(SourceId(0), at, at + 1),
    }
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
    trail.record_access(atom("task", None, Mode::Write), site("m.f", 0));
    trail.record_access(atom("clock", None, Mode::Read), site("m.f", 1));
    trail.record_access(atom("clock", None, Mode::Write), site("m.f", 2));
    assert_eq!(trail.steps()[0].accesses.len(), 0);
    assert_eq!(trail.steps()[0].site, None, "bookkeeping places no step");

    trail.record_access(atom("random", None, Mode::Write), site("m.f", 3));
    trail.record_access(atom("db", Some("orders"), Mode::Write), site("m.f", 4));
    trail.record_access(
        Access::Cell {
            id: Slot::new(3, 0),
            mode: Mode::Write,
        },
        site("m.f", 5),
    );
    assert_eq!(trail.steps()[0].accesses.len(), 3);
}

/// A race names where each of its steps first touched something a task can share, and the
/// definition that did it, whatever the step went on to touch and wherever it gave control back.
#[test]
fn a_step_is_placed_at_its_first_shared_access() {
    let (mut sched, mut clock, mut trail) = solo(0);
    let Turn::Run { .. } = sched
        .next(&mut clock, &mut trail)
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    trail.record_access(atom("clock", None, Mode::Read), site("m.tick", 1));
    trail.record_access(
        atom("bank", Some("accounts"), Mode::Read),
        site("m.transfer", 10),
    );
    trail.record_access(
        Access::Cell {
            id: Slot::new(1, 0),
            mode: Mode::Read,
        },
        site("m.test#0", 20),
    );
    trail.end_step(site("m.transfer", 30));
    sched.suspend(suspended(), Value::Unit).expect("running");

    let Turn::Run { .. } = sched.next(&mut clock, &mut trail).expect("still enabled") else {
        panic!("expected a second step");
    };
    trail.end_step(site("m.idle", 40));
    sched.suspend(suspended(), Value::Unit).expect("running");

    let Turn::Run { .. } = sched.next(&mut clock, &mut trail).expect("still enabled") else {
        panic!("expected a third step");
    };
    trail.record_access(Access::Alloc, site("m.open", 50));

    let placed: Vec<(Option<String>, Span)> = trail
        .record()
        .steps
        .iter()
        .map(|step| (step.definition.as_ref().map(|d| d.to_string()), step.span))
        .collect();
    let named = |s: StepSite| (s.definition.map(|d| d.to_string()), s.span);
    assert_eq!(
        placed,
        [
            named(site("m.transfer", 10)),
            // Nothing shared: where it gave control back.
            named(site("m.idle", 40)),
            // The run failed in it, after its first access.
            named(site("m.open", 50)),
        ]
    );
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
    trail.record_access(
        Access::Cell {
            id: Slot::new(1, 0),
            mode: Mode::Write,
        },
        site("m.f", 0),
    );
    sched.suspend(suspended(), Value::Unit).expect("running");
    let Turn::Run { .. } = sched.next(&mut clock, &mut trail).expect("still enabled") else {
        panic!("expected a second step");
    };
    trail.record_access(
        Access::Cell {
            id: Slot::new(1, 0),
            mode: Mode::Read,
        },
        site("m.g", 0),
    );
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
        let a = run(&plain, Seed::at(root, Vec::new())).expect("completes");
        let b = run(&drawing, Seed::at(root, Vec::new())).expect("completes");
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
    let check = crate::fixture::port_check(&[("m", source)]);
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
    let trail = Trail::new(Seed::at(9, Vec::new()));
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
    let other = sched.spawn(Value::Unit, Span::DUMMY);
    sched
        .join(suspended(), &other, Span::DUMMY)
        .expect("the root is running");
    let Turn::Run { task, .. } = sched.next_host(&Idle).expect("the spawned task is enabled")
    else {
        panic!("expected the spawned task's first step");
    };
    assert_eq!(task, other.id());
    sched
        .join(
            suspended(),
            &TaskHandle::unowned(SimId(0), ROOT),
            Span::DUMMY,
        )
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
            .join(suspended(), &joined, Span::DUMMY)
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

/// The production region runs the one scheduler a seeded region does, so a select waits, is woken
/// and answers there as it does under a seed.
#[test]
fn a_production_region_selects_as_a_seeded_one_does() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    sched.channel(suspended(), 0).expect("the root is running");
    let Resumption::Resume {
        value: Value::Chan(meet),
        ..
    } = until_root(&mut sched, &Idle)
    else {
        panic!("the root resumes from making a channel");
    };
    sched
        .select(suspended(), vec![(meet, None)], false, Span::DUMMY)
        .expect("the root is running");
    let Resumption::Resume { value, .. } = until_root(&mut sched, &Idle) else {
        panic!("the root resumes from a select that does not wait");
    };
    assert_eq!(selected(&value), Some(None));

    // The root waits to send, and the receive of the task it spawned is what wakes it.
    let receiver = sched.spawn(Value::Unit, Span::DUMMY);
    sched
        .select(
            suspended(),
            vec![(meet, Some(Value::Int(5)))],
            true,
            Span::DUMMY,
        )
        .expect("the root is running");
    let Turn::Run { task, .. } = sched.next_host(&Idle).expect("the receiver is enabled") else {
        panic!("the region ended with its root waiting");
    };
    assert_eq!(task, receiver.id());
    sched
        .recv(suspended(), &meet, Span::DUMMY)
        .expect("the receiver is running");
    let mut heard = Vec::new();
    for _ in 0..2 {
        let Turn::Run { task, resumption } = sched.next_host(&Idle).expect("a task is enabled")
        else {
            panic!("the region ended before both had heard");
        };
        let Resumption::Resume { value, .. } = resumption else {
            panic!("{task} was not answered");
        };
        heard.push((task == ROOT, value));
        sched
            .suspend(suspended(), Value::Unit)
            .expect("the task is running");
    }
    heard.sort_by_key(|(root, _)| *root);
    assert_eq!(heard[0].1, some_int(5), "the receiver takes what was sent");
    assert_eq!(selected(&heard[1].1), Some(Some((0, Some(5)))));
}

/// A task cancelled before its join and one cancelled under it fail the join alike: as a raise in
/// the joiner, whose own handlers answer it, and never as the region's failure.
#[test]
fn joining_a_task_already_cancelled_resumes_the_joiner_with_a_raise() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let target = sched.spawn(Value::Unit, Span::DUMMY);
    let stopped = sched
        .cancel(suspended(), &target, Span::DUMMY, None)
        .expect("the root is running");
    assert_eq!(
        stopped.unstarted,
        Some(Value::Unit),
        "the task never started"
    );
    let Resumption::Resume { value, .. } = until_root(&mut sched, &Idle) else {
        panic!("the root resumes from its cancel");
    };
    assert_eq!(value, Value::Bool(true));
    sched
        .join(suspended(), &target, Span::DUMMY)
        .expect("a join of a cancelled task is the joiner's to answer");
    let Resumption::Raise { failure, .. } = until_root(&mut sched, &Idle) else {
        panic!("the join of a cancelled task did not raise in the joiner");
    };
    assert!(
        failure.message.contains("was cancelled"),
        "{}",
        failure.message
    );
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
                .join(suspended(), handle, Span::DUMMY)
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

/// A runtime whose clock a test reads and moves. A park on a deadline moves it there, as the
/// host's would wait.
#[derive(Default)]
struct Ticking {
    now: std::cell::Cell<i64>,
    /// Each deadline the region parked until, in order.
    parked_until: std::cell::RefCell<Vec<i64>>,
}

impl HostRuntime for Ticking {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("nothing here parks on a token, yet `{pending}` was watched"),
        ))
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "a region with a sleeper was parked on no deadline",
        ))
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "nothing here waits",
        ))
    }

    fn now(&self) -> Result<i64, Diagnostic> {
        Ok(self.now.get())
    }

    fn park_until(&self, deadline: i64) -> Result<(), Diagnostic> {
        self.parked_until.borrow_mut().push(deadline);
        self.now.set(self.now.get().max(deadline));
        Ok(())
    }
}

fn sleeps(sched: &mut Sched, nanos: i64, rt: &dyn HostRuntime) {
    sched
        .sleep_on_host(suspended(), nanos, Span::DUMMY, rt)
        .expect("the task is running");
}

fn next_task(sched: &mut Sched, rt: &dyn HostRuntime) -> (TaskId, Resumption<usize, Value>) {
    match sched.next_host(rt) {
        Ok(Turn::Run { task, resumption }) => (task, resumption),
        Ok(Turn::Complete(_)) => panic!("the region ended with a task still to run"),
        Err(d) => panic!("no task could run: {}", d.message),
    }
}

#[test]
fn a_sleeping_production_task_is_parked_alone_and_the_others_run() {
    let rt = Ticking::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let beside = sched.spawn(Value::Unit, Span::DUMMY);
    sleeps(&mut sched, 100, &rt);
    for _ in 0..5 {
        let (task, _) = next_task(&mut sched, &rt);
        assert_eq!(task, beside.id(), "the sleeper ran before its deadline");
        sched.suspend(suspended(), Value::Unit).expect("running");
    }

    // The deadline comes while the other task is still runnable, and the sleeper has the next turn.
    rt.now.set(100);
    let (task, resumption) = next_task(&mut sched, &rt);
    assert_eq!(task, ROOT);
    assert!(
        matches!(
            resumption,
            Resumption::Resume {
                value: Value::Unit,
                ..
            }
        ),
        "a sleep answers `Unit`"
    );
    assert!(
        rt.parked_until.borrow().is_empty(),
        "the thread waited while a task could run: {:?}",
        rt.parked_until.borrow()
    );
}

#[test]
fn with_nothing_runnable_a_production_region_waits_for_its_earliest_deadline() {
    let rt = Ticking::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let late = sched.spawn(Value::Unit, Span::DUMMY);
    let early = sched.spawn(Value::Unit, Span::DUMMY);
    sleeps(&mut sched, 300, &rt);
    assert_eq!(next_task(&mut sched, &rt).0, late.id());
    sleeps(&mut sched, 200, &rt);
    assert_eq!(next_task(&mut sched, &rt).0, early.id());
    sleeps(&mut sched, 100, &rt);

    let mut woke = Vec::new();
    loop {
        match sched.next_host(&rt).expect("each sleeper wakes") {
            Turn::Complete(_) => break,
            Turn::Run { task, .. } => {
                woke.push((task, rt.now.get()));
                sched.finish(Value::Unit).expect("the task is running");
            }
        }
    }
    assert_eq!(woke, vec![(early.id(), 100), (late.id(), 200), (ROOT, 300)]);
    assert_eq!(*rt.parked_until.borrow(), vec![100, 200, 300]);
}

#[test]
fn cancelling_a_sleeping_production_task_lets_go_of_its_deadline() {
    let rt = Ticking::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let sleeper = sched.spawn(Value::Unit, Span::DUMMY);
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the root is running");
    assert_eq!(next_task(&mut sched, &rt).0, sleeper.id());
    sleeps(&mut sched, 1_000, &rt);

    assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    let stopped = sched
        .cancel(suspended(), &sleeper, Span::DUMMY, None)
        .expect("the root is running");
    assert!(stopped.stopped);
    assert_eq!(stopped.let_go, LetGo::Nothing);

    let (task, resumption) = next_task(&mut sched, &rt);
    assert_eq!(task, sleeper.id());
    assert!(
        matches!(resumption, Resumption::Cancel { .. }),
        "a cancelled sleeper only unwinds"
    );
    sched.finish_cancelled().expect("the sleeper is running");

    // The root sleeps past where the cancelled task would have woken.
    assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    sleeps(&mut sched, 2_000, &rt);
    assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    assert_eq!(
        *rt.parked_until.borrow(),
        vec![2_000],
        "the region waited on a deadline nothing sleeps until"
    );
}

#[test]
fn a_cancel_inside_a_release_leaves_a_production_sleeper_asleep_until_its_deadline() {
    let rt = Ticking::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let sleeper = sched.spawn(Value::Unit, Span::DUMMY);
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the root is running");
    assert_eq!(next_task(&mut sched, &rt).0, sleeper.id());
    assert_eq!(sched.shield(Shield::Release), Some(sleeper.id()));
    sleeps(&mut sched, 500, &rt);

    assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    let stopped = sched
        .cancel(suspended(), &sleeper, Span::DUMMY, None)
        .expect("the root is running");
    assert!(stopped.stopped);
    assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    sched
        .await_task(suspended(), &sleeper, Span::DUMMY)
        .expect("the root is running");

    let (task, resumption) = next_task(&mut sched, &rt);
    assert_eq!(task, sleeper.id());
    assert!(
        matches!(resumption, Resumption::Resume { .. }),
        "the release's sleep was cut short"
    );
    assert_eq!(*rt.parked_until.borrow(), vec![500]);
    assert!(
        sched.unshield(sleeper.id(), Shield::Release),
        "the cancel lands when the release returns"
    );
    sched.finish_cancelled().expect("the sleeper is running");
    let (task, resumption) = next_task(&mut sched, &rt);
    assert_eq!(task, ROOT);
    assert!(
        matches!(&resumption, Resumption::Resume { value, .. } if *value == Value::ctor("None", Vec::new())),
        "an await of a cancelled task hears `None`"
    );
}

#[test]
fn a_production_sleep_of_no_span_is_a_yield() {
    let rt = Ticking::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let beside = sched.spawn(Value::Unit, Span::DUMMY);
    for nanos in [0, -5] {
        sleeps(&mut sched, nanos, &rt);
        assert_eq!(next_task(&mut sched, &rt).0, beside.id());
        sched.suspend(suspended(), Value::Unit).expect("running");
        assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    }
    assert!(rt.parked_until.borrow().is_empty());
}

#[test]
fn a_production_sleep_needs_a_runtime_that_keeps_time() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let err = sched
        .sleep_on_host(suspended(), 5, Span::DUMMY, &Idle)
        .expect_err("the idle runtime reads no clock");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert_eq!(
        sched.current(),
        Some(ROOT),
        "the refused task was put to sleep anyway"
    );
}

/// Virtual time is the seed's: a seeded region that slept on the host's clock would stop being a
/// function of it.
#[test]
fn a_seeded_region_refuses_a_sleep_on_the_hosts_clock() {
    let (mut sched, mut clock, mut trail) = solo(0);
    let Turn::Run { .. } = sched
        .next(&mut clock, &mut trail)
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    let err = sched
        .sleep_on_host(suspended(), 5, Span::DUMMY, &Ticking::default())
        .expect_err("a simulated region may not wait on the host's clock");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert!(err.message.contains("seeded region"), "{}", err.message);
    assert!(
        sched.current().is_some(),
        "the task was put to sleep anyway"
    );
}

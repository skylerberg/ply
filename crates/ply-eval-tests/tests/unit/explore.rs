use ply_core::{EffectAtom, Resource};
use ply_eval::arena::Slot;
use ply_eval::cont::SimId;
use ply_eval::explore::*;
use ply_eval::sched::{Stamp, happens_before};
use ply_eval::sim::{Access, Domain, Stream};
use ply_eval::sim::{Naive, Plan, Seed, StepFootprint, TaskId};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Mode;
use std::collections::{BTreeMap, BTreeSet};

/// A model scheduler: enough of the scheduler on the control stack to exercise the search, and none of the machine.
#[derive(Clone, Debug)]
enum Op {
    /// Read a cell into this task's register.
    Load(u32),
    /// Write `register + 1` into a cell.
    Store(u32),
    /// `db.<mode>[resource]`.
    Perform(&'static str, Mode),
    Spawn(&'static str, Vec<Op>),
    /// Join this task's `n`th child, by spawn order rather than by id, so a program means the
    /// same thing under every interleaving.
    Join(usize),
    Yield,
}

#[derive(Clone)]
struct ModelTask {
    name: Symbol,
    ops: Vec<Op>,
    pc: usize,
    register: i64,
    children: Vec<usize>,
    blocked: Option<usize>,
    /// The same vector clock the real scheduler keeps, so the search's happens-before filter is
    /// exercised here rather than only against the machine.
    clock: Stamp,
}

#[derive(Clone)]
struct Model {
    tasks: Vec<ModelTask>,
    cells: BTreeMap<u32, i64>,
    expect: Vec<(u32, i64)>,
    runs: usize,
    /// Every interleaving this model was asked for, as the task order it produced.
    traces: Vec<Vec<TaskId>>,
    /// The final world of each interleaving.
    outcomes: Vec<Vec<(u32, i64)>>,
}

const STEP_CAP: usize = 4096;

impl Model {
    fn new(main: Vec<Op>) -> Model {
        Model {
            tasks: vec![ModelTask {
                name: Symbol::new("main"),
                ops: main,
                pc: 0,
                register: 0,
                children: Vec::new(),
                blocked: None,
                clock: vec![0],
            }],
            cells: BTreeMap::new(),
            expect: Vec::new(),
            runs: 0,
            traces: Vec::new(),
            outcomes: Vec::new(),
        }
    }

    fn expecting(mut self, cells: &[(u32, i64)]) -> Model {
        self.expect = cells.to_vec();
        self
    }

    fn finished(tasks: &[ModelTask], t: usize) -> bool {
        tasks[t].pc == tasks[t].ops.len()
            && tasks[t].blocked.is_none_or(|on| Model::finished(tasks, on))
    }

    fn tick(tasks: &mut [ModelTask], t: usize) {
        let width = tasks.len();
        for task in tasks.iter_mut() {
            task.clock.resize(width, 0);
        }
        tasks[t].clock[t] += 1;
    }

    fn absorb(tasks: &mut [ModelTask], into: usize, from: usize) {
        let source = tasks[from].clock.clone();
        let target = &mut tasks[into].clock;
        if target.len() < source.len() {
            target.resize(source.len(), 0);
        }
        for (slot, seen) in target.iter_mut().zip(source) {
            *slot = (*slot).max(seen);
        }
    }

    fn enabled(tasks: &[ModelTask]) -> Vec<usize> {
        (0..tasks.len())
            .filter(|&t| {
                tasks[t].pc < tasks[t].ops.len()
                    && tasks[t].blocked.is_none_or(|on| Model::finished(tasks, on))
            })
            .collect()
    }
}

impl Simulation for Model {
    fn run(&mut self, seed: &Seed) -> Interleaving {
        self.runs += 1;
        let mut tasks = self.tasks.clone();
        let mut cells = self.cells.clone();
        let mut sched = Stream::new(seed.root, Domain::Sched);
        let mut steps = Vec::new();
        let mut order = Vec::new();

        loop {
            let enabled = Model::enabled(&tasks);
            if enabled.is_empty() {
                break;
            }
            if steps.len() >= STEP_CAP {
                return Interleaving::failed(
                    steps,
                    Diagnostic::error(codes::DEADLOCK, "the model ran out of steps"),
                );
            }
            let point = steps.len();
            let chosen = match seed.choice(point) {
                Some(fixed) => usize::from(fixed),
                None => sched.below(enabled.len() as u64).unwrap_or(0) as usize,
            };
            let chosen = chosen.min(enabled.len() - 1);
            let t = enabled[chosen];
            let op = tasks[t].ops[tasks[t].pc].clone();
            tasks[t].pc += 1;
            Model::tick(&mut tasks, t);
            let mut accesses = StepFootprint::new();
            match op {
                Op::Load(cell) => {
                    tasks[t].register = *cells.get(&cell).unwrap_or(&0);
                    accesses.insert(Access::Cell {
                        id: Slot::new(cell, 0),
                        mode: Mode::Read,
                    });
                }
                Op::Store(cell) => {
                    cells.insert(cell, tasks[t].register + 1);
                    accesses.insert(Access::Cell {
                        id: Slot::new(cell, 0),
                        mode: Mode::Write,
                    });
                }
                Op::Perform(resource, mode) => {
                    accesses.insert(Access::Atom(EffectAtom::new(
                        "db",
                        Resource::Named(Symbol::new(resource)),
                        mode,
                    )));
                }
                Op::Spawn(name, ops) => {
                    let child = tasks.len();
                    let inherited = tasks[t].clock.clone();
                    tasks.push(ModelTask {
                        name: Symbol::new(name),
                        ops,
                        pc: 0,
                        register: 0,
                        children: Vec::new(),
                        blocked: None,
                        clock: inherited,
                    });
                    tasks[t].children.push(child);
                }
                Op::Join(nth) => {
                    let child = tasks[t].children[nth];
                    if Model::finished(&tasks, child) {
                        Model::absorb(&mut tasks, t, child);
                    } else {
                        tasks[t].blocked = Some(child);
                    }
                }
                Op::Yield => {}
            }
            // A task blocked on a join observes everything its target did, the moment the
            // target finishes.
            for i in 0..tasks.len() {
                if let Some(on) = tasks[i].blocked
                    && Model::finished(&tasks, on)
                {
                    Model::absorb(&mut tasks, i, on);
                }
            }
            order.push(TaskId(t as u32));
            steps.push(Step {
                region: SimId(0),
                task: TaskId(t as u32),
                enabled: enabled.iter().map(|&t| TaskId(t as u32)).collect(),
                choice: chosen as u16,
                accesses,
                definition: Some(tasks[t].name.clone()),
                span: Span::DUMMY,
                stamp: tasks[t].clock.clone(),
            });
        }

        self.traces.push(order);
        self.outcomes
            .push(cells.iter().map(|(&c, &v)| (c, v)).collect());
        if (0..tasks.len()).any(|t| !Model::finished(&tasks, t)) {
            return Interleaving::failed(
                steps,
                Diagnostic::error(codes::DEADLOCK, "no task can make progress"),
            );
        }
        for (cell, want) in &self.expect {
            let got = cells.get(cell).copied().unwrap_or(0);
            if got != *want {
                return Interleaving::failed(
                    steps,
                    Diagnostic::error(
                        codes::ASSERTION_FAILED,
                        format!("expected {want} in cell #{cell}, found {got}"),
                    ),
                );
            }
        }
        Interleaving::passed(steps)
    }
}

fn dpor(budget: u32) -> Plan {
    Plan {
        budget,
        ..Plan::default()
    }
}

fn read(resource: &'static str) -> Op {
    Op::Perform(resource, Mode::Read)
}

fn write(resource: &'static str) -> Op {
    Op::Perform(resource, Mode::Write)
}

/// Did the two tasks appear in both orders across the interleavings run?
fn both_orders(model: &Model, a: TaskId, b: TaskId) -> bool {
    let order = |trace: &Vec<TaskId>| {
        let ia = trace.iter().position(|&t| t == a);
        let ib = trace.iter().position(|&t| t == b);
        match (ia, ib) {
            (Some(ia), Some(ib)) => Some(ia < ib),
            _ => None,
        }
    };
    model.traces.iter().filter_map(order).any(|x| x)
        && model.traces.iter().filter_map(order).any(|x| !x)
}

/// The headline.
#[test]
fn tasks_that_never_conflict_explore_exactly_one_interleaving() {
    let mut model = Model::new(vec![
        Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
        Op::Spawn("b", vec![Op::Load(2), Op::Store(2)]),
        Op::Join(0),
        Op::Join(1),
    ])
    .expecting(&[(1, 1), (2, 1)]);

    let explored = explore(&dpor(64), &mut model);
    assert_eq!(explored.exploration.explored, 1);
    assert!(explored.exploration.exhaustive);
    assert!(!explored.exploration.exhausted);
    assert!(explored.passed());
}

/// ...and the same program under an unpruned search runs seventy-one, which is the measurement
/// the whole claim rests on.
#[test]
fn the_naive_count_for_the_same_program_is_larger() {
    let mut model = Model::new(vec![
        Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
        Op::Spawn("b", vec![Op::Load(2), Op::Store(2)]),
        Op::Join(0),
        Op::Join(1),
    ])
    .expecting(&[(1, 1), (2, 1)]);

    let explored = measure_reduction(&dpor(NAIVE_BUDGET), &mut model);
    assert_eq!(explored.exploration.explored, 1);
    assert_eq!(
        explored.exploration.naive,
        Some(Naive {
            explored: 71,
            bounded: false
        })
    );
    assert_eq!(explored.exploration.reduction(), Some(71.0));
}

/// Two tasks that *do* share a cell: the search runs nine interleavings against the unpruned
/// seventy-one.
#[test]
fn a_shared_cell_costs_interleavings_and_still_reduces() {
    let program = || {
        Model::new(vec![
            Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
            Op::Spawn("b", vec![Op::Load(1), Op::Store(1)]),
            Op::Join(0),
            Op::Join(1),
        ])
    };
    let mut model = program();
    let explored = explore(&dpor(NAIVE_BUDGET), &mut model);
    assert_eq!(explored.exploration.explored, 9);
    assert!(explored.exploration.exhaustive);

    let mut whole = program();
    let unpruned = explore_under(&dpor(NAIVE_BUDGET), Dependence::All, &mut whole);
    assert_eq!(unpruned.exploration.explored, 71);
}

/// Small enough to enumerate by hand.
#[test]
fn the_naive_count_is_exact_on_a_fixture_that_can_be_counted_by_hand() {
    let mut model = Model::new(vec![
        Op::Spawn("a", vec![read("x")]),
        Op::Spawn("b", vec![read("y")]),
    ]);
    let explored = measure_reduction(&dpor(64), &mut model);
    assert_eq!(explored.exploration.explored, 1);
    assert_eq!(
        explored.exploration.naive,
        Some(Naive {
            explored: 3,
            bounded: false
        })
    );
}

/// A budget the naive search spends is reported as a lower bound and never as an exact number
/// nobody observed.
#[test]
fn a_spent_naive_budget_is_reported_as_a_bound() {
    let mut model = Model::new(vec![
        Op::Spawn("a", vec![read("x"), read("x"), read("x"), read("x")]),
        Op::Spawn("b", vec![read("y"), read("y"), read("y"), read("y")]),
        Op::Spawn("c", vec![read("z"), read("z"), read("z"), read("z")]),
        Op::Join(0),
        Op::Join(1),
        Op::Join(2),
    ]);
    let plan = Plan {
        budget: 8,
        ..Plan::default()
    };
    let mut explored = measure_reduction(&plan, &mut model);
    // The naive budget is the larger of the plan's and NAIVE_BUDGET, so shrink the space
    // instead of the budget by asserting on the flag.
    let naive = explored.exploration.naive.take().expect("measured");
    assert!(naive.bounded, "expected a bounded count, got {naive}");
    assert_eq!(naive.explored, NAIVE_BUDGET);
    assert!(naive.to_string().starts_with(">= "));
}

/// Two writes to one cell.
#[test]
fn a_conflicting_pair_is_explored_in_both_orders() {
    let mut model = Model::new(vec![
        Op::Spawn("a", vec![Op::Store(1)]),
        Op::Spawn("b", vec![Op::Store(1)]),
        Op::Join(0),
        Op::Join(1),
    ])
    .expecting(&[(1, 1)]);

    let explored = explore(&dpor(64), &mut model);
    assert!(explored.passed());
    assert!(explored.exploration.exhaustive);
    assert!(explored.exploration.explored >= 2);
    assert!(
        both_orders(&model, TaskId(1), TaskId(2)),
        "explored {:?}",
        model.traces
    );
}

/// The lost update, as two steps of the model above: the search finds the interleaving in which
/// both tasks load before either stores, and reports the seed that reproduces it.
#[test]
fn a_genuine_race_is_found_and_named() {
    let mut model = Model::new(vec![
        Op::Spawn("credit", vec![Op::Load(1), Op::Store(1)]),
        Op::Spawn("debit", vec![Op::Load(1), Op::Store(1)]),
        Op::Join(0),
        Op::Join(1),
    ])
    .expecting(&[(1, 2)]);

    let explored = explore(&dpor(256), &mut model);
    assert!(!explored.passed(), "the lost update was not found");
    let seed = explored
        .exploration
        .failure
        .clone()
        .expect("a failing seed");
    let race = explored.exploration.race.clone().expect("an observed race");
    assert_ne!(race.left.task, race.right.task);
    assert!(race.left.access.starts_with("cell."));
    assert!(race.right.access.starts_with("cell."));
    assert!(
        race.left.definition.is_some() && race.right.definition.is_some(),
        "the race sites name the definitions they were in"
    );
    assert_eq!(
        explored.diagnostic.expect("a diagnostic").code,
        codes::ASSERTION_FAILED
    );

    // The seed the artifact prints is the seed that reproduces it.
    let mut replay = Model::new(model.tasks[0].ops.clone()).expecting(&[(1, 2)]);
    let again = explore(&Plan::once(seed), &mut replay);
    assert!(!again.passed());
    assert_eq!(again.exploration.explored, 1);
    // A sampled run observed no flip, so it reports no race rather than an inferred one.
    assert_eq!(again.exploration.race, None);
}

/// The pair conflicts on one resource and not on the other.
#[test]
fn footprints_that_conflict_on_one_resource_only() {
    let disjoint = vec![
        Op::Spawn("a", vec![read("x"), write("y")]),
        Op::Spawn("b", vec![read("x"), write("z")]),
        Op::Join(0),
        Op::Join(1),
    ];
    let mut shared_read = Model::new(disjoint.clone());
    let read_only = explore(&dpor(64), &mut shared_read);
    assert_eq!(read_only.exploration.explored, 1, "two readers commute");
    assert!(read_only.exploration.exhaustive);

    let contended = vec![
        Op::Spawn("a", vec![write("x"), write("y")]),
        Op::Spawn("b", vec![read("x"), write("z")]),
        Op::Join(0),
        Op::Join(1),
    ];
    let mut shared_write = Model::new(contended);
    let contested = explore(&dpor(64), &mut shared_write);
    assert!(
        contested.exploration.explored > read_only.exploration.explored,
        "a write against a read of the same resource is a real difference"
    );
    assert!(contested.exploration.exhaustive);
    assert!(both_orders(&shared_write, TaskId(1), TaskId(2)));
    // ...and still far short of the unpruned space, because `y` and `z` conflict with nothing.
    let mut measured = Model::new(disjoint);
    let naive = measure_reduction(&dpor(64), &mut measured)
        .exploration
        .naive
        .expect("measured");
    assert!(naive.explored > contested.exploration.explored);
}

/// The property pruning has to preserve, checked directly rather than argued: over each
/// program, the interleavings the pruned search runs produce **the same set of final worlds**
/// as the interleavings an unpruned enumeration runs.
#[test]
fn pruning_preserves_every_outcome_the_unpruned_search_observes() {
    let programs: Vec<(&str, Vec<Op>)> = vec![
        (
            "lost update",
            vec![
                Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
                Op::Spawn("b", vec![Op::Load(1), Op::Store(1)]),
                Op::Join(0),
                Op::Join(1),
            ],
        ),
        (
            "a racer behind a join",
            vec![
                Op::Spawn(
                    "late",
                    vec![
                        Op::Spawn("barrier", vec![Op::Yield]),
                        Op::Join(0),
                        Op::Load(1),
                        Op::Store(1),
                    ],
                ),
                Op::Load(1),
                Op::Store(1),
                Op::Join(0),
            ],
        ),
        (
            "a nested spawn racing its parent's sibling",
            vec![
                Op::Spawn(
                    "outer",
                    vec![Op::Spawn("inner", vec![Op::Store(1)]), Op::Join(0)],
                ),
                Op::Spawn("other", vec![Op::Load(1)]),
                Op::Join(0),
                Op::Join(1),
            ],
        ),
        (
            "two resources, one contended",
            vec![
                Op::Spawn("a", vec![write("x"), Op::Store(1)]),
                Op::Spawn("b", vec![read("x"), Op::Store(2)]),
                Op::Join(0),
                Op::Join(1),
            ],
        ),
    ];

    for (name, ops) in programs {
        let plan = Plan {
            budget: NAIVE_BUDGET,
            ..Plan::default()
        };
        let mut pruned_model = Model::new(ops.clone());
        let pruned = explore_under(&plan, Dependence::Exact, &mut pruned_model);
        let mut whole_model = Model::new(ops);
        let whole = explore_under(&plan, Dependence::All, &mut whole_model);
        assert!(
            pruned.exploration.exhaustive && whole.exploration.exhaustive,
            "{name}: both searches must reach their frontier for the comparison to mean \
             anything"
        );
        let seen: BTreeSet<&Vec<(u32, i64)>> = pruned_model.outcomes.iter().collect();
        let all: BTreeSet<&Vec<(u32, i64)>> = whole_model.outcomes.iter().collect();
        assert_eq!(
            seen, all,
            "{name}: pruning hid an outcome ({} interleavings against {})",
            pruned.exploration.explored, whole.exploration.explored
        );
        assert!(
            pruned.exploration.explored <= whole.exploration.explored,
            "{name}: pruning must not cost more than not pruning"
        );
    }
}

#[test]
fn a_read_read_pair_is_one_interleaving_and_a_read_write_pair_is_both_orders() {
    let mut readers = Model::new(vec![
        Op::Spawn("a", vec![read("x")]),
        Op::Spawn("b", vec![read("x")]),
        Op::Join(0),
        Op::Join(1),
    ]);
    assert_eq!(explore(&dpor(64), &mut readers).exploration.explored, 1);

    let mut writer = Model::new(vec![
        Op::Spawn("a", vec![read("x")]),
        Op::Spawn("b", vec![write("x")]),
        Op::Join(0),
        Op::Join(1),
    ]);
    let explored = explore(&dpor(64), &mut writer);
    assert!((2..=3).contains(&explored.exploration.explored));
    assert!(explored.exploration.exhaustive);
    assert!(both_orders(&writer, TaskId(1), TaskId(2)));
}

/// The relation is at cell granularity, so two cells that would share one `[r]` label are two
/// locations and do not contend.
#[test]
fn two_cells_under_one_label_do_not_contend() {
    let mut model = Model::new(vec![
        Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
        Op::Spawn("b", vec![Op::Load(2), Op::Store(2)]),
        Op::Join(0),
        Op::Join(1),
    ]);
    assert_eq!(explore(&dpor(64), &mut model).exploration.explored, 1);
}

/// Tasks that appear part way through a run: the enabled set grows, so a choice index at one
/// scheduling point means something different from the same index at another, and a backtrack
/// point is only meaningful against the enabled set that was recorded with it.
#[test]
fn nested_spawns_are_explored() {
    let mut model = Model::new(vec![
        Op::Spawn(
            "outer",
            vec![
                Op::Spawn("inner", vec![Op::Load(1), Op::Store(1)]),
                Op::Join(0),
            ],
        ),
        Op::Spawn("other", vec![Op::Load(1), Op::Store(1)]),
        Op::Join(0),
        Op::Join(1),
    ])
    .expecting(&[(1, 2)]);

    let explored = explore(&dpor(256), &mut model);
    assert!(
        !explored.passed(),
        "the grandchild races the sibling and the search must reach it"
    );
    assert!(explored.exploration.race.is_some());
    // @3 is the grandchild: spawned by @1, so it exists in no enabled set until @1 has run
    // twice.
    let race = explored.exploration.race.expect("a race");
    assert!(
        [race.left.task, race.right.task].contains(&TaskId(3)),
        "the race names the nested task, not its parent"
    );
}

/// A passing nested program is still enumerated to a frontier rather than sampled:
/// exhaustiveness is the headline, and it must survive tasks that did not exist when the search
/// started.
#[test]
fn a_nested_spawn_that_conflicts_with_nothing_is_one_interleaving() {
    let mut model = Model::new(vec![
        Op::Spawn(
            "outer",
            vec![Op::Spawn("inner", vec![Op::Store(2)]), Op::Join(0)],
        ),
        Op::Spawn("other", vec![Op::Store(3)]),
        Op::Join(0),
        Op::Join(1),
    ])
    .expecting(&[(2, 1), (3, 1)]);

    let explored = explore(&dpor(64), &mut model);
    assert!(explored.passed());
    assert_eq!(explored.exploration.explored, 1);
    assert!(explored.exploration.exhaustive);
}

/// The soundness case the backtrack-set search's pseudocode drops.
#[test]
fn a_race_with_a_task_that_was_blocked_at_the_backtrack_point_is_still_found() {
    for root in 0..8u64 {
        let mut model = Model::new(vec![
            Op::Spawn(
                "late",
                vec![
                    Op::Spawn("barrier", vec![Op::Yield]),
                    Op::Join(0),
                    Op::Load(1),
                    Op::Store(1),
                ],
            ),
            Op::Load(1),
            Op::Store(1),
            Op::Join(0),
        ])
        .expecting(&[(1, 2)]);
        let plan = Plan {
            roots: vec![root],
            budget: 512,
            ..Plan::default()
        };
        let explored = explore(&plan, &mut model);
        assert!(
            !explored.passed(),
            "root {root} missed the lost update behind a join",
        );
    }
}

/// The same case, as a unit of the rule rather than of the search: when the racing task is not
/// enabled at the backtrack point, the alternatives that could unblock it are queued.
#[test]
fn the_backtrack_rule_queues_alternatives_when_the_racer_is_not_enabled() {
    let cell = |id: u32, mode: Mode| {
        StepFootprint::from_accesses([Access::Cell {
            id: Slot::new(id, 0),
            mode,
        }])
    };
    let step = |task: u32, enabled: &[u32], choice: u16, accesses: StepFootprint| Step {
        region: SimId(0),
        task: TaskId(task),
        enabled: enabled.iter().map(|&t| TaskId(t)).collect(),
        choice,
        accesses,
        definition: None,
        span: Span::DUMMY,
        stamp: Stamp::new(),
    };
    // @2 is blocked at point 0 and writes the same cell at point 2; only @1 running at point 0
    // can ever unblock it.
    let blocked = vec![
        step(0, &[0, 1], 0, cell(1, Mode::Write)),
        step(1, &[0, 1], 1, StepFootprint::new()),
        step(2, &[0, 1, 2], 2, cell(1, Mode::Write)),
    ];
    let out = backtracks(&blocked, Dependence::Exact);
    let at_zero = out.get(&0).expect("a backtrack point at 0");
    assert_eq!(
        at_zero.get(&TaskId(1)),
        Some(&None),
        "the alternative that can unblock @2 is queued, and names no race because \
         none was observed"
    );
    assert!(
        !at_zero.contains_key(&TaskId(2)),
        "@2 could not have run at point 0, so scheduling it there is not a schedule"
    );

    // Where the racer *is* enabled, the pair is named exactly, and that is the pair the failure
    // artifact prints.
    let enabled = vec![
        step(0, &[0, 1], 0, cell(1, Mode::Write)),
        step(1, &[0, 1], 1, cell(1, Mode::Write)),
    ];
    let direct = backtracks(&enabled, Dependence::Exact);
    assert_eq!(
        direct.get(&0).and_then(|m| m.get(&TaskId(1))),
        Some(&Some(1))
    );
}

/// The shape of nearly every concurrent test there is: spawn, join, then assert on what the
/// children wrote.
#[test]
fn asserting_on_what_a_joined_task_wrote_costs_no_interleavings() {
    let program = || {
        Model::new(vec![
            Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
            Op::Spawn("b", vec![Op::Load(2), Op::Store(2)]),
            Op::Join(0),
            Op::Join(1),
            // The assertions: the parent reads both cells after joining.
            Op::Load(1),
            Op::Load(2),
        ])
        .expecting(&[(1, 1), (2, 1)])
    };

    let mut model = program();
    let explored = explore(&dpor(NAIVE_BUDGET), &mut model);
    assert!(explored.passed());
    assert_eq!(
        explored.exploration.explored, 1,
        "a read the join already ordered is not a race"
    );
    assert!(explored.exploration.exhaustive);

    // The same program with the recording's clocks withheld, which is what this search did
    // before it read them.
    struct Blind(Model);
    impl Simulation for Blind {
        fn run(&mut self, seed: &Seed) -> Interleaving {
            let mut run = self.0.run(seed);
            for step in &mut run.steps {
                step.stamp.clear();
            }
            run
        }
    }
    let mut blind = Blind(program());
    let unsynchronized = explore(&dpor(NAIVE_BUDGET), &mut blind);
    assert!(unsynchronized.passed());
    // Pinned rather than bounded: this number moving is the news.
    assert_eq!(unsynchronized.exploration.explored, 6);
}

/// A stamp orders two steps only when the later task really had observed the earlier one.
#[test]
fn an_absent_clock_orders_nothing_and_a_present_one_orders_what_it_saw() {
    assert!(!happens_before(&Stamp::new(), TaskId(0), &vec![3, 1]));
    assert!(!happens_before(&vec![3, 1], TaskId(0), &Stamp::new()));
    // @0 has taken three steps; @1 has seen all three.
    assert!(happens_before(&vec![3, 0], TaskId(0), &vec![3, 1]));
    // ...and not when it has only seen two of them.
    assert!(!happens_before(&vec![3, 0], TaskId(0), &vec![2, 1]));
    // A task that has taken no step of its own orders nothing by it.
    assert!(!happens_before(&vec![0, 2], TaskId(0), &vec![0, 5]));
    // A shorter clock is read as zeroes rather than as agreement.
    assert!(!happens_before(&vec![0, 0, 2], TaskId(2), &vec![1, 1]));
}

/// Enabledness carries synchronization, not the dependence relation.
#[test]
fn a_joined_task_is_never_scheduled_before_its_target() {
    let mut model = Model::new(vec![
        Op::Spawn("producer", vec![Op::Store(1)]),
        Op::Join(0),
        Op::Spawn("consumer", vec![Op::Load(1)]),
        Op::Join(1),
    ]);
    let explored = explore(&dpor(64), &mut model);
    assert!(explored.passed());
    assert!(
        !both_orders(&model, TaskId(1), TaskId(2)),
        "the join orders the write before the read in every schedule: {:?}",
        model.traces
    );
}

/// Budgets bound the search, and a search that did not empty its frontier says so — an
/// exhausted run proved nothing about the interleavings it did not reach, and
/// `Exploration::is_cacheable` is what acts on that.
#[test]
fn a_spent_budget_is_exhausted_and_not_exhaustive() {
    let mut model = Model::new(vec![
        Op::Spawn("a", vec![write("x"), write("x"), write("x")]),
        Op::Spawn("b", vec![write("x"), write("x"), write("x")]),
        Op::Spawn("c", vec![write("x"), write("x"), write("x")]),
        Op::Join(0),
        Op::Join(1),
        Op::Join(2),
    ]);
    let explored = explore(&dpor(4), &mut model);
    assert_eq!(explored.exploration.explored, 4);
    assert!(explored.exploration.exhausted);
    assert!(!explored.exploration.exhaustive);
    assert!(!explored.exploration.is_cacheable());
}

/// The search is itself a function of the seed: two runs of one plan visit the same
/// interleavings in the same order.
#[test]
fn the_search_is_deterministic() {
    let program = || {
        Model::new(vec![
            Op::Spawn("a", vec![Op::Load(1), Op::Store(1), write("x")]),
            Op::Spawn("b", vec![Op::Load(1), Op::Store(1), read("x")]),
            Op::Join(0),
            Op::Join(1),
        ])
    };
    let mut first = program();
    let mut second = program();
    let a = explore(&dpor(64), &mut first);
    let b = explore(&dpor(64), &mut second);
    assert_eq!(a.seeds, b.seeds);
    assert_eq!(a.exploration.explored, b.exploration.explored);
    assert_eq!(a.exploration.failure, b.exploration.failure);
    assert_eq!(first.runs, second.runs);
}

/// Every interleaving the search runs is a distinct seed.
#[test]
fn no_interleaving_is_run_twice() {
    let mut model = Model::new(vec![
        Op::Spawn("a", vec![write("x"), write("y")]),
        Op::Spawn("b", vec![write("x"), write("y")]),
        Op::Spawn("c", vec![write("y")]),
        Op::Join(0),
        Op::Join(1),
        Op::Join(2),
    ]);
    let explored = explore(&dpor(256), &mut model);
    let unique: BTreeSet<&Seed> = explored.seeds.iter().collect();
    assert_eq!(unique.len(), explored.seeds.len());
    assert_eq!(explored.seeds.len(), explored.exploration.explored as usize);
}

/// A scheduler whose replay does not reproduce the recorded enabled set is Ply's fault, and it
/// is caught rather than silently searched over.
#[test]
fn a_replay_that_does_not_reproduce_the_enabled_set_is_a_divergence() {
    let mut calls = 0u32;
    let mut driver = |seed: &Seed| {
        calls += 1;
        let enabled = if seed.is_root() {
            vec![TaskId(0), TaskId(1)]
        } else {
            // The replay offers a different enabled set at the point the seed names, which
            // makes the choice mean something else.
            vec![TaskId(0), TaskId(1), TaskId(2)]
        };
        let write = |resource| {
            StepFootprint::from_accesses([Access::Atom(EffectAtom::new(
                "db",
                Resource::Named(Symbol::new(resource)),
                Mode::Write,
            ))])
        };
        let step = |task: u32, choice: u16, accesses| Step {
            region: SimId(0),
            task: TaskId(task),
            enabled: enabled.clone(),
            choice,
            accesses,
            definition: None,
            span: Span::DUMMY,
            stamp: Stamp::new(),
        };
        Interleaving::passed(vec![
            step(
                seed.choice(0).unwrap_or(0) as u32,
                seed.choice(0).unwrap_or(0),
                write("x"),
            ),
            step(1, 1, write("x")),
        ])
    };
    let explored = explore(&dpor(8), &mut driver);
    let diagnostic = explored.diagnostic.expect("a divergence");
    assert_eq!(diagnostic.code, codes::SIMULATION_DIVERGENCE);
    assert!(explored.exploration.failure.is_some());
}

/// A recording that does not describe a schedule is Ply's fault too, and it is refused before
/// the search draws conclusions from it.
#[test]
fn a_step_that_its_enabled_set_does_not_offer_is_an_internal_error() {
    let mut driver = |_: &Seed| {
        Interleaving::passed(vec![Step {
            region: SimId(0),
            task: TaskId(7),
            enabled: vec![TaskId(0), TaskId(1)],
            choice: 0,
            accesses: StepFootprint::new(),
            definition: None,
            span: Span::DUMMY,
            stamp: Stamp::new(),
        }])
    };
    let explored = explore(&dpor(8), &mut driver);
    assert_eq!(
        explored.diagnostic.expect("a defect").code,
        codes::INTERNAL_ERROR
    );
}

/// `once` is the replay path: exactly the interleaving the seed names, no search, and no claim
/// of exhaustiveness from a sample of one.
#[test]
fn once_runs_exactly_the_interleaving_its_seed_names() {
    let mut model = Model::new(vec![
        Op::Spawn("a", vec![write("x")]),
        Op::Spawn("b", vec![write("x")]),
        Op::Join(0),
        Op::Join(1),
    ]);
    let seed = Seed::at(3, vec![0, 0, 1]);
    let explored = explore(&Plan::once(seed.clone()), &mut model);
    assert_eq!(explored.seeds, vec![seed]);
    assert_eq!(explored.exploration.explored, 1);
    assert!(!explored.exploration.exhaustive);
    assert!(!explored.exploration.exhausted);
    assert!(explored.exploration.is_cacheable());
}

/// `random` is one interleaving per root and no state between them.
#[test]
fn random_runs_one_interleaving_per_root() {
    let mut model = Model::new(vec![
        Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
        Op::Spawn("b", vec![Op::Load(1), Op::Store(1)]),
        Op::Join(0),
        Op::Join(1),
    ])
    .expecting(&[(1, 2)]);
    let explored = explore(&Plan::random(16), &mut model);
    assert!(explored.exploration.explored <= 16);
    assert!(!explored.exploration.exhaustive);
    // A sample that happens to find the race reports no race pair, because nothing flipped:
    // there was no earlier passing interleaving to flip.
    assert_eq!(explored.exploration.race, None);
}

/// A pruned search that passes where the unpruned one fails means the relation missed an
/// access.
#[test]
fn a_failure_only_the_unpruned_search_reaches_is_reported() {
    // A driver that lies: every step reports an empty footprint, so the exact relation prunes
    // everything, while the program's outcome really does depend on the order.
    struct Liar(Model);
    impl Simulation for Liar {
        fn run(&mut self, seed: &Seed) -> Interleaving {
            let mut run = self.0.run(seed);
            for step in &mut run.steps {
                step.accesses = StepFootprint::new();
            }
            run
        }
    }
    let mut liar = Liar(
        Model::new(vec![
            Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
            Op::Spawn("b", vec![Op::Load(1), Op::Store(1)]),
            Op::Join(0),
            Op::Join(1),
        ])
        .expecting(&[(1, 2)]),
    );
    let honest = explore(&dpor(256), &mut liar);
    assert!(honest.passed(), "an empty footprint prunes everything");

    let mut liar = Liar(
        Model::new(vec![
            Op::Spawn("a", vec![Op::Load(1), Op::Store(1)]),
            Op::Spawn("b", vec![Op::Load(1), Op::Store(1)]),
            Op::Join(0),
            Op::Join(1),
        ])
        .expecting(&[(1, 2)]),
    );
    let measured = measure_reduction(&dpor(256), &mut liar);
    assert!(!measured.passed(), "the unpruned search reaches it");
    let diagnostic = measured.diagnostic.expect("a diagnostic");
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|n| n.contains("missing an access")),
        "the report must say the relation was wrong, not that the program is fine"
    );
}

/// A rule about how a type is *used* is a rule nobody enforces; a rule about which types may be
/// *named* is greppable.
#[test]
fn this_module_names_no_hash_based_collection_and_reads_no_clock() {
    let body = include_str!("../../../ply-eval/src/explore.rs");
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
    ] {
        assert!(
            !body.contains(banned),
            "`{banned}` appears in ply_eval::explore; which interleaving runs next must be a \
             function of the definitions and the seed and nothing else"
        );
    }
}

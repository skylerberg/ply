use ply_eval::Value;
use ply_eval::arena::Slot;
use ply_eval::sim::*;
use ply_span::{Span, Symbol, codes};
use ply_ty::EffectAtom;
use ply_ty::Mode;

#[test]
fn a_seed_round_trips_through_its_text_form() {
    for text in ["0", "7", "18446744073709551615", "7:3", "0:1.0.2"] {
        let seed = Seed::parse(text).expect("parses");
        assert_eq!(seed.to_string(), text, "{text} did not round-trip");
    }
}

#[test]
fn hexadecimal_roots_parse_and_print_as_decimal() {
    assert_eq!(Seed::parse("0xff"), Some(Seed::root(255)));
    assert_eq!(Seed::parse("0xFF:1"), Some(Seed::at(255, vec![1])));
}

/// A seed that parses loosely replays something other than what failed.
#[test]
fn everything_else_is_rejected() {
    for text in [
        "", "-1", "7:", ":3", "7:a", "7.3", "0x", "1_000", " 7", "7 ",
    ] {
        assert_eq!(Seed::parse(text), None, "`{text}` should not parse");
    }
}

#[test]
fn canonical_bytes_distinguish_a_path_from_a_longer_root() {
    // `7:1` and `7` differ, and no length prefix ambiguity can make the path of one seed look
    // like the root of another.
    assert_ne!(Seed::root(7).to_bytes(), Seed::at(7, vec![1]).to_bytes());
    assert_ne!(
        Seed::at(7, vec![1, 0]).to_bytes(),
        Seed::at(7, vec![1]).to_bytes()
    );
}

#[test]
fn branching_keeps_the_prefix_and_forgets_the_suffix() {
    let seed = Seed::at(3, vec![1, 2, 3, 4]);
    assert_eq!(seed.branch(2, 9), Seed::at(3, vec![1, 2, 9]));
    assert_eq!(seed.branch(0, 5), Seed::at(3, vec![5]));
    // Branching past the recorded path pads rather than panicking: a search may reach a
    // scheduling point the prefix never named, and the choice has to land at that index.
    let branched = seed.branch(6, 1);
    assert_eq!(branched.choice(6), Some(1));
    assert_eq!(branched.path.len(), 7);
}

#[test]
fn the_two_streams_are_independent() {
    let sched: Vec<u64> = (0..8).map(|i| Stream::draw(11, Domain::Sched, i)).collect();
    let rand: Vec<u64> = (0..8).map(|i| Stream::draw(11, Domain::Rand, i)).collect();
    assert_ne!(sched, rand);
    // Serving the rand stream must not disturb the sched stream, which is what stops a new
    // `random.next()` call from shifting the interleaving.
    let mut a = Stream::new(11, Domain::Sched);
    let mut r = Stream::new(11, Domain::Rand);
    let mut b = Stream::new(11, Domain::Sched);
    for expected in &sched {
        assert_eq!(a.next_u64(), *expected);
        r.next_u64();
        assert_eq!(b.next_u64(), *expected);
    }
}

#[test]
fn a_draw_is_a_function_of_root_domain_and_counter_only() {
    let mut s = Stream::new(42, Domain::Sched);
    for i in 0..16 {
        assert_eq!(s.next_u64(), Stream::draw(42, Domain::Sched, i));
    }
    assert_eq!(s.drawn(), 16);
}

#[test]
fn below_is_in_range_and_refuses_a_zero_bound() {
    let mut s = Stream::new(5, Domain::Rand);
    assert_eq!(s.below(0), None);
    assert_eq!(s.below(1), Some(0));
    for _ in 0..1000 {
        assert!(s.below(7).expect("nonzero bound") < 7);
    }
}

/// Not a statistical test — a distribution check that fails one run in a thousand is exactly
/// the flake this project exists to delete.
#[test]
fn below_rejects_only_above_the_limit() {
    let n = 3u64;
    let limit = (u64::MAX / n) * n;
    let mut counter = 0u64;
    let mut expected = None;
    while expected.is_none() {
        let x = Stream::draw(9, Domain::Rand, counter);
        counter += 1;
        if x < limit {
            expected = Some(x % n);
        }
    }
    let mut s = Stream::new(9, Domain::Rand);
    assert_eq!(s.below(n), expected);
    assert_eq!(s.drawn(), counter);
}

fn atom(effect: &str, resource: Option<&str>, mode: Mode) -> EffectAtom {
    use ply_ty::Resource;
    EffectAtom::new(
        effect,
        resource
            .map(|r| Resource::Named(Symbol::new(r)))
            .unwrap_or(Resource::Singleton),
        mode,
    )
}

#[test]
fn two_reads_of_one_resource_commute() {
    let a = StepFootprint::from_accesses([Access::Atom(atom("db", Some("u"), Mode::Read))]);
    let b = StepFootprint::from_accesses([Access::Atom(atom("db", Some("u"), Mode::Read))]);
    assert!(!a.conflicts_with(&b));
}

#[test]
fn a_write_does_not_commute_with_a_read_of_the_same_resource() {
    let r = StepFootprint::from_accesses([Access::Atom(atom("db", Some("u"), Mode::Read))]);
    let w = StepFootprint::from_accesses([Access::Atom(atom("db", Some("u"), Mode::Write))]);
    assert!(r.conflicts_with(&w));
    assert!(w.conflicts_with(&r));
    assert_eq!(r.contention(&w).len(), 1);
}

/// The relation is at cell granularity, not label granularity: two cells allocated under one
/// `with_cell[users]` are two locations.
#[test]
fn two_cells_are_two_locations_whatever_they_were_labelled() {
    let one = StepFootprint::from_accesses([Access::Cell {
        id: Slot::new(1, 0),
        mode: Mode::Write,
    }]);
    let two = StepFootprint::from_accesses([Access::Cell {
        id: Slot::new(2, 0),
        mode: Mode::Write,
    }]);
    let also_one = StepFootprint::from_accesses([Access::Cell {
        id: Slot::new(1, 0),
        mode: Mode::Read,
    }]);
    assert!(!one.conflicts_with(&two));
    assert!(one.conflicts_with(&also_one));
}

/// The mistake the dependence relation exists to prevent: two tasks share one world, so a `cell` access
/// is part of the dependence relation.
#[test]
fn cell_accesses_are_in_the_relation() {
    let write = StepFootprint::from_accesses([Access::Cell {
        id: Slot::new(7, 0),
        mode: Mode::Write,
    }]);
    assert!(write.conflicts_with(&write));
    assert!(!write.is_empty());
}

#[test]
fn an_atom_and_a_cell_name_disjoint_state() {
    let atoms = StepFootprint::from_accesses([Access::Atom(atom("cell", Some("u"), Mode::Write))]);
    let cells = StepFootprint::from_accesses([Access::Cell {
        id: Slot::new(0, 0),
        mode: Mode::Write,
    }]);
    assert!(!atoms.conflicts_with(&cells));
}

#[test]
fn the_empty_step_commutes_with_everything() {
    let empty = StepFootprint::new();
    let w = StepFootprint::from_accesses([Access::Atom(atom("db", Some("u"), Mode::Write))]);
    assert!(!empty.conflicts_with(&w));
    assert!(!w.conflicts_with(&empty));
}

#[test]
fn a_plan_digest_ignores_the_order_roots_were_written_in() {
    let a = Plan {
        roots: vec![3, 1, 1, 2],
        ..Plan::default()
    };
    let b = Plan {
        roots: vec![1, 2, 3],
        ..Plan::default()
    };
    assert_eq!(a.digest(), b.digest());
}

#[test]
fn every_plan_field_reaches_the_digest() {
    let base = Plan::default();
    let variants = [
        Plan {
            mode: SimMode::Random,
            ..base.clone()
        },
        Plan {
            roots: vec![0, 1],
            ..base.clone()
        },
        Plan {
            budget: base.budget + 1,
            ..base.clone()
        },
        Plan {
            steps: base.steps + 1,
            ..base.clone()
        },
        Plan::once(Seed::at(0, vec![1])),
        Plan::once(Seed::at(0, vec![2])),
    ];
    let mut seen = vec![base.digest()];
    for plan in variants {
        let digest = plan.digest();
        assert!(
            !seen.contains(&digest),
            "{plan:?} collided with an earlier plan"
        );
        seen.push(digest);
    }
}

#[test]
fn normalization_drops_a_path_outside_once() {
    let plan = Plan {
        mode: SimMode::Dpor,
        path: vec![1, 2],
        ..Plan::default()
    }
    .normalized();
    assert!(plan.path.is_empty());
    assert_eq!(Plan::once(Seed::at(4, vec![1])).normalized().path, vec![1]);
}

#[test]
fn a_once_plan_names_exactly_the_seed_it_replays() {
    let seed = Seed::at(9, vec![0, 3]);
    assert_eq!(Plan::once(seed.clone()).seeds(), vec![seed]);
}

/// An exhausted search proved nothing about the interleavings it did not reach, so its green
/// verdict may not be cached — the first green `det` test in the language that is not
/// cacheable, and correctly so.
#[test]
fn an_exhausted_search_is_not_cacheable() {
    let exhausted = Exploration {
        explored: 256,
        exhausted: true,
        ..Exploration::default()
    };
    assert!(!exhausted.is_cacheable());

    let complete = Exploration {
        explored: 12,
        exhaustive: true,
        ..Exploration::default()
    };
    assert!(complete.is_cacheable());

    let failed = Exploration {
        explored: 4,
        exhaustive: true,
        failure: Some(Seed::root(0)),
        ..Exploration::default()
    };
    assert!(!failed.is_cacheable());
}

#[test]
fn a_bounded_naive_count_renders_as_a_lower_bound() {
    assert_eq!(
        Naive {
            explored: 4096,
            bounded: true
        }
        .to_string(),
        ">= 4096"
    );
    assert_eq!(
        Naive {
            explored: 720,
            bounded: false
        }
        .to_string(),
        "720"
    );
}

#[test]
fn reduction_is_none_until_it_is_measured() {
    let mut e = Exploration {
        explored: 12,
        ..Exploration::default()
    };
    assert_eq!(e.reduction(), None);
    e.naive = Some(Naive {
        explored: 720,
        bounded: false,
    });
    assert_eq!(e.reduction(), Some(60.0));
}

fn span() -> Span {
    Span::new(ply_span::SourceId(0), 12, 20)
}

fn sig(effect: &str, op: &str) -> &'static OpSignature {
    signature(effect, op).expect("a seeded operation")
}

/// Every answer a region delivered, rendered.
fn transcript(answers: &[Answer]) -> Vec<String> {
    answers
        .iter()
        .map(|answer| match answer {
            Answer::Value(v) => v.render(),
            Answer::Sleeping { deadline } => format!("sleeping until {deadline}"),
        })
        .collect()
}

fn run(root: u64, script: &[(&str, &str, Vec<Value>)]) -> (Vec<Answer>, i64, u64) {
    let mut handlers = Handlers::new(root);
    let answers: Vec<Answer> = script
        .iter()
        .enumerate()
        .map(|(i, (effect, op, args))| {
            handlers
                .dispatch(sig(effect, op), TaskId(i as u32), args, span())
                .expect("a well-typed request")
        })
        .collect();
    let now = handlers.clock().now();
    let drawn = handlers.rand().drawn();
    (answers, now, drawn)
}

fn script() -> Vec<(&'static str, &'static str, Vec<Value>)> {
    vec![
        ("clock", "now", vec![]),
        ("random", "next", vec![]),
        ("random", "below", vec![Value::Int(6)]),
        ("clock", "sleep", vec![Value::Int(0)]),
        ("random", "next", vec![]),
        ("clock", "now", vec![]),
        ("clock", "sleep", vec![Value::Int(500)]),
    ]
}

#[test]
fn one_seed_answers_one_sequence() {
    let (first, now, drawn) = run(7, &script());
    let (again, now_again, drawn_again) = run(7, &script());
    assert_eq!(transcript(&first), transcript(&again));
    assert_eq!((now, drawn), (now_again, drawn_again));
    assert_eq!(drawn, 3, "one draw per `random` request, and no others");
}

#[test]
fn another_seed_answers_another_sequence() {
    let (a, _, _) = run(7, &script());
    let (b, _, _) = run(8, &script());
    assert_ne!(transcript(&a), transcript(&b));
}

/// There is no `clock` stream.
#[test]
fn the_clock_is_not_drawn_from_the_seed() {
    let clock_of = |root| {
        let mut handlers = Handlers::new(root);
        let mut times = Vec::new();
        for nanos in [0, 40, 0] {
            let answer = handlers
                .dispatch(
                    sig("clock", "sleep"),
                    TaskId(0),
                    &[Value::Int(nanos)],
                    span(),
                )
                .expect("a well-typed sleep");
            times.push(transcript(&[answer]).remove(0));
            handlers.clock_mut().advance();
        }
        (times, handlers.clock().now())
    };
    assert_eq!(clock_of(1), clock_of(999_999));
}

/// The whole of "virtual time advances only via the scheduler": `now()` observes, `sleep`
/// schedules, a draw is a draw, and none of them moves it.
#[test]
fn nothing_a_task_performs_moves_virtual_time() {
    let mut handlers = Handlers::new(3);
    for (effect, op, args) in script() {
        handlers
            .dispatch(sig(effect, op), TaskId(0), &args, span())
            .expect("a well-typed request");
        assert_eq!(handlers.clock().now(), 0, "`{effect}.{op}` moved the clock");
    }
    let wake = handlers.clock_mut().advance().expect("a pending timer");
    assert_eq!(wake.now, 500);
    assert_eq!(handlers.clock().now(), 500);
}

#[test]
fn sleeping_for_no_time_is_a_yield() {
    let mut clock = Clock::new();
    for nanos in [0, -1, i64::MIN] {
        let slept = clock.sleep(TaskId(0), nanos, span()).expect("a yield");
        assert_eq!(slept, Sleep::Yield);
    }
    assert_eq!(clock.sleepers(), 0);
    assert_eq!(clock.next_deadline(), None);
}

/// Thirty simulated seconds are a jump, not thirty seconds: nothing here waits on anything, so
/// the only cost of a long sleep is the arithmetic.
#[test]
fn a_long_sleep_advances_exactly_that_far() {
    let mut clock = Clock::new();
    let slept = clock
        .sleep(TaskId(1), 30_000_000_000, span())
        .expect("a valid sleep");
    assert_eq!(slept, Sleep::Until(30_000_000_000));
    assert_eq!(clock.now(), 0);
    let wake = clock.advance().expect("a pending timer");
    assert_eq!(wake.now, 30_000_000_000);
    assert_eq!(wake.woken, vec![TaskId(1)]);
    assert_eq!(clock.now(), 30_000_000_000);
    assert!(!clock.is_sleeping(TaskId(1)));
}

/// Sleeps are relative to the time the sleeping task observed, so a task that sleeps twice for
/// 40ns wakes at 80ns rather than at 40ns again.
#[test]
fn a_deadline_is_measured_from_the_time_the_task_saw() {
    let mut clock = Clock::new();
    clock.sleep(TaskId(0), 40, span()).expect("a valid sleep");
    clock.advance();
    let slept = clock.sleep(TaskId(0), 40, span()).expect("a valid sleep");
    assert_eq!(slept, Sleep::Until(80));
    assert_eq!(clock.advance().map(|w| w.now), Some(80));
}

#[test]
fn tasks_sharing_a_deadline_wake_together_and_in_task_order() {
    let mut clock = Clock::new();
    for task in [TaskId(3), TaskId(1), TaskId(2)] {
        clock.sleep(task, 10, span()).expect("a valid sleep");
    }
    clock.sleep(TaskId(4), 20, span()).expect("a valid sleep");
    let wake = clock.advance().expect("a pending timer");
    assert_eq!(wake.now, 10);
    assert_eq!(wake.woken, vec![TaskId(1), TaskId(2), TaskId(3)]);
    assert_eq!(clock.deadline_of(TaskId(4)), Some(20));
}

/// A timeout is `clock.sleep` racing something else, and the earliest deadline is the only one
/// that can fire.
#[test]
fn a_timeout_fires_at_its_deadline_and_never_before_a_nearer_one() {
    let mut clock = Clock::new();
    let timeout = TaskId(0);
    let work = TaskId(1);
    clock
        .sleep(timeout, 5_000_000_000, span())
        .expect("a valid sleep");
    for _ in 0..3 {
        clock
            .sleep(work, 100_000_000, span())
            .expect("a valid sleep");
        let wake = clock.advance().expect("a pending timer");
        assert_eq!(wake.woken, vec![work], "the timeout fired early");
    }
    assert_eq!(clock.now(), 300_000_000);
    let wake = clock.advance().expect("the timeout is still pending");
    assert_eq!(wake.now, 5_000_000_000);
    assert_eq!(wake.woken, vec![timeout]);
}

/// With nothing enabled and no timer pending the region is stuck, which the scheduler reports
/// as `E0414`.
#[test]
fn no_timer_means_no_advance() {
    let mut clock = Clock::new();
    assert_eq!(clock.advance(), None);
    clock.sleep(TaskId(0), 5, span()).expect("a valid sleep");
    assert!(clock.advance().is_some());
    assert_eq!(clock.advance(), None);
    assert_eq!(clock.now(), 5, "a refused advance left time alone");
}

#[test]
fn a_sleep_past_the_end_of_virtual_time_is_a_diagnostic() {
    let mut clock = Clock::new();
    clock
        .sleep(TaskId(0), i64::MAX, span())
        .expect("a valid sleep");
    clock.advance();
    let err = clock
        .sleep(TaskId(0), i64::MAX, span())
        .expect_err("the deadline overflows");
    assert_eq!(err.code, codes::RUNTIME_ERROR);
    assert_eq!(err.labels[0].span, span());
}

#[test]
fn a_bound_of_zero_or_less_is_a_runtime_error_with_a_real_span() {
    let mut rand = Rand::new(2);
    for bound in [0, -1, i64::MIN] {
        let err = rand.below(bound, span()).expect_err("an empty range");
        assert_eq!(err.code, codes::RUNTIME_ERROR);
        assert_eq!(err.labels[0].span, span());
    }
    assert_eq!(rand.drawn(), 0, "a refused bound drew nothing");
    assert!((0..6).contains(&rand.below(6, span()).expect("a valid bound")));
}

#[test]
fn a_draw_uses_the_rand_stream_and_the_whole_range_of_an_int() {
    let mut rand = Rand::new(4);
    let mut stream = Stream::new(4, Domain::Rand);
    for _ in 0..64 {
        assert_eq!(rand.next_int(), stream.next_u64() as i64);
    }
    let mut negatives = 0;
    for _ in 0..64 {
        if rand.next_int() < 0 {
            negatives += 1;
        }
    }
    assert!(negatives > 0, "`random.next` answers the whole of `Int`");
}

/// Excluding the terminating scheduler op is what leaves a reduction to measure; keeping
/// `random.write` is what stops the search from calling two draws commutative when the program
/// can tell them apart.
#[test]
fn only_a_draw_is_an_access_of_the_step_it_ends() {
    for sig in SEEDED_OPS {
        let access = sig.step_access();
        match sig.effect {
            "clock" => assert!(access.is_none(), "`{sig}` is scheduler bookkeeping"),
            _ => assert_eq!(access, Some(Access::Atom(sig.atom()))),
        }
    }
    let draw = StepFootprint::from_accesses(sig("random", "next").step_access());
    assert!(draw.conflicts_with(&draw));
}

#[test]
fn the_table_names_each_operation_once_and_answers_all_of_them() {
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for sig in SEEDED_OPS {
        assert!(
            !seen.contains(&(sig.effect, sig.op)),
            "`{sig}` is in the table twice"
        );
        seen.push((sig.effect, sig.op));
        assert!(SEEDED_EFFECTS.contains(&sig.effect));

        let args: Vec<Value> = sig.params.iter().map(|_| Value::Int(1)).collect();
        let answer = Handlers::new(0)
            .dispatch(sig, TaskId(0), &args, span())
            .expect("the table's own arguments are well typed");
        match answer {
            // A woken sleeper resumes with `clock.sleep`'s declared return.
            Answer::Sleeping { .. } => assert_eq!(sig.ret, SimTy::Unit),
            Answer::Value(v) => assert!(
                sig.ret.holds(&v),
                "`{sig}` promises {} and answered {}",
                sig.ret.as_str(),
                v.render()
            ),
        }
    }
    assert_eq!(
        signature("task", "spawn"),
        None,
        "`task` is the scheduler's"
    );
    assert_eq!(signature("clock", "tick"), None);
}

#[test]
fn a_miscounted_argument_list_is_a_diagnostic_rather_than_a_panic() {
    let err = Handlers::new(0)
        .dispatch(sig("clock", "sleep"), TaskId(0), &[], span())
        .expect_err("`clock.sleep` takes one argument");
    assert_eq!(err.code, codes::ARITY_MISMATCH);

    let err = Handlers::new(0)
        .dispatch(sig("random", "below"), TaskId(0), &[Value::Unit], span())
        .expect_err("`random.below` takes an `Int`");
    assert_eq!(err.code, codes::RUNTIME_ERROR);
}

/// A sleeping task is not enabled, so the scheduler cannot resume it into a second sleep.
#[test]
fn a_task_cannot_hold_two_timers() {
    let mut clock = Clock::new();
    clock.sleep(TaskId(0), 5, span()).expect("a valid sleep");
    let err = clock
        .sleep(TaskId(0), 5, span())
        .expect_err("already sleeping");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert_eq!(clock.sleepers(), 1);
}

/// A rule about how a type is *used* is a rule nobody enforces; a rule about which types may be
/// *named* is greppable.
#[test]
fn this_module_names_no_hash_based_collection_and_reads_no_clock() {
    let body = include_str!("../../../../ply-eval/src/sim.rs");
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
            "`{banned}` appears in ply_eval::sim; a seeded run must be a \
             function of its definitions and its seed and nothing else"
        );
    }
}

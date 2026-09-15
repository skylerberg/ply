use ply_core::{EffectAtom, Footprint, LawBinder, Resource, Type};
use ply_eval::arena::Slot;
use ply_eval::explore::Step;
use ply_eval::{
    Access, Domain, Interleaving, Plan, Seed, SimId, SimMode, Simulation, StepFootprint, Stream,
    TaskId, explore,
};
use ply_hash::DefHash;
use ply_prove::concurrency::{
    BodyRun, LawSearch, Searched, ValueDomain, audit_interleaving_proof, discharge, refutation,
    replay_command,
};
use ply_prove::key::result_key;
use ply_prove::{
    Binding, Certificate, Discharge, Evidence, Frame, Gap, Obligation, ObligationKind, ProvePlan,
    Rule, Tier, Vacuity, VacuityKind, interleaving_proves,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Mode;

fn body_was_false(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::OBLIGATION_REFUTED,
        "this law does not hold under every interleaving",
    )
    .primary(span, "evaluated to `false` in this interleaving")
}

/// `tasks` tasks, each reading a shared counter and writing it back — the lost update, which is
/// the shape every concurrency law worth writing is about.
struct Model {
    tasks: usize,
    /// What the law claims of the counter when every task has finished.
    claims: fn(i64) -> bool,
    /// Points whose runs reach no `simulate` region.
    unobserved: bool,
    /// Points whose runs raise instead of coming to a Boolean.
    raises: bool,
    /// Every choice sequence this model was asked for, in order.
    traces: Vec<Vec<u16>>,
}

impl Model {
    fn new(tasks: usize, claims: fn(i64) -> bool) -> Model {
        Model {
            tasks,
            claims,
            unobserved: false,
            raises: false,
            traces: Vec::new(),
        }
    }

    fn interleave(&mut self, seed: &Seed) -> Interleaving {
        let mut sched = Stream::new(seed.root, Domain::Sched);
        let mut pc = vec![0usize; self.tasks];
        let mut register = vec![0i64; self.tasks];
        let mut counter = 0i64;
        let mut steps = Vec::new();
        let mut taken = Vec::new();

        loop {
            let enabled: Vec<TaskId> = (0..self.tasks)
                .filter(|&t| pc[t] < 2)
                .map(|t| TaskId(t as u32))
                .collect();
            if enabled.is_empty() {
                break;
            }
            let choice = match seed.choice(steps.len()) {
                Some(fixed) => usize::from(fixed).min(enabled.len() - 1),
                None => sched.below(enabled.len() as u64).unwrap_or(0) as usize,
            };
            let task = enabled[choice];
            let t = task.0 as usize;
            let mode = if pc[t] == 0 {
                register[t] = counter;
                Mode::Read
            } else {
                counter = register[t] + 1;
                Mode::Write
            };
            pc[t] += 1;
            taken.push(choice as u16);
            steps.push(Step {
                region: SimId(0),
                task,
                enabled,
                choice: choice as u16,
                accesses: StepFootprint::from_accesses([Access::Cell {
                    id: Slot::new(0, 0),
                    mode,
                }]),
                definition: Some(Symbol::new("transfer")),
                span: Span::DUMMY,
                // No synchronization at all, so no pair is ordered and every dependent pair is
                // a candidate.
                stamp: Vec::new(),
            });
        }

        self.traces.push(taken);
        if (self.claims)(counter) {
            Interleaving::passed(steps)
        } else {
            Interleaving::failed(steps, body_was_false(Span::DUMMY))
        }
    }
}

impl LawSearch for Model {
    fn run(&mut self, _point: u64, seed: &Seed) -> BodyRun {
        if self.raises {
            return BodyRun::model(
                Interleaving::failed(
                    Vec::new(),
                    Diagnostic::error(codes::RUNTIME_ERROR, "divided by zero"),
                ),
                false,
                true,
            );
        }
        if self.unobserved {
            // What a body that never reaches a `simulate` region hands the search: a run with
            // no steps and the body's own verdict.
            return BodyRun::model(Interleaving::passed(Vec::new()), false, false);
        }
        BodyRun::model(self.interleave(seed), true, false)
    }

    fn bindings(&self, point: u64) -> Vec<Binding> {
        vec![Binding {
            name: Symbol::new("n"),
            ty: Type::int(),
            rendered: point.to_string(),
        }]
    }
}

fn law(binders: usize) -> Obligation {
    Obligation {
        key: DefHash([3; 32]),
        owner: Symbol::new("bank.transfers conserve value"),
        kind: ObligationKind::Law,
        span: Span::DUMMY,
        frame: Frame::Pure,
        binders: (0..binders)
            .map(|i| LawBinder {
                name: Symbol::new(format!("n{i}")),
                ty: Type::int(),
                span: Span::DUMMY,
            })
            .collect(),
        guarded: false,
        host: false,
        footprint: Footprint::from_atoms([EffectAtom::new("sim", Resource::Singleton, Mode::Read)]),
    }
}

fn dpor(budget: u32) -> Plan {
    Plan {
        budget,
        ..Plan::default()
    }
}

fn certificate(searched: &Searched) -> &Certificate {
    match &searched.discharge {
        Discharge::Held(Evidence::Proof(c)) => c,
        other => panic!("expected a proof, got {other:?}"),
    }
}

/// A law that holds under **every** interleaving, discharged as a proof.
#[test]
fn a_law_that_holds_under_every_interleaving_is_proved() {
    let obligation = law(0);
    let mut model = Model::new(2, |counter| counter <= 2);
    let searched = discharge(&obligation, &dpor(64), &ValueDomain::ground(), &mut model);

    assert_eq!(searched.discharge.tier(), Some(Tier::Proved));
    assert!(searched.exhaustive);
    assert!(!searched.exhausted);
    assert!(searched.observed);
    assert!(searched.interleavings > 1, "the search found real choices");

    let certificate = certificate(&searched);
    assert_eq!(
        certificate.rules,
        vec![Rule::ExhaustiveInterleaving {
            interleavings: searched.interleavings
        }],
        "a ground law's certificate names the interleaving search and nothing else"
    );
    assert!(certificate.guard_satisfiable);
    assert!(certificate.sorts.is_empty());
    assert_eq!(audit_interleaving_proof(&obligation, certificate), Ok(()));
}

/// A law that holds under *some* interleavings: a failure carrying the seed that reproduces it.
#[test]
fn a_law_that_holds_only_sometimes_is_refuted_with_a_seed() {
    let obligation = law(0);
    // The lost update: two tasks that each read the counter and write it back reach 1 in the
    // interleavings where the reads precede both writes.
    let mut model = Model::new(2, |counter| counter == 2);
    let searched = discharge(&obligation, &dpor(64), &ValueDomain::ground(), &mut model);

    let Discharge::Refuted(counterexample) = &searched.discharge else {
        panic!("expected a refutation, got {:?}", searched.discharge);
    };
    assert_eq!(searched.discharge.tier(), None);
    let seed = counterexample
        .sim_seed
        .as_ref()
        .expect("a concurrency failure names the interleaving that produced it");
    assert!(
        !seed.is_root(),
        "the failing interleaving is a path, not just a root: {seed}"
    );
    assert!(
        counterexample.race.is_some(),
        "the search observed the flip, so it names the pair of steps"
    );
    assert!(
        !searched.exhaustive,
        "a search that stopped at a failure covered nothing"
    );
}

/// A search that spends its budget proved nothing about the interleavings it did not reach, so
/// it reports the sampled tier and says how many it ran.
#[test]
fn a_search_that_spends_its_budget_is_property_and_says_how_many() {
    let obligation = law(0);
    let mut model = Model::new(3, |counter| counter <= 3);
    let searched = discharge(&obligation, &dpor(30), &ValueDomain::ground(), &mut model);

    assert_eq!(searched.discharge.tier(), Some(Tier::Property));
    assert!(searched.exhausted);
    assert!(!searched.exhaustive);
    let Discharge::Held(Evidence::Cases(report)) = &searched.discharge else {
        panic!("expected sampled evidence, got {:?}", searched.discharge);
    };
    assert_eq!(report.kept, 30, "the count is the interleavings it ran");
    assert_eq!(report.kept, searched.evaluations);
    assert!(searched.line().unwrap().contains("budget spent"));
}

/// A spent budget is a claim about the plan that spent it, so it may never be read back under a
/// wider one.
#[test]
fn a_spent_budget_is_not_written_under_the_bare_key() {
    let obligation = law(0);
    let mut model = Model::new(3, |counter| counter <= 3);
    let searched = discharge(&obligation, &dpor(30), &ValueDomain::ground(), &mut model);
    let plan = ProvePlan::default();
    assert_ne!(
        result_key(obligation.key, searched.discharge.tier(), &plan),
        obligation.key
    );
}

/// The artifact is M7's: `--seed` replays the interleaving exactly, and the replay refutes the
/// same law with the same seed and the same trace.
#[test]
fn a_reported_failure_replays_exactly() {
    let obligation = law(0);
    let mut search = Model::new(2, |counter| counter == 2);
    let found = discharge(&obligation, &dpor(64), &ValueDomain::ground(), &mut search);
    let Discharge::Refuted(counterexample) = &found.discharge else {
        panic!("expected a refutation");
    };
    let seed = counterexample.sim_seed.clone().unwrap();
    let failing_trace = search.traces.last().cloned().unwrap();

    let mut replay = Model::new(2, |counter| counter == 2);
    let again = discharge(
        &obligation,
        &Plan::once(seed.clone()),
        &ValueDomain::ground(),
        &mut replay,
    );
    let Discharge::Refuted(replayed) = &again.discharge else {
        panic!(
            "the replay must reproduce the refutation, got {:?}",
            again.discharge
        );
    };
    assert_eq!(replayed.sim_seed.as_ref(), Some(&seed));
    assert_eq!(
        replay.traces,
        vec![failing_trace],
        "byte-for-byte the same interleaving"
    );
    // `once` observes no flip, so it invents no race — the seed is the exact half and the pair
    // is the half the search happened to see.
    assert!(replayed.race.is_none());
    assert_eq!(again.discharge.tier(), None);
}

/// The concurrency-law conditions's condition 5, and the required test that goes with it: the same law with a
/// binder is `property` however exhaustive the schedules were.
#[test]
fn an_exhaustive_search_over_sampled_values_is_never_proved() {
    let obligation = law(1);
    let mut model = Model::new(2, |counter| counter <= 2);
    let searched = discharge(
        &obligation,
        &dpor(64),
        &ValueDomain::Sampled {
            generated: 200,
            kept: 200,
            rejected: 0,
            instantiations: Vec::new(),
        },
        &mut model,
    );
    assert!(searched.exhaustive, "every point's frontier emptied");
    assert_eq!(
        searched.discharge.tier(),
        Some(Tier::Property),
        "exhaustive over schedules says nothing about the values that were sampled"
    );
}

/// The same law over a domain that *was* covered is proved, and its certificate names both
/// coverage claims so an audit can check condition 5 without re-deriving it.
#[test]
fn an_enumerated_value_domain_proves_and_names_its_enumeration() {
    let obligation = law(1);
    let mut model = Model::new(2, |counter| counter <= 2);
    let searched = discharge(
        &obligation,
        &dpor(64),
        &ValueDomain::Enumerated {
            domain: Symbol::new("Bool"),
            points: 2,
            kept: 2,
        },
        &mut model,
    );
    let certificate = certificate(&searched);
    assert!(certificate.rules.contains(&Rule::ExhaustiveEnumeration {
        domain: Symbol::new("Bool"),
        points: 2,
    }));
    assert!(
        certificate
            .rules
            .iter()
            .any(|r| matches!(r, Rule::ExhaustiveInterleaving { .. }))
    );
    assert_eq!(audit_interleaving_proof(&obligation, certificate), Ok(()));
    assert_eq!(searched.points, 2);
}

/// The audit catches a certificate that claims an exhaustive search over a law whose values
/// nobody covered.
#[test]
fn the_audit_rejects_an_interleaving_proof_that_covered_no_value_domain() {
    let forged = Certificate {
        rules: vec![Rule::ExhaustiveInterleaving { interleavings: 12 }],
        steps: 12,
        guard_satisfiable: true,
        sorts: Vec::new(),
    };
    assert!(audit_interleaving_proof(&law(1), &forged).is_err());
    assert_eq!(audit_interleaving_proof(&law(0), &forged), Ok(()));

    let empty = Certificate {
        rules: vec![Rule::ExhaustiveInterleaving { interleavings: 0 }],
        ..forged.clone()
    };
    assert!(audit_interleaving_proof(&law(0), &empty).is_err());

    let unguarded = Certificate {
        guard_satisfiable: false,
        ..forged
    };
    assert!(audit_interleaving_proof(&law(0), &unguarded).is_err());
}

/// **The sixth condition.**
#[test]
fn a_search_that_reached_no_region_is_exhaustive_over_nothing() {
    let plan = dpor(64);
    let mut nothing = Model::new(2, |_| true);
    nothing.unobserved = true;

    // What `Exploration` alone says, which is the overclaim: the search driven by the bare runs,
    // without the region check `discharge` adds.
    struct Probe<'a>(&'a mut Model);
    impl Simulation for Probe<'_> {
        fn run(&mut self, seed: &Seed) -> Interleaving {
            self.0.run(0, seed).interleaving().clone()
        }
    }
    let mut probe = Probe(&mut nothing);
    let explored = explore(&plan, &mut probe);
    assert!(
        explored.exploration.exhaustive,
        "this is the flag M8 is invited to read as a proof"
    );
    assert!(interleaving_proves(&plan, &explored.exploration, true));

    // What this module reports, having asked whether a region ran.
    let searched = discharge(&law(0), &plan, &ValueDomain::ground(), &mut nothing);
    assert!(!searched.observed);
    assert_eq!(searched.interleavings, 0);
    assert_ne!(
        searched.discharge.tier(),
        Some(Tier::Proved),
        "a search that scheduled nothing proves nothing"
    );
    assert!(searched.line().unwrap().contains("no `simulate` region"));
}

/// Under `once` and `random` there is no frontier to empty, so there is nothing exhaustive to
/// claim whatever the run reports.
#[test]
fn a_sampled_plan_never_proves() {
    for plan in [Plan::random(4), Plan::once(Seed::root(7))] {
        assert_ne!(plan.mode, SimMode::Dpor);
        let mut model = Model::new(2, |counter| counter <= 2);
        let searched = discharge(&law(0), &plan, &ValueDomain::ground(), &mut model);
        assert_ne!(searched.discharge.tier(), Some(Tier::Proved));
        assert!(searched.discharge.holds());
    }
}

/// A law that raises is not a law that is false, so a raise is a gap and the raising input is
/// reported rather than presented as a counterexample.
#[test]
fn a_body_that_raises_is_a_gap_and_not_a_refutation() {
    let mut model = Model::new(2, |_| true);
    model.raises = true;
    let searched = discharge(&law(1), &dpor(64), &ValueDomain::ground(), &mut model);
    let Discharge::Unattempted(Gap::Raised {
        bindings,
        diagnostic,
    }) = &searched.discharge
    else {
        panic!("expected a gap, got {:?}", searched.discharge);
    };
    assert_eq!(diagnostic.code, codes::RUNTIME_ERROR);
    assert_eq!(bindings.len(), 1);
}

/// A guard that admits nothing makes the obligation trivially valid and therefore silent.
#[test]
fn a_domain_the_guard_emptied_is_vacuous_and_not_proved() {
    let mut model = Model::new(2, |_| true);
    let enumerated = discharge(
        &law(1),
        &dpor(64),
        &ValueDomain::Enumerated {
            domain: Symbol::new("Bool"),
            points: 2,
            kept: 0,
        },
        &mut model,
    );
    assert!(matches!(
        &enumerated.discharge,
        Discharge::Vacuous(Vacuity {
            kind: VacuityKind::ProvedUnsatisfiable,
            ..
        })
    ));
    assert_eq!(enumerated.discharge.tier(), None);

    let sampled = discharge(
        &law(1),
        &dpor(64),
        &ValueDomain::Sampled {
            generated: 200,
            kept: 0,
            rejected: 200,
            instantiations: Vec::new(),
        },
        &mut model,
    );
    assert!(matches!(
        &sampled.discharge,
        Discharge::Vacuous(Vacuity {
            kind: VacuityKind::NoCaseKept { generated: 200 },
            ..
        })
    ));
}

/// One point of a law's domain whose search spent its budget is a law whose search spent its
/// budget: a claim about every value is only as strong as its weakest point.
#[test]
fn one_unexhausted_point_costs_the_whole_law_its_proof() {
    struct Mixed {
        inner: Model,
        /// The point whose search is given no room.
        starved: u64,
    }

    impl LawSearch for Mixed {
        fn run(&mut self, point: u64, seed: &Seed) -> BodyRun {
            if point == self.starved && seed.path.len() > 1 {
                // A run the search cannot branch past, standing in for a point whose space is
                // larger than the budget.
                return BodyRun::model(Interleaving::passed(Vec::new()), true, false);
            }
            self.inner.run(point, seed)
        }

        fn bindings(&self, point: u64) -> Vec<Binding> {
            self.inner.bindings(point)
        }
    }

    let mut mixed = Mixed {
        inner: Model::new(3, |counter| counter <= 3),
        starved: 1,
    };
    let searched = discharge(
        &law(1),
        &dpor(20),
        &ValueDomain::Enumerated {
            domain: Symbol::new("Bool"),
            points: 2,
            kept: 2,
        },
        &mut mixed,
    );
    assert!(searched.exhausted);
    assert!(!searched.exhaustive);
    assert_ne!(searched.discharge.tier(), Some(Tier::Proved));
}

/// Two runs over one law produce one artifact — the same tier, the same counts, the same seed.
#[test]
fn two_runs_over_one_law_agree() {
    let run = || {
        let mut model = Model::new(3, |counter| counter == 3);
        let searched = discharge(&law(0), &dpor(64), &ValueDomain::ground(), &mut model);
        let seed = match &searched.discharge {
            Discharge::Refuted(c) => c.sim_seed.clone(),
            other => panic!("expected a refutation, got {other:?}"),
        };
        (seed, searched.interleavings, model.traces)
    };
    assert_eq!(run(), run());
}

#[test]
fn the_replay_command_is_the_command() {
    assert_eq!(
        replay_command(&Seed::at(0, vec![1, 0, 3]), "transfers conserve value"),
        "ply prove --seed 0:1.0.3 --filter \"transfers conserve value\""
    );
}

/// The failure artifact carries what M7's carries: the seed, the race sites, and the command
/// that replays it.
#[test]
fn a_refutation_reports_the_seed_the_race_and_the_replay() {
    let obligation = law(1);
    let mut model = Model::new(2, |counter| counter == 2);
    let searched = discharge(
        &obligation,
        &dpor(64),
        &ValueDomain::Sampled {
            generated: 4,
            kept: 4,
            rejected: 0,
            instantiations: Vec::new(),
        },
        &mut model,
    );
    let Discharge::Refuted(counterexample) = &searched.discharge else {
        panic!("expected a refutation");
    };
    let rendered = refutation(
        "transfers conserve value",
        counterexample,
        body_was_false(Span::DUMMY),
    );
    let notes = rendered.notes.join("\n");
    assert!(notes.contains("seed: "));
    assert!(notes.contains("race: "));
    assert!(notes.contains("replay: ply prove --seed "));
    assert!(notes.contains("n = 0"));
}

/// A concurrency law is discharged by execution, and `is_concurrency_law` is what routes it
/// here.
#[test]
fn only_a_law_carrying_sim_read_is_routed_to_a_search() {
    assert!(law(0).is_concurrency_law());
    let pure = Obligation {
        footprint: Footprint::empty(),
        ..law(0)
    };
    assert!(!pure.is_concurrency_law());
}

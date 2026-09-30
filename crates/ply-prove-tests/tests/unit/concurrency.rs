use ply_eval::arena::Slot;
use ply_eval::explore::Step;
use ply_eval::{
    Access, DefHash, Diagnostic, Domain, Interleaving, Mode, Plan, Seed, SimId, SimMode,
    Simulation, Span, StepFootprint, Stream, Symbol, TaskId, codes, explore,
};
use ply_prove::concurrency::{BodyRun, LawSearch, Searched, ValueDomain, discharge};
use ply_prove::{
    Binder, Binding, Certificate, Discharge, Evidence, Gap, Obligation, ObligationKind, Points,
    Rule, Sort, Strategy, Tier, Vacuity, VacuityKind, interleaving_proves,
};

fn body_was_false(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::OBLIGATION_REFUTED,
        "this law does not hold under every interleaving",
    )
    .primary(span, "evaluated to `false` in this interleaving")
}

/// `tasks` tasks, each reading a shared counter and writing it back: the lost update.
struct Model {
    tasks: usize,
    claims: fn(i64) -> bool,
    /// Points whose runs reach no `simulate` region.
    unobserved: bool,
    raises: bool,
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
                .map(|t| TaskId(t as u64))
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
                // No synchronization, so every dependent pair is a candidate.
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
            // A body that never reaches a `simulate` region: no steps and the body's own verdict.
            return BodyRun::model(Interleaving::passed(Vec::new()), false, false);
        }
        BodyRun::model(self.interleave(seed), true, false)
    }

    fn bindings(&self, point: u64) -> Vec<Binding> {
        vec![Binding {
            name: Symbol::new("n"),
            ty: "Int".to_string(),
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
        binders: (0..binders)
            .map(|i| Binder {
                name: Symbol::new(format!("n{i}")),
                sort: Sort::int(),
                text: "Int".to_string(),
            })
            .collect(),
        result: None,
        variables: Vec::new(),
        footprint: Some("{sim.read}".to_string()),
        strategy: Strategy::Interleave(Points::Drawn),
    }
}

/// A ground law's one point, as `proof.world` measures it.
fn ground() -> ValueDomain {
    ValueDomain::Enumerated {
        domain: Symbol::new("unit"),
        points: 1,
        kept: 1,
    }
}

fn tier(discharge: &Discharge) -> Option<Tier> {
    match discharge {
        Discharge::Held(evidence) => Some(evidence.tier()),
        _ => None,
    }
}

/// What makes an interleaving proof one: an admitted guard, a search that ran, every value domain
/// covered as well, and a claim about one program rather than a polymorphic one.
#[track_caller]
fn audited(obligation: &Obligation, certificate: &Certificate) {
    let interleavings = certificate.rules.iter().find_map(|rule| match rule {
        Rule::ExhaustiveInterleaving { interleavings } => Some(*interleavings),
        _ => None,
    });
    assert!(interleavings.is_some_and(|n| n > 0), "{certificate:?}");
    assert!(certificate.guard_satisfiable);
    assert!(certificate.sorts.is_empty());
    assert_eq!(
        certificate
            .rules
            .iter()
            .any(|rule| matches!(rule, Rule::ExhaustiveEnumeration { .. })),
        !obligation.binders.is_empty(),
        "a law with binders names the enumeration of its values, and a ground one does not: \
         {certificate:?}"
    );
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

#[test]
fn a_law_that_holds_under_every_interleaving_is_proved() {
    let obligation = law(0);
    let mut model = Model::new(2, |counter| counter <= 2);
    let searched = discharge(&obligation, &dpor(64), &ground(), &mut model);

    assert_eq!(tier(&searched.discharge), Some(Tier::Proved));
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
    audited(&obligation, certificate);
}

#[test]
fn a_law_that_holds_only_sometimes_is_refuted_with_a_seed() {
    let obligation = law(0);
    // Reaches 1 when both reads precede both writes.
    let mut model = Model::new(2, |counter| counter == 2);
    let searched = discharge(&obligation, &dpor(64), &ground(), &mut model);

    let Discharge::Refuted(counterexample) = &searched.discharge else {
        panic!("expected a refutation, got {:?}", searched.discharge);
    };
    assert_eq!(tier(&searched.discharge), None);
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

#[test]
fn a_search_that_spends_its_budget_is_property_and_says_how_many() {
    let obligation = law(0);
    let mut model = Model::new(3, |counter| counter <= 3);
    let searched = discharge(&obligation, &dpor(30), &ground(), &mut model);

    assert_eq!(tier(&searched.discharge), Some(Tier::Property));
    assert!(searched.exhausted);
    assert!(!searched.exhaustive);
    let Discharge::Held(Evidence::Cases(report)) = &searched.discharge else {
        panic!("expected sampled evidence, got {:?}", searched.discharge);
    };
    assert_eq!(report.kept, 30, "the count is the interleavings it ran");
    assert_eq!(report.kept, searched.evaluations);
}

#[test]
fn a_reported_failure_replays_exactly() {
    let obligation = law(0);
    let mut search = Model::new(2, |counter| counter == 2);
    let found = discharge(&obligation, &dpor(64), &ground(), &mut search);
    let Discharge::Refuted(counterexample) = &found.discharge else {
        panic!("expected a refutation");
    };
    let seed = counterexample.sim_seed.clone().unwrap();
    let failing_trace = search.traces.last().cloned().unwrap();

    let mut replay = Model::new(2, |counter| counter == 2);
    let again = discharge(
        &obligation,
        &Plan::once(seed.clone()),
        &ground(),
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
    // `once` observes no flip, so it invents no race.
    assert!(replayed.race.is_none());
    assert_eq!(tier(&again.discharge), None);
}

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
        tier(&searched.discharge),
        Some(Tier::Property),
        "exhaustive over schedules says nothing about the values that were sampled"
    );
}

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
    audited(&obligation, certificate);
    assert_eq!(searched.points, 2);
}

#[test]
fn a_search_that_reached_no_region_is_exhaustive_over_nothing() {
    let plan = dpor(64);
    let mut nothing = Model::new(2, |_| true);
    nothing.unobserved = true;

    // The bare search, without the region check `discharge` adds: the overclaim.
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

    let searched = discharge(&law(0), &plan, &ground(), &mut nothing);
    assert!(!searched.observed);
    assert_eq!(searched.interleavings, 0);
    assert_ne!(
        tier(&searched.discharge),
        Some(Tier::Proved),
        "a search that scheduled nothing proves nothing"
    );
}

#[test]
fn a_sampled_plan_never_proves() {
    for plan in [Plan::random(4), Plan::once(Seed::root(7))] {
        assert_ne!(plan.mode, SimMode::Dpor);
        let mut model = Model::new(2, |counter| counter <= 2);
        let searched = discharge(&law(0), &plan, &ground(), &mut model);
        assert_ne!(tier(&searched.discharge), Some(Tier::Proved));
        assert!(matches!(searched.discharge, Discharge::Held(_)));
    }
}

#[test]
fn a_body_that_raises_is_a_gap_and_not_a_refutation() {
    let mut model = Model::new(2, |_| true);
    model.raises = true;
    let searched = discharge(&law(1), &dpor(64), &ground(), &mut model);
    let Discharge::Unattempted(Gap::Raised {
        bindings,
        diagnostic,
        ..
    }) = &searched.discharge
    else {
        panic!("expected a gap, got {:?}", searched.discharge);
    };
    assert_eq!(diagnostic.code, codes::RUNTIME_ERROR);
    assert_eq!(bindings.len(), 1);
}

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
    assert_eq!(tier(&enumerated.discharge), None);

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

#[test]
fn one_unexhausted_point_costs_the_whole_law_its_proof() {
    struct Mixed {
        inner: Model,
        starved: u64,
    }

    impl LawSearch for Mixed {
        fn run(&mut self, point: u64, seed: &Seed) -> BodyRun {
            if point == self.starved && seed.path.len() > 1 {
                // Stands in for a point whose space is larger than the budget.
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
    assert_ne!(tier(&searched.discharge), Some(Tier::Proved));
}

#[test]
fn two_runs_over_one_law_agree() {
    let run = || {
        let mut model = Model::new(3, |counter| counter == 3);
        let searched = discharge(&law(0), &dpor(64), &ground(), &mut model);
        let seed = match &searched.discharge {
            Discharge::Refuted(c) => c.sim_seed.clone(),
            other => panic!("expected a refutation, got {other:?}"),
        };
        (seed, searched.interleavings, model.traces)
    };
    assert_eq!(run(), run());
}

#[test]
fn a_refutation_over_sampled_values_carries_its_seed_its_race_and_its_point() {
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
    assert!(counterexample.sim_seed.is_some());
    assert!(counterexample.race.is_some());
    assert_eq!(counterexample.case, 0);
    assert_eq!(
        counterexample
            .bindings
            .iter()
            .map(|b| (b.name.as_str(), b.rendered.as_str()))
            .collect::<Vec<_>>(),
        [("n", "0")]
    );
}

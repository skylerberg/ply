use ply_eval::{Exploration, Naive, Plan, Seed, SimMode};
use ply_test::key::Engine;
use ply_test::sim::{Record, SimSummary, record_under, replay_command};
use ply_ty::DefHash;

fn hash(byte: u8) -> DefHash {
    DefHash([byte; 32])
}

fn passing(explored: u32) -> Exploration {
    Exploration {
        explored,
        exhaustive: true,
        ..Exploration::default()
    }
}

#[test]
fn an_unsimulated_test_is_written_under_its_own_hash_and_nothing_else() {
    let plan = Plan::default();
    assert_eq!(
        record_under(hash(1), false, &plan, &plan, None, &Engine::Evaluator),
        Record::Under(vec![hash(1)])
    );
}

#[test]
fn a_seeded_test_is_never_written_under_its_bare_hash() {
    let plan = Plan::default();
    let record = record_under(
        hash(1),
        true,
        &plan,
        &plan,
        Some(&passing(12)),
        &Engine::Evaluator,
    );
    assert!(record.is_written());
    assert!(!record.keys().contains(&hash(1)));
    assert_eq!(record.keys(), [ply_test::sim_key(hash(1), &plan)]);
}

#[test]
fn a_dpor_search_writes_no_per_root_key() {
    let plan = Plan {
        roots: vec![0, 1, 2],
        ..Plan::default()
    };
    assert_eq!(plan.mode, SimMode::Dpor);
    let record = record_under(
        hash(1),
        true,
        &plan,
        &plan,
        Some(&passing(9)),
        &Engine::Evaluator,
    );
    assert_eq!(record.keys().len(), 1);
}

#[test]
fn a_random_search_writes_one_key_per_root_it_ran_plus_the_plan() {
    let run = Plan::random(4);
    let ran = Plan {
        roots: vec![2, 3],
        ..run.clone()
    };
    let record = record_under(
        hash(1),
        true,
        &run,
        &ran,
        Some(&passing(2)),
        &Engine::Evaluator,
    );
    assert_eq!(
        record.keys(),
        [
            ply_test::seed_key(hash(1), &Seed::root(2), &Engine::Evaluator),
            ply_test::seed_key(hash(1), &Seed::root(3), &Engine::Evaluator),
            ply_test::sim_key(hash(1), &run),
        ]
    );
}

#[test]
fn a_spent_budget_writes_nothing_under_either_mode() {
    let spent = Exploration {
        explored: 256,
        exhausted: true,
        ..Exploration::default()
    };
    for plan in [Plan::default(), Plan::random(4)] {
        let record = record_under(
            hash(1),
            true,
            &plan,
            &plan,
            Some(&spent),
            &Engine::Evaluator,
        );
        assert_eq!(record, Record::Exhausted);
        assert!(record.keys().is_empty());
    }
}

/// A handler for `sim.seed()` drops `sim.read` from the row, but the region inside still searched.
#[test]
fn a_spent_budget_stops_an_unseeded_test_caching_too() {
    let plan = Plan::default();
    let spent = Exploration {
        explored: 256,
        exhausted: true,
        ..Exploration::default()
    };
    assert_eq!(
        record_under(
            hash(1),
            false,
            &plan,
            &plan,
            Some(&spent),
            &Engine::Evaluator
        ),
        Record::Exhausted
    );
}

#[test]
fn a_seeded_test_whose_search_was_not_observed_writes_nothing() {
    let plan = Plan::default();
    assert_eq!(
        record_under(hash(1), true, &plan, &plan, None, &Engine::Evaluator),
        Record::Unobserved
    );
}

#[test]
fn the_summary_line_names_the_counts_and_is_silent_without_a_region() {
    assert_eq!(SimSummary::default().line(), None);
    let summary = SimSummary {
        simulated: 3,
        total: 47,
        seeds: 3,
        interleavings: 61,
        exhaustive: 3,
        exhausted: 0,
        failed: 0,
    };
    assert_eq!(
        summary.line().unwrap(),
        "simulated: 3 of 47 · 61 interleavings · 3 exhaustive"
    );
}

#[test]
fn a_spent_budget_is_said_out_loud_in_the_summary() {
    let summary = SimSummary {
        simulated: 1,
        total: 1,
        seeds: 1,
        interleavings: 256,
        exhausted: 1,
        ..SimSummary::default()
    };
    assert!(summary.line().unwrap().contains("not cached"));
}

#[test]
fn the_replay_command_is_the_command() {
    assert_eq!(
        replay_command(&Seed::at(0, vec![1, 0, 3]), "balance never goes negative"),
        "ply test --seed 0:1.0.3 --filter \"balance never goes negative\""
    );
}

#[test]
fn a_measured_reduction_does_not_change_what_is_written() {
    let plan = Plan::default();
    let measured = Exploration {
        naive: Some(Naive {
            explored: 720,
            bounded: false,
        }),
        ..passing(12)
    };
    assert_eq!(
        record_under(
            hash(1),
            true,
            &plan,
            &plan,
            Some(&measured),
            &Engine::Evaluator
        )
        .keys(),
        [ply_test::sim_key(hash(1), &plan)]
    );
}

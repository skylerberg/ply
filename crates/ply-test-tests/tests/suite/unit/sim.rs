use ply_eval::{Exploration, Naive, Seed};
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

/// Where a pass goes is the program's to say: the runtime writes exactly what it was handed.
#[test]
fn a_pass_is_written_under_exactly_the_keys_it_was_filed_under() {
    assert_eq!(
        record_under(&[hash(1)], false, None),
        Record::Under(vec![hash(1)])
    );
    assert_eq!(
        record_under(&[hash(2), hash(3)], true, Some(&passing(12))),
        Record::Under(vec![hash(2), hash(3)])
    );
}

#[test]
fn a_spent_budget_writes_nothing_under_either_mode() {
    let spent = Exploration {
        explored: 256,
        exhausted: true,
        ..Exploration::default()
    };
    let record = record_under(&[hash(1), hash(2)], true, Some(&spent));
    assert_eq!(record, Record::Exhausted);
    assert!(record.keys().is_empty());
}

/// A handler for `sim.seed()` drops `sim.read` from the row, but the region inside still searched.
#[test]
fn a_spent_budget_stops_an_unseeded_test_caching_too() {
    let spent = Exploration {
        explored: 256,
        exhausted: true,
        ..Exploration::default()
    };
    assert_eq!(
        record_under(&[hash(1)], false, Some(&spent)),
        Record::Exhausted
    );
}

#[test]
fn a_seeded_test_whose_search_was_not_observed_writes_nothing() {
    assert_eq!(record_under(&[hash(1)], true, None), Record::Unobserved);
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
    let measured = Exploration {
        naive: Some(Naive {
            explored: 720,
            bounded: false,
        }),
        ..passing(12)
    };
    assert_eq!(
        record_under(&[hash(1)], true, Some(&measured)).keys(),
        [hash(1)]
    );
}

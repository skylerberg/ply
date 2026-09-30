use ply_eval::{Exploration, Naive};
use ply_span::Symbol;
use ply_test::sim::{Record, is_seeded, record_under};
use ply_ty::{DefHash, EffectAtom, Footprint, Mode, Resource};

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

/// A seed read is what makes a test searched: the runtime runs it once per interleaving.
#[test]
fn a_test_is_searched_exactly_when_its_footprint_reads_a_seed() {
    let seed = EffectAtom::new("sim", Resource::Singleton, Mode::Read);
    let cell = EffectAtom::new("cell", Resource::Named(Symbol::new("users")), Mode::Write);
    assert!(is_seeded(&Footprint::from_atoms([seed.clone()])));
    assert!(is_seeded(&Footprint::from_atoms([seed, cell.clone()])));
    assert!(!is_seeded(&Footprint::from_atoms([cell])));
    assert!(!is_seeded(&Footprint::empty()));
    // User effects are module-qualified, so a program's own `sim` cannot pass for the seed.
    let impostor = EffectAtom::new("m.sim", Resource::Singleton, Mode::Read);
    assert!(!is_seeded(&Footprint::from_atoms([impostor])));
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

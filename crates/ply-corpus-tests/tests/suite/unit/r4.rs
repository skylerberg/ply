use ply_corpus::r4::{BASELINE, Criteria, Lever, Measured, Verdict, judge};

fn at(lever: Lever, after: f64) -> Measured {
    Measured {
        lever,
        after,
        time_ratio: 1.0,
        divergences: 0,
    }
}

#[test]
fn no_levers_floor_is_above_what_the_attribution_places_under_it() {
    for lever in [
        Lever::ArgumentVectors,
        Lever::ConstantValues,
        Lever::RecordLayout,
    ] {
        assert!(
            lever.floor() < lever.attributed_share(),
            "{lever:?} is asked for {:.3} and only {:.3} was ever counted under it",
            lever.floor(),
            lever.attributed_share()
        );
    }
}

#[test]
fn a_lever_that_removes_everything_attributed_to_it_is_kept() {
    for lever in [
        Lever::ArgumentVectors,
        Lever::ConstantValues,
        Lever::RecordLayout,
    ] {
        let after = BASELINE * (1.0 - lever.attributed_share());
        assert_eq!(
            judge(&Criteria::default(), &at(lever, after)),
            Verdict::Keep
        );
    }
}

#[test]
fn a_lever_that_fires_under_its_floor_is_short_rather_than_kept() {
    let lever = Lever::ArgumentVectors;
    let after = BASELINE * (1.0 - lever.floor() / 2.0);
    assert_eq!(
        judge(&Criteria::default(), &at(lever, after)),
        Verdict::Short
    );
}

#[test]
fn a_lever_that_moved_nothing_is_undecided_rather_than_short() {
    assert_eq!(
        judge(&Criteria::default(), &at(Lever::ConstantValues, BASELINE)),
        Verdict::Undecided
    );
}

#[test]
fn a_lever_that_saved_allocations_and_lost_time_is_reverted() {
    let lever = Lever::ArgumentVectors;
    let m = Measured {
        time_ratio: 1.05,
        ..at(lever, BASELINE * (1.0 - lever.attributed_share()))
    };
    assert_eq!(judge(&Criteria::default(), &m), Verdict::Revert);
}

#[test]
fn one_divergence_reverts_whatever_it_saved() {
    let lever = Lever::ConstantValues;
    let m = Measured {
        divergences: 1,
        ..at(lever, BASELINE * (1.0 - lever.attributed_share()))
    };
    assert_eq!(judge(&Criteria::default(), &m), Verdict::Revert);
}

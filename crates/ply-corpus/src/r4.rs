//! The bar each of ADR 0019's levers has to clear, written before the lever is.

/// The window pair every figure a verdict reads must be fitted from.
pub const WINDOW: (usize, usize) = (20, 200);

/// Allocations per `/health` over SimNet at [`WINDOW`], before any lever.
pub const BASELINE: f64 = 911.5;

/// One of ADR 0019's changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lever {
    ArgumentVectors,
    ConstantValues,
    RecordLayout,
}

impl Lever {
    /// The allocations per request the attribution places under this lever, as a share of
    /// [`BASELINE`].
    pub fn attributed_share(self) -> f64 {
        match self {
            // 341.4 transient argument vectors of the 372.4 built; the other 31.0 are retained as
            // `Ctor.args` and are not the pool's to take.
            Lever::ArgumentVectors => 341.4 / BASELINE,
            // 65.0 literal `Str`/`Bytes` + 21.0 nullary constructor mentions + 24.0
            // constructor-closure mentions.
            Lever::ConstantValues => 110.0 / BASELINE,
            // 33.0 B-tree nodes.
            Lever::RecordLayout => 33.0 / BASELINE,
        }
    }

    /// The share of [`BASELINE`] this lever must actually remove to be kept.
    pub fn floor(self) -> f64 {
        match self {
            Lever::ArgumentVectors => 0.20,
            Lever::ConstantValues => 0.07,
            Lever::RecordLayout => 0.02,
        }
    }
}

/// Bars that apply to every lever, whatever it does.
#[derive(Clone, Copy, Debug)]
pub struct Criteria {
    /// Wall-clock regression a lever may not exceed on the served request.
    pub max_time_regression: f64,
    /// Below this the ladder did not separate and nothing is decided from it.
    pub min_separation: f64,
    /// Divergences the backend audit may report over the corpora on disk.
    pub max_divergences: usize,
}

impl Default for Criteria {
    fn default() -> Criteria {
        Criteria {
            max_time_regression: 1.02,
            min_separation: 0.01,
            max_divergences: 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// The lever cleared its floor and cost nothing measurable.
    Keep,
    /// It fired, but under its own floor.
    Short,
    /// It cost more than it saved somewhere else.
    Revert,
    /// The measurement did not decide it.
    Undecided,
}

/// What a build agent reports for one lever.
#[derive(Clone, Copy, Debug)]
pub struct Measured {
    pub lever: Lever,
    /// Allocations per request at [`WINDOW`] with the lever in.
    pub after: f64,
    /// Served-request wall clock with the lever in, over the same without it.
    pub time_ratio: f64,
    pub divergences: usize,
}

/// The verdict, from the numbers and the thresholds above and nothing else.
pub fn judge(c: &Criteria, m: &Measured) -> Verdict {
    if m.divergences > c.max_divergences {
        return Verdict::Revert;
    }
    let saved = (BASELINE - m.after) / BASELINE;
    if saved.abs() < c.min_separation {
        return Verdict::Undecided;
    }
    if saved < 0.0 || m.time_ratio > c.max_time_regression {
        return Verdict::Revert;
    }
    if saved >= m.lever.floor() {
        Verdict::Keep
    } else {
        Verdict::Short
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

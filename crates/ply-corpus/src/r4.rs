//! The bar each of ADR 0019's levers has to clear, written before the lever is.
//!
//! `ply_corpus::w6::Criteria` is the model and the reason: a threshold a
//! measurement supplies is a threshold the measurement cannot fail. Everything
//! here is fixed by the attribution that motivated ADR 0019 and by nothing that
//! comes after it.
//!
//! # The route a lever is judged on
//!
//! `/health` over SimNet, not the pure-call routing rung. The two disagree on
//! ranking and both are reported by
//! `cargo test -p ply-corpus --release --test r4_value_construction --
//! --nocapture`; the SimNet path is the only one that pays for framing, the
//! host boundary and the response encode, so it is the one a served request
//! resembles.
//!
//! # Allocations, not bytes
//!
//! `CONTRIBUTING.md` §"Things known to be broken" item 8 is reproduced on this
//! path — `r4_value_construction::the_per_request_slope_is_the_same_between_the_second_and_third_window`
//! prints an allocation slope that holds to 1.0% and a byte slope that moves
//! 95.3% between the (20, 200) and (200, 400) window pairs. So a verdict here
//! reads the allocation count, the window pair is pinned, and no threshold in
//! this file is stated in bytes.

/// The window pair every figure a verdict reads must be fitted from.
///
/// Pinned because a slope taken at another pair is not comparable to the
/// baseline: `w3::Loaded::over_sim` builds one `Machine` per script, so a
/// window charges every one-time cost to the requests in it, and the intercept
/// only divides out at the pair the baseline used.
pub const WINDOW: (usize, usize) = (20, 200);

/// Allocations per `/health` over SimNet at [`WINDOW`], before any lever.
///
/// Not a threshold — the number a threshold is a fraction of. Re-taken rather
/// than quoted: it is the `fit:` line of the `/health` section printed by the
/// command in this module's note.
pub const BASELINE: f64 = 911.5;

/// One of ADR 0019's changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lever {
    /// §1. Recycle the call-argument vector instead of freeing it.
    ArgumentVectors,
    /// §2. Build a compile-time constant's `Value` once and clone the handle.
    ConstantValues,
    /// §3. A record's fields as one flat sorted allocation.
    RecordLayout,
}

impl Lever {
    /// The allocations per request the attribution places under this lever, as
    /// a share of [`BASELINE`]. A lever cannot save more than this, so a report
    /// claiming it did is a measurement error rather than a result.
    pub fn attributed_share(self) -> f64 {
        match self {
            // 341.4 transient argument vectors of the 372.4 built; the other
            // 31.0 are retained as `Ctor.args` and are not the pool's to take.
            //
            // **That number is wrong and is deliberately left standing.** It
            // assumes every transient buffer reaches `Machine::enter_code`.
            // Measured after the lever landed: 178.0 do, 140.4 are consumed by
            // `ply_eval::builtins::call` — which takes its `Vec<Value>` by
            // value, so it cannot hand one back — and 23.0 are wider than the
            // free list's four capacity classes. The share the lever could ever
            // remove is 178.0/BASELINE = 19.53%, under the 20% floor below it,
            // so `judge` answers `Verdict::Short` on a lever that removed
            // everything its mechanism can reach. Editing either number after
            // the fact is exactly what this module exists to prevent, so
            // neither has been; the correction is in
            // `docs/adr/0019-value-representation.md` §1 with the original
            // beside it, and re-deriving the floor is a decision for whoever
            // amends that ADR.
            Lever::ArgumentVectors => 341.4 / BASELINE,
            // 65.0 literal `Str`/`Bytes` + 21.0 nullary constructor mentions +
            // 24.0 constructor-closure mentions.
            Lever::ConstantValues => 110.0 / BASELINE,
            // 33.0 B-tree nodes.
            Lever::RecordLayout => 33.0 / BASELINE,
        }
    }

    /// The share of [`BASELINE`] this lever must actually remove to be kept.
    ///
    /// Each is a little over half of what the attribution places under it,
    /// which is the margin between "the mechanism works" and "the mechanism
    /// fires on the cases that were counted". A lever that lands under its own
    /// floor removed something, but not the thing it was built for, and the
    /// answer to that is another attribution rather than the next lever —
    /// which is R3's lesson and is why the floor is a fraction of a measured
    /// share rather than a round number.
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
    /// A free list that trades an allocation for a branch has to be shown not
    /// to, and ADR 0018 §2 says in as many words that this was never measured.
    pub max_time_regression: f64,
    /// Below this the ladder did not separate and nothing is decided from it.
    /// `w6::Criteria::max_negative_share` is the same idea.
    pub min_separation: f64,
    /// Divergences `--engine both` may report over the corpora on disk.
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
    /// The lever cleared its floor and cost nothing measurable. Keep it.
    Keep,
    /// It fired, but under its own floor. The next step is an attribution run,
    /// not the next lever.
    Short,
    /// It cost more than it saved somewhere else. Revert.
    Revert,
    /// The measurement did not decide it. Not the same as `Short`.
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

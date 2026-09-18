//! The bar each value-representation lever has to clear.

/// The window pair every figure a verdict reads must be fitted from.
pub const WINDOW: (usize, usize) = (20, 200);

/// Allocations per `/health` over SimNet at [`WINDOW`], before any lever.
pub const BASELINE: f64 = 911.5;

/// One of the value-representation work's changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lever {
    ArgumentVectors,
    ConstantValues,
    RecordLayout,
}

impl Lever {
    /// Allocations per request attributed to this lever, as a share of [`BASELINE`].
    pub fn attributed_share(self) -> f64 {
        match self {
            Lever::ArgumentVectors => 341.4 / BASELINE,
            Lever::ConstantValues => 110.0 / BASELINE,
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

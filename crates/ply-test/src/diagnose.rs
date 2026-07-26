//! Turning one failure into the artifact of ADR 0004.
//!
//! The artifact answers, in this order: which change caused this, what actually
//! ran, what else could have, and what was asserted. This module owns the first
//! three — it runs the preconditions, builds the delta, drives the search, and
//! folds the trace and the bisection into the ranked suspect list. It decides
//! nothing a run has no evidence for: every gate below refuses rather than
//! guesses, because naming the wrong definition is worse than naming none.

use crate::bisect::{
    Baseline, Bisection, Budget, Classify, Delta, DepEdges, Diff, Hybrid, Mode, NoHybrid,
    Regression, Skipped, Verdict, bisect, diff, precheck,
};
use crate::slice::{CausalSlice, Tracing};
use crate::{Attribution, Suspect};
use ply_hash::{DefHash, HashOutput};
use ply_span::Symbol;

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub bisect: Mode,
    pub trace: Tracing,
    pub budget: Budget,
}

impl Options {
    pub fn never() -> Options {
        Options {
            bisect: Mode::Never,
            trace: Tracing::Never,
            budget: Budget::DEFAULT,
        }
    }
}

/// One failure, and everything a run knows about it that is not in the
/// diagnostic.
pub struct Evidence<'a> {
    /// `<module>.<label>`.
    pub key: &'a Symbol,
    pub test_hash: Option<DefHash>,
    pub nondet: bool,
    /// A panic is a defect in Ply, not a change to attribute.
    pub panicked: bool,
    /// The raw closure ∩ changed intersection, by name.
    pub suspects: &'a [Symbol],
    pub hashes: &'a HashOutput,
    pub baseline: Option<&'a Baseline>,
    pub slice: Option<CausalSlice>,
}

/// `hybrid` is `None` when no hybrid program can be built. `absent` says which
/// of the reasons it was, and is reported only to a search that would have had
/// to run one — the answers that need no mixture are unaffected either way.
pub fn diagnose(
    evidence: Evidence<'_>,
    options: &Options,
    edges: &DepEdges,
    classify: &mut dyn Classify,
    hybrid: Option<&mut dyn Hybrid>,
    absent: Skipped,
) -> Attribution {
    let mut attribution = Attribution::from_suspects(evidence.suspects, evidence.hashes);

    // The delta is worth building whatever happens to the search: which suspects
    // anybody actually edited is the field that shrinks an agent's reading list,
    // and it needs a baseline rather than a hybrid.
    let gate = precheck(
        options.bisect,
        evidence.panicked,
        evidence.nondet,
        evidence.baseline,
    );
    let diff = evidence.baseline.map(|baseline| {
        let regression = Regression {
            key: evidence.key,
            test_hash: evidence.test_hash,
            baseline,
            hashes: evidence.hashes,
        };
        diff(&regression, classify, edges)
    });

    let bisection = match (gate, &diff) {
        (Err(why), _) => Bisection::not_attempted(why),
        (Ok(()), None) => Bisection::not_attempted(Skipped::NeverPassed),
        (Ok(()), Some(diff)) => {
            let budget = options.bisect.budget(options.budget);
            match hybrid {
                Some(hybrid) => search(diff, hybrid, budget),
                None if needs_a_hybrid(&diff.delta) => Bisection::not_attempted(absent),
                None => search(diff, &mut NoHybrid, budget),
            }
        }
    };

    if let Some(diff) = &diff {
        attribution.annotate(&diff.delta);
    }
    attribution.resolve(bisection, evidence.slice);
    attribution
}

/// One cluster and an unedited test is the overwhelmingly common case — one
/// edit — and it is decided by counting, not by evaluating. Withholding that
/// answer because the body store is cold would make the default useless on
/// exactly the failure it exists for.
fn needs_a_hybrid(delta: &Delta) -> bool {
    match delta.clusters.len() {
        0 => false,
        1 => delta.test.is_some(),
        _ => true,
    }
}

fn search(diff: &Diff, hybrid: &mut dyn Hybrid, budget: Budget) -> Bisection {
    let mut bisection = bisect(&diff.delta, hybrid, budget);
    if !diff.unclassified.is_empty()
        && bisection.verdict != Verdict::NotAttempted(Skipped::NoChanges)
    {
        bisection.reason.push_str(&format!(
            "; {} of the changed definitions could not be told apart from a hash that \
             merely moved, so the search considered them all",
            diff.unclassified.len()
        ));
    }
    if diff.test_unclassified && bisection.verdict == Verdict::NotInTheGraph {
        bisection.reason.push_str(
            "; the test's own hash also moved and could not be classified, so an edit to \
             the test has not been ruled out",
        );
    }
    bisection
}

impl Attribution {
    /// Fills in what the two configurations disagreed about. Separate from
    /// [`Attribution::resolve`] because it is available whenever a baseline is —
    /// no bodies, no hybrid and no trace required — and marking the `derived`
    /// changes is most of the value on its own.
    pub fn annotate(&mut self, delta: &Delta) {
        for suspect in &mut self.suspects {
            if let Some(change) = delta.change(&suspect.name) {
                suspect.before = change.before;
                suspect.change = Some(change.kind);
            }
        }
        // A candidate the search names has to be in the list it is ranked
        // against, or `suspects[0]` is not the best guess.
        for change in &delta.changes {
            if !self.suspects.iter().any(|s| s.name == change.name) {
                let mut extra = Suspect::new(change.name.clone(), change.after);
                extra.before = change.before;
                extra.change = Some(change.kind);
                self.suspects.push(extra);
            }
        }
        self.suspects.sort_by(|a, b| a.rank().cmp(&b.rank()));
    }
}

#[cfg(test)]
mod tests;

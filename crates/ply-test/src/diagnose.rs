//! Turning one failure into the artifact of ADR 0004.

use crate::bisect::{
    Baseline, Bisection, Budget, Classify, Delta, DepEdges, Diff, Gate, Hybrid, Mode, NoHybrid,
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

/// One failure, and everything a run knows about it that is not in the diagnostic.
pub struct Evidence<'a> {
    /// `<module>.<label>`.
    pub key: &'a Symbol,
    pub test_hash: Option<DefHash>,
    pub nondet: bool,
    /// The evaluator failed rather than the program — a defect in Ply, not a change to attribute.
    pub defect: bool,
    /// The failing run reached a host handler, read off what the runtime did rather than off the
    /// prediction selection made.
    pub host: bool,
    /// The raw closure ∩ changed intersection, by name.
    pub suspects: &'a [Symbol],
    pub hashes: &'a HashOutput,
    pub baseline: Option<&'a Baseline>,
    pub slice: Option<CausalSlice>,
}

/// `hybrid` is `None` when no hybrid program can be built.
pub fn diagnose(
    evidence: Evidence<'_>,
    options: &Options,
    edges: &DepEdges,
    classify: &mut dyn Classify,
    hybrid: Option<&mut dyn Hybrid>,
    absent: Skipped,
) -> Attribution {
    let mut attribution = Attribution::from_suspects(evidence.suspects, evidence.hashes);

    // The delta is worth building whatever happens to the search: which suspects anybody actually
    // edited is the field that shrinks an agent's reading list, and it needs a baseline rather than
    // a hybrid.
    let gate = precheck(
        Gate::new(
            options.bisect,
            evidence.defect,
            evidence.nondet,
            evidence.baseline,
        )
        .hosted(evidence.host),
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

/// One cluster and an unedited test is the overwhelmingly common case — one edit — and it is
/// decided by counting, not by evaluating.
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
    /// Fills in what the two configurations disagreed about.
    pub fn annotate(&mut self, delta: &Delta) {
        for suspect in &mut self.suspects {
            if let Some(change) = delta.change(&suspect.name) {
                suspect.before = change.before;
                suspect.change = Some(change.kind);
            }
        }
        // A candidate the search names has to be in the list it is ranked against, or `suspects[0]`
        // is not the best guess.
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

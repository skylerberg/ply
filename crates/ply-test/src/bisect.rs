//! Bisection over the definition graph.

pub mod classify;
pub mod rehash;

pub use classify::{Classify, StoreClassify, Unknown};
pub use rehash::Rehashed;

use ply_span::Symbol;
use ply_ty::{DefHash, HashOutput};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum Ns {
    #[default]
    Value,
    Decl,
}

impl Ns {
    pub fn as_str(self) -> &'static str {
        match self {
            Ns::Value => "value",
            Ns::Decl => "declaration",
        }
    }
}

/// A definition's identity across two configurations: its program-wide name and namespace.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DefKey {
    pub name: Symbol,
    pub ns: Ns,
}

impl DefKey {
    pub fn value(name: Symbol) -> DefKey {
        DefKey {
            name,
            ns: Ns::Value,
        }
    }

    pub fn decl(name: Symbol) -> DefKey {
        DefKey { name, ns: Ns::Decl }
    }

    pub fn is_decl(&self) -> bool {
        self.ns == Ns::Decl
    }
}

/// The definition set a test was last seen to pass at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Baseline {
    pub test_hash: DefHash,
    /// Program-wide name -> hash, for every *function* in the closure then.
    pub closure: BTreeMap<Symbol, DefHash>,
    /// The same for the `type` and `effect` declarations.
    pub decls: BTreeMap<Symbol, DefHash>,
}

impl Baseline {
    pub fn new(test_hash: DefHash, closure: BTreeMap<Symbol, DefHash>) -> Baseline {
        Baseline {
            test_hash,
            closure,
            decls: BTreeMap::new(),
        }
    }

    pub fn with_decls(
        test_hash: DefHash,
        closure: BTreeMap<Symbol, DefHash>,
        decls: BTreeMap<Symbol, DefHash>,
    ) -> Baseline {
        Baseline {
            test_hash,
            closure,
            decls,
        }
    }

    pub fn hash(&self, name: &Symbol) -> Option<DefHash> {
        self.closure.get(name).copied()
    }

    pub fn hash_of(&self, key: &DefKey) -> Option<DefHash> {
        match key.ns {
            Ns::Value => self.closure.get(&key.name).copied(),
            Ns::Decl => self.decls.get(&key.name).copied(),
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = DefKey> {
        self.closure
            .keys()
            .map(|n| DefKey::value(n.clone()))
            .chain(self.decls.keys().map(|n| DefKey::decl(n.clone())))
    }

    pub fn hashes(&self) -> impl Iterator<Item = DefHash> {
        self.closure.values().chain(self.decls.values()).copied()
    }
}

/// What the runtime knows about one definition in a failing test's closure, in either era: the facts
/// a change set is classified from. Which kind of change they make is the program's to decide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: DefKey,
    /// Its hash when the test last passed, and now.
    pub before: Option<DefHash>,
    pub after: Option<DefHash>,
    /// Its current body hashed as the baseline wrote references; `None` when the rehasher had no
    /// answer.
    pub rehashed: Option<DefHash>,
    /// Whether its published interface is the same on both sides; `None` when nothing could say, or
    /// when its body did not move on its own.
    pub stable: Option<bool>,
    /// Its baseline hash is still somewhere in the current program.
    pub kept: bool,
    /// Its baseline hash is one the current program re-normalizes to.
    pub renamed: bool,
    /// The members of its recursive component, when it has more than one.
    pub component: Vec<DefKey>,
    /// The closure's names that mention it.
    pub referrers: Vec<Symbol>,
}

/// One failure's facts: the test's own hashes, and a row for every definition either era's closure
/// holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSet {
    pub test: Symbol,
    /// The test's hash when it last passed, now, and its current body hashed as the baseline wrote
    /// references.
    pub before: DefHash,
    pub after: Option<DefHash>,
    pub rehashed: Option<DefHash>,
    pub rows: Vec<Row>,
}

pub struct Regression<'a> {
    /// `<module>.<label>`.
    pub key: &'a Symbol,
    pub test_hash: Option<DefHash>,
    pub baseline: &'a Baseline,
    pub hashes: &'a HashOutput,
}

pub fn change_set(regression: &Regression<'_>, classify: &mut dyn Classify) -> ChangeSet {
    let current = regression.hashes;
    let baseline = regression.baseline;

    let mut keys: BTreeSet<DefKey> = baseline.keys().collect();
    for name in current.closure.get(regression.key).into_iter().flatten() {
        if current.defs.contains_key(name) {
            keys.insert(DefKey::value(name.clone()));
        }
        if current.decls.contains_key(name) {
            keys.insert(DefKey::decl(name.clone()));
        }
    }
    let names: BTreeSet<&Symbol> = keys.iter().map(|k| &k.name).collect();
    let mut referrers: BTreeMap<&Symbol, BTreeSet<Symbol>> = BTreeMap::new();
    for (from, deps) in &current.deps {
        if !names.contains(from) {
            continue;
        }
        for to in deps {
            if names.contains(to) {
                referrers.entry(to).or_default().insert(from.clone());
            }
        }
    }
    let now: BTreeSet<DefHash> = current
        .defs
        .values()
        .chain(current.decls.values())
        .copied()
        .collect();
    let image = classify.baseline_image();

    let rows = keys
        .iter()
        .map(|key| {
            let before = baseline.hash_of(key);
            let after = match key.ns {
                Ns::Value => current.defs.get(&key.name).copied(),
                Ns::Decl => current.decls.get(&key.name).copied(),
            };
            let rehashed = classify.renormalized(key);
            // Comparing interfaces reads the baseline's out of the store, so it is asked only of a
            // body that moved on its own.
            let stable = match (before, after) {
                (Some(was), Some(is)) if was != is && rehashed != Some(was) => {
                    classify.interface_stable(key, was)
                }
                _ => None,
            };
            Row {
                key: key.clone(),
                before,
                after,
                rehashed,
                stable,
                kept: before.is_some_and(|was| now.contains(&was)),
                renamed: before.is_some_and(|was| image.contains(&was)),
                component: classify.component(key),
                referrers: referrers
                    .get(&key.name)
                    .map(|from| from.iter().cloned().collect())
                    .unwrap_or_default(),
            }
        })
        .collect();

    ChangeSet {
        test: regression.key.clone(),
        before: baseline.test_hash,
        after: regression.test_hash,
        rehashed: classify.renormalized_test(regression.key),
        rows,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SearchStats {
    pub candidates: usize,
    pub clusters: usize,
    /// Hybrids actually built and run.
    pub evaluated: usize,
    pub cached: usize,
    /// Subsets the search would have asked about twice.
    pub memoized: usize,
    pub unresolved: usize,
    /// The budget ran out, so the result is a superset of the cause rather than a minimal set.
    pub exhausted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Skipped {
    /// `--bisect=never`, or a run that never asked.
    NotRequested,
    NeverPassed,
    Host,
    /// `test/nondet` outcomes are not a function of the definition set, so a hybrid proves nothing.
    Nondet,
    Panicked,
    /// Baseline and current agree on every definition in the closure.
    NoChanges,
    /// The store cannot produce the bodies a hybrid needs.
    NoBodies,
    /// The bodies are there, but this build cannot assemble them into a mixed program.
    NoHybrids,
}

impl Skipped {
    pub fn as_str(self) -> &'static str {
        match self {
            Skipped::NotRequested => "not_requested",
            Skipped::NeverPassed => "never_passed",
            Skipped::Host => "host",
            Skipped::Nondet => "nondet",
            Skipped::Panicked => "panicked",
            Skipped::NoChanges => "no_changes",
            Skipped::NoBodies => "no_bodies",
            Skipped::NoHybrids => "no_hybrids",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Skipped::NotRequested => "bisection was not requested for this run",
            Skipped::NeverPassed => {
                "this test has never passed, so there is no earlier definition set to compare against"
            }
            Skipped::Host => {
                "this failure came from a run that reached a host handler, and bisecting it would re-run the test — and repeat whatever it did outside the program — once per candidate definition set"
            }
            Skipped::Nondet => {
                "`test/nondet` is not a function of the definition set, so bisecting it would prove nothing"
            }
            Skipped::Panicked => {
                "the interpreter failed rather than the program; this is a defect in Ply, and no change in the program explains it"
            }
            Skipped::NoChanges => {
                "no definition in this test's closure changed since it last passed"
            }
            Skipped::NoBodies => {
                "the store does not hold the definition bodies a hybrid program would need"
            }
            Skipped::NoHybrids => {
                "this build cannot mix two eras of a definition graph, so the change set could not be narrowed by running it"
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Bisected,
    /// Exactly one change could be flipped, so the answer needed no runs at all.
    Sole,
    /// The baseline definitions with this test's current body already fail: the test edit matters.
    TestChanged,
    /// The same, but the test was not edited, so nothing in the definition graph explains it.
    NotInTheGraph,
    NotReproduced,
    /// Every hybrid the search could form was unresolved.
    Inconclusive,
    NotAttempted(Skipped),
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Bisected => "bisected",
            Verdict::Sole => "sole",
            Verdict::TestChanged => "test_changed",
            Verdict::NotInTheGraph => "not_in_the_graph",
            Verdict::NotReproduced => "not_reproduced",
            Verdict::Inconclusive => "inconclusive",
            Verdict::NotAttempted(_) => "not_attempted",
        }
    }

    pub fn skipped(self) -> Option<Skipped> {
        match self {
            Verdict::NotAttempted(s) => Some(s),
            _ => None,
        }
    }

    /// Whether the culprit set is an answer rather than a fallback.
    pub fn names_a_culprit(self) -> bool {
        matches!(
            self,
            Verdict::Bisected | Verdict::Sole | Verdict::TestChanged
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unresolved {
    /// Old and new disagree about a signature, so this mixture is ill-typed.
    DoesNotCheck,
    /// It failed, but not with the failure being explained.
    DifferentFailure,
    MissingBody,
    BudgetSpent,
}

impl Unresolved {
    pub fn as_str(self) -> &'static str {
        match self {
            Unresolved::DoesNotCheck => "does not typecheck",
            Unresolved::DifferentFailure => "a different failure",
            Unresolved::MissingBody => "a body is missing from the store",
            Unresolved::BudgetSpent => "budget spent",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrialOutcome {
    /// Reproduced the failure being explained, and no other.
    Fails,
    Passes,
    Unresolved(Unresolved),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Trial {
    pub outcome: TrialOutcome,
    /// Answered from the result cache rather than by evaluating anything.
    pub cached: bool,
}

impl Trial {
    pub fn fails() -> Trial {
        Trial {
            outcome: TrialOutcome::Fails,
            cached: false,
        }
    }
    pub fn passes() -> Trial {
        Trial {
            outcome: TrialOutcome::Passes,
            cached: false,
        }
    }
    pub fn unresolved(why: Unresolved) -> Trial {
        Trial {
            outcome: TrialOutcome::Unresolved(why),
            cached: false,
        }
    }
    pub fn from_cache(mut self) -> Trial {
        self.cached = true;
        self
    }
}

/// How much the culprit set may be trusted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confidence {
    /// One definition per group, and dropping any group makes the failure go away: 1-minimal.
    Minimal,
    /// Some group could not be split, because its members' interfaces changed together.
    Fused,
    Partial,
    None,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Minimal => "minimal",
            Confidence::Fused => "fused",
            Confidence::Partial => "partial",
            Confidence::None => "none",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Bisection {
    pub verdict: Verdict,
    pub confidence: Confidence,
    /// The minimal failure-inducing change set, one entry per fused group.
    pub groups: Vec<Vec<Symbol>>,
    /// One sentence saying what happened and what to do about it.
    pub reason: String,
    pub search: SearchStats,
}

impl Bisection {
    pub fn not_attempted(why: Skipped) -> Bisection {
        Bisection {
            verdict: Verdict::NotAttempted(why),
            confidence: Confidence::None,
            groups: Vec::new(),
            reason: why.describe().to_string(),
            search: SearchStats::default(),
        }
    }

    pub fn culprits(&self) -> Vec<Symbol> {
        let mut out: Vec<Symbol> = self.groups.iter().flatten().cloned().collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn is_conclusive(&self) -> bool {
        self.verdict.names_a_culprit() && !self.groups.is_empty()
    }
}

impl Default for Bisection {
    fn default() -> Bisection {
        Bisection::not_attempted(Skipped::NotRequested)
    }
}

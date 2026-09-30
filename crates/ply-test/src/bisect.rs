//! Bisection over the definition graph.

pub mod classify;
pub mod rehash;

pub use classify::{Classify, StoreClassify, Unknown};
pub use rehash::Rehashed;

use ply_eval::{DefHash, HashOutput, Symbol};
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

/// Why no mixture of a failure's two eras can be tried.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Absent {
    /// The store cannot produce the bodies a mixture needs.
    NoBodies,
    /// The bodies are there, and the failure still cannot be re-run as a mixture.
    NoHybrids,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unresolved {
    /// Old and new disagree about a signature, so this mixture is ill-typed.
    DoesNotCheck,
    /// It failed, but not with the failure being explained.
    DifferentFailure,
    MissingBody,
}

impl Unresolved {
    pub fn as_str(self) -> &'static str {
        match self {
            Unresolved::DoesNotCheck => "does not typecheck",
            Unresolved::DifferentFailure => "a different failure",
            Unresolved::MissingBody => "a body is missing from the store",
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

//! Bisection over the definition graph.

pub mod classify;
pub mod rehash;

pub use classify::{Classify, StoreClassify, Unknown};
pub use rehash::Rehashed;

use ply_hash::{DefHash, HashOutput};
use ply_span::Symbol;
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChangeKind {
    /// Its own normalized body differs.
    Edited,
    /// Its body is byte-identical; its hash moved only because a dependency's did.
    Derived,
    Added,
    Removed,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeKind::Edited => "edited",
            ChangeKind::Derived => "derived",
            ChangeKind::Added => "added",
            ChangeKind::Removed => "removed",
        }
    }

    pub fn is_candidate(self) -> bool {
        !matches!(self, ChangeKind::Derived)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The program-wide name — `store.orders.place`.
    pub name: Symbol,
    pub ns: Ns,
    /// Its hash when the test last passed.
    pub before: Option<DefHash>,
    pub after: Option<DefHash>,
    pub kind: ChangeKind,
    /// Whether its published interface (scheme and footprint) is the same on both sides.
    pub independent: bool,
}

impl Change {
    pub fn edited(name: Symbol, before: DefHash, after: DefHash, independent: bool) -> Change {
        Change {
            name,
            ns: Ns::Value,
            before: Some(before),
            after: Some(after),
            kind: ChangeKind::Edited,
            independent,
        }
    }

    pub fn in_namespace(mut self, ns: Ns) -> Change {
        self.ns = ns;
        self
    }

    pub fn key(&self) -> DefKey {
        DefKey {
            name: self.name.clone(),
            ns: self.ns,
        }
    }

    pub fn derived(name: Symbol, before: DefHash, after: DefHash) -> Change {
        Change {
            name,
            ns: Ns::Value,
            before: Some(before),
            after: Some(after),
            kind: ChangeKind::Derived,
            independent: true,
        }
    }

    /// Never independent: nothing referencing it can be flipped without it.
    pub fn added(name: Symbol, after: DefHash) -> Change {
        Change {
            name,
            ns: Ns::Value,
            before: None,
            after: Some(after),
            kind: ChangeKind::Added,
            independent: false,
        }
    }

    /// Never independent: a baseline body still referencing it cannot be kept while it is deleted.
    pub fn removed(name: Symbol, before: DefHash) -> Change {
        Change {
            name,
            ns: Ns::Value,
            before: Some(before),
            after: None,
            kind: ChangeKind::Removed,
            independent: false,
        }
    }

    pub fn is_candidate(&self) -> bool {
        self.kind.is_candidate()
    }
}

/// Which definitions mention which, unioned over both configurations.
#[derive(Clone, Debug, Default)]
pub struct DepEdges {
    /// referent -> everything that mentions it.
    referrers: BTreeMap<Symbol, BTreeSet<Symbol>>,
}

impl DepEdges {
    pub fn new() -> DepEdges {
        DepEdges::default()
    }

    /// `from` mentions `to`.
    pub fn add(&mut self, from: Symbol, to: Symbol) {
        self.referrers.entry(to).or_default().insert(from);
    }

    pub fn extend_from_hashes(&mut self, hashes: &HashOutput) {
        for (from, deps) in &hashes.deps {
            for to in deps {
                self.add(from.clone(), to.clone());
            }
        }
    }

    pub fn referrers(&self, name: &Symbol) -> impl Iterator<Item = &Symbol> {
        self.referrers.get(name).into_iter().flatten()
    }

    pub fn is_empty(&self) -> bool {
        self.referrers.is_empty()
    }
}

impl From<&HashOutput> for DepEdges {
    fn from(hashes: &HashOutput) -> DepEdges {
        let mut edges = DepEdges::new();
        edges.extend_from_hashes(hashes);
        edges
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FusionReason {
    /// Its interface is unchanged, so it stands alone and the search can name it exactly.
    Independent,
    /// Its scheme or footprint moved, so splitting it from its callers would not typecheck.
    InterfaceChanged,
    /// It exists on only one side, so nothing that mentions it can be flipped without it.
    Existence,
    /// Its members are mutually recursive, so they share one component hash and one stored body.
    Component,
}

impl FusionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            FusionReason::Independent => "independent",
            FusionReason::InterfaceChanged => "interface changed",
            FusionReason::Existence => "added or removed",
            FusionReason::Component => "one recursive component",
        }
    }

    /// Says *why* a fused group's members are inseparable.
    pub fn describe(self) -> &'static str {
        match self {
            FusionReason::Independent => "nothing forced these together",
            FusionReason::InterfaceChanged => {
                "their published interfaces moved together, so no hybrid that split them \
                 would typecheck"
            }
            FusionReason::Existence => {
                "one of them exists on only one side, so nothing that mentions it can be \
                 flipped without it"
            }
            FusionReason::Component => {
                "they are mutually recursive and share one component hash, so no hybrid can \
                 flip one without the other"
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cluster {
    /// Program-wide names, ascending: the order is part of the artifact.
    pub members: Vec<Symbol>,
    /// `members` with namespaces: a name that is both a `fn` and a `type` must say which to swap.
    pub keys: Vec<DefKey>,
    pub reason: FusionReason,
}

impl Cluster {
    pub fn is_singleton(&self) -> bool {
        self.members.len() == 1
    }
}

/// What moved since the test last passed, classified and grouped into what the search may flip.
#[derive(Clone, Debug, Default)]
pub struct Delta {
    /// The test's own definition, when the test body itself was edited.
    pub test: Option<Change>,
    pub changes: Vec<Change>,
    /// The atoms of the search, in ascending order of first member.
    pub clusters: Vec<Cluster>,
    /// How many changes could not be told apart from a hash that merely moved.
    pub unclassified: usize,
}

impl Delta {
    /// Classifies `changes` and fuses those that cannot be flipped apart.
    pub fn new(test: Option<Change>, changes: Vec<Change>, edges: &DepEdges) -> Delta {
        Delta::with_components(test, changes, edges, &[])
    }

    /// [`Delta::new`], also fusing the members of each strongly connected component.
    pub fn with_components(
        test: Option<Change>,
        changes: Vec<Change>,
        edges: &DepEdges,
        components: &[Vec<DefKey>],
    ) -> Delta {
        let candidates: Vec<usize> = (0..changes.len())
            .filter(|&i| changes[i].is_candidate())
            .collect();
        let mut slot: BTreeMap<DefKey, usize> = BTreeMap::new();
        for (at, &i) in candidates.iter().enumerate() {
            slot.insert(changes[i].key(), at);
        }
        // Fused by name: `DepEdges` is a name graph, and a mention reaches either namespace.
        let mut by_name: BTreeMap<&Symbol, Vec<usize>> = BTreeMap::new();
        for (at, &i) in candidates.iter().enumerate() {
            by_name.entry(&changes[i].name).or_default().push(at);
        }

        let mut parent: Vec<usize> = (0..candidates.len()).collect();
        let mut component_fused = vec![false; candidates.len()];
        for (at, &i) in candidates.iter().enumerate() {
            if changes[i].independent {
                continue;
            }
            for referrer in edges.referrers(&changes[i].name) {
                for &other in by_name.get(referrer).into_iter().flatten() {
                    union(&mut parent, at, other);
                }
            }
        }
        for component in components {
            let members: Vec<usize> = component
                .iter()
                .filter_map(|k| slot.get(k))
                .copied()
                .collect();
            let Some((&first, rest)) = members.split_first() else {
                continue;
            };
            for &other in rest {
                component_fused[first] = true;
                component_fused[other] = true;
                union(&mut parent, first, other);
            }
        }

        let mut grouped: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for at in 0..candidates.len() {
            grouped.entry(find(&mut parent, at)).or_default().push(at);
        }

        let mut clusters: Vec<Cluster> = grouped
            .into_values()
            .map(|slots| {
                let members: Vec<usize> = slots.iter().map(|&at| candidates[at]).collect();
                let reason = if members
                    .iter()
                    .any(|&i| matches!(changes[i].kind, ChangeKind::Added | ChangeKind::Removed))
                {
                    FusionReason::Existence
                } else if slots.iter().any(|&at| component_fused[at]) {
                    FusionReason::Component
                } else if members.iter().all(|&i| changes[i].independent) {
                    FusionReason::Independent
                } else {
                    FusionReason::InterfaceChanged
                };
                let mut keys: Vec<DefKey> = members.iter().map(|&i| changes[i].key()).collect();
                keys.sort();
                let mut members: Vec<Symbol> = keys.iter().map(|k| k.name.clone()).collect();
                members.sort();
                members.dedup();
                Cluster {
                    members,
                    keys,
                    reason,
                }
            })
            .collect();
        clusters.sort_by(|a, b| a.keys.cmp(&b.keys));

        Delta {
            test,
            changes,
            clusters,
            unclassified: 0,
        }
    }

    pub fn candidates(&self) -> usize {
        self.changes.iter().filter(|c| c.is_candidate()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }

    /// Prefers a candidate over a `Derived` change sharing its name, so the edit is what shows.
    pub fn change(&self, name: &Symbol) -> Option<&Change> {
        self.changes
            .iter()
            .find(|c| &c.name == name && c.is_candidate())
            .or_else(|| self.changes.iter().find(|c| &c.name == name))
    }

    pub fn change_of(&self, key: &DefKey) -> Option<&Change> {
        self.changes.iter().find(|c| c.key() == *key)
    }

    /// The definitions a hybrid takes from the post-edit side, given the chosen cluster indices.
    pub fn flipped_names(&self, flipped: &[usize]) -> Vec<Symbol> {
        let mut out: Vec<Symbol> = flipped
            .iter()
            .filter_map(|&i| self.clusters.get(i))
            .flat_map(|c| c.members.iter().cloned())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// [`Delta::flipped_names`] without the namespace collapse.
    pub fn flipped_keys(&self, flipped: &[usize]) -> Vec<DefKey> {
        let mut out: Vec<DefKey> = flipped
            .iter()
            .filter_map(|&i| self.clusters.get(i))
            .flat_map(|c| c.keys.iter().cloned())
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (a, b) = (find(parent, a), find(parent, b));
    if a != b {
        parent[a.max(b)] = a.min(b);
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

#[derive(Clone, Debug, Default)]
pub struct Diff {
    pub delta: Delta,
    /// Definitions whose `Edited`/`Derived` split could not be decided.
    pub unclassified: Vec<Symbol>,
    /// The test's own hash moved and nothing could say whether its body was edited.
    pub test_unclassified: bool,
}

pub struct Regression<'a> {
    /// `<module>.<label>`.
    pub key: &'a Symbol,
    pub test_hash: Option<DefHash>,
    pub baseline: &'a Baseline,
    pub hashes: &'a HashOutput,
}

pub fn diff(regression: &Regression<'_>, classify: &mut dyn Classify, edges: &DepEdges) -> Diff {
    let key = regression.key;
    let current = regression.hashes;
    let baseline = regression.baseline;

    // Both namespaces, separately.
    let mut keys: BTreeSet<DefKey> = baseline.keys().collect();
    for name in current.closure.get(key).into_iter().flatten() {
        if current.defs.contains_key(name) {
            keys.insert(DefKey::value(name.clone()));
        }
        if current.decls.contains_key(name) {
            keys.insert(DefKey::decl(name.clone()));
        }
    }
    keys.remove(&DefKey::value(key.clone()));
    keys.remove(&DefKey::decl(key.clone()));

    // A rename moves a name but no hash, so a vanished name whose hash remains did not go anywhere.
    let now: BTreeSet<DefHash> = current
        .defs
        .values()
        .chain(current.decls.values())
        .copied()
        .collect();
    let then: BTreeSet<DefHash> = baseline.hashes().collect();
    let renamed_into = classify.baseline_image();

    let mut unclassified = Vec::new();
    let mut changes = Vec::new();
    for key in &keys {
        let before = baseline.hash_of(key);
        let after = match key.ns {
            Ns::Value => current.defs.get(&key.name).copied(),
            Ns::Decl => current.decls.get(&key.name).copied(),
        };
        let change = match (before, after) {
            (Some(before), Some(after)) if before == after => continue,
            (None, None) => continue,
            (None, Some(after)) if then.contains(&after) => continue,
            (Some(before), None) if now.contains(&before) => continue,
            (Some(before), None) if renamed_into.contains(&before) => continue,
            (None, Some(after)) => match classify.renormalized(key) {
                // Renamed, and its hash moved as well because something under it was edited.
                Some(was) if then.contains(&was) => Change::derived(key.name.clone(), was, after),
                _ => Change::added(key.name.clone(), after),
            },
            (Some(before), None) => Change::removed(key.name.clone(), before),
            (Some(before), Some(after)) => match classify.renormalized(key) {
                Some(rehashed) if rehashed == before => {
                    Change::derived(key.name.clone(), before, after)
                }
                answer => {
                    if answer.is_none() {
                        unclassified.push(key.name.clone());
                    }
                    let independent = classify.interface_stable(key, before) == Some(true);
                    Change::edited(key.name.clone(), before, after, independent)
                }
            },
        };
        changes.push(change.in_namespace(key.ns));
    }

    let mut components: Vec<Vec<DefKey>> = Vec::new();
    for change in &changes {
        if !change.is_candidate() {
            continue;
        }
        let members = classify.component(&change.key());
        if members.len() > 1 && !components.contains(&members) {
            components.push(members);
        }
    }

    let mut test_unclassified = false;
    let test = match regression.test_hash {
        Some(after) if after != baseline.test_hash => match classify.renormalized_test(key) {
            Some(rehashed) if rehashed == baseline.test_hash => None,
            Some(_) => Some(Change::edited(key.clone(), baseline.test_hash, after, true)),
            None => {
                test_unclassified = true;
                None
            }
        },
        _ => None,
    };

    let mut delta = Delta::with_components(test, changes, edges, &components);
    delta.unclassified = unclassified.len() + usize::from(test_unclassified);

    Diff {
        delta,
        unclassified,
        test_unclassified,
    }
}

/// `--bisect`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Auto,
    /// Ignore the budget, nothing else: waiving a precondition would invent evidence.
    Always,
    Never,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Always => "always",
            Mode::Never => "never",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "auto" => Some(Mode::Auto),
            "always" => Some(Mode::Always),
            "never" => Some(Mode::Never),
            _ => None,
        }
    }

    pub fn budget(self, requested: Budget) -> Budget {
        match self {
            Mode::Always => Budget::UNLIMITED,
            _ => requested,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Gate<'a> {
    pub mode: Mode,
    pub defect: bool,
    pub host: bool,
    pub nondet: bool,
    pub baseline: Option<&'a Baseline>,
}

impl<'a> Gate<'a> {
    /// The hermetic gate: everything a caller with no host binding needs.
    pub fn new(mode: Mode, defect: bool, nondet: bool, baseline: Option<&'a Baseline>) -> Gate<'a> {
        Gate {
            mode,
            defect,
            host: false,
            nondet,
            baseline,
        }
    }

    pub fn hosted(mut self, host: bool) -> Gate<'a> {
        self.host = host;
        self
    }
}

/// Checked in order of what each answer is worth to a consumer.
pub fn precheck(gate: Gate<'_>) -> Result<(), Skipped> {
    if gate.mode == Mode::Never {
        return Err(Skipped::NotRequested);
    }
    if gate.defect {
        return Err(Skipped::Panicked);
    }
    if gate.host {
        return Err(Skipped::Host);
    }
    if gate.nondet {
        return Err(Skipped::Nondet);
    }
    if gate.baseline.is_none() {
        return Err(Skipped::NeverPassed);
    }
    Ok(())
}

pub struct NoHybrid;

impl Hybrid for NoHybrid {
    fn trial(&mut self, _: &Delta, _: &[usize]) -> Trial {
        Trial::unresolved(Unresolved::MissingBody)
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

pub trait Hybrid {
    fn trial(&mut self, delta: &Delta, flipped: &[usize]) -> Trial;
}

/// A cap in hybrid *evaluations*, not seconds, so the artifact does not vary with machine load.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Budget {
    pub max_trials: usize,
}

impl Budget {
    /// Enough for a clean bisection over ~2^30 candidates, small enough not to outlast the run.
    pub const DEFAULT: Budget = Budget { max_trials: 64 };

    pub fn new(max_trials: usize) -> Budget {
        Budget { max_trials }
    }

    /// `--bisect=always` still needs a ceiling; this is one nothing realistic reaches.
    pub const UNLIMITED: Budget = Budget {
        max_trials: usize::MAX,
    };
}

impl Default for Budget {
    fn default() -> Budget {
        Budget::DEFAULT
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

/// Finds the smallest set of changes that reproduces the failure.
pub fn bisect(delta: &Delta, hybrid: &mut dyn Hybrid, budget: Budget) -> Bisection {
    let mut search = Search {
        delta,
        hybrid,
        budget,
        stats: SearchStats {
            candidates: delta.candidates(),
            clusters: delta.clusters.len(),
            ..SearchStats::default()
        },
        memo: BTreeMap::new(),
    };
    search.run()
}

struct Search<'a> {
    delta: &'a Delta,
    hybrid: &'a mut dyn Hybrid,
    budget: Budget,
    stats: SearchStats,
    memo: BTreeMap<Vec<usize>, TrialOutcome>,
}

impl Search<'_> {
    fn run(&mut self) -> Bisection {
        let n = self.delta.clusters.len();
        if n == 0 {
            return match &self.delta.test {
                Some(test) => self.test_changed(test.name.clone()),
                None => Bisection::not_attempted(Skipped::NoChanges),
            };
        }
        if n == 1 {
            // One cluster is free unless the test was edited too; then `H(∅)` tells the two apart.
            let sole = self.delta.test.is_none() || self.ask(&[]) != TrialOutcome::Fails;
            let cluster = &self.delta.clusters[0];
            if sole {
                return self.conclude(
                    Verdict::Sole,
                    vec![cluster.members.clone()],
                    format!(
                        "only one change could be flipped: {}",
                        join(&cluster.members)
                    ),
                );
            }
            let name = self.delta.test.as_ref().map(|t| t.name.clone());
            return match name {
                Some(name) => self.test_changed(name),
                None => Bisection::not_attempted(Skipped::NoChanges),
            };
        }

        let all: Vec<usize> = (0..n).collect();
        match self.ask(&all) {
            TrialOutcome::Fails => {}
            TrialOutcome::Passes => {
                return self.conclude(
                    Verdict::NotReproduced,
                    Vec::new(),
                    "replaying the current program did not reproduce the failure; \
                     re-run the test before acting on it"
                        .to_string(),
                );
            }
            TrialOutcome::Unresolved(why) => {
                return self.conclude(
                    Verdict::Inconclusive,
                    Vec::new(),
                    format!(
                        "the current program could not be replayed: {}",
                        why.as_str()
                    ),
                );
            }
        }

        if self.ask(&[]) == TrialOutcome::Fails {
            return match &self.delta.test {
                Some(test) => {
                    let name = test.name.clone();
                    self.test_changed(name)
                }
                None => self.conclude(
                    Verdict::NotInTheGraph,
                    Vec::new(),
                    "the failure reproduces against the definitions as they were when this test \
                     last passed, and the test itself did not change — nothing in the definition \
                     graph explains it. Look for a `nondet` effect, something outside the program, \
                     or a defect in Ply"
                        .to_string(),
                ),
            };
        }

        let minimal = self.ddmin(all);
        let groups: Vec<Vec<Symbol>> = minimal
            .iter()
            .map(|&i| self.delta.clusters[i].members.clone())
            .collect();

        // Narrowing nothing around unanswerable mixtures is not a bisection.
        if minimal.len() == n && self.stats.unresolved > 0 {
            return self.conclude(
                Verdict::Inconclusive,
                groups,
                format!(
                    "no mixture of the {n} changes could be evaluated: {} of them did not \
                     typecheck or could not be built, so every change is still a candidate",
                    self.stats.unresolved
                ),
            );
        }

        let reason = format!(
            "narrowed {} changed {} to {} in {} {} ({} answered from the cache)",
            self.stats.candidates,
            plural(self.stats.candidates, "definition"),
            join(&groups.concat()),
            self.stats.evaluated,
            plural(self.stats.evaluated, "run"),
            self.stats.cached,
        );
        self.conclude(Verdict::Bisected, groups, reason)
    }

    /// The constraint that fused a multi-member group, so it is not read as a failed search.
    fn fused_because(&self) -> Option<&'static str> {
        let mut reasons: Vec<FusionReason> = self
            .delta
            .clusters
            .iter()
            .filter(|c| !c.is_singleton())
            .map(|c| c.reason)
            .collect();
        reasons.sort_by_key(|r| r.as_str());
        reasons.dedup();
        match reasons.as_slice() {
            [only] => Some(only.describe()),
            _ => None,
        }
    }

    fn test_changed(&self, name: Symbol) -> Bisection {
        self.conclude(
            Verdict::TestChanged,
            vec![vec![name.clone()]],
            format!(
                "the failure does not turn on any definition that changed, and `{name}` was \
                 itself edited — the edit to the test is what to look at"
            ),
        )
    }

    /// Zeller's ddmin over cluster indices, three-valued so an unresolved mixture is not evidence.
    fn ddmin(&mut self, mut set: Vec<usize>) -> Vec<usize> {
        let mut parts = 2usize;
        'outer: while set.len() > 1 && !self.stats.exhausted {
            let chunks = split(&set, parts.min(set.len()));

            for chunk in &chunks {
                if self.ask(chunk) == TrialOutcome::Fails {
                    set = chunk.clone();
                    parts = 2;
                    continue 'outer;
                }
                if self.stats.exhausted {
                    break 'outer;
                }
            }

            for chunk in &chunks {
                let rest: Vec<usize> = set.iter().copied().filter(|i| !chunk.contains(i)).collect();
                if rest.is_empty() {
                    continue;
                }
                if self.ask(&rest) == TrialOutcome::Fails {
                    set = rest;
                    parts = parts.saturating_sub(1).max(2);
                    continue 'outer;
                }
                if self.stats.exhausted {
                    break 'outer;
                }
            }

            if parts >= set.len() {
                break;
            }
            parts = (parts * 2).min(set.len());
        }
        set
    }

    fn ask(&mut self, set: &[usize]) -> TrialOutcome {
        let mut key = set.to_vec();
        key.sort_unstable();
        key.dedup();
        if let Some(outcome) = self.memo.get(&key) {
            self.stats.memoized += 1;
            return *outcome;
        }
        if self.stats.evaluated >= self.budget.max_trials {
            self.stats.exhausted = true;
            return TrialOutcome::Unresolved(Unresolved::BudgetSpent);
        }

        let trial = self.hybrid.trial(self.delta, &key);
        if trial.cached {
            self.stats.cached += 1;
        } else {
            self.stats.evaluated += 1;
        }
        if matches!(trial.outcome, TrialOutcome::Unresolved(_)) {
            self.stats.unresolved += 1;
        }
        self.memo.insert(key, trial.outcome);
        trial.outcome
    }

    fn conclude(&self, verdict: Verdict, groups: Vec<Vec<Symbol>>, reason: String) -> Bisection {
        // Any unresolved trial disqualifies minimality: the search walked around a question.
        let confidence = if groups.is_empty() {
            Confidence::None
        } else if self.stats.exhausted || self.stats.unresolved > 0 || self.delta.unclassified > 0 {
            Confidence::Partial
        } else if groups.iter().all(|g| g.len() == 1) {
            Confidence::Minimal
        } else {
            Confidence::Fused
        };
        let reason = match self.fused_because() {
            Some(why) => format!("{reason}; {why}"),
            None => reason,
        };
        Bisection {
            verdict,
            confidence,
            groups,
            reason,
            search: self.stats,
        }
    }
}

/// Near-equal chunks in index order, so the trial sequence is reproducible.
fn split(set: &[usize], parts: usize) -> Vec<Vec<usize>> {
    let parts = parts.clamp(1, set.len().max(1));
    let mut chunks = Vec::with_capacity(parts);
    let mut start = 0usize;
    for p in 0..parts {
        let end = set.len() * (p + 1) / parts;
        if end > start {
            chunks.push(set[start..end].to_vec());
            start = end;
        }
    }
    chunks
}

fn join(names: &[Symbol]) -> String {
    names
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

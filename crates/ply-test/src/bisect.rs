//! Bisection over the definition graph.
//!
//! A test that passed at one definition set and fails at another was broken by
//! some subset of the definitions that moved between them. Because compilation
//! is content-addressed, a *hybrid* program — some definitions at their old
//! hashes, the rest at their new ones — is a legitimate program whose test hash
//! is a legitimate cache key, so most of the search is answered without
//! evaluating anything. That is why this milestone comes after the incremental
//! front end rather than before it.
//!
//! This module owns the part of that which is a pure search problem: what the
//! candidates are, which of them may be flipped apart, and how to find the
//! smallest failure-inducing set. Materializing a hybrid program from stored
//! bodies is behind [`Hybrid`], because it needs a store that holds definition
//! bodies — see `docs/adr/0003` and `docs/adr/0004`.

pub mod classify;
pub mod renormalize;

pub use classify::{Classify, StoreClassify, Unknown};
pub use renormalize::{EraTable, Renormalizer};

#[cfg(test)]
mod delta_tests;

use ply_hash::{DefHash, HashOutput};
use ply_span::Symbol;
use std::collections::{BTreeMap, BTreeSet};

// --------------------------------------------------------------- namespaces

/// Which namespace a program-wide name is being read in.
///
/// A `fn`, a `type` and an `effect` may share a name — they are separate
/// namespaces — and they hash into separate maps. A bare name is therefore not
/// enough to look a definition up by, and resolving one as
/// `defs.get(name).or(decls.get(name))` silently drops whichever the other one
/// is: the declaration's hash is never recorded, its edit never compared, and
/// the definitions that mention it are re-normalized against a table that hands
/// them the function's hash instead.
///
/// `Value` covers functions and constructors; `Decl` covers `type` and `effect`.
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

/// The identity of one definition across two configurations: its program-wide
/// name and the namespace that name is read in.
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

// ------------------------------------------------------------------ changes

/// Why a definition's hash differs between the two configurations.
///
/// The distinction that matters is `Edited` versus `Derived`. A reference
/// contributes the referent's hash, so editing one definition moves the hash of
/// every transitive dependent. Only the edited ones are candidates: there is no
/// change to attribute to a definition whose text nobody touched, and offering
/// one as a suspect is the noise M5 exists to remove.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChangeKind {
    /// Its own normalized body differs.
    Edited,
    /// Its body is byte-identical; its hash moved only because a dependency's
    /// did.
    Derived,
    /// Present now, absent from the baseline.
    Added,
    /// Present in the baseline, absent now.
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

    /// Whether flipping this definition is a question worth asking. A derived
    /// change carries no edit, and its body is the same on both sides, so
    /// flipping it is a no-op.
    pub fn is_candidate(self) -> bool {
        !matches!(self, ChangeKind::Derived)
    }
}

/// One definition's identity on both sides of the edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The program-wide name — `store.orders.place`.
    pub name: Symbol,
    pub ns: Ns,
    /// Its hash when the test last passed. `None` for a definition that did not
    /// exist then.
    pub before: Option<DefHash>,
    /// Its hash now. `None` for a definition that has since been deleted.
    pub after: Option<DefHash>,
    pub kind: ChangeKind,
    /// Whether this definition's *published interface* — its scheme and its
    /// footprint — is the same on both sides.
    ///
    /// This is the whole answer to "does the hybrid typecheck". A definition
    /// whose interface is unchanged can be swapped under its callers without any
    /// of them noticing; one whose interface moved cannot, and is fused with the
    /// callers that had to change with it. The caller computes this by comparing
    /// the stored interface for [`Change::before`] against the freshly inferred
    /// one; when the stored side is unavailable it must pass `false`, which
    /// costs a larger cluster and never a wrong answer.
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

    /// The same change, read in `ns`. Every constructor above defaults to the
    /// value namespace, which is where all but a `type` or an `effect` lives.
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

    /// A hash that moved only because a dependency's did. Never a candidate, so
    /// its `independent` flag is not consulted.
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

    /// Nothing that references a definition can be flipped without the
    /// definition itself, so an added one is never independent.
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

    /// Symmetrically: a baseline body that still references it cannot be kept
    /// while it is deleted.
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

// ------------------------------------------------------------------- edges

/// Which definitions mention which, unioned over both configurations.
///
/// Both eras are needed and neither is optional. Fusing on the current graph
/// alone misses a baseline body that references a definition since deleted;
/// fusing on the baseline alone misses a caller written against a definition
/// since added. Unioning over-approximates, which merges two clusters that could
/// have been searched apart — a slower search, never a wrong flip.
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

    /// Every direct reference in the program as it stands now.
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

// ---------------------------------------------------------------- clusters

/// Why a group of changes has to be flipped as one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FusionReason {
    /// Nothing forced it: this definition's interface is unchanged, so it stands
    /// alone and the search can name it exactly.
    Independent,
    /// Its scheme or footprint moved, so its callers had to move with it and a
    /// hybrid that split them would not typecheck.
    InterfaceChanged,
    /// It exists on only one side, so nothing that mentions it can be flipped
    /// without it.
    Existence,
    /// Its members are mutually recursive, so they share one component hash and
    /// one stored body. There is no mixture that flips one of them without the
    /// others: a body kept at its baseline still names its partner's baseline
    /// hash, so a hybrid that split them would silently measure the baseline and
    /// pass. Handing the search that partition would have it read two passing
    /// singletons as "each is independently necessary".
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

    /// The clause the artifact prints so that a fused group says *why* its
    /// members are inseparable rather than only that they are.
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

/// A set of changes the search treats as one atom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cluster {
    /// Program-wide names, ascending — the order is part of the artifact and has
    /// to be reproducible.
    pub members: Vec<Symbol>,
    /// The same members with their namespaces, which is what a hybrid has to
    /// flip: `members` deduplicates a name that is both a `fn` and a `type`, and
    /// a builder handed only that cannot tell which of the two to swap.
    pub keys: Vec<DefKey>,
    pub reason: FusionReason,
}

impl Cluster {
    pub fn is_singleton(&self) -> bool {
        self.members.len() == 1
    }
}

/// Everything that moved between the configuration a test last passed at and
/// the one it fails at, classified and grouped into what the search may flip.
#[derive(Clone, Debug, Default)]
pub struct Delta {
    /// The test's own definition, when the test body itself was edited. It is
    /// never a candidate: every hybrid runs the test as it is written now,
    /// because the failure being explained is *this* test's failure.
    pub test: Option<Change>,
    pub changes: Vec<Change>,
    /// The atoms of the search, in ascending order of first member.
    pub clusters: Vec<Cluster>,
    /// How many changes could not be told apart from a hash that merely moved.
    /// Each one is a split the search had to guess at, so a non-zero count is
    /// the same disqualification an unresolved *trial* is: the answer may be
    /// right, but it is not one the run can call minimal.
    pub unclassified: usize,
}

impl Delta {
    /// Classifies `changes` and fuses those that cannot be flipped apart.
    ///
    /// The fusion rule is one line: a change that is not `independent` is fused
    /// with every candidate that mentions it. Transitivity comes out of the
    /// union-find. That rule is exactly the typecheck condition — a caller only
    /// notices a callee being swapped when the callee's published interface
    /// moved, and a caller that had to be edited for that is itself a candidate.
    pub fn new(test: Option<Change>, changes: Vec<Change>, edges: &DepEdges) -> Delta {
        Delta::with_components(test, changes, edges, &[])
    }

    /// [`Delta::new`] plus the second thing no hybrid can separate: the members
    /// of a strongly connected component. They are hashed `blake3(component ‖
    /// index)` off one shared body, so flipping one alone is not a mixture that
    /// exists — see [`FusionReason::Component`]. Each entry of `components` is
    /// the membership of one such component; entries naming fewer than two
    /// candidates fuse nothing.
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
        // Fusion by reference is decided on names, because `DepEdges` is a name
        // graph: a mention reaches whichever namespace the referent lives in.
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

    /// Changes that carry an actual edit. Derived ones are excluded.
    pub fn candidates(&self) -> usize {
        self.changes.iter().filter(|c| c.is_candidate()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }

    /// A candidate is preferred over a `Derived` one sharing the name: a suspect
    /// annotated `derived` is one an agent stops reading, so where a `fn` and a
    /// `type` share a name and only one of them was edited, the edit is what the
    /// artifact has to show.
    pub fn change(&self, name: &Symbol) -> Option<&Change> {
        self.changes
            .iter()
            .find(|c| &c.name == name && c.is_candidate())
            .or_else(|| self.changes.iter().find(|c| &c.name == name))
    }

    pub fn change_of(&self, key: &DefKey) -> Option<&Change> {
        self.changes.iter().find(|c| c.key() == *key)
    }

    /// The definitions a hybrid must take from the post-edit side, given the
    /// cluster indices the search chose. This is the argument a [`Hybrid`]
    /// implementation actually needs.
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

    /// [`Delta::flipped_names`] without the namespace collapse. This is what a
    /// hybrid builder must use; the name list is for the artifact.
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

// ---------------------------------------------------------------- baseline

/// The definition set a test was last seen to pass at.
///
/// Keyed by the test's `<module>.<label>` rather than by its hash, which is the
/// one place in Ply a *name* is load-bearing for a cache and is deliberate: a
/// test's hash covers its whole closure, so a regression has a different hash
/// and there would be nothing to look up. Renaming a test's label therefore
/// loses its baseline, and that costs one missing bisection, never a wrong one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Baseline {
    pub test_hash: DefHash,
    /// Program-wide name -> hash, for every *function* in the closure then.
    pub closure: BTreeMap<Symbol, DefHash>,
    /// The same for the `type` and `effect` declarations. A second map rather
    /// than a namespace-keyed one because the first is what the recorded shape
    /// has always been; keeping the declarations out of it is what once hid an
    /// edit to a `type` whose name a `fn` also carried.
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
    /// Definitions whose `Edited`/`Derived` split could not be decided, and
    /// which are therefore candidates that may not have needed to be. A wider
    /// search, never a wrong one.
    pub unclassified: Vec<Symbol>,
    /// The test's own hash moved and nothing could say whether its body was
    /// edited or merely inherited the move. `Verdict::TestChanged` is withheld
    /// in that case, because accusing a test nobody touched is worse than
    /// reporting that no change explains the failure.
    pub test_unclassified: bool,
}

/// The failing test, and the configuration to compare it against.
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

    // Both namespaces, separately. Collapsing them — `defs.get(n).or(decls.get(n))`
    // — records one hash for a name that denotes two definitions, so an edit to
    // whichever loses is invisible and the winner's hash is handed to everything
    // that mentions either.
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

    // A rename moves a name and no hash, so a definition that has apparently
    // vanished but whose hash is still somewhere in the program did not go
    // anywhere. Reading it as a removal — and its new name as an addition —
    // would manufacture two candidates out of an edit nobody made, and would
    // break the one invariant the whole design rests on.
    //
    // The hash it kept is not enough on its own: rename a definition while
    // editing something under it and its hash moves too. What did not move is
    // its identity *in the baseline era*, which is what the re-normalized hash
    // is, so that is compared as well.
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
                // Renamed, and its hash moved as well because something under it
                // was edited. It is the same definition under a new label, so it
                // is no more a candidate than any other inherited move.
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

// ------------------------------------------------------------ preconditions

/// `--bisect`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Auto,
    /// Ignore the budget, and nothing else: no precondition can be waived
    /// without inventing evidence.
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

/// The order is the order the answers are worth: a consumer that reads
/// `never_passed` stops looking for a bug in the cache, and one that reads
/// `no_bodies` goes and un-prunes it.
pub fn precheck(
    mode: Mode,
    panicked: bool,
    nondet: bool,
    baseline: Option<&Baseline>,
) -> Result<(), Skipped> {
    if mode == Mode::Never {
        return Err(Skipped::NotRequested);
    }
    if panicked {
        return Err(Skipped::Panicked);
    }
    if nondet {
        return Err(Skipped::Nondet);
    }
    if baseline.is_none() {
        return Err(Skipped::NeverPassed);
    }
    Ok(())
}

/// The absence of a hybrid builder. Every mixture is unanswerable, so a search
/// handed this one can still return the verdicts that need no mixture and must
/// refuse the rest rather than concluding from silence.
pub struct NoHybrid;

impl Hybrid for NoHybrid {
    fn trial(&mut self, _: &Delta, _: &[usize]) -> Trial {
        Trial::unresolved(Unresolved::MissingBody)
    }
}

// ------------------------------------------------------------------- trials

/// Why a hybrid could not answer the question.
///
/// None of these is an error to report to a user. They are the rough ground the
/// search walks around, and their count is in the artifact so that a consumer
/// can tell a clean bisection from one that had to guess.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unresolved {
    /// Old and new disagree about a signature, so this particular mixture is not
    /// a well-typed program. Common rather than exotic — see the ADR.
    DoesNotCheck,
    /// It failed, but not with the failure being explained. A different failure
    /// is not evidence about this one.
    DifferentFailure,
    /// The store cannot produce a body for some hash this mixture needs.
    MissingBody,
    /// The search hit its budget before asking.
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
    /// Answered from the result cache rather than by evaluating anything. A
    /// cached trial costs nothing and is not charged against the budget, which
    /// is the concrete form of "bisection is nearly free once builds are
    /// cached".
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

/// Builds and evaluates one hybrid program.
///
/// The implementation lives wherever definition bodies do. It must run the test
/// **as it is written now** against a program in which exactly the definitions
/// named by `delta.flipped_names(flipped)` carry their post-edit bodies and
/// every other definition in the test's closure carries its baseline body.
pub trait Hybrid {
    fn trial(&mut self, delta: &Delta, flipped: &[usize]) -> Trial;
}

// ------------------------------------------------------------------ budget

/// A cap in hybrid *evaluations*, deliberately not in seconds: a failure
/// artifact that varies with machine load is not one an agent can diff against
/// yesterday's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Budget {
    pub max_trials: usize,
}

impl Budget {
    /// Enough for a clean bisection over roughly 2^30 candidates, and small
    /// enough that a pathological search cannot outlast the test run it explains.
    pub const DEFAULT: Budget = Budget { max_trials: 64 };

    pub fn new(max_trials: usize) -> Budget {
        Budget { max_trials }
    }

    /// `--bisect=always` still needs a ceiling; this is one nothing realistic
    /// reaches.
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
    /// Hybrids actually built and run. The budget caps this and nothing else.
    pub evaluated: usize,
    /// Hybrids the result cache answered for free.
    pub cached: usize,
    /// Subsets the search would have asked about twice.
    pub memoized: usize,
    pub unresolved: usize,
    /// The budget ran out before the search finished, so the result is a
    /// superset of the cause rather than a minimal set.
    pub exhausted: bool,
}

// ----------------------------------------------------------------- verdict

/// What was skipped, and why. Each variant is a different thing for a consumer
/// to do about it, which is the only reason to distinguish them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Skipped {
    /// `--bisect=never`, or a run that never asked.
    NotRequested,
    /// This test key has no recorded pass. There is no "before" to bisect
    /// against, and a first-ever red test is a different situation from a
    /// regression.
    NeverPassed,
    /// `test/nondet` outcomes are not a function of the definition set, so a
    /// hybrid's answer would not be evidence about anything.
    Nondet,
    /// The interpreter panicked. That is a defect in Ply, not a change to
    /// attribute.
    Panicked,
    /// Baseline and current agree on every definition in the closure.
    NoChanges,
    /// The store cannot produce the bodies a hybrid needs.
    NoBodies,
    /// The bodies are there, but this build has no way to assemble them into a
    /// mixed program. Distinct from [`Skipped::NoBodies`] because the two ask
    /// different things of a consumer: that one is fixed by not pruning, this
    /// one cannot be fixed from outside.
    ///
    /// A body is hash-linked, so a caller kept at its baseline still names its
    /// callee's baseline hash. Flipping the callee alone therefore leaves the
    /// caller pointing at the version that was not flipped, and the trial
    /// silently measures nothing. Mixing eras needs the intermediate bodies
    /// re-linked against the mixture's own hash table, which is a relinker
    /// `ply-hash` does not have yet.
    NoHybrids,
}

impl Skipped {
    pub fn as_str(self) -> &'static str {
        match self {
            Skipped::NotRequested => "not_requested",
            Skipped::NeverPassed => "never_passed",
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
            Skipped::Nondet => {
                "`test/nondet` is not a function of the definition set, so bisecting it would prove nothing"
            }
            Skipped::Panicked => {
                "the interpreter panicked; this is a defect in Ply, not in the program"
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
    /// The search ran and narrowed the change set.
    Bisected,
    /// Exactly one change could be flipped, so the answer needed no runs at all.
    Sole,
    /// The baseline definitions with this test's current body already fail: the
    /// edit to the test is the change that matters.
    TestChanged,
    /// The same, but the test was not edited — so nothing in the definition
    /// graph explains this failure. Look at a `nondet` effect, the environment,
    /// or Ply itself.
    NotInTheGraph,
    /// The current program did not reproduce the failure when replayed.
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

/// How much the culprit set may be trusted. A consumer acts differently on each.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confidence {
    /// One definition per group, and dropping any group makes the failure go
    /// away: the set is 1-minimal.
    Minimal,
    /// Some group could not be split, because its members' interfaces changed
    /// together. One of them is the cause.
    Fused,
    /// The search stopped early. The set contains the cause and probably more.
    Partial,
    /// No search ran.
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

/// The result of attributing one failure to a change.
#[derive(Clone, Debug)]
pub struct Bisection {
    pub verdict: Verdict,
    pub confidence: Confidence,
    /// The minimal failure-inducing change set, one entry per fused group.
    /// Groups and members are both sorted, so two runs over the same inputs
    /// produce byte-identical artifacts.
    pub groups: Vec<Vec<Symbol>>,
    /// One sentence saying what happened and what to do about it. Rendered
    /// verbatim in the terminal and carried in the JSON.
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

// ------------------------------------------------------------------ search

/// Finds the smallest set of changes that reproduces the failure.
///
/// Delta debugging rather than a plain binary search, because "flip half and see"
/// assumes one cause and two edits that only break the test together are not
/// exotic. The single-cause case is the fast path through the same algorithm and
/// costs `2·log2(n)` trials, which is what makes this affordable at all.
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
            // Nothing to flip. If the test moved, it is the only thing that did
            // and no hybrid can say more than that.
            return match &self.delta.test {
                Some(test) => self.test_changed(test.name.clone()),
                None => Bisection::not_attempted(Skipped::NoChanges),
            };
        }
        if n == 1 {
            // One cluster is answered for free — unless the test was edited too,
            // in which case the one definition that moved may be innocent and
            // `H(∅)` is the question that separates them. Naming it without
            // asking is how a bisection gets a confident wrong answer.
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

        // Narrowing nothing while walking around unanswerable mixtures is not a
        // bisection, and calling it one would have a consumer act on the whole
        // change set as if the search had endorsed it.
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

    /// A group of more than one is an answer a consumer has to read differently,
    /// so the artifact says which constraint fused it rather than leaving the
    /// reader to guess whether the search failed or the graph forbade the split.
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

    /// Zeller's ddmin over cluster indices, with the outcome three-valued so an
    /// unresolved mixture simply is not evidence and the partition refines past
    /// it.
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
        // An unresolved trial anywhere disqualifies the minimality claim, even
        // one off the path to the answer: the search walked around a question it
        // could not ask, so it cannot say that dropping any group would make the
        // failure go away.
        //
        // An unresolved *classification* costs exactly the same. A change that
        // could not be told apart from a hash that merely moved was carried into
        // the search as a candidate on a guess, so the partition itself is a
        // guess — and a `minimal` verdict over a guessed partition is the one
        // way this artifact can be confidently wrong.
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

/// Near-equal chunks in index order, so a run's partition — and therefore its
/// trial sequence — is reproducible.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(s: &str) -> Symbol {
        Symbol::new(s)
    }

    fn hash(byte: u8) -> DefHash {
        DefHash([byte; 32])
    }

    fn edges(pairs: &[(&str, &str)]) -> DepEdges {
        let mut edges = DepEdges::new();
        for (from, to) in pairs {
            edges.add(sym(from), sym(to));
        }
        edges
    }

    /// Answers `Fails` exactly when the flipped set covers every culprit.
    struct Culprits {
        culprits: Vec<Symbol>,
        /// Sets whose flipped names split one of these pairs do not typecheck.
        inseparable: Vec<(Symbol, Symbol)>,
        asked: Vec<Vec<usize>>,
        cached: BTreeSet<Vec<usize>>,
    }

    impl Culprits {
        fn new(culprits: &[&str]) -> Culprits {
            Culprits {
                culprits: culprits.iter().map(|c| sym(c)).collect(),
                inseparable: Vec::new(),
                asked: Vec::new(),
                cached: BTreeSet::new(),
            }
        }
    }

    impl Hybrid for Culprits {
        fn trial(&mut self, delta: &Delta, flipped: &[usize]) -> Trial {
            self.asked.push(flipped.to_vec());
            let names = delta.flipped_names(flipped);
            for (a, b) in &self.inseparable {
                if names.contains(a) != names.contains(b) {
                    return Trial::unresolved(Unresolved::DoesNotCheck);
                }
            }
            let trial = if self.culprits.iter().all(|c| names.contains(c)) {
                Trial::fails()
            } else {
                Trial::passes()
            };
            if self.cached.contains(flipped) {
                trial.from_cache()
            } else {
                trial
            }
        }
    }

    fn independent_changes(names: &[&str]) -> Vec<Change> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| edited_change(n, i as u8))
            .collect()
    }

    fn edited_change(name: &str, seed: u8) -> Change {
        Change::edited(sym(name), hash(seed), hash(seed.wrapping_add(128)), true)
    }

    #[test]
    fn an_interface_stable_edit_is_its_own_cluster() {
        let delta = Delta::new(
            None,
            independent_changes(&["a", "b", "c"]),
            &edges(&[("b", "a"), ("c", "b")]),
        );
        assert_eq!(delta.clusters.len(), 3);
        assert!(
            delta
                .clusters
                .iter()
                .all(|c| c.reason == FusionReason::Independent)
        );
    }

    #[test]
    fn an_interface_change_fuses_with_the_callers_that_had_to_change_with_it() {
        let mut changes = independent_changes(&["caller", "other"]);
        changes.push(Change::edited(sym("callee"), hash(9), hash(10), false));
        // `caller` mentions `callee`; `other` mentions nothing that moved.
        let delta = Delta::new(None, changes, &edges(&[("caller", "callee")]));

        assert_eq!(delta.clusters.len(), 2);
        let fused = delta
            .clusters
            .iter()
            .find(|c| c.members.len() == 2)
            .expect("a fused cluster");
        assert_eq!(fused.members, vec![sym("callee"), sym("caller")]);
        assert_eq!(fused.reason, FusionReason::InterfaceChanged);
    }

    #[test]
    fn fusion_is_transitive_through_the_union_find() {
        let changes = vec![
            Change::edited(sym("a"), hash(1), hash(2), false),
            Change::edited(sym("b"), hash(3), hash(4), false),
            Change::edited(sym("c"), hash(5), hash(6), true),
        ];
        // c mentions b, b mentions a; a and b both moved their interfaces.
        let delta = Delta::new(None, changes, &edges(&[("b", "a"), ("c", "b")]));
        assert_eq!(delta.clusters.len(), 1);
        assert_eq!(
            delta.clusters[0].members,
            vec![sym("a"), sym("b"), sym("c")]
        );
    }

    #[test]
    fn an_added_definition_drags_in_everything_that_mentions_it() {
        let mut changes = independent_changes(&["caller"]);
        changes.push(Change::added(sym("helper"), hash(7)));
        let delta = Delta::new(None, changes, &edges(&[("caller", "helper")]));
        assert_eq!(delta.clusters.len(), 1);
        assert_eq!(delta.clusters[0].reason, FusionReason::Existence);
    }

    /// A removed definition is only reachable through the *baseline* edges, so
    /// the current program's graph alone would leave a dangling reference.
    #[test]
    fn a_removed_definition_fuses_through_baseline_edges() {
        let mut changes = independent_changes(&["keeper"]);
        changes.push(Change::removed(sym("gone"), hash(4)));
        let delta = Delta::new(None, changes, &edges(&[("keeper", "gone")]));
        assert_eq!(delta.clusters.len(), 1);
        assert_eq!(delta.clusters[0].members, vec![sym("gone"), sym("keeper")]);
    }

    #[test]
    fn a_derived_change_is_never_a_candidate() {
        let mut changes = independent_changes(&["edited"]);
        changes.push(Change::derived(sym("dependent"), hash(1), hash(2)));
        let delta = Delta::new(None, changes, &edges(&[("dependent", "edited")]));
        assert_eq!(delta.candidates(), 1);
        assert_eq!(delta.clusters.len(), 1);
        assert_eq!(delta.clusters[0].members, vec![sym("edited")]);
    }

    #[test]
    fn one_candidate_is_answered_without_running_anything() {
        let delta = Delta::new(None, independent_changes(&["only"]), &DepEdges::new());
        let mut oracle = Culprits::new(&["only"]);
        let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

        assert_eq!(out.verdict, Verdict::Sole);
        assert_eq!(out.confidence, Confidence::Minimal);
        assert_eq!(out.culprits(), vec![sym("only")]);
        assert_eq!(out.search.evaluated, 0);
        assert!(oracle.asked.is_empty());
    }

    #[test]
    fn a_single_culprit_among_sixteen_is_found_logarithmically() {
        let names: Vec<String> = (0..16).map(|i| format!("d{i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let delta = Delta::new(None, independent_changes(&refs), &DepEdges::new());
        let mut oracle = Culprits::new(&["d11"]);
        let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

        assert_eq!(out.verdict, Verdict::Bisected);
        assert_eq!(out.confidence, Confidence::Minimal);
        assert_eq!(out.culprits(), vec![sym("d11")]);
        // 2·log2(16) halvings, plus the reproduction trial and the baseline one.
        assert!(out.search.evaluated <= 12, "{:?}", out.search);
    }

    #[test]
    fn two_changes_that_only_fail_together_are_both_reported() {
        let delta = Delta::new(
            None,
            independent_changes(&["a", "b", "c", "d"]),
            &DepEdges::new(),
        );
        let mut oracle = Culprits::new(&["a", "d"]);
        let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

        assert_eq!(out.verdict, Verdict::Bisected);
        assert_eq!(out.culprits(), vec![sym("a"), sym("d")]);
        assert_eq!(out.confidence, Confidence::Minimal);
    }

    #[test]
    fn a_fused_group_is_reported_as_fused_rather_than_as_two_answers() {
        let mut changes = independent_changes(&["x", "y"]);
        changes.push(Change::edited(sym("callee"), hash(20), hash(21), false));
        let delta = Delta::new(None, changes, &edges(&[("x", "callee")]));
        let mut oracle = Culprits::new(&["callee"]);
        let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

        assert_eq!(out.verdict, Verdict::Bisected);
        assert_eq!(out.confidence, Confidence::Fused);
        assert_eq!(out.groups, vec![vec![sym("callee"), sym("x")]]);
    }

    /// The case the ADR insists is common: a mixture that does not typecheck is
    /// not evidence, so the search walks around it, keeps the pair it could not
    /// separate, and refuses to call the result minimal.
    ///
    /// This is also what the fusion pre-pass exists to avoid — had the caller
    /// marked `b` interface-breaking, `b` and `c` would have been one cluster
    /// and the answer would have been exact rather than a pair.
    #[test]
    fn hybrids_that_do_not_typecheck_are_not_evidence() {
        let delta = Delta::new(
            None,
            independent_changes(&["a", "b", "c", "d"]),
            &DepEdges::new(),
        );
        let mut oracle = Culprits::new(&["b"]);
        oracle.inseparable.push((sym("b"), sym("c")));
        let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

        assert_eq!(out.verdict, Verdict::Bisected);
        assert_eq!(out.culprits(), vec![sym("b"), sym("c")]);
        assert!(out.search.unresolved > 0, "{:?}", out.search);
        assert_eq!(out.confidence, Confidence::Partial);
    }

    #[test]
    fn a_spent_budget_downgrades_the_confidence_rather_than_lying() {
        let names: Vec<String> = (0..32).map(|i| format!("d{i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let delta = Delta::new(None, independent_changes(&refs), &DepEdges::new());
        let mut oracle = Culprits::new(&["d31"]);
        let out = bisect(&delta, &mut oracle, Budget::new(3));

        assert!(out.search.exhausted);
        assert_eq!(out.confidence, Confidence::Partial);
        assert_eq!(out.search.evaluated, 3);
        assert!(out.culprits().contains(&sym("d31")));
    }

    #[test]
    fn a_cached_trial_is_not_charged_against_the_budget() {
        let delta = Delta::new(None, independent_changes(&["a", "b"]), &DepEdges::new());
        let mut oracle = Culprits::new(&["a"]);
        oracle.cached.insert(vec![0, 1]);
        oracle.cached.insert(vec![]);
        let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

        assert_eq!(out.search.cached, 2);
        assert!(out.search.evaluated < out.search.cached + out.search.evaluated + 1);
        assert_eq!(out.culprits(), vec![sym("a")]);
    }

    #[test]
    fn a_failure_the_baseline_also_shows_is_attributed_to_the_test_when_it_moved() {
        let delta = Delta::new(
            Some(Change::edited(sym("m.a test"), hash(1), hash(2), true)),
            independent_changes(&["a", "b"]),
            &DepEdges::new(),
        );
        let mut oracle = Culprits::new(&[]); // fails for every subset, including none
        let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

        assert_eq!(out.verdict, Verdict::TestChanged);
        assert_eq!(out.culprits(), vec![sym("m.a test")]);
        assert!(out.reason.contains("edit to the test"));
    }

    #[test]
    fn a_failure_no_change_explains_says_so_instead_of_naming_someone() {
        let delta = Delta::new(None, independent_changes(&["a", "b"]), &DepEdges::new());
        let mut oracle = Culprits::new(&[]);
        let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

        assert_eq!(out.verdict, Verdict::NotInTheGraph);
        assert!(out.culprits().is_empty());
        assert_eq!(out.confidence, Confidence::None);
    }

    #[test]
    fn a_search_that_narrowed_nothing_is_inconclusive_rather_than_bisected() {
        struct Rough;
        impl Hybrid for Rough {
            fn trial(&mut self, delta: &Delta, flipped: &[usize]) -> Trial {
                if flipped.len() == delta.clusters.len() {
                    Trial::fails()
                } else if flipped.is_empty() {
                    Trial::passes()
                } else {
                    Trial::unresolved(Unresolved::DoesNotCheck)
                }
            }
        }
        let delta = Delta::new(
            None,
            independent_changes(&["a", "b", "c"]),
            &DepEdges::new(),
        );
        let out = bisect(&delta, &mut Rough, Budget::DEFAULT);

        assert_eq!(out.verdict, Verdict::Inconclusive);
        assert_eq!(out.confidence, Confidence::Partial);
        assert_eq!(out.culprits(), vec![sym("a"), sym("b"), sym("c")]);
        assert!(out.reason.contains("did not typecheck"));
    }

    #[test]
    fn a_failure_that_does_not_replay_is_reported_rather_than_bisected() {
        struct Green;
        impl Hybrid for Green {
            fn trial(&mut self, _: &Delta, _: &[usize]) -> Trial {
                Trial::passes()
            }
        }
        let delta = Delta::new(None, independent_changes(&["a", "b"]), &DepEdges::new());
        let out = bisect(&delta, &mut Green, Budget::DEFAULT);
        assert_eq!(out.verdict, Verdict::NotReproduced);
        assert!(out.culprits().is_empty());
    }

    #[test]
    fn nothing_changed_is_not_attempted_rather_than_inconclusive() {
        let delta = Delta::new(None, Vec::new(), &DepEdges::new());
        let mut oracle = Culprits::new(&[]);
        let out = bisect(&delta, &mut oracle, Budget::DEFAULT);
        assert_eq!(out.verdict, Verdict::NotAttempted(Skipped::NoChanges));
        assert!(oracle.asked.is_empty());
    }

    #[test]
    fn the_search_never_asks_the_same_question_twice() {
        let names: Vec<String> = (0..12).map(|i| format!("d{i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let delta = Delta::new(None, independent_changes(&refs), &DepEdges::new());
        let mut oracle = Culprits::new(&["d05"]);
        bisect(&delta, &mut oracle, Budget::DEFAULT);

        let mut seen: Vec<Vec<usize>> = oracle.asked.clone();
        let before = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), before, "a subset was evaluated twice");
    }

    /// The artifact is diffed against yesterday's, so the same inputs have to
    /// produce the same bytes — including the order of the culprit groups.
    #[test]
    fn the_same_inputs_produce_the_same_answer_every_time() {
        let names: Vec<String> = (0..9).map(|i| format!("d{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let run = || {
            let delta = Delta::new(None, independent_changes(&refs), &DepEdges::new());
            let mut oracle = Culprits::new(&["d3", "d7"]);
            let out = bisect(&delta, &mut oracle, Budget::DEFAULT);
            (out.groups, out.search, oracle.asked)
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn every_skip_reason_explains_itself_distinctly() {
        let all = [
            Skipped::NotRequested,
            Skipped::NeverPassed,
            Skipped::Nondet,
            Skipped::Panicked,
            Skipped::NoChanges,
            Skipped::NoBodies,
        ];
        let mut described: Vec<&str> = all.iter().map(|s| s.describe()).collect();
        described.sort_unstable();
        described.dedup();
        assert_eq!(described.len(), all.len());

        let mut codes: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
    }
}

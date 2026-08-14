//! Which selected tests may run at the same time.
//!
//! The conflict graph exists because tests share real state. State reached
//! through a `with_cell` region does not: it lives in a world, every test runs
//! against its own fork, and no reference crosses between two forks. Two tests
//! that both retain `cell.write[users]` name two different pieces of state, so
//! the graph is built over [`shared_footprint`]s — the atoms that escape.
//!
//! That is a claim about the execution model rather than about two atoms, which
//! is why `ply_core::EffectAtom::conflicts_with` is unchanged and the scheduler
//! is the one place that knows two tests get two worlds.
//!
//! **This module is about two tests, and none of it transfers to two tasks.**
//! Two tests hold two worlds; two tasks inside one simulated run hold one, so
//! `ply_eval::sim` keeps `cell` accesses in its dependence relation and must not
//! reuse [`shared_footprint`]. Dropping them there would prune away every
//! shared-memory race in the corpus while reporting a larger reduction for it.

use ply_core::{EffectAtom, Footprint};
use serde::Serialize;

/// Effects whose atoms name state that lives in a `World`. Exactly one at v0:
/// the builtin `cell`, which `with_cell` regions publish under and which no
/// user program may redeclare.
pub const WORLD_BACKED: &[&str] = &["cell"];

/// Effects whose atoms name an input no test can write. Exactly one at v0: the
/// builtin `sim`, whose `sim.read` is a simulated region's seed.
///
/// A seed is supplied to a test rather than shared between tests, so a read of
/// one contends with nothing. Without this, every simulated test would drop out
/// of the `isolated: n of m` number for no reason at all.
pub const AMBIENT: &[&str] = &["sim"];

/// The effects the language simulates: `simulate { .. }` discharges exactly
/// these and nothing else. A user's own `nondet` effect inside a region is still
/// `E0412`, because the language does not get to claim it simulated something it
/// has never heard of.
pub const SIMULATED: &[&str] = &["task", "clock", "random"];

/// The seed effect. Deliberately not `nondet`: a seed is an input.
pub const SIM_EFFECT: &str = "sim";

pub fn is_world_backed(atom: &EffectAtom) -> bool {
    WORLD_BACKED.contains(&atom.effect.as_str())
}

pub fn is_ambient(atom: &EffectAtom) -> bool {
    AMBIENT.contains(&atom.effect.as_str())
}

/// Whether this atom can bring one test into contention with another.
pub fn contends(atom: &EffectAtom) -> bool {
    !is_world_backed(atom) && !is_ambient(atom)
}

/// Nothing this test touches can be reached from another one. The empty
/// footprint is world-isolated.
pub fn world_isolated(f: &Footprint) -> bool {
    !f.atoms().any(contends)
}

/// The atoms that can contend across tests: `f` minus what only it can reach.
pub fn shared_footprint(f: &Footprint) -> Footprint {
    Footprint::from_atoms(f.atoms().filter(|a| contends(a)).cloned())
}

/// This test's outcome is a function of its definition set **and** a seed:
/// something in its closure entered a `simulate` region. Such a test is keyed in
/// the result cache by [`crate::sim_key`] and never by its bare `DefHash`.
pub fn is_seeded(f: &Footprint) -> bool {
    f.atoms().any(is_ambient)
}

/// Whether a test can interfere with any other test at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// The footprint is entirely world-backed: this test conflicts with
    /// nothing, whatever else the corpus contains.
    World,
    /// At least one atom escapes to a resource another test can reach.
    Shared,
}

impl Isolation {
    pub fn of(footprint: &Footprint) -> Self {
        if world_isolated(footprint) {
            Isolation::World
        } else {
            Isolation::Shared
        }
    }

    pub fn is_world(self) -> bool {
        self == Isolation::World
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Isolation::World => "world",
            Isolation::Shared => "shared",
        }
    }
}

/// How much of the corpus is trivially parallel, and what the rest costs.
///
/// `total`, `isolated` and `shared` count every test the run reports on,
/// cached ones included, because isolation is a property of a test rather than
/// of this run's cache state. The group counts cover `scheduled` only, since
/// those are the tests being coloured.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize)]
pub struct Parallelism {
    pub total: usize,
    pub isolated: usize,
    pub shared: usize,
    /// Selected tests: what `groups` and `shared_groups` are counted over.
    pub scheduled: usize,
    pub groups: usize,
    /// What the *shared* selected tests need on their own. An isolated test is
    /// free, so it can never be the reason a group exists — except for the one
    /// group that has to exist for anything to run at all.
    pub shared_groups: usize,
}

impl Parallelism {
    /// The property `--explain` publishes, so that a future change cannot
    /// quietly lose it.
    pub fn holds(&self) -> bool {
        let floor = usize::from(self.scheduled > 0);
        self.groups == self.shared_groups.max(floor)
    }
}

/// `universe` is every test the run reports on; `scheduled` is the subset being
/// coloured, with its footprints, and `groups` is what [`group_by_conflict`]
/// made of them.
pub fn parallelism<'a>(
    universe: impl IntoIterator<Item = &'a Footprint>,
    scheduled: &[(usize, Footprint)],
    groups: &[Vec<usize>],
) -> Parallelism {
    let mut total = 0usize;
    let mut isolated = 0usize;
    for footprint in universe {
        total += 1;
        isolated += usize::from(world_isolated(footprint));
    }

    let shared_only: Vec<(usize, Footprint)> = scheduled
        .iter()
        .filter(|(_, f)| !world_isolated(f))
        .cloned()
        .collect();

    Parallelism {
        total,
        isolated,
        shared: total - isolated,
        scheduled: scheduled.len(),
        groups: groups.len(),
        shared_groups: group_by_conflict(&shared_only).len(),
    }
}

/// Greedy colouring of the conflict graph over shared footprints, largest
/// first. A group runs concurrently without any locking.
///
/// The ordering is the whole trick: a test that conflicts with many others has
/// the fewest colours available to it, so it must choose while the classes are
/// still mostly empty. Colouring in source order routinely produces one more
/// group than it needs to, and a group costs a full round of wall-clock time.
///
/// A world-isolated test clears every class, so it always lands in group 0 and
/// never creates a group. That is what makes adding one free.
pub fn group_by_conflict(tests: &[(usize, Footprint)]) -> Vec<Vec<usize>> {
    let shared: Vec<Footprint> = tests.iter().map(|(_, f)| shared_footprint(f)).collect();

    let mut order: Vec<usize> = (0..tests.len()).collect();
    order.sort_by(|&a, &b| {
        shared[b]
            .0
            .len()
            .cmp(&shared[a].0.len())
            .then(tests[a].0.cmp(&tests[b].0))
    });

    let mut classes: Vec<Vec<usize>> = Vec::new();
    for &p in &order {
        let footprint = &shared[p];
        // Conflict is not transitive, so a colour class is only safe if the
        // candidate clears every member of it, not just one representative.
        let slot = classes
            .iter()
            .position(|class| class.iter().all(|&q| !footprint.conflicts_with(&shared[q])));
        match slot {
            Some(k) => classes[k].push(p),
            None => classes.push(vec![p]),
        }
    }

    classes
        .into_iter()
        .map(|class| {
            let mut group: Vec<usize> = class.into_iter().map(|p| tests[p].0).collect();
            group.sort_unstable();
            group
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_core::Resource;
    use ply_span::Symbol;
    use ply_syntax::ast::Mode;

    fn atom(effect: &str, resource: Option<&str>, mode: Mode) -> EffectAtom {
        EffectAtom::new(
            effect,
            resource
                .map(|r| Resource::Named(Symbol::new(r)))
                .unwrap_or(Resource::Singleton),
            mode,
        )
    }

    /// Without this, every simulated test would drop out of `isolated: n of m`
    /// for no reason: a seed is handed to a test, never shared between two.
    #[test]
    fn a_seed_read_does_not_make_a_test_shared() {
        let f = Footprint::from_atoms([
            atom(SIM_EFFECT, None, Mode::Read),
            atom("cell", Some("users"), Mode::Write),
        ]);
        assert!(is_seeded(&f));
        assert!(world_isolated(&f));
        assert_eq!(Isolation::of(&f), Isolation::World);
        assert!(shared_footprint(&f).is_empty());
    }

    /// The spawned body's effects flow through `task.spawn`'s row, so a test
    /// that simulates concurrent writes to a real resource still contends with
    /// the tests that read it.
    #[test]
    fn a_simulated_test_still_contends_over_what_its_tasks_touch() {
        let writer = Footprint::from_atoms([
            atom(SIM_EFFECT, None, Mode::Read),
            atom("db", Some("orders"), Mode::Write),
        ]);
        let reader = Footprint::from_atoms([atom("db", Some("orders"), Mode::Read)]);
        assert!(!world_isolated(&writer));
        assert!(shared_footprint(&writer).conflicts_with(&shared_footprint(&reader)));
    }

    #[test]
    fn a_test_that_never_simulated_is_not_seeded() {
        let f = Footprint::from_atoms([atom("db", Some("users"), Mode::Read)]);
        assert!(!is_seeded(&f));
        assert!(!is_seeded(&Footprint::empty()));
    }

    /// Adding world-isolated *simulated* tests must be as free as adding any
    /// other world-isolated test — ADR 0005 §5's property, preserved.
    #[test]
    fn adding_isolated_simulated_tests_changes_no_group_count() {
        let shared: Vec<(usize, Footprint)> = vec![
            (
                0,
                Footprint::from_atoms([atom("db", Some("a"), Mode::Write)]),
            ),
            (
                1,
                Footprint::from_atoms([atom("db", Some("a"), Mode::Write)]),
            ),
        ];
        let before = group_by_conflict(&shared).len();
        let mut wider = shared.clone();
        for i in 0..100 {
            wider.push((
                2 + i,
                Footprint::from_atoms([
                    atom(SIM_EFFECT, None, Mode::Read),
                    atom("cell", Some("s"), Mode::Write),
                ]),
            ));
        }
        assert_eq!(group_by_conflict(&wider).len(), before);
    }
}

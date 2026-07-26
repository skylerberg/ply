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

use ply_core::{EffectAtom, Footprint};
use serde::Serialize;

/// Effects whose atoms name state that lives in a `World`. Exactly one at v0:
/// the builtin `cell`, which `with_cell` regions publish under and which no
/// user program may redeclare.
pub const WORLD_BACKED: &[&str] = &["cell"];

pub fn is_world_backed(atom: &EffectAtom) -> bool {
    WORLD_BACKED.contains(&atom.effect.as_str())
}

/// Nothing this test touches can be reached from another one. The empty
/// footprint is world-isolated.
pub fn world_isolated(f: &Footprint) -> bool {
    f.atoms().all(is_world_backed)
}

/// The atoms that can contend across tests: `f` minus its world-backed atoms.
pub fn shared_footprint(f: &Footprint) -> Footprint {
    Footprint::from_atoms(f.atoms().filter(|a| !is_world_backed(a)).cloned())
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

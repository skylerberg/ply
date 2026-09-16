//! Which selected tests may run at the same time.

use ply_ty::{EffectAtom, Footprint};
use serde::Serialize;

/// Effects whose atoms name a region label.
pub const REGION_SCOPED: &[&str] = &["cell"];

/// Effects whose atoms name an input no test can write.
pub const AMBIENT: &[&str] = &["sim"];

/// The effects the language simulates: `simulate { .. }` discharges exactly these and nothing else.
pub const SIMULATED: &[&str] = &["task", "clock", "random"];

/// The seed effect.
pub const SIM_EFFECT: &str = "sim";

pub fn is_region_scoped(atom: &EffectAtom) -> bool {
    REGION_SCOPED.contains(&atom.effect.as_str())
}

pub fn is_ambient(atom: &EffectAtom) -> bool {
    AMBIENT.contains(&atom.effect.as_str())
}

/// Whether this atom can bring one test into contention with another.
pub fn contends(atom: &EffectAtom) -> bool {
    !is_ambient(atom)
}

/// Nothing this test names can be reached from another one.
pub fn region_isolated(f: &Footprint) -> bool {
    !f.atoms().any(contends)
}

/// The atoms that can contend across tests: `f` minus the inputs only it is handed.
pub fn shared_footprint(f: &Footprint) -> Footprint {
    Footprint::from_atoms(f.atoms().filter(|a| contends(a)).cloned())
}

/// This test contends, and only over region labels.
pub fn contends_only_over_regions(f: &Footprint) -> bool {
    let shared = shared_footprint(f);
    !shared.is_empty() && shared.atoms().all(is_region_scoped)
}

/// This test's outcome is a function of its definition set **and** a seed: something in its closure
/// entered a `simulate` region.
pub fn is_seeded(f: &Footprint) -> bool {
    f.atoms().any(is_ambient)
}

/// Whether a test can interfere with any other test at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// This test names nothing another test can reach: its allocations live in a region closed when
    /// it ends, and its footprint carries no atom that contends.
    Region,
    /// At least one atom names state another test can reach — a resource outside the program, or a
    /// region label a sibling also writes.
    Shared,
}

impl Isolation {
    pub fn of(footprint: &Footprint) -> Self {
        if region_isolated(footprint) {
            Isolation::Region
        } else {
            Isolation::Shared
        }
    }

    pub fn is_isolated(self) -> bool {
        self == Isolation::Region
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Isolation::Region => "region",
            Isolation::Shared => "shared",
        }
    }
}

/// How much of the corpus is trivially parallel, and what the rest costs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize)]
pub struct Parallelism {
    pub total: usize,
    pub isolated: usize,
    pub shared: usize,
    /// Of `shared`: how many contend *only* over a region label.
    pub region_contended: usize,
    /// Selected tests: what `groups` and `shared_groups` are counted over.
    pub scheduled: usize,
    pub groups: usize,
    /// What the *shared* selected tests need on their own.
    pub shared_groups: usize,
}

impl Parallelism {
    /// The property `--explain` publishes, so that a future change cannot quietly lose it.
    pub fn holds(&self) -> bool {
        let floor = usize::from(self.scheduled > 0);
        self.groups == self.shared_groups.max(floor)
    }
}

/// `universe` is every test the run reports on; `scheduled` is the subset being coloured, with its
/// footprints, and `groups` is what [`group_by_conflict`] made of them.
pub fn parallelism<'a>(
    universe: impl IntoIterator<Item = &'a Footprint>,
    scheduled: &[(usize, Footprint)],
    groups: &[Vec<usize>],
) -> Parallelism {
    let mut total = 0usize;
    let mut isolated = 0usize;
    let mut region_contended = 0usize;
    for footprint in universe {
        total += 1;
        if region_isolated(footprint) {
            isolated += 1;
        } else if contends_only_over_regions(footprint) {
            region_contended += 1;
        }
    }

    let shared_only: Vec<(usize, Footprint)> = scheduled
        .iter()
        .filter(|(_, f)| !region_isolated(f))
        .cloned()
        .collect();

    Parallelism {
        total,
        isolated,
        shared: total - isolated,
        region_contended,
        scheduled: scheduled.len(),
        groups: groups.len(),
        shared_groups: group_by_conflict(&shared_only).len(),
    }
}

/// Greedy colouring of the conflict graph over shared footprints, largest first.
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
        // Conflict is not transitive, so a colour class is only safe if the candidate clears every
        // member of it, not just one representative.
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

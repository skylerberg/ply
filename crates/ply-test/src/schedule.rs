//! Which selected tests may run at the same time.
//!
//! The conflict graph exists because tests share real state. Under ADR 0005 a
//! `cell` atom named state that lived in a `World`, every test ran against its
//! own fork of one, and two tests that both retained `cell.write[users]` named
//! two different pieces of state — so the graph was built over the atoms that
//! *escaped* a fork.
//!
//! ADR 0017 §6 removes the fork. A test's allocations live in a region closed
//! when the test ends, so tests still cannot observe each other's allocations,
//! but a region **label** is a name two tests can both write and the scheduler
//! can no longer prove their state is disjoint: the group's fixture is built
//! once and mutated in place, so two tests naming one label are two tests
//! naming one piece of state. `cell` therefore contends like anything else, and
//! that lost case is the whole cost of the change — measured, not assumed, by
//! `ply_corpus::regions`.
//!
//! What stays exempt is [`AMBIENT`]: a seed is an input handed to a test rather
//! than state shared between two, and no memory model changes that.
//!
//! **This module is about two tests, and none of it transfers to two tasks.**
//! Two tasks inside one simulated run share one region, so `ply_eval::sim` keeps
//! `cell` accesses in its dependence relation and must not reuse
//! [`shared_footprint`]. Dropping them there would prune away every
//! shared-memory race in the corpus while reporting a larger reduction for it.

use ply_core::{EffectAtom, Footprint};
use serde::Serialize;

/// Effects whose atoms name a region label. Exactly one at v0: the builtin
/// `cell`, which `with_cell[r]` regions publish under and which no user program
/// may redeclare.
///
/// This is **not** an exemption. It is what lets a report say *why* two tests
/// contend — sharing a label is a fact a reader can act on by renaming one,
/// where sharing a database table is not — and it is the population ADR 0017 §6
/// costs the change in.
pub const REGION_SCOPED: &[&str] = &["cell"];

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

/// Nothing this test names can be reached from another one. The empty footprint
/// is region-isolated, and so is a footprint carrying nothing but a seed read.
pub fn region_isolated(f: &Footprint) -> bool {
    !f.atoms().any(contends)
}

/// The atoms that can contend across tests: `f` minus the inputs only it is
/// handed.
pub fn shared_footprint(f: &Footprint) -> Footprint {
    Footprint::from_atoms(f.atoms().filter(|a| contends(a)).cloned())
}

/// This test contends, and only over region labels. It was isolated under the
/// forkable world and is grouped by footprint conflict now, so it is exactly
/// what ADR 0017 §6 costs — and the one contention a reader can remove by
/// renaming a label.
pub fn contends_only_over_regions(f: &Footprint) -> bool {
    let shared = shared_footprint(f);
    !shared.is_empty() && shared.atoms().all(is_region_scoped)
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
    /// This test names nothing another test can reach: its allocations live in
    /// a region closed when it ends, and its footprint carries no atom that
    /// contends. It conflicts with nothing, whatever else the corpus contains.
    Region,
    /// At least one atom names state another test can reach — a resource
    /// outside the program, or a region label a sibling also writes.
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
    /// Of `shared`: how many contend *only* over a region label. Under the
    /// forkable world these were isolated and free; they are coloured now.
    ///
    /// Reported on every run rather than modelled once, because ADR 0008 §6's
    /// trap is that a count which stops being true is still printed. A project
    /// watching `isolated` go up needs to see this go up too, or the change
    /// that cost it the parallelism is invisible.
    pub region_contended: usize,
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

/// Greedy colouring of the conflict graph over shared footprints, largest
/// first. A group runs concurrently without any locking.
///
/// The ordering is the whole trick: a test that conflicts with many others has
/// the fewest colours available to it, so it must choose while the classes are
/// still mostly empty. Colouring in source order routinely produces one more
/// group than it needs to, and a group costs a full round of wall-clock time.
///
/// A region-isolated test clears every class, so it always lands in group 0 and
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
        let f = Footprint::from_atoms([atom(SIM_EFFECT, None, Mode::Read)]);
        assert!(is_seeded(&f));
        assert!(region_isolated(&f));
        assert_eq!(Isolation::of(&f), Isolation::Region);
        assert!(shared_footprint(&f).is_empty());
    }

    /// The case ADR 0017 §6 costs, at its smallest: a seed keeps its exemption
    /// and the region label does not, so the test is shared *and* the seed is
    /// not what made it shared.
    #[test]
    fn a_region_label_contends_and_the_seed_beside_it_still_does_not() {
        let f = Footprint::from_atoms([
            atom(SIM_EFFECT, None, Mode::Read),
            atom("cell", Some("users"), Mode::Write),
        ]);
        assert!(is_seeded(&f));
        assert!(!region_isolated(&f));
        assert_eq!(Isolation::of(&f), Isolation::Shared);
        assert!(contends_only_over_regions(&f));
        assert_eq!(
            shared_footprint(&f).to_string(),
            "{cell.write[users]}",
            "the seed must not appear among the atoms that made it shared"
        );
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
        assert!(!region_isolated(&writer));
        assert!(!contends_only_over_regions(&writer));
        assert!(shared_footprint(&writer).conflicts_with(&shared_footprint(&reader)));
    }

    #[test]
    fn a_test_that_never_simulated_is_not_seeded() {
        let f = Footprint::from_atoms([atom("db", Some("users"), Mode::Read)]);
        assert!(!is_seeded(&f));
        assert!(!is_seeded(&Footprint::empty()));
    }

    /// ADR 0005 §5's property, preserved with its population changed: what is
    /// free to add is a test that names nothing, not a test whose state used to
    /// be forked.
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
                Footprint::from_atoms([atom(SIM_EFFECT, None, Mode::Read)]),
            ));
        }
        assert_eq!(group_by_conflict(&wider).len(), before);
    }

    /// Two tests naming one label are coloured apart; two tests naming
    /// different labels are not. Counting the population rather than the
    /// conflicts is the easiest way to over-state what this change costs.
    #[test]
    fn one_label_separates_its_writers_and_two_labels_do_not() {
        let same: Vec<(usize, Footprint)> = (0..4)
            .map(|i| {
                (
                    i,
                    Footprint::from_atoms([atom("cell", Some("users"), Mode::Write)]),
                )
            })
            .collect();
        assert_eq!(group_by_conflict(&same).len(), 4);

        let distinct: Vec<(usize, Footprint)> = (0..4)
            .map(|i| {
                (
                    i,
                    Footprint::from_atoms([atom("cell", Some(&format!("r{i}")), Mode::Write)]),
                )
            })
            .collect();
        assert_eq!(group_by_conflict(&distinct).len(), 1);
    }

    #[test]
    fn parallelism_counts_the_region_contended_apart_from_the_rest() {
        let footprints = [
            Footprint::empty(),
            Footprint::from_atoms([atom("cell", Some("users"), Mode::Write)]),
            Footprint::from_atoms([atom("cell", Some("users"), Mode::Write)]),
            Footprint::from_atoms([atom("db", Some("orders"), Mode::Write)]),
            Footprint::from_atoms([
                atom("cell", Some("users"), Mode::Write),
                atom("db", Some("orders"), Mode::Write),
            ]),
        ];
        let scheduled: Vec<(usize, Footprint)> = footprints.iter().cloned().enumerate().collect();
        let groups = group_by_conflict(&scheduled);
        let p = parallelism(footprints.iter(), &scheduled, &groups);

        assert_eq!(p.total, 5);
        assert_eq!(p.isolated, 1);
        assert_eq!(p.shared, 4);
        assert_eq!(
            p.region_contended, 2,
            "the mixed footprint contends over a real resource too and is not this number"
        );
        assert!(p.holds(), "{p:?}");
    }
}

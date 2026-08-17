//! ADR 0017 §6: what removing the forkable world costs the scheduler.
//!
//! The claim to be priced is that tests which "share a resource label but have
//! disjoint state" stop parallelizing once every test no longer gets its own
//! forked [`World`](ply_eval::World). Today's headline — `isolated 176 of 186` —
//! overstates it, because a pure test has an empty footprint and conflicts with
//! nothing whether or not anything is forked.
//!
//! So the number this module computes is the counterfactual: colour the same
//! test set twice, once with the world-backed exemption the scheduler applied
//! under ADR 0005 and once without it, and report the difference in groups, in
//! the critical path, and in modelled wall clock.
//!
//! Three things about the model are load-bearing.
//!
//! **Only `cell` changes.** `ply_test::REGION_SCOPED` is exactly `["cell"]`, so
//! that exemption was the entire mechanism by which forking bought the scheduler
//! anything. `ply_test::AMBIENT` — `sim.read`, a seed — is a claim about inputs
//! rather than about memory, and ADR 0017 does not touch it, so it stays exempt
//! on both sides. Dropping it too would report a loss this design does not cause.
//!
//! **The colouring is the runner's own.** [`colour`] is `group_by_conflict`
//! with the projection lifted out, and a test asserts it reproduces
//! `ply_test::group_by_conflict` exactly on the projection that crate applies.
//! A counterfactual coloured by a second-best heuristic would report a cost
//! that is the heuristic's. `ply-test` applies [`region_footprint`] now, so that
//! is the side the assertion is made against and [`forked_footprint`] is stated
//! here — a baseline that is a call into the thing being changed measures
//! nothing.
//!
//! **Groups are a barrier.** `ply_test::run` executes one group to completion
//! before starting the next, and inside a group `jobs` workers pull the next
//! index off a counter. [`makespan`] replays exactly that, so the modelled
//! number can be checked against a measured run — [`IsolationCost::model_error`]
//! is that check, and it is printed rather than assumed.

use crate::rng::Rng;
use anyhow::{Context, Result, bail};
use ply_core::{EffectAtom, Footprint};
use ply_eval::{EngineChoice, Plan};
use ply_span::Symbol;
use ply_store::Store;
use ply_syntax::ast::Mode;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

// ---------------------------------------------------------------- projections

/// The atoms the scheduler let contend under the forkable world: the ambient
/// ones dropped, and the region-scoped ones dropped too because every test ran
/// against its own fork.
///
/// Stated here rather than called on `ply-test`, which no longer applies it:
/// the baseline of a counterfactual has to survive the change it is measuring.
pub fn forked_footprint(f: &Footprint) -> Footprint {
    Footprint::from_atoms(
        f.atoms()
            .filter(|a| !ply_test::is_ambient(a) && !ply_test::is_region_scoped(a))
            .cloned(),
    )
}

/// The atoms that contend once a test no longer gets its own world — which is
/// what `ply_test::shared_footprint` now is.
///
/// `sim` stays exempt: a seed is handed to a test rather than shared between
/// two, and no memory model changes that. `cell` does not, because a region
/// closed at the end of a test is not a fork — two tests naming one label are
/// two tests naming one label.
pub fn region_footprint(f: &Footprint) -> Footprint {
    ply_test::shared_footprint(f)
}

/// Whether this test was isolated under the forkable world only because its
/// state was forked: it carries an atom the world-backed exemption hid.
pub fn isolated_by_forking(f: &Footprint) -> bool {
    forked_footprint(f).is_empty() && f.atoms().any(ply_test::is_region_scoped)
}

/// Isolated under the forkable world: the classification `isolated n of m`
/// counted before ADR 0017 §6, kept here because the counterfactual's baseline
/// is that number and `ply-test` no longer computes it.
fn was_world_isolated(f: &Footprint) -> bool {
    forked_footprint(f).is_empty()
}

// ------------------------------------------------------------------ colouring

/// `ply_test::group_by_conflict` with the projection lifted out, so the
/// baseline and the counterfactual are coloured by one function.
///
/// `projected[i]` is the footprint `tests[i]` contends over. The greedy
/// largest-first order is the runner's and is not incidental: colouring in
/// source order routinely produces one more group than it needs to, and a group
/// costs a whole round of wall clock.
pub fn colour(tests: &[(usize, Footprint)], projected: &[Footprint]) -> Vec<Vec<usize>> {
    assert_eq!(tests.len(), projected.len());

    let mut order: Vec<usize> = (0..tests.len()).collect();
    order.sort_by(|&a, &b| {
        projected[b]
            .0
            .len()
            .cmp(&projected[a].0.len())
            .then(tests[a].0.cmp(&tests[b].0))
    });

    let mut classes: Vec<Vec<usize>> = Vec::new();
    for &p in &order {
        let footprint = &projected[p];
        let slot = classes.iter().position(|class| {
            class
                .iter()
                .all(|&q| !footprint.conflicts_with(&projected[q]))
        });
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

/// Wall clock for a schedule, in the shape `ply_test::run` actually executes it:
/// one group at a time, and within a group `jobs` workers each taking the next
/// index off a shared counter as they come free.
///
/// `millis` is indexed by test index. `jobs == 0` means unbounded, which makes
/// a group cost its slowest member and the suite cost the critical path.
///
/// `setup_millis` is what one worker costs to build. `execute_group` builds a
/// worker per pool thread **per group**, so it is charged per group rather than
/// once — which is most of the reason an extra group is not free, and leaving it
/// out would flatter every counterfactual that adds one.
pub fn makespan(groups: &[Vec<usize>], millis: &[f64], jobs: usize, setup_millis: f64) -> f64 {
    let mut total = 0.0;
    for group in groups {
        if group.is_empty() {
            continue;
        }
        let workers = if jobs == 0 {
            group.len()
        } else {
            jobs.min(group.len())
        };
        let mut free = vec![setup_millis; workers];
        for &index in group {
            let cost = millis.get(index).copied().unwrap_or(0.0);
            let slot = free
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.total_cmp(b.1))
                .map(|(i, _)| i)
                .expect("a group always has at least one worker");
            free[slot] += cost;
        }
        total += free.into_iter().fold(0.0f64, f64::max);
    }
    total
}

// -------------------------------------------------------------------- results

/// One colouring, priced.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Split {
    pub groups: usize,
    pub largest_group: usize,
    /// Tests outside group 0: what a barrier is actually charged for.
    pub after_the_first_group: usize,
    /// The suite at `jobs` workers, modelled the way the runner schedules.
    pub makespan_millis: f64,
    /// The same at unbounded workers: `Σ over groups of the slowest member`.
    /// No amount of hardware goes below it.
    pub critical_path_millis: f64,
    /// Every test one after another. The denominator a speedup is read against.
    pub sequential_millis: f64,
}

fn split(groups: &[Vec<usize>], millis: &[f64], jobs: usize, setup: f64) -> Split {
    Split {
        groups: groups.len(),
        largest_group: groups.iter().map(|g| g.len()).max().unwrap_or(0),
        after_the_first_group: groups.iter().skip(1).map(|g| g.len()).sum(),
        makespan_millis: makespan(groups, millis, jobs, setup),
        critical_path_millis: makespan(groups, millis, 0, setup),
        sequential_millis: makespan(
            &[groups.iter().flatten().copied().collect::<Vec<usize>>()],
            millis,
            1,
            setup,
        ),
    }
}

/// A test that changes classification, named rather than counted, because a
/// count nobody can check is the shape of claim this project keeps finding wrong.
#[derive(Clone, Debug, Serialize)]
pub struct NewlySerialized {
    pub index: usize,
    pub key: String,
    pub footprint: String,
    /// A test it now conflicts with, and did not before.
    pub conflicts_with: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct IsolationCost {
    pub root: String,
    pub jobs: usize,
    pub tests: usize,

    /// `isolated n of m`, as `ply test` prints it.
    pub isolated_today: usize,
    pub shared_today: usize,
    /// Of the isolated: how many have an empty footprint and therefore conflict
    /// with nothing whatever the memory model is.
    pub pure: usize,
    /// Of the isolated: how many carry only `sim.read`, which ADR 0017 does not
    /// touch.
    pub seeded_only: usize,
    /// Of the isolated: how many carry a `cell` atom — the exemption forking
    /// pays for, and the only population that can lose anything here.
    pub world_backed: usize,

    /// The answer. Tests isolated today that conflict with another test once the
    /// exemption is gone.
    pub newly_serialized: usize,
    pub newly_serialized_tests: Vec<NewlySerialized>,

    pub today: Split,
    pub without_forking: Split,

    /// Measured, at `jobs`, over the whole suite. `None` when durations were
    /// supplied rather than taken.
    pub measured_suite_millis: Option<f64>,
    /// The same suite measured at one worker: the denominator the parallel run
    /// is actually buying against.
    pub measured_sequential_millis: Option<f64>,
    pub worker_setup_millis: f64,
    /// `modelled / measured − 1` for today's schedule. The model's own error
    /// bar, printed rather than assumed, because the counterfactual's number is
    /// only as good as the baseline's. It is negative here: a test measured
    /// alone is faster than the same test measured against seven others, and no
    /// per-test duration taken at one job can carry that. Both colourings pay
    /// it, so it cancels out of the ratio and does not out of the absolutes.
    pub model_error: Option<f64>,
}

impl IsolationCost {
    /// What the change costs the suite at `jobs`, as a multiple. `1.0` is free.
    pub fn wall_clock_ratio(&self) -> f64 {
        if self.today.makespan_millis <= 0.0 {
            return 1.0;
        }
        self.without_forking.makespan_millis / self.today.makespan_millis
    }

    pub fn groups_added(&self) -> i64 {
        self.without_forking.groups as i64 - self.today.groups as i64
    }
}

// ------------------------------------------------------------------- analysis

/// The footprints and per-test costs an analysis runs on. Splitting it out is
/// what lets the same colouring be applied to a measured corpus and to a
/// hypothetical one.
pub struct Corpus {
    pub root: String,
    pub keys: Vec<String>,
    pub footprints: Vec<Footprint>,
    /// Positional, aligned with `footprints`. All-equal is a legitimate input;
    /// it makes the wall-clock columns counts of tests rather than milliseconds.
    pub millis: Vec<f64>,
    /// What one worker costs to build. Charged once per pool thread per group.
    pub worker_setup_millis: f64,
    pub measured_suite_millis: Option<f64>,
    pub measured_sequential_millis: Option<f64>,
}

pub fn analyse(corpus: &Corpus, jobs: usize) -> IsolationCost {
    let scheduled: Vec<(usize, Footprint)> =
        corpus.footprints.iter().cloned().enumerate().collect();
    let today_projection: Vec<Footprint> = corpus.footprints.iter().map(forked_footprint).collect();
    let region_projection: Vec<Footprint> =
        corpus.footprints.iter().map(region_footprint).collect();

    let today_groups = colour(&scheduled, &today_projection);
    let region_groups = colour(&scheduled, &region_projection);

    let mut newly = Vec::new();
    for (i, f) in corpus.footprints.iter().enumerate() {
        if !was_world_isolated(f) {
            continue;
        }
        let mine = &region_projection[i];
        if mine.is_empty() {
            continue;
        }
        if let Some(j) = (0..corpus.footprints.len())
            .find(|&j| j != i && mine.conflicts_with(&region_projection[j]))
        {
            newly.push(NewlySerialized {
                index: i,
                key: corpus.keys.get(i).cloned().unwrap_or_default(),
                footprint: f.to_string(),
                conflicts_with: corpus.keys.get(j).cloned().unwrap_or_default(),
            });
        }
    }

    let isolated_today = corpus
        .footprints
        .iter()
        .filter(|f| was_world_isolated(f))
        .count();
    let pure = corpus.footprints.iter().filter(|f| f.is_empty()).count();
    let world_backed = corpus
        .footprints
        .iter()
        .filter(|f| isolated_by_forking(f))
        .count();

    let today = split(
        &today_groups,
        &corpus.millis,
        jobs,
        corpus.worker_setup_millis,
    );
    let without_forking = split(
        &region_groups,
        &corpus.millis,
        jobs,
        corpus.worker_setup_millis,
    );
    let model_error = corpus
        .measured_suite_millis
        .filter(|m| *m > 0.0)
        .map(|m| today.makespan_millis / m - 1.0);

    IsolationCost {
        root: corpus.root.clone(),
        jobs,
        tests: corpus.footprints.len(),
        isolated_today,
        shared_today: corpus.footprints.len() - isolated_today,
        pure,
        seeded_only: isolated_today - pure - world_backed,
        world_backed,
        newly_serialized: newly.len(),
        newly_serialized_tests: newly,
        today,
        without_forking,
        measured_suite_millis: corpus.measured_suite_millis,
        measured_sequential_millis: corpus.measured_sequential_millis,
        worker_setup_millis: corpus.worker_setup_millis,
        model_error,
    }
}

// ------------------------------------------------------------------ measuring

/// Loads a project the way `ply` does — shipped modules resolve, which
/// `crate::pipeline::front` deliberately does not arrange — runs every test
/// once at one job, and returns the per-test durations the runner itself
/// measured.
///
/// Scope is `ply_cli::commands::test::Plan`'s, so the denominator here is the
/// `n of m` a reader sees on the terminal. Without it a shipped module's tests
/// would join the corpus and the counts would answer a question nobody asked.
///
/// The cache lives in a scratch directory rather than under `root`: a
/// measurement that wrote into `examples/` would change the next run it took.
pub fn measure(root: &Path, jobs: usize, std_tests: bool) -> Result<Corpus> {
    let loaded = ply_cli::load::load(root).map_err(|e| {
        anyhow::anyhow!(
            "`{}` does not compile ({} diagnostic(s)): {}",
            root.display(),
            e.diagnostics.len(),
            e.diagnostics
                .iter()
                .take(3)
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    let hashes = loaded.hashes().map_err(|d| {
        anyhow::anyhow!(
            "hashing `{}` failed: {} diagnostic(s)",
            root.display(),
            d.len()
        )
    })?;

    let scratch = tempfile::tempdir().context("opening a scratch cache")?;
    let mut store = Store::open(scratch.path()).context("opening a scratch cache")?;

    let plan_of = |store: &mut Store| {
        let bare = ply_test::select(&loaded.check, &hashes, store, &Plan::default());
        ply_cli::commands::test::Plan::new(bare, &loaded.check, None, std_tests)
    };

    let visible = plan_of(&mut store).visible;
    if visible.is_empty() {
        bail!("`{}` declares no tests in scope", root.display());
    }
    let keys: Vec<String> = visible
        .iter()
        .map(|&i| loaded.check.tests[i].key.to_string())
        .collect();
    let footprints: Vec<Footprint> = visible
        .iter()
        .map(|&i| loaded.check.tests[i].footprint.clone())
        .collect();

    let mut run = |jobs: usize| -> Result<(Vec<f64>, f64)> {
        store.clear()?;
        let plan = plan_of(&mut store);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build()
            .context("building the worker pool")?;
        let started = Instant::now();
        let report = pool.install(|| {
            ply_test::run(
                &plan.selection,
                &loaded.program,
                &loaded.resolved,
                &loaded.check,
                &hashes,
                &mut store,
                EngineChoice::Machine,
                ply_test::Search::of(&plan.selection),
                ply_test::Hosting::hermetic(),
            )
        });
        let wall = started.elapsed().as_secs_f64() * 1000.0;
        if report.failed > 0 {
            bail!(
                "{} of {} tests failed while being timed; a suite that is not green times nothing",
                report.failed,
                plan.selection.total
            );
        }
        let mut by_index = vec![0.0f64; loaded.check.tests.len()];
        for r in &report.results {
            by_index[r.index] = r.duration.as_secs_f64() * 1000.0;
        }
        Ok((visible.iter().map(|&i| by_index[i]).collect(), wall))
    };

    // Per-test costs come from a one-job pass, where a test's duration is its
    // own work rather than its share of a contended machine. The suite number
    // the model is checked against comes from a pass at `jobs`, because that is
    // the schedule being modelled.
    let (millis, sequential) = run(1)?;
    let (_, measured) = run(jobs)?;

    let setup = (0..3)
        .map(|_| {
            let started = Instant::now();
            std::hint::black_box(ply_eval::Machine::new(
                &loaded.program,
                &loaded.resolved,
                &loaded.check,
            ));
            started.elapsed()
        })
        .min()
        .expect("three attempts always run")
        .as_secs_f64()
        * 1000.0;

    Ok(Corpus {
        root: root.display().to_string(),
        keys,
        footprints,
        millis,
        worker_setup_millis: setup,
        measured_suite_millis: Some(measured),
        measured_sequential_millis: Some(sequential),
    })
}

// -------------------------------------------------------------- hypotheticals

/// A corpus that does not exist, so that the risk can be priced rather than
/// asserted away.
///
/// A `cell` atom in a test's footprint is what forking pays for, and the
/// measured corpora carry none. These are footprints only — no source, no
/// evaluation — because what is being coloured is a footprint and inventing a
/// program to carry one would price the program.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Hypothetical {
    /// Tests carrying a `cell` atom.
    pub cell_tests: usize,
    /// Distinct region labels they spread over. One label is the worst case.
    pub labels: usize,
    /// Tests carrying a real, contending resource atom, so the counterfactual
    /// is read against a graph that already has edges.
    pub shared_tests: usize,
    /// Distinct labels those spread over.
    pub shared_labels: usize,
    pub pure_tests: usize,
    pub seed: u64,
}

pub fn hypothetical(h: Hypothetical) -> Corpus {
    fn atom(effect: &str, label: usize, mode: Mode) -> EffectAtom {
        EffectAtom::new(
            effect,
            ply_core::Resource::Named(Symbol::new(format!("r{label}"))),
            mode,
        )
    }

    let mut rng = Rng::new(h.seed);
    let mut keys = Vec::new();
    let mut footprints = Vec::new();

    for i in 0..h.cell_tests {
        let label = rng.below(h.labels.max(1));
        keys.push(format!("hypothetical.cell{i}"));
        footprints.push(Footprint::from_atoms([
            atom("cell", label, Mode::Read),
            atom("cell", label, Mode::Write),
        ]));
    }
    for i in 0..h.shared_tests {
        let label = rng.below(h.shared_labels.max(1));
        keys.push(format!("hypothetical.shared{i}"));
        footprints.push(Footprint::from_atoms([atom("db", label, Mode::Write)]));
    }
    for i in 0..h.pure_tests {
        keys.push(format!("hypothetical.pure{i}"));
        footprints.push(Footprint::empty());
    }

    let millis = vec![1.0; footprints.len()];
    Corpus {
        root: format!(
            "hypothetical: {} cell tests over {} labels",
            h.cell_tests, h.labels
        ),
        keys,
        footprints,
        millis,
        worker_setup_millis: 0.0,
        measured_suite_millis: None,
        measured_sequential_millis: None,
    }
}

// ------------------------------------------------------------------ rendering

pub fn render(costs: &[IsolationCost]) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "{:<34} {:>6} {:>9} {:>7} {:>7} {:>6} {:>7} {:>7} {:>9} {:>9} {:>7}\n",
        "corpus",
        "tests",
        "isolated",
        "pure",
        "seeded",
        "cell",
        "newly",
        "groups",
        "ms fork",
        "ms region",
        "ratio",
    ));
    for c in costs {
        let root = c.root.rsplit('/').next().unwrap_or(&c.root);
        s.push_str(&format!(
            "{:<34} {:>6} {:>9} {:>7} {:>7} {:>6} {:>7} {:>3}→{:<3} {:>9.1} {:>9.1} {:>6.2}x\n",
            truncate(root, 34),
            c.tests,
            c.isolated_today,
            c.pure,
            c.seeded_only,
            c.world_backed,
            c.newly_serialized,
            c.today.groups,
            c.without_forking.groups,
            c.today.makespan_millis,
            c.without_forking.makespan_millis,
            c.wall_clock_ratio(),
        ));
    }
    for c in costs {
        if let Some(err) = c.model_error {
            s.push_str(&format!(
                "\n{}: measured {:.1} ms at {} jobs and {:.1} ms at 1; modelled {:.1} ms ({:+.1}%), \
                 worker setup {:.2} ms; critical path {:.1} ms → {:.1} ms\n",
                truncate(c.root.rsplit('/').next().unwrap_or(&c.root), 34),
                c.measured_suite_millis.unwrap_or_default(),
                c.jobs,
                c.measured_sequential_millis.unwrap_or_default(),
                c.today.makespan_millis,
                err * 100.0,
                c.worker_setup_millis,
                c.today.critical_path_millis,
                c.without_forking.critical_path_millis,
            ));
        }
        for t in &c.newly_serialized_tests {
            s.push_str(&format!(
                "  newly serialized: {} {} — conflicts with {}\n",
                t.key, t.footprint, t.conflicts_with
            ));
        }
    }
    s
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

// ---------------------------------------------------------------------- audit

/// Effects that appear in a test footprint anywhere in the corpus, which is how
/// a claim about `cell` is checked rather than believed.
pub fn effects_present(footprints: &[Footprint]) -> BTreeSet<String> {
    footprints
        .iter()
        .flat_map(|f| f.atoms())
        .map(|a| a.effect.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_core::Resource;

    fn atom(effect: &str, resource: &str, mode: Mode) -> EffectAtom {
        EffectAtom::new(effect, Resource::Named(Symbol::new(resource)), mode)
    }

    fn fp(atoms: impl IntoIterator<Item = EffectAtom>) -> Footprint {
        Footprint::from_atoms(atoms)
    }

    /// The counterfactual is only worth anything if the baseline is the
    /// runner's. `colour` is `group_by_conflict` with the projection lifted
    /// out, and this is the assertion that keeps it that way.
    #[test]
    fn colouring_reproduces_the_runners_own_grouping() {
        let tests: Vec<(usize, Footprint)> = vec![
            (0, fp([atom("db", "a", Mode::Write)])),
            (1, fp([atom("db", "a", Mode::Read)])),
            (2, fp([atom("db", "b", Mode::Write)])),
            (3, fp([atom("cell", "s", Mode::Write)])),
            (4, Footprint::empty()),
            (
                5,
                fp([atom("db", "a", Mode::Write), atom("db", "b", Mode::Write)]),
            ),
            (6, fp([atom("sim", "x", Mode::Read)])),
        ];
        let projected: Vec<Footprint> = tests.iter().map(|(_, f)| region_footprint(f)).collect();
        assert_eq!(
            colour(&tests, &projected),
            ply_test::group_by_conflict(&tests)
        );
    }

    /// The mechanism, in the smallest corpus that has it: two tests whose only
    /// atoms name one `cell` label. Forking makes them one group; a region does
    /// not.
    #[test]
    fn two_tests_sharing_one_cell_label_split_without_forking() {
        let corpus = Corpus {
            root: "unit".into(),
            keys: vec!["m.a".into(), "m.b".into()],
            footprints: vec![
                fp([atom("cell", "users", Mode::Write)]),
                fp([atom("cell", "users", Mode::Write)]),
            ],
            millis: vec![1.0, 1.0],
            worker_setup_millis: 0.0,
            measured_suite_millis: None,
            measured_sequential_millis: None,
        };
        let cost = analyse(&corpus, 8);
        assert_eq!(cost.isolated_today, 2);
        assert_eq!(cost.today.groups, 1);
        assert_eq!(cost.without_forking.groups, 2);
        assert_eq!(cost.newly_serialized, 2);
        assert_eq!(cost.wall_clock_ratio(), 2.0);
    }

    /// The overstatement ADR 0017 §6 warns about, stated as a test: a pure test
    /// is free either way, so adding a hundred of them must move nothing.
    #[test]
    fn pure_tests_cost_nothing_under_either_model() {
        let mut footprints = vec![fp([atom("db", "a", Mode::Write)])];
        footprints.extend((0..100).map(|_| Footprint::empty()));
        let corpus = Corpus {
            root: "unit".into(),
            keys: (0..footprints.len()).map(|i| format!("m.t{i}")).collect(),
            footprints,
            millis: vec![1.0; 101],
            worker_setup_millis: 0.0,
            measured_suite_millis: None,
            measured_sequential_millis: None,
        };
        let cost = analyse(&corpus, 8);
        assert_eq!(cost.newly_serialized, 0);
        assert_eq!(cost.today.groups, cost.without_forking.groups);
        assert_eq!(cost.pure, 100);
    }

    /// A seed is an input, not memory. Losing the world must not be reported as
    /// losing the ambient exemption too, or the cost is inflated by every
    /// simulated test in the corpus.
    #[test]
    fn a_seeded_test_is_untouched_by_the_change() {
        let corpus = Corpus {
            root: "unit".into(),
            keys: vec!["m.a".into(), "m.b".into()],
            footprints: vec![
                fp([atom("sim", "x", Mode::Read)]),
                fp([atom("sim", "x", Mode::Read)]),
            ],
            millis: vec![1.0, 1.0],
            worker_setup_millis: 0.0,
            measured_suite_millis: None,
            measured_sequential_millis: None,
        };
        let cost = analyse(&corpus, 8);
        assert_eq!(cost.newly_serialized, 0);
        assert_eq!(cost.seeded_only, 2);
        assert_eq!(cost.without_forking.groups, 1);
    }

    /// A `cell` atom that no other test names conflicts with nothing, so it
    /// does not serialize even though the exemption stopped applying to it.
    /// Counting it would be counting the population rather than the cost.
    #[test]
    fn a_lone_cell_label_does_not_serialize() {
        let corpus = Corpus {
            root: "unit".into(),
            keys: vec!["m.a".into(), "m.b".into()],
            footprints: vec![
                fp([atom("cell", "users", Mode::Write)]),
                fp([atom("cell", "orders", Mode::Write)]),
            ],
            millis: vec![1.0, 1.0],
            worker_setup_millis: 0.0,
            measured_suite_millis: None,
            measured_sequential_millis: None,
        };
        let cost = analyse(&corpus, 8);
        assert_eq!(cost.world_backed, 2);
        assert_eq!(cost.newly_serialized, 0);
        assert_eq!(cost.without_forking.groups, 1);
    }

    /// Groups are a barrier and a group costs its slowest member, so a schedule
    /// is not `sum / jobs`. One slow test alone in group 1 is a whole round.
    #[test]
    fn makespan_charges_a_barrier_between_groups() {
        let groups = vec![vec![0, 1, 2, 3], vec![4]];
        let millis = vec![1.0, 1.0, 1.0, 1.0, 10.0];
        assert_eq!(makespan(&groups, &millis, 2, 0.0), 2.0 + 10.0);
        assert_eq!(makespan(&groups, &millis, 0, 0.0), 1.0 + 10.0);
        assert_eq!(makespan(&groups, &millis, 1, 0.0), 4.0 + 10.0);
    }

    #[test]
    fn a_hypothetical_corpus_is_a_pure_function_of_its_shape() {
        let shape = Hypothetical {
            cell_tests: 40,
            labels: 4,
            shared_tests: 10,
            shared_labels: 3,
            pure_tests: 100,
            seed: 7,
        };
        let a = hypothetical(shape);
        let b = hypothetical(shape);
        assert_eq!(a.footprints, b.footprints);
        assert_eq!(a.footprints.len(), 150);
    }
}

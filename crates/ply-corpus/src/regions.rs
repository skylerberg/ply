//! Region isolation: what removing the forkable world costs the scheduler.

use crate::rng::Rng;
use anyhow::{Context, Result, bail};
use ply_eval::Plan;
use ply_span::Symbol;
use ply_store::Store;
use ply_ty::Mode;
use ply_ty::{EffectAtom, Footprint};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

/// The atoms that contended under the forkable world: neither ambient nor region-scoped.
pub fn forked_footprint(f: &Footprint) -> Footprint {
    Footprint::from_atoms(
        f.atoms()
            .filter(|a| !ply_test::is_ambient(a) && !ply_test::is_region_scoped(a))
            .cloned(),
    )
}

/// The atoms that contend once a test no longer gets its own world.
pub fn region_footprint(f: &Footprint) -> Footprint {
    ply_test::shared_footprint(f)
}

/// Isolated under the forkable world only because its region-scoped state was forked.
pub fn isolated_by_forking(f: &Footprint) -> bool {
    forked_footprint(f).is_empty() && f.atoms().any(ply_test::is_region_scoped)
}

/// Isolated under the forkable world: the counterfactual's baseline.
fn was_world_isolated(f: &Footprint) -> bool {
    forked_footprint(f).is_empty()
}

/// `ply_test::group_by_conflict` with the projection lifted out, so both sides share one colouring.
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

/// Wall clock for a schedule as `ply_test::run_with` executes it: groups in turn, `jobs` workers each.
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
    pub critical_path_millis: f64,
    /// Every test one after another.
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

/// A test that changes classification.
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
    /// Of the isolated: empty footprints, which conflict with nothing.
    pub pure: usize,
    /// Of the isolated: how many carry only `sim.read`, which the region model does not touch.
    pub seeded_only: usize,
    /// Of the isolated: those carrying a `cell` atom, the only ones that can lose anything.
    pub world_backed: usize,

    pub newly_serialized: usize,
    pub newly_serialized_tests: Vec<NewlySerialized>,

    pub today: Split,
    pub without_forking: Split,

    /// Measured, at `jobs`, over the whole suite.
    pub measured_suite_millis: Option<f64>,
    /// The same suite measured at one worker.
    pub measured_sequential_millis: Option<f64>,
    pub worker_setup_millis: f64,
    /// `modelled / measured − 1` for today's schedule.
    pub model_error: Option<f64>,
}

impl IsolationCost {
    /// What the change costs the suite at `jobs`, as a multiple.
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

/// The footprints and per-test costs an analysis runs on.
pub struct Corpus {
    pub root: String,
    pub keys: Vec<String>,
    pub footprints: Vec<Footprint>,
    /// Positional, aligned with `footprints`.
    pub millis: Vec<f64>,
    /// What one worker costs to build.
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

/// Loads a project as `ply` does (std resolves, unlike `pipeline::front`) and times every test.
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
        let bare = ply_test::select(
            &loaded.check,
            &hashes,
            store,
            &Plan::default(),
            &ply_test::Engine::Evaluator,
        );
        ply_cli::test::Plan::new(bare, &loaded.check, None, std_tests)
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
            ply_machine::support::run_on_tier(
                &loaded,
                &plan.selection,
                ply_test::Hosting::hermetic(),
                &mut store,
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

    // Per-test costs come from a one-job pass, free of contention.
    let (millis, sequential) = run(1)?;
    let (_, measured) = run(jobs)?;

    let setup = (0..3)
        .map(|_| {
            let started = Instant::now();
            std::hint::black_box(crate::tier_machine(&loaded.front, &loaded.sources));
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

/// A synthetic corpus, so the risk can be priced.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Hypothetical {
    /// Tests carrying a `cell` atom.
    pub cell_tests: usize,
    /// Distinct region labels they spread over.
    pub labels: usize,
    /// Tests carrying a contending resource atom, so the graph already has edges.
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
            ply_ty::Resource::Named(Symbol::new(format!("r{label}"))),
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

/// Effects present in any test footprint of the corpus.
pub fn effects_present(footprints: &[Footprint]) -> BTreeSet<String> {
    footprints
        .iter()
        .flat_map(|f| f.atoms())
        .map(|a| a.effect.to_string())
        .collect()
}

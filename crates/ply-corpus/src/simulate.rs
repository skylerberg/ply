//! The three numbers M7 spends and, before this module, never priced.
//!
//! [`crate::measure`] prices the machine ADR 0005 built; this prices the search
//! ADR 0006 built on top of it:
//!
//! - **reduction** — interleavings the footprint-guided search runs against the
//!   interleavings an unpruned enumeration runs, per test, at whatever conflict
//!   density the corpus was generated at. This is the evidence that
//!   resource-granular effects earned their complexity, and it is the one number
//!   a milestone about partial-order reduction may not leave unmeasured.
//! - **race power** — how many seeds it takes to reach a failing interleaving
//!   under the search against under sampling. A ratio here is the practical form
//!   of the reduction: a search that reaches a race in two interleavings where
//!   sampling needs forty is a search worth running.
//! - **throughput** — seeds per second. M7's premise is thousands of seeds per
//!   change, and whether that is affordable is a wall-clock question that no
//!   count of interleavings answers.
//!
//! Every measurement drives the real machine through the real scheduler, so a
//! number here is a number `ply test` would also produce. The one thing this
//! module does that the CLI cannot is choose the root set per trial, which is
//! what makes a median over many trials possible at all.

use crate::pipeline::{Front, front};
use anyhow::{Context, Result, bail};
use ply_eval::explore::{Dependence, Interleaving, Simulation, explore_under};
use ply_eval::{Machine, Plan, Seed, SimMode};
use ply_span::Symbol;
use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};

/// A test the search has something to say about: its footprint carries
/// `sim.read`, so its outcome is a function of its definitions *and* a seed.
struct Seeded {
    key: String,
    /// Where the machine finds it, which is not its index in `CheckOutput`: the
    /// incremental front end reports tests from modules it never parsed.
    module: Symbol,
    ordinal: usize,
}

fn seeded_tests(front: &Front) -> Vec<Seeded> {
    let mut ordinals: std::collections::BTreeMap<Symbol, usize> = Default::default();
    front
        .check
        .tests
        .iter()
        .filter_map(|test| {
            let module = test.module.as_symbol().clone();
            let ordinal = ordinals.entry(module.clone()).or_default();
            let at = *ordinal;
            *ordinal += 1;
            ply_test::is_seeded(&test.footprint).then(|| Seeded {
                key: format!("{}.{}", test.module, test.name),
                module,
                ordinal: at,
            })
        })
        .collect()
}

/// Whole-test replay at one seed, which is what `ply_test::InterpExecutor` does
/// and this repeats rather than reuses: the runner drives a whole suite through
/// a thread pool and a cache, and a measurement of the search must not be a
/// measurement of those.
struct Driver<'a> {
    front: &'a Front,
    test: &'a Seeded,
    steps: u32,
    /// Interleavings run. The search reports this too; keeping our own is what
    /// lets a trial that ends in a failure still be counted.
    runs: u32,
    /// Hand the search a recording with no vector clocks on it, which is
    /// [`ply_eval::sched::happens_before`]'s documented "no synchronization
    /// known" case and therefore exactly the search as it behaved before clocks
    /// existed. The third column of the reduction table, and the only honest way
    /// to price a filter without checking out the tree that predates it.
    blind: bool,
}

impl<'a> Driver<'a> {
    fn new(front: &'a Front, test: &'a Seeded, steps: u32) -> Driver<'a> {
        Driver {
            front,
            test,
            steps,
            runs: 0,
            blind: false,
        }
    }

    fn blind(mut self) -> Driver<'a> {
        self.blind = true;
        self
    }
}

impl Simulation for Driver<'_> {
    fn run(&mut self, seed: &Seed) -> Interleaving {
        self.runs += 1;
        let mut machine =
            Machine::new(&self.front.program, &self.front.resolved, &self.front.check);
        machine.share_region_kinds(self.front.shared_region_kinds());
        ply_test::sim::seed_run(&mut machine, seed, self.steps);
        let outcome = machine.eval_test_in(&self.test.module, self.test.ordinal);
        match ply_test::sim::interleaving_of(&machine, &outcome) {
            Some(mut interleaving) => {
                if self.blind {
                    for step in &mut interleaving.steps {
                        step.stamp.clear();
                    }
                }
                interleaving
            }
            None => match outcome {
                Ok(()) => Interleaving::passed(Vec::new()),
                Err(diagnostic) => Interleaving::failed(Vec::new(), diagnostic),
            },
        }
    }
}

// ----------------------------------------------------------------- reduction

#[derive(Clone, Debug, Serialize)]
pub struct TestReduction {
    pub key: String,
    /// Interleavings the footprint-guided search ran.
    pub pruned: u32,
    /// The same search with the recording's vector clocks withheld, so a pair
    /// the join graph already ordered is queued as though it were a race. What
    /// the search did before it read them.
    pub unsynchronized: u32,
    pub unsynchronized_bounded: bool,
    /// Interleavings the same search ran with the dependence relation forced to
    /// `true`, which is exhaustive enumeration of every schedule respecting
    /// per-task order and enabledness.
    pub naive: u32,
    /// The naive search spent its budget, so `naive` is a lower bound and the
    /// ratio is one too.
    pub naive_bounded: bool,
    /// The pruned search spent its budget, so it proved nothing about the
    /// interleavings it did not reach and the ratio understates both sides.
    pub pruned_bounded: bool,
    pub pruned_exhaustive: bool,
    pub reduction: f64,
    pub pruned_millis: f64,
    pub naive_millis: f64,
}

/// The reduction for every seeded test under `root`.
///
/// Both searches run against the same driver and the same budget, so the only
/// difference between them is [`Dependence`] — which is the whole point of
/// measuring it this way rather than against a second implementation that could
/// disagree with the first for reasons nobody controls.
pub fn reduction(root: &Path, budget: u32, steps: u32) -> Result<Vec<TestReduction>> {
    let front = front(root)?;
    let tests = seeded_tests(&front);
    if tests.is_empty() {
        bail!(
            "no test under `{}` carries `sim.read`, so there is no search to measure",
            root.display()
        );
    }

    let plan = Plan {
        mode: SimMode::Dpor,
        roots: vec![0],
        budget,
        steps,
        path: Vec::new(),
    };

    tests
        .iter()
        .map(|test| {
            let mut driver = Driver::new(&front, test, steps);
            let started = Instant::now();
            let pruned = explore_under(&plan, Dependence::Exact, &mut driver);
            let pruned_millis = millis(started.elapsed());

            let mut driver = Driver::new(&front, test, steps).blind();
            let blind = explore_under(&plan, Dependence::Exact, &mut driver);

            let mut driver = Driver::new(&front, test, steps);
            let started = Instant::now();
            let naive = explore_under(&plan, Dependence::All, &mut driver);
            let naive_millis = millis(started.elapsed());

            let ran = pruned.exploration.explored.max(1);
            Ok(TestReduction {
                key: test.key.clone(),
                pruned: pruned.exploration.explored,
                unsynchronized: blind.exploration.explored,
                unsynchronized_bounded: blind.exploration.exhausted,
                naive: naive.exploration.explored,
                naive_bounded: naive.exploration.exhausted,
                pruned_bounded: pruned.exploration.exhausted,
                pruned_exhaustive: pruned.exploration.exhaustive,
                reduction: f64::from(naive.exploration.explored) / f64::from(ran),
                pruned_millis,
                naive_millis,
            })
        })
        .collect()
}

// ---------------------------------------------------------------- race power

/// One trial: a search started from one root, and what it cost to reach the
/// failure — or that it did not reach one.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Trial {
    pub root: u64,
    /// Interleavings run before the failure. `None` when the trial's budget was
    /// spent without one, which is the case a median must not silently drop.
    pub interleavings: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RacePower {
    pub key: String,
    pub trials: usize,
    /// One `dpor` search per trial, each from its own root.
    pub dpor: Vec<Trial>,
    /// One interleaving per seed, seeds drawn in order from the trial's root.
    pub sampled: Vec<Trial>,
    pub dpor_median: Option<f64>,
    pub dpor_worst: Option<u32>,
    pub dpor_misses: usize,
    pub sampled_median: Option<f64>,
    pub sampled_worst: Option<u32>,
    pub sampled_misses: usize,
    /// Sampled median over search median. `None` when either side never found
    /// it, because a ratio against a miss is a ratio nobody observed.
    pub median_ratio: Option<f64>,
}

/// How many seeds each strategy needs to reach the failing interleaving.
///
/// A trial is a root. Under `dpor` the root seeds one search, which explores
/// until it fails or empties its frontier; under sampling the root starts a run
/// of independent single-interleaving seeds. Counting interleavings on both
/// sides rather than seeds on one and interleavings on the other is what makes
/// the ratio mean "runs of the program", which is the unit that costs wall
/// clock.
pub fn race_power(root: &Path, trials: u32, budget: u32, steps: u32) -> Result<Vec<RacePower>> {
    let front = front(root)?;
    let tests = seeded_tests(&front);
    if tests.is_empty() {
        bail!("no test under `{}` carries `sim.read`", root.display());
    }

    let mut out = Vec::new();
    for test in &tests {
        let dpor: Vec<Trial> = (0..u64::from(trials))
            .map(|root| {
                let plan = Plan {
                    mode: SimMode::Dpor,
                    roots: vec![root],
                    budget,
                    steps,
                    path: Vec::new(),
                };
                let mut driver = Driver::new(&front, test, steps);
                let explored = explore_under(&plan, Dependence::Exact, &mut driver);
                Trial {
                    root,
                    interleavings: explored
                        .exploration
                        .failure
                        .is_some()
                        .then_some(driver.runs),
                }
            })
            .collect();

        let sampled: Vec<Trial> = (0..u64::from(trials))
            .map(|trial| {
                let base = u64::from(trials) * (trial + 1);
                let plan = Plan {
                    mode: SimMode::Random,
                    roots: (base..base + u64::from(budget)).collect(),
                    budget: 1,
                    steps,
                    path: Vec::new(),
                };
                let mut driver = Driver::new(&front, test, steps);
                let explored = explore_under(&plan, Dependence::Exact, &mut driver);
                Trial {
                    root: base,
                    interleavings: explored
                        .exploration
                        .failure
                        .is_some()
                        .then_some(driver.runs),
                }
            })
            .collect();

        let (dpor_median, dpor_worst, dpor_misses) = summarize(&dpor);
        let (sampled_median, sampled_worst, sampled_misses) = summarize(&sampled);
        out.push(RacePower {
            key: test.key.clone(),
            trials: trials as usize,
            median_ratio: match (dpor_median, sampled_median) {
                (Some(d), Some(s)) if d > 0.0 && dpor_misses == 0 && sampled_misses == 0 => {
                    Some(s / d)
                }
                _ => None,
            },
            dpor_median,
            dpor_worst,
            dpor_misses,
            sampled_median,
            sampled_worst,
            sampled_misses,
            dpor,
            sampled,
        });
    }
    Ok(out)
}

/// Median, worst and misses. A trial that never found the failure is counted
/// rather than dropped: a median over the trials that happened to succeed is the
/// most flattering statistic available and the least honest one.
fn summarize(trials: &[Trial]) -> (Option<f64>, Option<u32>, usize) {
    let mut found: Vec<u32> = trials.iter().filter_map(|t| t.interleavings).collect();
    let misses = trials.len() - found.len();
    if found.is_empty() {
        return (None, None, misses);
    }
    found.sort_unstable();
    let mid = found.len() / 2;
    let median = if found.len().is_multiple_of(2) {
        f64::from(found[mid - 1] + found[mid]) / 2.0
    } else {
        f64::from(found[mid])
    };
    (Some(median), found.last().copied(), misses)
}

// ---------------------------------------------------------------- throughput

#[derive(Clone, Debug, Serialize)]
pub struct SeedRate {
    pub key: String,
    pub definitions: usize,
    /// Interleavings run, which is what the wall clock was spent on.
    pub interleavings: u32,
    pub millis: f64,
    pub seeds_per_second: f64,
    /// Scheduling steps across the run, so a rate can be read against the size
    /// of what it was scheduling rather than only against the program's.
    pub steps: u64,
    /// The sample hit a failing interleaving and stopped, so fewer seeds ran
    /// than were asked for. The rate still stands — it is the rate of the seeds
    /// that ran — and the flag is what stops it being read as a full sample.
    pub stopped_early: bool,
}

/// Seeds per second for every seeded test under `root`.
///
/// Whole-test replay means one seed is one entire test — including the setup
/// that precedes the region — so this is the rate that decides whether "thousands
/// of seeds per change" is affordable, and it is deliberately not the rate of the
/// scheduler alone.
pub fn seed_rate(root: &Path, budget: u32, steps: u32) -> Result<Vec<SeedRate>> {
    let front = front(root)?;
    let tests = seeded_tests(&front);
    if tests.is_empty() {
        bail!("no test under `{}` carries `sim.read`", root.display());
    }
    let definitions = front.check.defs.len();

    tests
        .iter()
        .map(|test| {
            let plan = Plan {
                mode: SimMode::Random,
                roots: (0..u64::from(budget)).collect(),
                budget: 1,
                steps,
                path: Vec::new(),
            };
            let mut driver = Driver::new(&front, test, steps);
            let started = Instant::now();
            let explored = explore_under(&plan, Dependence::Exact, &mut driver);
            let taken = started.elapsed();
            let seconds = taken.as_secs_f64().max(f64::EPSILON);
            Ok(SeedRate {
                key: test.key.clone(),
                definitions,
                interleavings: explored.exploration.explored,
                millis: millis(taken),
                seeds_per_second: f64::from(explored.exploration.explored) / seconds,
                steps: explored.exploration.steps,
                stopped_early: explored.exploration.failure.is_some(),
            })
        })
        .collect()
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

// -------------------------------------------------------------------- render

#[derive(Clone, Debug, Serialize)]
pub struct SimMeasurements {
    pub root: String,
    pub reduction: Vec<TestReduction>,
    pub race: Vec<RacePower>,
    pub rate: Vec<SeedRate>,
}

pub fn render(m: &SimMeasurements) -> String {
    let mut out = String::new();
    out.push_str(&format!("{}\n", m.root));

    if !m.reduction.is_empty() {
        out.push_str("\n  exploration\n");
        out.push_str(&format!(
            "    {:<40} {:>8} {:>8} {:>9} {:>10} {:>11} {:>9}\n",
            "test", "pruned", "no-sync", "naive", "reduction", "exhaustive", "pruned ms"
        ));
        for r in &m.reduction {
            out.push_str(&format!(
                "    {:<40} {:>8} {:>8} {:>9} {:>10} {:>11} {:>9.1}\n",
                clip(&r.key, 40),
                bound(r.pruned, r.pruned_bounded),
                bound(r.unsynchronized, r.unsynchronized_bounded),
                bound(r.naive, r.naive_bounded),
                ratio(r.reduction, r.naive_bounded),
                r.pruned_exhaustive,
                r.pruned_millis
            ));
        }
    }

    if !m.race.is_empty() {
        out.push_str("\n  race finding — interleavings to the first failure\n");
        out.push_str(&format!(
            "    {:<38} {:>7} {:>9} {:>8} {:>10} {:>9} {:>8}\n",
            "test", "trials", "dpor med", "worst", "sampled", "worst", "ratio"
        ));
        for r in &m.race {
            out.push_str(&format!(
                "    {:<38} {:>7} {:>9} {:>8} {:>10} {:>9} {:>8}\n",
                clip(&r.key, 38),
                r.trials,
                stat(r.dpor_median, r.dpor_misses),
                stat(r.dpor_worst.map(f64::from), r.dpor_misses),
                stat(r.sampled_median, r.sampled_misses),
                stat(r.sampled_worst.map(f64::from), r.sampled_misses),
                r.median_ratio
                    .map(|x| format!("{x:.1}×"))
                    .unwrap_or_else(|| "—".to_string()),
            ));
        }
    }

    if !m.rate.is_empty() {
        out.push_str("\n  throughput\n");
        out.push_str(&format!(
            "    {:<44} {:>6} {:>9} {:>10} {:>12}\n",
            "test", "defs", "seeds", "ms", "seeds/s"
        ));
        for r in &m.rate {
            out.push_str(&format!(
                "    {:<44} {:>6} {:>9} {:>10.1} {:>12.0}{}\n",
                clip(&r.key, 44),
                r.definitions,
                r.interleavings,
                r.millis,
                r.seeds_per_second,
                if r.stopped_early {
                    "  (stopped at a failure)"
                } else {
                    ""
                }
            ));
        }
    }
    out
}

/// A statistic over trials that did not all find the failure is reported with
/// the misses attached, never as a bare number.
fn stat(value: Option<f64>, misses: usize) -> String {
    match (value, misses) {
        (None, _) => "never".to_string(),
        (Some(v), 0) => format!("{v:.0}"),
        (Some(v), n) => format!("{v:.0}+{n}✗"),
    }
}

/// A count whose search stopped at its budget is a lower bound and prints as
/// one; a count nobody bounded prints bare.
fn bound(count: u32, bounded: bool) -> String {
    if bounded {
        format!(">={count}")
    } else {
        count.to_string()
    }
}

/// A ratio over a bounded numerator is bounded too, and printing it bare is the
/// one place this table could claim a number nobody observed.
fn ratio(reduction: f64, bounded: bool) -> String {
    if bounded {
        format!(">={reduction:.1}×")
    } else {
        format!("{reduction:.1}×")
    }
}

fn clip(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let head: String = s.chars().take(width.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Everything, against one corpus.
pub fn measure(
    root: &Path,
    trials: u32,
    budget: u32,
    steps: u32,
    rate_seeds: u32,
) -> Result<SimMeasurements> {
    Ok(SimMeasurements {
        root: root.display().to_string(),
        reduction: reduction(root, budget, steps)
            .with_context(|| format!("measuring the reduction over `{}`", root.display()))?,
        race: race_power(root, trials, budget, steps)?,
        rate: seed_rate(root, rate_seeds, steps)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::generate;
    use crate::spec::CorpusSpec;
    use crate::write;

    fn corpus(density: f64, tasks: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let spec = CorpusSpec {
            seed: 17,
            modules: 2,
            defs_per_module: 4,
            tests: 2,
            depth: 1,
            concurrent_tests: 1,
            tasks_per_test: tasks,
            steps_per_task: 2,
            conflict_density: density,
            ..CorpusSpec::default()
        };
        write::write(dir.path(), &spec, &generate(&spec)).unwrap();
        dir
    }

    /// The claim, at the size a unit test can afford: pruning never runs more
    /// interleavings than not pruning, and the search reaches its frontier.
    #[test]
    fn pruning_never_costs_more_than_not_pruning() {
        let dir = corpus(0.0, 2);
        let measured = reduction(dir.path(), 512, 100_000).unwrap();
        assert!(!measured.is_empty());
        for r in &measured {
            assert!(
                r.pruned <= r.naive,
                "{}: {} pruned against {} naive",
                r.key,
                r.pruned,
                r.naive
            );
            assert!(r.reduction >= 1.0);
        }
    }

    /// Withholding the recording's clocks may only cost interleavings, never
    /// save them: the filter refuses reorderings that are unreachable, so a
    /// search that cannot see the ordering has strictly more to do.
    #[test]
    fn a_search_that_cannot_see_the_synchronization_never_runs_fewer() {
        let dir = corpus(0.0, 3);
        for r in reduction(dir.path(), 4096, 100_000).unwrap() {
            assert!(
                r.pruned <= r.unsynchronized,
                "{}: {} with clocks against {} without",
                r.key,
                r.pruned,
                r.unsynchronized
            );
        }
    }

    /// A corpus with no failing test reports misses rather than a median over
    /// nothing, and never a zero that reads as "found immediately".
    #[test]
    fn a_test_that_never_fails_reports_misses_and_no_ratio() {
        let dir = corpus(1.0, 2);
        let power = race_power(dir.path(), 2, 16, 100_000).unwrap();
        for p in &power {
            assert_eq!(p.dpor_misses, p.trials);
            assert_eq!(p.dpor_median, None);
            assert_eq!(p.median_ratio, None);
        }
    }

    #[test]
    fn a_rate_is_reported_per_seeded_test_and_counts_the_seeds_it_ran() {
        let dir = corpus(0.5, 2);
        let rates = seed_rate(dir.path(), 8, 100_000).unwrap();
        assert!(!rates.is_empty());
        for r in &rates {
            assert_eq!(r.interleavings, 8);
            assert!(r.seeds_per_second > 0.0);
        }
    }

    /// A statistic that dropped its misses would report the search as better
    /// than it is, which is the one direction this module must not err in.
    #[test]
    fn a_summary_carries_its_misses() {
        let trials = [
            Trial {
                root: 0,
                interleavings: Some(4),
            },
            Trial {
                root: 1,
                interleavings: None,
            },
            Trial {
                root: 2,
                interleavings: Some(2),
            },
        ];
        let (median, worst, misses) = summarize(&trials);
        assert_eq!(median, Some(3.0));
        assert_eq!(worst, Some(4));
        assert_eq!(misses, 1);
        assert_eq!(stat(median, misses), "3+1✗");
    }
}

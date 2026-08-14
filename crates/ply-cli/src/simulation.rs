//! Turning the simulation flags into the plan a run is cached under.
//!
//! Every field of a `Plan` is in a seeded test's cache key, so this is the one
//! place a flag becomes a claim. Two rules follow and both are enforced here
//! rather than left to the reader: a flag that cannot mean anything under the
//! chosen mode is refused rather than ignored, and `--seed` names one
//! interleaving and therefore excludes every flag that would widen the search.

use crate::cli::SimOptions;
use ply_eval::sim::{DEFAULT_BUDGET, DEFAULT_RANDOM_ROOTS, DEFAULT_STEPS};
use ply_eval::{Plan, Seed, SimMode};

/// The seeds a plan starts from, when the user did not say. `dpor` already
/// enumerates equivalence classes from one seed, so a second explores the same
/// ones in a different order; `random` is a sample, so it needs many.
fn default_seeds(mode: SimMode) -> u32 {
    match mode {
        SimMode::Random => DEFAULT_RANDOM_ROOTS,
        SimMode::Once | SimMode::Dpor => 1,
    }
}

pub fn plan(options: &SimOptions) -> Plan {
    if let Some(seed) = &options.seed {
        return Plan {
            steps: options.sim_steps.unwrap_or(DEFAULT_STEPS),
            ..Plan::once(seed.clone())
        }
        .normalized();
    }

    let mode: SimMode = options.sim.into();
    let seeds = options.seeds.unwrap_or_else(|| default_seeds(mode));
    Plan {
        mode,
        roots: (0..u64::from(seeds)).collect(),
        budget: match mode {
            // `random` is one interleaving per seed by definition, and `once`
            // is the one the seed names. Only `dpor` has a budget to spend.
            SimMode::Random | SimMode::Once => 1,
            SimMode::Dpor => options.sim_budget.unwrap_or(DEFAULT_BUDGET),
        },
        steps: options.sim_steps.unwrap_or(DEFAULT_STEPS),
        path: Vec::new(),
    }
    .normalized()
}

/// The plan `ply run` evaluates under: exactly one interleaving, the one the
/// seed names. Exploration is a test-time activity, so there is no mode here.
pub fn run_plan(seed: Option<&Seed>) -> Plan {
    Plan::once(seed.cloned().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::SimArg;

    fn options() -> SimOptions {
        SimOptions {
            seed: None,
            sim: SimArg::default(),
            seeds: None,
            sim_budget: None,
            sim_steps: None,
            measure_reduction: false,
        }
    }

    #[test]
    fn the_default_plan_is_one_dpor_seed_at_the_default_budget() {
        let built = plan(&options());
        assert_eq!(built.mode, SimMode::Dpor);
        assert_eq!(built.roots, vec![0]);
        assert_eq!(built.budget, DEFAULT_BUDGET);
        assert_eq!(built.steps, DEFAULT_STEPS);
        assert_eq!(built, Plan::default().normalized());
    }

    #[test]
    fn seeds_widens_the_root_set_under_either_mode() {
        let built = plan(&SimOptions {
            seeds: Some(8),
            ..options()
        });
        assert_eq!(built.roots, (0..8).collect::<Vec<u64>>());

        let sampled = plan(&SimOptions {
            sim: SimArg::Random,
            ..options()
        });
        assert_eq!(sampled.roots.len(), DEFAULT_RANDOM_ROOTS as usize);
        assert_eq!(sampled.budget, 1);
    }

    #[test]
    fn a_seed_replays_exactly_one_interleaving() {
        let built = plan(&SimOptions {
            seed: Some(Seed::at(7, vec![3, 0, 2])),
            ..options()
        });
        assert_eq!(built.mode, SimMode::Once);
        assert_eq!(built.roots, vec![7]);
        assert_eq!(built.path, vec![3, 0, 2]);
        assert_eq!(built.budget, 1);
        assert_eq!(built.seeds(), vec![Seed::at(7, vec![3, 0, 2])]);
    }

    /// The step bound is not a widening: a replay still needs one, and it is the
    /// only search flag `--seed` leaves alone.
    #[test]
    fn a_replay_keeps_its_step_bound() {
        let built = plan(&SimOptions {
            seed: Some(Seed::root(7)),
            sim_steps: Some(64),
            ..options()
        });
        assert_eq!(built.steps, 64);
        assert_eq!(built.mode, SimMode::Once);
    }

    /// A flag that cannot mean anything is refused rather than ignored: a
    /// silently dropped `--sim-budget` reads as a search that was widened.
    #[test]
    fn a_budget_under_random_is_refused() {
        assert!(
            SimOptions {
                sim: SimArg::Random,
                sim_budget: Some(16),
                ..options()
            }
            .conflict()
            .is_some()
        );
        assert!(options().conflict().is_none());
        assert!(
            SimOptions {
                sim_budget: Some(16),
                ..options()
            }
            .conflict()
            .is_none()
        );
    }

    #[test]
    fn every_flag_that_widens_the_search_changes_the_key() {
        let base = plan(&options());
        let variants = [
            plan(&SimOptions {
                seeds: Some(2),
                ..options()
            }),
            plan(&SimOptions {
                sim_budget: Some(1),
                ..options()
            }),
            plan(&SimOptions {
                sim_steps: Some(1),
                ..options()
            }),
            plan(&SimOptions {
                sim: SimArg::Random,
                ..options()
            }),
            plan(&SimOptions {
                seed: Some(Seed::root(0)),
                ..options()
            }),
        ];
        let mut seen = vec![base.digest()];
        for variant in variants {
            let digest = variant.digest();
            assert!(!seen.contains(&digest), "{variant:?} collided");
            seen.push(digest);
        }
    }

    /// `--measure-reduction` reports a number; it does not change what was
    /// searched, so it must not split the cache.
    #[test]
    fn measuring_the_reduction_does_not_change_the_plan() {
        assert_eq!(
            plan(&options()),
            plan(&SimOptions {
                measure_reduction: true,
                ..options()
            })
        );
    }

    #[test]
    fn ply_run_explores_the_one_interleaving_its_seed_names() {
        assert_eq!(run_plan(None), Plan::once(Seed::default()));
        assert_eq!(
            run_plan(Some(&Seed::at(9, vec![1]))).seeds(),
            vec![Seed::at(9, vec![1])]
        );
    }
}

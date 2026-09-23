use ply_cli::simulation::*;
use ply_eval::sim::{DEFAULT_BUDGET, DEFAULT_RANDOM_ROOTS, DEFAULT_STEPS};
use ply_eval::{Plan, Seed, SimMode};
use ply_machine::simulation::SimOptions;

fn options() -> SimOptions {
    SimOptions {
        seed: None,
        sim: SimMode::default(),
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
        sim: SimMode::Random,
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

/// A replay still needs the step bound; it is the only search flag `--seed` leaves alone.
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
            sim: SimMode::Random,
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

/// `--measure-reduction` reports a number without changing what was searched, so it must not split the cache.
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

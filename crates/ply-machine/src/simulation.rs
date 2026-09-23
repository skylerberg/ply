//! Turning the simulation flags into the plan a run is cached under.

/// What a run searches, as plain data the machines read; the shell's parsed flags convert.
#[derive(Clone, Debug, Default)]
pub struct SimOptions {
    pub seed: Option<ply_eval::Seed>,
    pub sim: ply_eval::SimMode,
    pub seeds: Option<u32>,
    pub sim_budget: Option<u32>,
    pub sim_steps: Option<u32>,
    pub measure_reduction: bool,
}

/// Search bounds; they key every cached result weaker than a proof.
#[derive(Clone, Debug, Default)]
pub struct ProveOptions {
    pub prove_cases: Option<u32>,
    pub prove_roots: Option<u32>,
    pub prove_budget: Option<u32>,
    pub shrink_budget: Option<u32>,
    pub prove_steps: Option<i64>,
}
use ply_eval::sim::{DEFAULT_BUDGET, DEFAULT_RANDOM_ROOTS, DEFAULT_STEPS};
use ply_eval::{Plan, Seed, SimMode};
use ply_prove::{DEFAULT_CASES, DEFAULT_PROVE_BUDGET, DEFAULT_SHRINK_BUDGET, ProvePlan};

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

    let mode: SimMode = options.sim;
    let seeds = options.seeds.unwrap_or_else(|| default_seeds(mode));
    Plan {
        mode,
        roots: (0..u64::from(seeds)).collect(),
        budget: match mode {
            SimMode::Random | SimMode::Once => 1,
            SimMode::Dpor => options.sim_budget.unwrap_or(DEFAULT_BUDGET),
        },
        steps: options.sim_steps.unwrap_or(DEFAULT_STEPS),
        path: Vec::new(),
    }
    .normalized()
}

/// What an obligation weaker than a proof is discharged against and cached under.
pub fn prove_plan(options: &ProveOptions, simulation: &SimOptions) -> ProvePlan {
    let roots = options.prove_roots.unwrap_or(1);
    ProvePlan {
        cases: options.prove_cases.unwrap_or(DEFAULT_CASES),
        roots: (0..u64::from(roots)).collect(),
        prove_budget: options.prove_budget.unwrap_or(DEFAULT_PROVE_BUDGET),
        shrink_budget: options.shrink_budget.unwrap_or(DEFAULT_SHRINK_BUDGET),
        step_budget: options.prove_steps.unwrap_or(ply_eval::DEFAULT_STEP_BUDGET),
        sim: plan(simulation),
    }
    .normalized()
}

/// Exactly one interleaving, the one the seed names.
pub fn run_plan(seed: Option<&Seed>) -> Plan {
    Plan::once(seed.cloned().unwrap_or_default())
}

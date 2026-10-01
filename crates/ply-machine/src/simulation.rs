//! The bounds a run of the prover is discharged and cached under.

use ply_prove::{DEFAULT_CASES, DEFAULT_PROVE_BUDGET, DEFAULT_SHRINK_BUDGET, ProvePlan};

/// Search bounds; they key every cached result weaker than a proof.
#[derive(Clone, Debug, Default)]
pub struct ProveOptions {
    pub prove_cases: Option<u32>,
    pub prove_roots: Option<u32>,
    pub prove_budget: Option<u32>,
    pub shrink_budget: Option<u32>,
    pub prove_steps: Option<i64>,
}

/// What an obligation weaker than a proof is discharged against and cached under.
pub fn prove_plan(options: &ProveOptions) -> ProvePlan {
    let roots = options.prove_roots.unwrap_or(1);
    ProvePlan {
        cases: options.prove_cases.unwrap_or(DEFAULT_CASES),
        roots: (0..u64::from(roots)).collect(),
        prove_budget: options.prove_budget.unwrap_or(DEFAULT_PROVE_BUDGET),
        shrink_budget: options.shrink_budget.unwrap_or(DEFAULT_SHRINK_BUDGET),
        step_budget: options.prove_steps.unwrap_or(ply_eval::DEFAULT_STEP_BUDGET),
    }
    .normalized()
}

//! Turning the simulation flags into the plan a run is cached under.

/// What a run searches, as plain data the machines read; the shell's parsed flags convert.
#[derive(Clone, Debug, Default)]
pub struct SimOptions {
    pub seed: Option<ply_eval::Seed>,
    pub sim: ply_eval::SimMode,
    /// The seeds searched; the mode's default count from 0 when `None`.
    pub roots: Option<std::ops::Range<u64>>,
    pub sim_budget: Option<u32>,
    pub sim_steps: Option<u32>,
    pub measure_reduction: bool,
}

/// The simulation options as the program builds them from the parsed line. The program validated
/// already, so a bad value here is an internal error.
pub fn sim_options_of(
    v: &ply_eval::Value,
    span: ply_span::Span,
) -> Result<SimOptions, ply_span::Diagnostic> {
    use crate::payload::{field_of, missing, opt_int_at, opt_str_at, option_of};
    let int = |v: &ply_eval::Value, name: &str| -> Result<u64, ply_span::Diagnostic> {
        u64::try_from(field_of(v, name, span)?.as_int(span, name)?)
            .map_err(|_| missing("a natural", span))
    };
    Ok(SimOptions {
        seed: match opt_str_at(v, "seed", span)? {
            Some(text) => {
                Some(ply_eval::Seed::parse(&text).ok_or_else(|| missing("a parsed seed", span))?)
            }
            None => None,
        },
        sim: match field_of(v, "mode", span)?.as_str(span, "the simulation's mode")? {
            "once" => ply_eval::SimMode::Once,
            "random" => ply_eval::SimMode::Random,
            _ => ply_eval::SimMode::Dpor,
        },
        roots: match option_of(field_of(v, "roots", span)?, "the seeds searched", span)? {
            Some(range) => Some(int(range, "from")?..int(range, "to")?),
            None => None,
        },
        sim_budget: opt_int_at(v, "budget", span)?.map(|n| n as u32),
        sim_steps: opt_int_at(v, "steps", span)?.map(|n| n as u32),
        measure_reduction: field_of(v, "measure_reduction", span)?
            .as_bool(span, "measure_reduction")?,
    })
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
    let roots = options
        .roots
        .clone()
        .unwrap_or_else(|| 0..u64::from(default_seeds(mode)));
    Plan {
        mode,
        roots: roots.collect(),
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

//! Which tests are searched, what a search's green verdict may be written under, and what the run
//! reports about its searches.

use ply_eval::explore::Interleaving;
use ply_eval::{DefHash, Diagnostic, EffectAtom, Exploration, Footprint, Machine, Seed};

/// The effect whose atom a `simulate` region leaves in a footprint: the seed it reads.
const SIM_EFFECT: &str = "sim";

/// Something in its closure entered a `simulate` region, so it runs once per interleaving.
pub fn is_seeded(f: &Footprint) -> bool {
    f.atoms().any(is_seed)
}

/// An atom that reads the seed a search hands the test.
pub fn is_seed(a: &EffectAtom) -> bool {
    a.effect.as_str() == SIM_EFFECT
}

pub fn seed_run(machine: &mut Machine<'_>, seed: &Seed, steps: u32) {
    machine.set_seed(seed.clone(), steps);
}

/// The interleaving the last entry point took, given how it ended.
pub fn interleaving_of(
    machine: &Machine<'_>,
    outcome: &Result<(), Diagnostic>,
) -> Option<Interleaving> {
    machine
        .simulated()
        .map(|record| record.interleaving(outcome))
}

/// Where a green verdict may be written, or why it may not be.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Record {
    /// Write `Pass` under each of these, in order.
    Under(Vec<DefHash>),
    /// The search spent its budget.
    Exhausted,
    /// The footprint carries `sim.read` but the evaluator reported no search.
    Unobserved,
    /// The run reached a host handler, so its green verdict is about one socket at one moment.
    Host,
}

impl Record {
    pub fn keys(&self) -> &[DefHash] {
        match self {
            Record::Under(keys) => keys,
            _ => &[],
        }
    }

    pub fn is_written(&self) -> bool {
        matches!(self, Record::Under(_))
    }
}

/// Whether a green verdict may be written under the keys the program filed it under: a search that
/// spent its budget, or one that was never observed, proved nothing.
pub fn record_under(filed: &[DefHash], seeded: bool, exploration: Option<&Exploration>) -> Record {
    if seeded && exploration.is_none() {
        return Record::Unobserved;
    }
    // Unseeded tests too: a `sim.seed()` handler hides `sim.read`, but the region still searched.
    if exploration.is_some_and(|e| !e.is_cacheable()) {
        return Record::Exhausted;
    }
    Record::Under(filed.to_vec())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SimSummary {
    pub simulated: usize,
    /// Every test the run executed, simulated or not.
    pub total: usize,
    /// Seeds started from, summed over the simulated tests.
    pub seeds: usize,
    pub interleavings: u64,
    /// Searches that emptied their frontier.
    pub exhaustive: usize,
    /// Searches that spent their budget.
    pub exhausted: usize,
    pub failed: usize,
}

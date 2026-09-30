//! What a run's searches are written under, and what it reports about them.

use ply_eval::explore::Interleaving;
use ply_eval::{Exploration, Machine, Seed};
use ply_span::Diagnostic;
use ply_ty::DefHash;

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

impl SimSummary {
    pub fn any(&self) -> bool {
        self.simulated > 0
    }

    /// `simulated: 3 of 47 · 61 interleavings · 3 exhaustive`.
    pub fn line(&self) -> Option<String> {
        if !self.any() {
            return None;
        }
        let mut line = format!(
            "simulated: {} of {} · {} interleaving{}",
            self.simulated,
            self.total,
            self.interleavings,
            if self.interleavings == 1 { "" } else { "s" }
        );
        if self.exhaustive > 0 {
            line.push_str(&format!(" · {} exhaustive", self.exhaustive));
        }
        if self.exhausted > 0 {
            line.push_str(&format!(" · {} budget spent, not cached", self.exhausted));
        }
        Some(line)
    }
}

pub fn replay_command(seed: &Seed, test_name: &str) -> String {
    format!("ply test --seed {seed} --filter \"{test_name}\"")
}

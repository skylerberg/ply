//! What a run's searches are written under, and what it reports about them.
//!
//! [`crate::key`] answers "which `DefHash` names this claim"; this module
//! answers the prior question of whether there is a claim to write at all. A
//! green simulated run is not automatically a cacheable one: a search that spent
//! its budget proved nothing about the interleavings it did not reach, and a
//! test whose row says it simulated but whose evaluator reported no search is a
//! run nobody observed.
//!
//! Both of those are reported green and re-run next time. That is the direction
//! to err in — the opposite mistake caches a pass a different seed would have
//! failed, and it is silent.

use crate::key::{result_key, seed_key, writes_seed_keys};
use ply_eval::explore::Interleaving;
use ply_eval::{Exploration, Machine, Plan, Seed};
use ply_hash::DefHash;
use ply_span::Diagnostic;

/// Fix the interleaving the next entry point will take.
///
/// This and [`interleaving_of`] are the whole of what `ply-test` owes
/// `ply-eval`: the machine installs `ply_eval::sched::Scheduler` as a native
/// prompt at a `simulate` region, resumes the task the seed names at every
/// scheduling point, and reports the steps it took. The search over those
/// interleavings is `ply_eval::explore`, and driving it is this crate's job —
/// exploration is a test-time activity, because a `simulate` region's value is
/// the one its seed names and every other interleaving is a search.
pub fn seed_run(machine: &mut Machine<'_>, seed: &Seed, steps: u32) {
    machine.set_seed(seed.clone(), steps);
}

/// The interleaving the last entry point took, given how it ended.
///
/// `None` means no interleaving was observed. That is not a silent hole: a test
/// whose footprint carries `sim.read` and whose search observed nothing is
/// [`Record::Unobserved`] — reported green, warned about, and never cached.
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
    /// The search spent its budget. Green, and not a proof.
    Exhausted,
    /// The test's footprint carries `sim.read` and the evaluator reported no
    /// search, so what actually ran is unknown.
    Unobserved,
    /// The run reached a host handler, so its green verdict is a statement about
    /// a socket at one moment and about nothing else.
    ///
    /// Decided by what the machine **did**, never by the prediction selection
    /// made from footprints: the prediction drives which tests run, and this
    /// drives the write. When the two disagree the run has observed a footprint
    /// that under-reports, which is the failure mode the whole boundary is built
    /// around, so it is reported rather than silently uncached.
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

/// `run` is the plan the run was selected against and whose key the result is
/// published under; `ran` is what this test actually searched, which differs
/// only when `random` narrowed a widened root set to the roots no per-seed key
/// covered.
///
/// The full plan's key is still written in that case, and legitimately: the
/// roots that were narrowed away were narrowed away *because* their per-seed
/// keys already hold a pass.
pub fn record_under(
    test_hash: DefHash,
    seeded: bool,
    run: &Plan,
    ran: &Plan,
    exploration: Option<&Exploration>,
) -> Record {
    if seeded && exploration.is_none() {
        return Record::Unobserved;
    }
    // Applied to an unseeded test too: a handler answering `sim.seed()` closes
    // `sim.read` out of the row, and the region inside it still searched under a
    // budget it may have spent.
    if exploration.is_some_and(|e| !e.is_cacheable()) {
        return Record::Exhausted;
    }
    if !seeded {
        return Record::Under(vec![test_hash]);
    }
    let mut keys = Vec::with_capacity(ran.roots.len() + 1);
    if writes_seed_keys(run) {
        keys.extend(
            ran.roots
                .iter()
                .map(|&root| seed_key(test_hash, &Seed::root(root))),
        );
    }
    keys.push(result_key(test_hash, true, run));
    Record::Under(keys)
}

/// What this run's simulated tests searched, aggregated.
///
/// `simulated` counts the tests that reached a `simulate` region rather than the
/// tests that ran, because a test that reached none contributes nothing a
/// consumer could tell from a zero.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SimSummary {
    pub simulated: usize,
    /// Every test the run executed, simulated or not.
    pub total: usize,
    /// Seeds the plan started the simulated tests from, summed over them.
    pub seeds: usize,
    pub interleavings: u64,
    /// Searches that emptied their frontier: every interleaving ran.
    pub exhaustive: usize,
    /// Searches that spent their budget. Reported green and not cached.
    pub exhausted: usize,
    /// Searches that reported a failing seed.
    pub failed: usize,
}

impl SimSummary {
    pub fn any(&self) -> bool {
        self.simulated > 0
    }

    /// `simulated: 3 of 47 · 61 interleavings · 3 exhaustive`. Empty when
    /// nothing simulated, so a corpus with no `simulate` region reads exactly as
    /// it does today.
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

/// The command that replays exactly this failure. The point of the milestone is
/// that the repro handed to an agent is a seed, so the artifact prints the
/// command rather than describing it.
pub fn replay_command(seed: &Seed, test_name: &str) -> String {
    format!("ply test --seed {seed} --filter \"{test_name}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_eval::{Naive, SimMode};

    fn hash(byte: u8) -> DefHash {
        DefHash([byte; 32])
    }

    fn passing(explored: u32) -> Exploration {
        Exploration {
            explored,
            exhaustive: true,
            ..Exploration::default()
        }
    }

    #[test]
    fn an_unsimulated_test_is_written_under_its_own_hash_and_nothing_else() {
        let plan = Plan::default();
        assert_eq!(
            record_under(hash(1), false, &plan, &plan, None),
            Record::Under(vec![hash(1)])
        );
    }

    /// The rule whose absence is silent: a run under one plan must not be able
    /// to read a pass another plan earned.
    #[test]
    fn a_seeded_test_is_never_written_under_its_bare_hash() {
        let plan = Plan::default();
        let record = record_under(hash(1), true, &plan, &plan, Some(&passing(12)));
        assert!(record.is_written());
        assert!(!record.keys().contains(&hash(1)));
        assert_eq!(record.keys(), [crate::sim_key(hash(1), &plan)]);
    }

    #[test]
    fn a_dpor_search_writes_no_per_root_key() {
        let plan = Plan {
            roots: vec![0, 1, 2],
            ..Plan::default()
        };
        assert_eq!(plan.mode, SimMode::Dpor);
        let record = record_under(hash(1), true, &plan, &plan, Some(&passing(9)));
        assert_eq!(record.keys().len(), 1);
    }

    #[test]
    fn a_random_search_writes_one_key_per_root_it_ran_plus_the_plan() {
        let run = Plan::random(4);
        let ran = Plan {
            roots: vec![2, 3],
            ..run.clone()
        };
        let record = record_under(hash(1), true, &run, &ran, Some(&passing(2)));
        assert_eq!(
            record.keys(),
            [
                crate::seed_key(hash(1), &Seed::root(2)),
                crate::seed_key(hash(1), &Seed::root(3)),
                crate::sim_key(hash(1), &run),
            ]
        );
    }

    /// The first green `det` test in the language that is not cacheable, and it
    /// is correct that it is not.
    #[test]
    fn a_spent_budget_writes_nothing_under_either_mode() {
        let spent = Exploration {
            explored: 256,
            exhausted: true,
            ..Exploration::default()
        };
        for plan in [Plan::default(), Plan::random(4)] {
            let record = record_under(hash(1), true, &plan, &plan, Some(&spent));
            assert_eq!(record, Record::Exhausted);
            assert!(record.keys().is_empty());
        }
    }

    /// A handler answering `sim.seed()` takes `sim.read` out of the row, but the
    /// region inside it still searched under a budget it may have spent.
    #[test]
    fn a_spent_budget_stops_an_unseeded_test_caching_too() {
        let plan = Plan::default();
        let spent = Exploration {
            explored: 256,
            exhausted: true,
            ..Exploration::default()
        };
        assert_eq!(
            record_under(hash(1), false, &plan, &plan, Some(&spent)),
            Record::Exhausted
        );
    }

    #[test]
    fn a_seeded_test_whose_search_was_not_observed_writes_nothing() {
        let plan = Plan::default();
        assert_eq!(
            record_under(hash(1), true, &plan, &plan, None),
            Record::Unobserved
        );
    }

    #[test]
    fn the_summary_line_names_the_counts_and_is_silent_without_a_region() {
        assert_eq!(SimSummary::default().line(), None);
        let summary = SimSummary {
            simulated: 3,
            total: 47,
            seeds: 3,
            interleavings: 61,
            exhaustive: 3,
            exhausted: 0,
            failed: 0,
        };
        assert_eq!(
            summary.line().unwrap(),
            "simulated: 3 of 47 · 61 interleavings · 3 exhaustive"
        );
    }

    #[test]
    fn a_spent_budget_is_said_out_loud_in_the_summary() {
        let summary = SimSummary {
            simulated: 1,
            total: 1,
            seeds: 1,
            interleavings: 256,
            exhausted: 1,
            ..SimSummary::default()
        };
        assert!(summary.line().unwrap().contains("not cached"));
    }

    #[test]
    fn the_replay_command_is_the_command() {
        assert_eq!(
            replay_command(&Seed::at(0, vec![1, 0, 3]), "balance never goes negative"),
            "ply test --seed 0:1.0.3 --filter \"balance never goes negative\""
        );
    }

    #[test]
    fn a_measured_reduction_does_not_change_what_is_written() {
        let plan = Plan::default();
        let measured = Exploration {
            naive: Some(Naive {
                explored: 720,
                bounded: false,
            }),
            ..passing(12)
        };
        assert_eq!(
            record_under(hash(1), true, &plan, &plan, Some(&measured)).keys(),
            [crate::sim_key(hash(1), &plan)]
        );
    }
}

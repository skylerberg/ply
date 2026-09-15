//! What a run's searches are written under, and what it reports about them.

use crate::key::{Engine, result_key, seed_key, writes_seed_keys};
use ply_eval::explore::{Interleaving, Verdict};
use ply_eval::sim::Access;
use ply_eval::{Exploration, Machine, Plan, Seed};
use ply_hash::DefHash;
use ply_span::Diagnostic;
use std::collections::BTreeMap;

/// Fix the interleaving the next entry point will take.
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
    /// The test's footprint carries `sim.read` and the evaluator reported no search, so what
    /// actually ran is unknown.
    Unobserved,
    /// The run reached a host handler, so its green verdict is a statement about a socket at one
    /// moment and about nothing else.
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

/// `run` is the plan the run was selected against and whose key the result is published under;
/// `ran` is what this test actually searched, which differs only when `random` narrowed a widened
/// root set to the roots no per-seed key covered.
pub fn record_under(
    test_hash: DefHash,
    seeded: bool,
    run: &Plan,
    ran: &Plan,
    exploration: Option<&Exploration>,
    engine: &Engine,
) -> Record {
    if seeded && exploration.is_none() {
        return Record::Unobserved;
    }
    // Applied to an unseeded test too: a handler answering `sim.seed()` closes `sim.read` out of
    // the row, and the region inside it still searched under a budget it may have spent.
    if exploration.is_some_and(|e| !e.is_cacheable()) {
        return Record::Exhausted;
    }
    if !seeded {
        return Record::Under(vec![result_key(test_hash, false, run, engine)]);
    }
    let mut keys = Vec::with_capacity(ran.roots.len() + 1);
    if writes_seed_keys(run) {
        keys.extend(
            ran.roots
                .iter()
                .map(|&root| seed_key(test_hash, &Seed::root(root), engine)),
        );
    }
    keys.push(result_key(test_hash, true, run, engine));
    Record::Under(keys)
}

/// What this run's simulated tests searched, aggregated.
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
    /// Searches that spent their budget.
    pub exhausted: usize,
    /// Searches that reported a failing seed.
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

/// The command that replays exactly this failure.
pub fn replay_command(seed: &Seed, test_name: &str) -> String {
    format!("ply test --seed {seed} --filter \"{test_name}\"")
}

/// The first way two runs of one seed disagree, or `None` when they are one schedule.
pub fn schedules_differ(ours: &Interleaving, theirs: &Interleaving) -> Option<String> {
    let verdict = |i: &Interleaving| match &i.verdict {
        Verdict::Passed => "passed".to_string(),
        Verdict::Failed(d) => format!("failed: {}", d.message),
    };
    if matches!(ours.verdict, Verdict::Passed) != matches!(theirs.verdict, Verdict::Passed) {
        return Some(format!(
            "the compiled tier {} and the machine {}",
            verdict(ours),
            verdict(theirs)
        ));
    }
    if ours.steps.len() != theirs.steps.len() {
        return Some(format!(
            "the compiled tier took {} step(s) and the machine {}",
            ours.steps.len(),
            theirs.steps.len()
        ));
    }
    // A cell is named by its slot in an arena, and the two engines' arenas number theirs
    // differently; what must agree is which accesses touch the same cell, so each side's cells
    // are renamed by first appearance before the footprints are compared.
    let canonical = |steps: &[ply_eval::explore::Step]| -> Vec<Vec<String>> {
        let mut names: BTreeMap<String, usize> = BTreeMap::new();
        steps
            .iter()
            .map(|step| {
                step.accesses
                    .accesses()
                    .map(|access| match access {
                        Access::Cell { id, mode } => {
                            let key = format!("{id:?}");
                            let next = names.len();
                            let n = *names.entry(key).or_insert(next);
                            format!("cell #{n} {mode:?}")
                        }
                        other => format!("{other:?}"),
                    })
                    .collect()
            })
            .collect()
    };
    let (ours_cells, theirs_cells) = (canonical(&ours.steps), canonical(&theirs.steps));
    for (i, (a, b)) in ours.steps.iter().zip(&theirs.steps).enumerate() {
        if a.task != b.task || a.enabled != b.enabled || a.choice != b.choice {
            return Some(format!(
                "at step {i} the compiled tier ran {:?} (choice {} of {:?}) and the machine {:?} (choice {} of {:?})",
                a.task, a.choice, a.enabled, b.task, b.choice, b.enabled
            ));
        }
        if ours_cells[i] != theirs_cells[i] {
            return Some(format!(
                "at step {i} {:?} touched {:?} in the compiled tier and {:?} in the machine",
                a.task, ours_cells[i], theirs_cells[i]
            ));
        }
    }
    if ours.virtual_time != theirs.virtual_time {
        return Some(format!(
            "the compiled tier ended at virtual time {} and the machine at {}",
            ours.virtual_time, theirs.virtual_time
        ));
    }
    None
}

//! Two projections of one run: lines for a person and JSON for an agent.
//!
//! The JSON is not a summary of the text. A failure carries its full diagnostic
//! and its suspect set, because the point of the whole system is that an agent
//! can act on a failure without re-deriving which of its edits caused it.

use crate::{Failure, Reason, RunReport, Selection, Status, TestResult};
use ply_core::CheckOutput;
use ply_hash::HashOutput;
use serde_json::{Value, json};
use std::time::Duration;

fn millis(d: Duration) -> f64 {
    (d.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

impl Selection {
    /// One line per test plus one per group, in the order `run` will execute
    /// them.
    pub fn explain(&self, check: &CheckOutput, hashes: &HashOutput) -> Vec<String> {
        let mut lines = Vec::with_capacity(self.total + self.groups.len());
        for (index, reason) in self.reasons.iter().enumerate() {
            let name = check.tests.get(index).map(|t| t.name.as_str()).unwrap_or("<unknown>");
            let hash = hashes
                .tests
                .get(index)
                .map(|h| h.short())
                .unwrap_or_else(|| "-".repeat(12));
            let verb = if reason.runs() { "run " } else { "skip" };
            lines.push(format!("{verb} {hash}  {name}  ({})", explain_reason(*reason)));
        }
        for (g, group) in self.groups.iter().enumerate() {
            let names: Vec<&str> = group
                .iter()
                .map(|&i| check.tests.get(i).map(|t| t.name.as_str()).unwrap_or("<unknown>"))
                .collect();
            let footprint = group
                .iter()
                .filter_map(|&i| check.tests.get(i))
                .fold(ply_core::Footprint::empty(), |acc, t| acc.union(&t.footprint));
            lines.push(format!(
                "group {g}: {} test(s) {} — combined footprint {footprint}",
                group.len(),
                names.join(", ")
            ));
        }
        lines
    }

    pub fn to_json(&self, check: &CheckOutput, hashes: &HashOutput) -> Value {
        let tests: Vec<Value> = self
            .reasons
            .iter()
            .enumerate()
            .map(|(index, reason)| {
                json!({
                    "index": index,
                    "name": check.tests.get(index).map(|t| t.name.clone()),
                    "hash": hashes.tests.get(index).map(|h| h.to_hex()),
                    "selected": reason.runs(),
                    "reason": reason,
                    "group": self.group_of(index),
                    "footprint": check.tests.get(index).map(|t| t.footprint.to_string()),
                })
            })
            .collect();
        json!({
            "total": self.total,
            "selected": self.to_run.len(),
            "cached": self.cached.len(),
            "groups": self.groups,
            "tests": tests,
        })
    }
}

fn explain_reason(reason: Reason) -> &'static str {
    match reason {
        Reason::New => "hash absent from the cache",
        Reason::Nondet => "test/nondet always runs and is never cached",
        Reason::PreviousFailure => "cache holds a failure; failures are never trusted",
        Reason::Cached => "hash present and green",
        Reason::Unhashed => "no hash available for this test",
    }
}

impl RunReport {
    pub fn to_json(&self) -> Value {
        json!({
            "passed": self.passed,
            "failed": self.failed,
            "cached": self.cached,
            "duration_ms": millis(self.duration),
            "success": self.is_success(),
            "tests": self.results.iter().map(test_json).collect::<Vec<_>>(),
            "failures": self.failures.iter().map(failure_json).collect::<Vec<_>>(),
            "warnings": self.warnings,
        })
    }

    /// The human summary, without the per-test lines: one line of counts, then
    /// each failure with its suspects.
    pub fn summary(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "{} failed, {} passed, {} cached ({:.2}s)",
            self.failed,
            self.passed,
            self.cached,
            self.duration.as_secs_f64()
        )];
        for failure in &self.failures {
            lines.push(String::new());
            lines.push(failure.name.clone());
            lines.push(format!("  {}", failure.diagnostic.message));
            for note in &failure.diagnostic.notes {
                lines.push(format!("  = {note}"));
            }
            if !failure.suspects.is_empty() {
                let names: Vec<&str> = failure.suspects.iter().map(|s| s.as_str()).collect();
                lines.push(format!("  suspects: {}", names.join(", ")));
            }
        }
        lines
    }

}

impl TestResult {
    pub fn line(&self) -> String {
        let mark = match self.status {
            Status::Passed => "✓",
            Status::Failed => "✗",
            Status::Panicked => "!",
        };
        format!("{mark} {:<40} {:>8.1}ms", self.name, millis(self.duration))
    }
}

fn test_json(result: &TestResult) -> Value {
    json!({
        "index": result.index,
        "name": result.name,
        "hash": result.hash.map(|h| h.to_hex()),
        "group": result.group,
        "status": result.status,
        "duration_ms": millis(result.duration),
        "diagnostic": result.failure,
    })
}

fn failure_json(failure: &Failure) -> Value {
    json!({
        "name": failure.name,
        "diagnostic": failure.diagnostic,
        "suspects": failure.suspects,
    })
}

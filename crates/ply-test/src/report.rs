//! Two projections of one run: lines for a person and JSON for an agent.
//!
//! The JSON is not a summary of the text. A failure carries its full diagnostic
//! and its suspect set, because the point of the whole system is that an agent
//! can act on a failure without re-deriving which of its edits caused it.

use crate::bisect::Bisection;
use crate::slice::{Assertion, CausalSlice};
use crate::{Attribution, Failure, Reason, RunReport, Selection, Status, Suspect, TestResult};
use ply_core::CheckOutput;
use ply_hash::HashOutput;
use serde_json::{Value, json};
use std::time::Duration;

/// Bumped whenever a field in the failure artifact changes meaning or leaves.
/// A machine consumer that acts without asking a follow-up question needs to
/// know what it is parsing before it parses it.
pub const SCHEMA_VERSION: u32 = 2;

fn millis(d: Duration) -> f64 {
    (d.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

impl Selection {
    /// One line per test plus one per group, in the order `run` will execute
    /// them.
    pub fn explain(&self, check: &CheckOutput, hashes: &HashOutput) -> Vec<String> {
        let mut lines = Vec::with_capacity(self.total + self.groups.len());
        for (index, reason) in self.reasons.iter().enumerate() {
            let name = check
                .tests
                .get(index)
                .map(|t| t.key.as_str())
                .unwrap_or("<unknown>");
            let hash = hashes
                .tests
                .get(index)
                .map(|h| h.short())
                .unwrap_or_else(|| "-".repeat(12));
            let verb = if reason.runs() { "run " } else { "skip" };
            lines.push(format!(
                "{verb} {hash}  {name}  ({})",
                explain_reason(*reason)
            ));
        }
        for (g, group) in self.groups.iter().enumerate() {
            let names: Vec<&str> = group
                .iter()
                .map(|&i| {
                    check
                        .tests
                        .get(i)
                        .map(|t| t.key.as_str())
                        .unwrap_or("<unknown>")
                })
                .collect();
            let footprint = group
                .iter()
                .filter_map(|&i| check.tests.get(i))
                .fold(ply_core::Footprint::empty(), |acc, t| {
                    acc.union(&t.footprint)
                });
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
                    "key": check.tests.get(index).map(|t| t.key.clone()),
                    "name": check.tests.get(index).map(|t| t.name.clone()),
                    "module": check.tests.get(index).map(|t| t.module.to_string()),
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
            "schema_version": SCHEMA_VERSION,
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
    /// each failure led by its culprit.
    ///
    /// The culprit comes before the diff because the culprit is the answer. A
    /// reader who already knows which definition broke does not need to work
    /// backwards from an expected/actual pair to find out.
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
            lines.push(failure.key.to_string());
            lines.extend(culprit_lines(&failure.attribution));
            lines.push(format!("  {}", failure.diagnostic.message));
            for note in &failure.diagnostic.notes {
                lines.push(format!("  = {note}"));
            }
            if let Some(path) = ran_path(&failure.attribution) {
                lines.push(format!("  ran: {path}"));
            }
            if !failure.suspects.is_empty() {
                let names: Vec<&str> = failure.suspects.iter().map(|s| s.as_str()).collect();
                lines.push(format!("  suspects: {}", names.join(", ")));
            }
        }
        lines
    }
}

/// Silent when there is no culprit to lead with, so that a run which could not
/// bisect reads exactly as it does today rather than gaining a line of
/// apologies.
fn culprit_lines(attribution: &Attribution) -> Vec<String> {
    let bisection = &attribution.bisection;
    if !bisection.is_conclusive() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "  culprit: {}",
        bisection
            .groups
            .iter()
            .map(|group| group
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(" + "))
            .collect::<Vec<_>>()
            .join(", ")
    )];
    lines.push(format!("    {}", bisection.reason));
    lines
}

fn ran_path(attribution: &Attribution) -> Option<String> {
    let slice = attribution.slice.as_ref()?;
    if !slice.traced || slice.stack.is_empty() {
        return None;
    }
    Some(
        slice
            .path()
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" → "),
    )
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

/// The per-failure artifact. Every field answers a question a consumer would
/// otherwise have to come back and ask.
pub fn failure_json(failure: &Failure) -> Value {
    json!({
        "key": failure.key,
        "name": failure.name,
        "diagnostic": failure.diagnostic,
        "assertion": failure.assertion.as_ref().map(assertion_json),
        "culprit": culprit_json(&failure.attribution.bisection),
        "causal_slice": failure.attribution.slice.as_ref().map(slice_json),
        "suspects": failure.attribution.suspects.iter().map(suspect_json).collect::<Vec<_>>(),
    })
}

/// Leads with the answer: what to change, how sure the system is, and how it
/// found out. `verdict` is what a consumer branches on; `reason` is what it
/// prints when it has to hand the failure back to a person.
fn culprit_json(bisection: &Bisection) -> Value {
    let search = &bisection.search;
    json!({
        "verdict": bisection.verdict.as_str(),
        "skipped": bisection.verdict.skipped().map(|s| s.as_str()),
        "confidence": bisection.confidence.as_str(),
        "definitions": bisection.culprits(),
        "groups": bisection.groups,
        "reason": bisection.reason,
        "search": {
            "candidates": search.candidates,
            "clusters": search.clusters,
            "evaluated": search.evaluated,
            "cached": search.cached,
            "memoized": search.memoized,
            "unresolved": search.unresolved,
            "exhausted": search.exhausted,
        },
    })
}

fn slice_json(slice: &CausalSlice) -> Value {
    json!({
        "traced": slice.traced,
        "reproduced": slice.reproduced,
        "truncated": slice.truncated,
        "stack": slice
            .stack
            .iter()
            .map(|f| json!({
                "name": f.name,
                "hash": f.hash.map(|h| h.to_hex()),
                "span": span_json(f.call_site),
            }))
            .collect::<Vec<_>>(),
        "entered": slice
            .entered
            .iter()
            .map(|e| json!({
                "name": e.name,
                "hash": e.hash.map(|h| h.to_hex()),
                "calls": e.calls,
            }))
            .collect::<Vec<_>>(),
        "observed_footprint": slice
            .observed
            .atoms()
            .map(|a| a.to_string())
            .collect::<Vec<_>>(),
    })
}

fn suspect_json(suspect: &Suspect) -> Value {
    json!({
        "name": suspect.name,
        "hash": suspect.hash.map(|h| h.to_hex()),
        "before": suspect.before.map(|h| h.to_hex()),
        "change": suspect.change.map(|c| c.as_str()),
        "ran": suspect.ran,
        "depth": suspect.depth,
        "culprit": suspect.culprit,
    })
}

fn assertion_json(assertion: &Assertion) -> Value {
    json!({
        "kind": assertion.kind.as_str(),
        "expected": assertion.expected,
        "actual": assertion.actual,
        "message": assertion.message,
        "first_difference": assertion.first_difference.as_ref().map(|d| json!({
            "path": d.path,
            "expected": d.expected,
            "actual": d.actual,
        })),
    })
}

fn span_json(span: ply_span::Span) -> Option<Value> {
    if span.is_dummy() {
        return None;
    }
    Some(json!({ "source": span.source.0, "start": span.start, "end": span.end }))
}

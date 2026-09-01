//! What the spec tier actually buys, measured on a project rather than argued.

use anyhow::{Result, bail};
use ply_cli::engine::{Prover, Reach};
use ply_cli::load::load;
use ply_cli::obligations;
use ply_prove::prove::{Blocker, Decision, Reason};
use ply_prove::{Discharge, Evidence, Gap, Obligation, ProvePlan, Tier};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

/// One project's obligations, discharged and dissected.
#[derive(Clone, Debug, Serialize)]
pub struct Discharged {
    pub project: String,
    pub definitions: usize,
    pub covered: usize,
    pub obligations: usize,
    pub tiers: Tiers,
    pub reach: ReachTable,
    /// Why an obligation was not attempted at all, most common first.
    pub gaps: Vec<(String, usize)>,
    pub shrinks: Vec<Shrink>,
    pub discharge_millis: f64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Tiers {
    pub proved: usize,
    pub property: usize,
    pub example: usize,
    pub refuted: usize,
    pub vacuous: usize,
    pub unattempted: usize,
    /// `proved` obligations whose certificate came from execution — ground evaluation, a covered
    /// finite domain, an emptied interleaving frontier — rather than from a static argument.
    pub proved_by_execution: usize,
}

impl Tiers {
    pub fn held(&self) -> usize {
        self.proved + self.property + self.example
    }
}

/// What the static fragment answered, over the obligations it was asked.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ReachTable {
    pub attempted: usize,
    pub decided: usize,
    pub open: usize,
    pub budget_spent: usize,
    pub guard_unsatisfiable: usize,
    /// Why an undecided obligation left the fragment, most common first.
    pub blockers: Vec<(String, usize)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Shrink {
    pub label: String,
    /// Rendered characters across every binding, before and after.
    pub original_width: usize,
    pub shrunk_width: usize,
    pub steps: u32,
    pub original: String,
    pub shrunk: String,
}

pub fn discharge(path: &Path, plan: &ProvePlan) -> Result<Discharged> {
    let loaded = match load(path) {
        Ok(loaded) => loaded,
        Err(e) => bail!(
            "`{}` did not compile: {:?}",
            path.display(),
            e.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
        ),
    };
    let collected = obligations::collect(&loaded.program, &loaded.check, &loaded.hashes);
    let prover = Prover::new(&loaded.program, &loaded.resolved, &loaded.check);

    let started = Instant::now();
    let discharges: Vec<Discharge> = collected
        .obligations
        .iter()
        .map(|o| prover.discharge_with(o, plan))
        .collect();
    let discharge_millis = started.elapsed().as_secs_f64() * 1000.0;

    let mut tiers = Tiers::default();
    let mut gaps: BTreeMap<String, usize> = BTreeMap::new();
    let mut shrinks = Vec::new();
    let mut covered: Vec<&ply_span::Symbol> = Vec::new();
    for (obligation, discharge) in collected.obligations.iter().zip(&discharges) {
        tally(&mut tiers, discharge);
        if discharge.holds() {
            covered.push(&obligation.owner);
        }
        if let Discharge::Unattempted(gap) = discharge {
            *gaps.entry(gap_label(gap).to_string()).or_default() += 1;
        }
        if let Discharge::Refuted(counterexample) = discharge {
            shrinks.push(Shrink {
                label: obligation.owner.to_string(),
                original_width: width(&counterexample.original),
                shrunk_width: width(&counterexample.bindings),
                steps: counterexample.shrinks,
                original: bindings(&counterexample.original),
                shrunk: bindings(&counterexample.bindings),
            });
        }
    }
    covered.sort();
    covered.dedup();

    let reach = reach(&prover, &collected.obligations, plan);
    let mut gaps: Vec<(String, usize)> = gaps.into_iter().collect();
    gaps.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    Ok(Discharged {
        project: path.display().to_string(),
        definitions: loaded.check.defs.len(),
        covered: covered.len(),
        obligations: collected.obligations.len(),
        tiers,
        reach,
        gaps,
        shrinks,
        discharge_millis,
    })
}

fn tally(tiers: &mut Tiers, discharge: &Discharge) {
    match discharge {
        Discharge::Held(evidence) => {
            match evidence.tier() {
                Tier::Proved => tiers.proved += 1,
                Tier::Property => tiers.property += 1,
                Tier::Example => tiers.example += 1,
            }
            if let Evidence::Proof(certificate) = evidence
                && certificate.rules.iter().any(|r| r.is_execution())
            {
                tiers.proved_by_execution += 1;
            }
        }
        Discharge::Refuted(_) => tiers.refuted += 1,
        Discharge::Vacuous(_) => tiers.vacuous += 1,
        Discharge::Unattempted(_) => tiers.unattempted += 1,
    }
}

fn reach(prover: &Prover<'_>, obligations: &[Obligation], plan: &ProvePlan) -> ReachTable {
    let mut table = ReachTable::default();
    let mut histogram: BTreeMap<String, usize> = BTreeMap::new();
    for obligation in obligations {
        let Some(Reach { decision, blockers }) = prover.reach(obligation, plan) else {
            continue;
        };
        table.attempted += 1;
        match decision {
            Decision::Proved(_) => {
                table.decided += 1;
                continue;
            }
            Decision::GuardUnsatisfiable { .. } => {
                table.guard_unsatisfiable += 1;
                continue;
            }
            Decision::Unknown {
                reason: Reason::Open,
                ..
            } => table.open += 1,
            Decision::Unknown {
                reason: Reason::BudgetSpent,
                ..
            } => table.budget_spent += 1,
        }
        let mut seen: Vec<String> = blockers.iter().map(label).collect();
        seen.sort();
        seen.dedup();
        for blocker in seen {
            *histogram.entry(blocker).or_default() += 1;
        }
    }
    table.blockers = histogram.into_iter().collect();
    table
        .blockers
        .sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    table
}

/// A blocker's kind, without the definition it names.
fn label(blocker: &Blocker) -> String {
    match blocker {
        Blocker::RecursiveCall(_) => "recursive call (needs induction)",
        Blocker::EffectfulCall(_) => "call whose row is not known empty",
        Blocker::UnfoldLimit(_) => "unfold depth or term limit",
        Blocker::OpaqueCall(_) => "call with no body in view",
        Blocker::Division => "division or remainder",
        Blocker::NonlinearMultiplication => "multiplication of two symbolics",
        Blocker::CoefficientRange => "coefficient left range",
        Blocker::Lambda => "lambda",
        Blocker::StringConcat => "string concatenation",
        Blocker::BitOperator => "bit operator or shift",
        Blocker::Region => "perform, handle or simulate",
        Blocker::UnexpandedSugar => "a parse-time node expansion should have removed",
        Blocker::UndecidableMatchArm => "pattern outside the fragment",
        Blocker::DestructuringLet => "destructuring let",
        Blocker::FloatTerm => "a Float term (never proved)",
        Blocker::DecimalArithmetic => "Decimal arithmetic or ordering",
    }
    .to_string()
}

fn width(bindings: &[ply_prove::Binding]) -> usize {
    bindings.iter().map(|b| b.rendered.chars().count()).sum()
}

fn bindings(bindings: &[ply_prove::Binding]) -> String {
    bindings
        .iter()
        .map(|b| format!("{} = {}", b.name, b.rendered))
        .collect::<Vec<_>>()
        .join(", ")
}

fn gap_label(gap: &Gap) -> &'static str {
    match gap {
        Gap::UnhandledEffect(_) => "unhandled effect",
        Gap::Ungeneratable { .. } => "ungeneratable parameter",
        Gap::Raised { .. } => "evaluation raised",
        Gap::GuardNotSampled { .. } => "guard not sampled",
        Gap::ReachesHost(_) => "reaches the host",
    }
}

pub fn render(runs: &[Discharged]) -> String {
    use std::fmt::Write;
    let mut s = String::new();

    let _ = writeln!(s, "\ntiers");
    let _ = writeln!(
        s,
        "  {:<28} {:>6} {:>6} {:>8} {:>7} {:>7} {:>7} {:>11} {:>8}",
        "project",
        "defs",
        "oblig",
        "proved",
        "prop",
        "example",
        "refuted",
        "unattempted",
        "covered"
    );
    for run in runs {
        let _ = writeln!(
            s,
            "  {:<28} {:>6} {:>6} {:>8} {:>7} {:>7} {:>7} {:>11} {:>8}",
            short(&run.project),
            run.definitions,
            run.obligations,
            run.tiers.proved,
            run.tiers.property,
            run.tiers.example,
            run.tiers.refuted + run.tiers.vacuous,
            run.tiers.unattempted,
            format!("{}/{}", run.covered, run.definitions),
        );
    }

    let _ = writeln!(s, "\nreach — the static fragment, asked");
    let _ = writeln!(
        s,
        "  {:<28} {:>9} {:>8} {:>6} {:>13} {:>8}",
        "project", "attempted", "decided", "open", "budget spent", "vacuous"
    );
    for run in runs {
        let _ = writeln!(
            s,
            "  {:<28} {:>9} {:>8} {:>6} {:>13} {:>8}",
            short(&run.project),
            run.reach.attempted,
            run.reach.decided,
            run.reach.open,
            run.reach.budget_spent,
            run.reach.guard_unsatisfiable,
        );
    }

    for run in runs {
        if run.reach.blockers.is_empty() && run.gaps.is_empty() {
            continue;
        }
        let _ = writeln!(
            s,
            "\n  {} — why an obligation did not reach a proof",
            short(&run.project)
        );
        for (blocker, count) in &run.reach.blockers {
            let _ = writeln!(s, "    {count:>4}  {blocker}");
        }
        for (gap, count) in &run.gaps {
            let _ = writeln!(s, "    {count:>4}  (not attempted at all) {gap}");
        }
    }

    let shrinks: Vec<&Shrink> = runs.iter().flat_map(|r| r.shrinks.iter()).collect();
    if !shrinks.is_empty() {
        let _ = writeln!(s, "\nshrink");
        let _ = writeln!(
            s,
            "  {:<40} {:>9} {:>8} {:>7} {:>7}",
            "obligation", "before", "after", "steps", "ratio"
        );
        for shrink in &shrinks {
            let ratio = if shrink.original_width == 0 {
                1.0
            } else {
                shrink.shrunk_width as f64 / shrink.original_width as f64
            };
            let _ = writeln!(
                s,
                "  {:<40} {:>9} {:>8} {:>7} {:>6.2}x",
                truncate(&shrink.label, 40),
                shrink.original_width,
                shrink.shrunk_width,
                shrink.steps,
                ratio
            );
            let _ = writeln!(s, "      from {}", shrink.original);
            let _ = writeln!(s, "        to {}", shrink.shrunk);
        }
    }

    let _ = writeln!(s, "\ndischarge cost, in process, cache bypassed");
    let _ = writeln!(
        s,
        "  {:<28} {:>6} {:>12} {:>14}",
        "project", "oblig", "total (ms)", "per oblig (ms)"
    );
    for run in runs {
        let per = if run.obligations == 0 {
            0.0
        } else {
            run.discharge_millis / run.obligations as f64
        };
        let _ = writeln!(
            s,
            "  {:<28} {:>6} {:>12.2} {:>14.3}",
            short(&run.project),
            run.obligations,
            run.discharge_millis,
            per
        );
    }
    s
}

fn short(path: &str) -> String {
    truncate(
        Path::new(path)
            .file_name()
            .map_or(path, |n| n.to_str().unwrap_or(path)),
        28,
    )
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
}

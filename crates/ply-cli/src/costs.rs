//! `ply check --costs`: where an append copies, before the program runs.
//!
//! The checker is [`ply_eval::costs`] and this module only renders it. ADR 0025
//! §Decision 2a asked for the flag and ADR 0033 §11 S2 is where it was built —
//! between those two the checker shipped with **no caller outside its own
//! tests**, which is why the output shape below is the ADR's rather than one
//! grown from use.
//!
//! # Why this prints a cost and not a guarantee
//!
//! [`ply_eval::costs`] reads the lowered `Code` and changes no evaluation:
//! `push` keeps its dynamic `Arc::get_mut` guard, so a verdict here is a claim
//! *about* a run and never a permission granted to one. A wrong verdict costs a
//! reader a wrong expectation; it cannot cost a program its meaning. That is
//! also why [`Verdict::Unknown`] is printed rather than rounded to one of the
//! other two — `costs`'s module header lists the four shapes no analysis of one
//! body can decide, and rounding them would be the one thing that makes a
//! checker worse than none.
//!
//! # The width is pinned, for the reason `--types`' is
//! [`crate::signature`] renders at a fixed width because its output is diffed in
//! review. This block is diffed for the same reason — the interesting question
//! about it is *what changed since the last run* — so the columns are laid out
//! once, here, and `costs_reports_the_shipped_quadratic` in
//! `crates/ply-cli/tests/check.rs` pins them.

use ply_eval::costs::{Costs, DefKind, Definition, Report, Verdict};
use ply_span::{SourceMap, Span};
use ply_syntax::ast::Program;
use ply_syntax::resolve::Resolved;

use crate::style::Style;

/// Where the verdict column starts. Wide enough for `stdlib.ply:1234:56`, which
/// is the longest location the shipped modules produce.
const LOCATION: usize = 22;

/// Renders the whole report. `None` when the program declares no `push` at all,
/// so a caller can say that rather than printing an empty block.
pub fn lines(
    program: &Program,
    resolved: &Resolved,
    sources: &SourceMap,
    style: Style,
) -> Option<Vec<String>> {
    let report = Costs::new(program, resolved).check();
    let defs: Vec<&Definition> = report
        .all()
        .iter()
        .filter(|d| !d.sites.is_empty())
        .collect();
    if defs.is_empty() {
        return None;
    }

    let mut out = Vec::new();
    for def in &defs {
        out.push(String::new());
        let kind = match def.kind {
            DefKind::Fn => "",
            DefKind::Test => " (test)",
            DefKind::Law => " (law)",
        };
        out.push(format!(
            "  {}{}  {}",
            style.bold(&def.name),
            style.dim(kind),
            style.dim(&tally(def)),
        ));
        for site in &def.sites {
            let at = location(site.span, sources);
            let pad = LOCATION.saturating_sub(at.chars().count());
            let verdict = match site.verdict {
                Verdict::Reuses => style.green("reuses "),
                Verdict::Copies => style.red("COPIES "),
                Verdict::Unknown => style.yellow("unknown"),
            };
            out.push(format!(
                "    {at}{:pad$}  {verdict}  {}",
                "",
                site.reason,
                pad = pad
            ));
            if let Some(fix) = site.fix() {
                out.push(format!(
                    "    {:LOCATION$}  {}  {}",
                    "",
                    " ".repeat(7),
                    style.dim(&format!("fix: {fix}")),
                ));
            }
        }
    }
    out.push(String::new());
    out.push(format!("  {}", style.dim(&summary(&report))));
    Some(out)
}

fn tally(def: &Definition) -> String {
    let mut parts = Vec::new();
    if def.reuses() > 0 {
        parts.push(format!("{} reuses", def.reuses()));
    }
    if def.copies() > 0 {
        parts.push(format!("{} COPIES", def.copies()));
    }
    if def.unknown() > 0 {
        parts.push(format!("{} unknown", def.unknown()));
    }
    parts.join(", ")
}

/// The whole-program line.
///
/// `rounds` is printed rather than hidden because [`Report::rounds`] at its
/// ceiling means the fixpoint did not converge and the verdicts above are not
/// an answer — `costs`'s own harness prints it for the same reason.
fn summary(report: &Report) -> String {
    let (mut reuses, mut copies, mut unknown) = (0, 0, 0);
    for def in report.all() {
        reuses += def.reuses();
        copies += def.copies();
        unknown += def.unknown();
    }
    format!(
        "{} appends: {reuses} reuse, {copies} copy, {unknown} undecided — {} {}",
        reuses + copies + unknown,
        report.rounds,
        if report.rounds == 1 {
            "round"
        } else {
            "rounds"
        },
    )
}

fn location(span: Span, sources: &SourceMap) -> String {
    let Some(file) = sources.get(span.source) else {
        return "<unknown>".to_string();
    };
    let (line, col) = file.line_col(span.start);
    let name = file
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.path.display().to_string());
    format!("{name}:{line}:{col}")
}

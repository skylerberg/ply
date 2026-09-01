//! `ply check --costs`: where an append copies, before the program runs.

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

//! `ply check --costs`: where an append copies, before the program runs.

use ply_eval::costs::{Costs, DefKind, Definition, Report, Verdict};
use ply_span::{Diagnostic, SourceMap, Span, codes};
use ply_syntax::ast::{FnDef, Item, Program};
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

/// The promise a `reuse fn` makes, checked: every append in its body reuses its list for every
/// reason the body controls. An append onto the definition's own parameter keeps the promise
/// whatever a caller does with what it passed — the promise says nothing about callers, which is
/// what keeps the multi-shot counterexample from reaching it — and every other copy or undecided
/// site is E0127, with the edit that would keep the promise where one exists. Only the modules in
/// `program` are checked: a command that parsed part of a project checks the promises it parsed.
pub fn promises(program: &Program, resolved: &Resolved) -> Vec<Diagnostic> {
    let promised: Vec<(usize, &FnDef)> = program
        .modules
        .iter()
        .enumerate()
        .flat_map(|(module, m)| {
            m.items.iter().filter_map(move |item| match item {
                Item::Fn(def) if def.reuse.is_some() => Some((module, &**def)),
                _ => None,
            })
        })
        .collect();
    if promised.is_empty() {
        return Vec::new();
    }
    let report = Costs::new(program, resolved).check();
    let mut out = Vec::new();
    for def in report.all() {
        if def.kind != DefKind::Fn {
            continue;
        }
        let Some((_, decl)) = promised.iter().find(|(module, decl)| {
            *module == def.module && def.name.ends_with(&format!(".{}", decl.name.name))
        }) else {
            continue;
        };
        let promise = decl
            .reuse
            .expect("a promised definition carries its marker");
        for site in &def.sites {
            if site.verdict == Verdict::Reuses || site.param.is_some() {
                continue;
            }
            let what = match site.verdict {
                Verdict::Copies => "copies its list",
                _ => "cannot be shown to reuse its list",
            };
            let mut d = Diagnostic::error(
                codes::REUSE_BROKEN,
                format!(
                    "`{}` is a `reuse fn`, and this append {what}: {}",
                    decl.name.name, site.reason
                ),
            )
            .primary(site.span, "this append")
            .secondary(promise, "the promise");
            d = match site.fix() {
                Some(fix) => d.note(format!("fix: {fix}")),
                None => d.note(
                    "no edit inside this body removes the copy; the promise cannot be kept as \
                     written, so either restructure the append or drop `reuse`",
                ),
            };
            out.push(d);
        }
    }
    out
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

//! Running one program with a compiled backend attached and without, and comparing what they did.

use crate::arena::Arena;
use crate::evaluator::Machine;
use crate::task_regions::Fixture;
use crate::value::Value;
use ply_span::{Diagnostic, Label, Severity, Span, Symbol, codes};
use ply_syntax::ast::Expr;
use ply_ty::Footprint;
use std::fmt;

pub trait Evaluator {
    fn test_count(&self) -> usize;
    fn test_name(&self, index: usize) -> Option<&str>;
    fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic>;
    /// By module: the incremental front end reports tests from modules it never parsed.
    fn eval_test_in(&mut self, module: &Symbol, ordinal: usize) -> Result<(), Diagnostic>;
    fn eval_expr(&mut self, e: &Expr) -> Result<Value, Diagnostic>;
    /// The run's cells, ascending by slot.
    fn cells(&self) -> &Arena;
    fn cells_mut(&mut self) -> &mut Arena;
    fn set_fixture(&mut self, fixture: &Fixture);

    /// `None` for an engine that does not trace.
    fn observed_footprint(&self) -> Option<Footprint> {
        None
    }

    fn observed_performs(&self) -> Option<u64> {
        None
    }
}

impl Evaluator for Machine<'_> {
    fn test_count(&self) -> usize {
        Machine::test_count(self)
    }

    fn test_name(&self, index: usize) -> Option<&str> {
        Machine::test_name(self, index)
    }

    fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic> {
        Machine::eval_test(self, index)
    }

    fn eval_test_in(&mut self, module: &Symbol, ordinal: usize) -> Result<(), Diagnostic> {
        Machine::eval_test_in(self, module, ordinal)
    }

    fn eval_expr(&mut self, e: &Expr) -> Result<Value, Diagnostic> {
        self.eval_expr_for_test(e)
    }

    fn cells(&self) -> &Arena {
        Machine::cells(self)
    }

    fn cells_mut(&mut self) -> &mut Arena {
        Machine::cells_mut(self)
    }

    fn set_fixture(&mut self, fixture: &Fixture) {
        let (regions, _) = fixture.open();
        Machine::set_regions(self, regions);
    }

    fn observed_footprint(&self) -> Option<Footprint> {
        Some(self.trace().footprint().clone())
    }

    fn observed_performs(&self) -> Option<u64> {
        Some(self.trace().performs())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detail {
    Verdict,
    Diagnostic { field: String },
    Value,
    Footprint,
    Cells { at: String },
    Reclaimed { at: String },
}

impl Detail {
    fn what(&self) -> String {
        match self {
            Detail::Verdict => "verdict".to_string(),
            Detail::Diagnostic { field } => format!("diagnostic {field}"),
            Detail::Value => "result value".to_string(),
            Detail::Footprint => "observed footprint".to_string(),
            Detail::Cells { at } => format!("final cell at {at}"),
            Detail::Reclaimed { at } => format!("reclaimed cell #{at}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Divergence {
    /// The test's label, or the caller's name for an ad-hoc expression.
    pub subject: String,
    pub index: Option<usize>,
    pub detail: Detail,
    pub left: String,
    pub right: String,
}

impl Divergence {
    /// The blame is always the backend's: the machine gives it no route back in.
    pub fn to_backend_diagnostic(&self, span: Span) -> Diagnostic {
        Diagnostic::error(
            codes::ENGINE_DIVERGENCE,
            format!(
                "the compiled backend and `machine` disagree on `{}`",
                self.subject
            ),
        )
        .primary(span, format!("the backend's {} differs", self.detail.what()))
        .note(format!("machine, no backend: {}", self.left))
        .note(format!("machine with the backend: {}", self.right))
        .note("the boundary checks a backend's answer for kind and nothing else, so a wrong value crosses it")
        .note("re-run without `--backend` to confirm the program passes without one")
    }
}

impl fmt::Display for Divergence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} — left {}, right {}",
            self.subject,
            self.detail.what(),
            self.left,
            self.right
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub compared: usize,
    /// Of those, how many carried a footprint from both sides.
    pub footprints_compared: usize,
    pub divergences: Vec<Divergence>,
}

impl Report {
    pub fn new() -> Report {
        Report::default()
    }

    pub fn is_clean(&self) -> bool {
        self.divergences.is_empty()
    }

    pub fn into_result(self) -> Result<Report, Diagnostic> {
        match self.divergences.first() {
            Some(d) => Err(d.to_backend_diagnostic(Span::DUMMY)),
            None => Ok(self),
        }
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} compared, {} footprints, {} divergences",
            self.compared,
            self.footprints_compared,
            self.divergences.len()
        )?;
        for d in &self.divergences {
            writeln!(f, "  {d}")?;
        }
        Ok(())
    }
}

/// Both engines always step, so a divergence never leaves them at different points in the corpus.
pub fn compare_test(left: &mut dyn Evaluator, right: &mut dyn Evaluator, index: usize) -> Compared {
    let subject = left
        .test_name(index)
        .or_else(|| right.test_name(index))
        .unwrap_or("<unnamed>")
        .to_string();

    audit_state(left, right);
    let l = left.eval_test(index);
    let r = right.eval_test(index);
    match compare_outcomes(left, right, &subject, Some(index), &l, &r) {
        Some(d) => Compared::Diverged(d),
        None => Compared::Agreed,
    }
}

pub fn audit_state(left: &mut dyn Evaluator, right: &mut dyn Evaluator) {
    left.cells_mut().journal();
    right.cells_mut().journal();
}

fn compare_outcomes(
    left: &dyn Evaluator,
    right: &dyn Evaluator,
    subject: &str,
    index: Option<usize>,
    l: &Result<(), Diagnostic>,
    r: &Result<(), Diagnostic>,
) -> Option<Divergence> {
    outcome_divergence(l, r)
        .or_else(|| footprint_divergence(left, right))
        .or_else(|| cells_divergence(left.cells(), right.cells()))
        .or_else(|| reclaimed_divergence(left.cells(), right.cells()))
        .map(|(detail, a, b)| Divergence {
            subject: subject.to_string(),
            index,
            detail,
            left: a,
            right: b,
        })
}

pub enum Compared {
    Agreed,
    Diverged(Divergence),
}

pub fn compare_answers(
    left: &dyn Evaluator,
    right: &dyn Evaluator,
    subject: &str,
    l: &Result<Value, Diagnostic>,
    r: &Result<Value, Diagnostic>,
) -> Option<Divergence> {
    let discard = |v: &Result<Value, Diagnostic>| v.as_ref().map(|_| ()).map_err(Diagnostic::clone);
    let first = match (l, r) {
        (Ok(a), Ok(b)) => {
            let (x, y) = (a.render(), b.render());
            (x != y).then_some((Detail::Value, x, y))
        }
        _ => outcome_divergence(&discard(l), &discard(r)),
    };

    first
        .or_else(|| footprint_divergence(left, right))
        .or_else(|| cells_divergence(left.cells(), right.cells()))
        .or_else(|| reclaimed_divergence(left.cells(), right.cells()))
        .map(|(detail, a, b)| Divergence {
            subject: subject.to_string(),
            index: None,
            detail,
            left: a,
            right: b,
        })
}

pub fn compare_expr(
    left: &mut dyn Evaluator,
    right: &mut dyn Evaluator,
    subject: &str,
    e: &Expr,
) -> Option<Divergence> {
    let l = left.eval_expr(e);
    let r = right.eval_expr(e);
    compare_answers(left, right, subject, &l, &r)
}

/// Both evaluators must be built over the same program, so an index names the same test.
pub fn compare_tests(
    left: &mut dyn Evaluator,
    right: &mut dyn Evaluator,
    base: &Fixture,
) -> Report {
    let mut report = Report::new();

    left.set_fixture(base);
    right.set_fixture(base);

    let count = left.test_count();
    if count != right.test_count() {
        report.divergences.push(Divergence {
            subject: "<corpus>".to_string(),
            index: None,
            detail: Detail::Verdict,
            left: format!("{count} tests"),
            right: format!("{} tests", right.test_count()),
        });
        return report;
    }

    for index in 0..count {
        match compare_test(left, right, index) {
            Compared::Agreed => report.compared += 1,
            Compared::Diverged(d) => {
                report.compared += 1;
                report.divergences.push(d);
            }
        }
        // After the run: before it, the footprint is the previous test's.
        if left.observed_footprint().is_some() && right.observed_footprint().is_some() {
            report.footprints_compared += 1;
        }
    }
    report
}

/// The first differing field, in the order a reader scans a diagnostic.
fn outcome_divergence(
    left: &Result<(), Diagnostic>,
    right: &Result<(), Diagnostic>,
) -> Option<(Detail, String, String)> {
    match (left, right) {
        (Ok(()), Ok(())) => None,
        (Ok(()), Err(d)) => Some((Detail::Verdict, "passed".to_string(), describe(d))),
        (Err(d), Ok(())) => Some((Detail::Verdict, describe(d), "passed".to_string())),
        (Err(a), Err(b)) => diagnostic_divergence(a, b),
    }
}

fn diagnostic_divergence(a: &Diagnostic, b: &Diagnostic) -> Option<(Detail, String, String)> {
    let field = |name: &str| Detail::Diagnostic {
        field: name.to_string(),
    };

    if a.code != b.code {
        return Some((field("code"), a.code.to_string(), b.code.to_string()));
    }
    if a.severity != b.severity {
        return Some((
            field("severity"),
            severity(a.severity).to_string(),
            severity(b.severity).to_string(),
        ));
    }
    if a.message != b.message {
        return Some((field("message"), a.message.clone(), b.message.clone()));
    }
    if a.labels.len() != b.labels.len() {
        return Some((
            field("labels"),
            format!("{} labels", a.labels.len()),
            format!("{} labels", b.labels.len()),
        ));
    }
    for (i, (x, y)) in a.labels.iter().zip(b.labels.iter()).enumerate() {
        if !label_eq(x, y) {
            return Some((
                field(&format!("labels[{i}]")),
                render_label(x),
                render_label(y),
            ));
        }
    }
    if a.notes.len() != b.notes.len() {
        return Some((
            field("notes"),
            format!("{} notes", a.notes.len()),
            format!("{} notes", b.notes.len()),
        ));
    }
    for (i, (x, y)) in a.notes.iter().zip(b.notes.iter()).enumerate() {
        if x != y {
            return Some((field(&format!("notes[{i}]")), x.clone(), y.clone()));
        }
    }
    None
}

fn footprint_divergence(
    left: &dyn Evaluator,
    right: &dyn Evaluator,
) -> Option<(Detail, String, String)> {
    let (a, b) = (left.observed_footprint()?, right.observed_footprint()?);
    if a != b {
        return Some((Detail::Footprint, a.to_string(), b.to_string()));
    }
    let (n, m) = (left.observed_performs()?, right.observed_performs()?);
    (n != m).then(|| {
        (
            Detail::Footprint,
            format!("{a} performed {n} time{}", plural(n)),
            format!("{b} performed {m} time{}", plural(m)),
        )
    })
}

fn plural(n: u64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn cells_divergence(left: &Arena, right: &Arena) -> Option<(Detail, String, String)> {
    let mut a = left.slots();
    let mut b = right.slots();
    loop {
        match (a.next(), b.next()) {
            (None, None) => return None,
            (Some((slot, v)), None) => {
                return Some((
                    Detail::Cells {
                        at: slot.index().to_string(),
                    },
                    v.render(),
                    "no such cell".to_string(),
                ));
            }
            (None, Some((slot, v))) => {
                return Some((
                    Detail::Cells {
                        at: slot.index().to_string(),
                    },
                    "no such cell".to_string(),
                    v.render(),
                ));
            }
            (Some((x, p)), Some((y, q))) => {
                if x.index() != y.index() {
                    return Some((
                        Detail::Cells {
                            at: format!("{} vs {}", x.index(), y.index()),
                        },
                        x.index().to_string(),
                        y.index().to_string(),
                    ));
                }
                let (p, q) = (p.render(), q.render());
                if p != q {
                    return Some((
                        Detail::Cells {
                            at: x.index().to_string(),
                        },
                        p,
                        q,
                    ));
                }
            }
        }
    }
}

fn reclaimed_divergence(left: &Arena, right: &Arena) -> Option<(Detail, String, String)> {
    let (a, b) = (left.journalled(), right.journalled());
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        // Never by generation: that is a slot's reuse history, which agreeing runs can differ on.
        if x.0.index() != y.0.index() || x.1.render() != y.1.render() {
            return Some((
                Detail::Reclaimed { at: i.to_string() },
                format!("cell {} = {}", x.0.index(), x.1.render()),
                format!("cell {} = {}", y.0.index(), y.1.render()),
            ));
        }
    }
    if a.len() != b.len() {
        let at = a.len().min(b.len());
        let show = |side: &[(crate::arena::Slot, Value)]| match side.get(at) {
            Some((slot, v)) => format!("cell {} = {}", slot.index(), v.render()),
            None => "nothing more".to_string(),
        };
        return Some((Detail::Reclaimed { at: at.to_string() }, show(a), show(b)));
    }
    None
}

fn label_eq(a: &Label, b: &Label) -> bool {
    a.span == b.span && a.message == b.message && a.primary == b.primary
}

fn render_label(l: &Label) -> String {
    let kind = if l.primary { "primary" } else { "secondary" };
    format!(
        "{kind} {}..{} of source {}: {}",
        l.span.start, l.span.end, l.span.source.0, l.message
    )
}

fn severity(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    }
}

fn describe(d: &Diagnostic) -> String {
    format!("[{}] {}", d.code, d.message)
}

//! Running one program with a compiled backend attached and without, and comparing what they did.

use crate::arena::Arena;
use crate::machine::Machine;
use crate::task_regions::Fixture;
use crate::value::Value;
use ply_core::ty::Footprint;
use ply_span::{Diagnostic, Label, Severity, Span, Symbol, codes};
use ply_syntax::ast::Expr;
use std::fmt;

/// What the harness needs of an engine.
pub trait Evaluator {
    fn test_count(&self) -> usize;
    fn test_name(&self, index: usize) -> Option<&str>;
    fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic>;
    /// A position in a `CheckOutput` is not a position in the AST an engine holds, because the
    /// incremental front end reports tests from modules it never parsed.
    fn eval_test_in(&mut self, module: &Symbol, ordinal: usize) -> Result<(), Diagnostic>;
    fn eval_expr(&mut self, e: &Expr) -> Result<Value, Diagnostic>;
    /// The run's cells, ascending by slot: the state the two sides must agree on once a test has
    /// run.
    fn cells(&self) -> &Arena;
    /// The same arena, so the harness can ask it to journal what it reclaims.
    fn cells_mut(&mut self) -> &mut Arena;
    fn set_fixture(&mut self, fixture: &Fixture);

    /// The atoms actually performed, for an engine that traces.
    fn observed_footprint(&self) -> Option<Footprint> {
        None
    }

    /// How many atoms were performed in total, for an engine that traces.
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
    /// One engine accepted the program and the other refused it.
    Verdict,
    /// Both refused it, but not identically.
    Diagnostic {
        field: String,
    },
    /// Both accepted it and produced different values.
    Value,
    Footprint,
    /// The arenas differ.
    Cells {
        at: String,
    },
    /// The two runs reclaimed different cells, or reclaimed them in a different order.
    Reclaimed {
        at: String,
    },
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

/// A single disagreement, carrying both sides so the reader never has to re-run anything to see
/// what happened.
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
    /// Fails the run.
    /// Fails the run. The blame is the backend's, always: the machine hands it no
    /// route back in, so a wrong answer is the only thing it can contribute.
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
    /// Subjects both sides ran and the harness compared.
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

    /// The first divergence as a diagnostic, so a caller can fail a run without deciding how to
    /// summarize the rest.
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

/// Both engines are stepped even when the first has already diverged: a run that stops at the first
/// disagreement leaves the two evaluators at different points in the corpus and every later
/// comparison becomes meaningless.
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

/// Asks both arenas to record what their closes reclaim.
pub fn audit_state(left: &mut dyn Evaluator, right: &mut dyn Evaluator) {
    left.cells_mut().journal();
    right.cells_mut().journal();
}

/// What one subject's audit produced.
pub enum Compared {
    Agreed,
    Diverged(Divergence),
}

/// Compares two evaluators that have each already answered the same question.
pub fn compare_outcomes(
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

/// Compares one expression, for a snippet that is not a `test` in a program.
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

/// The two evaluators must have been built over the same program, so that an index means the same
/// test on both sides.
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
        // After the run, not before: before it, a footprint is the *previous* test's and counting
        // it would claim a comparison that never happened.
        if left.observed_footprint().is_some() && right.observed_footprint().is_some() {
            report.footprints_compared += 1;
        }
    }
    report
}

/// The first field of two outcomes that differs, in the order a reader scans a diagnostic: whether
/// it failed at all, then code, severity, message, labels, notes.
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

/// The atoms performed, and how many were performed in total.
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

/// Two arenas agree when they hold the same cells in the same order with the same rendered
/// contents.
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

/// Two arenas agree about what they reclaimed when their journals are equal.
fn reclaimed_divergence(left: &Arena, right: &Arena) -> Option<(Detail, String, String)> {
    let (a, b) = (left.journalled(), right.journalled());
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        // By index and value, never by generation: a generation counts how many entry points a
        // *position* has been through, which is a run's history and not the program's: two runs
        // can reach the same state having reclaimed a different number of
        // times.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::*;
    use ply_syntax::ast::{BinOp, Item, Mode, Program};
    use ply_syntax::resolve::Resolved;

    /// A machine except where a test asks it not to be.
    struct Perturbed<'a> {
        inner: Machine<'a>,
        /// Answers `eval_test` with this instead of running it.
        outcome: Option<Result<(), Diagnostic>>,
        /// Allocated into the arena after every test.
        extra_cell: Option<Value>,
        footprint: Option<Footprint>,
    }

    impl<'a> Perturbed<'a> {
        fn new(program: &'a Program, resolved: &'a Resolved) -> Perturbed<'a> {
            Perturbed {
                inner: Machine::for_program(program, resolved),
                outcome: None,
                extra_cell: None,
                footprint: None,
            }
        }
    }

    impl Evaluator for Perturbed<'_> {
        fn test_count(&self) -> usize {
            self.inner.test_count()
        }

        fn test_name(&self, index: usize) -> Option<&str> {
            self.inner.test_name(index)
        }

        fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic> {
            let real = self.inner.eval_test(index);
            if let Some(extra) = self.extra_cell.clone() {
                self.inner.cells_mut().alloc(extra);
            }
            match &self.outcome {
                Some(forced) => forced.clone(),
                None => real,
            }
        }

        fn eval_test_in(&mut self, module: &Symbol, ordinal: usize) -> Result<(), Diagnostic> {
            self.inner.eval_test_in(module, ordinal)
        }

        fn eval_expr(&mut self, e: &Expr) -> Result<Value, Diagnostic> {
            self.inner.eval_expr_for_test(e)
        }

        fn cells(&self) -> &Arena {
            self.inner.cells()
        }

        fn cells_mut(&mut self) -> &mut Arena {
            self.inner.cells_mut()
        }

        fn set_fixture(&mut self, fixture: &Fixture) {
            let (regions, _) = fixture.open();
            self.inner.set_regions(regions);
        }

        fn observed_footprint(&self) -> Option<Footprint> {
            self.footprint.clone()
        }
    }

    fn corpus() -> Vec<Item> {
        vec![
            fn_def("two", &[], int(2)),
            test_def(
                "arithmetic agrees",
                callv("assert_eq", vec![callv("two", vec![]), int(2)]),
            ),
            test_def(
                "a cell survives the test",
                with_cell(
                    "s",
                    int(1),
                    "c",
                    block(
                        vec![discard(callv("cell_set", vec![var("c"), int(41)]))],
                        Some(callv(
                            "assert_eq",
                            vec![callv("cell_get", vec![var("c")]), int(41)],
                        )),
                    ),
                ),
            ),
        ]
    }

    #[test]
    fn two_honest_evaluators_over_one_corpus_report_nothing() {
        let (program, resolved) = standalone(corpus());
        let mut left = Machine::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);
        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        assert_eq!(report.compared, 2);
        assert!(report.is_clean(), "{report}");
    }

    #[test]
    fn a_side_that_passes_what_the_other_failed_is_caught() {
        let (program, resolved) = standalone(vec![test_def(
            "fails on both, honestly",
            callv("assert_eq", vec![int(1), int(2)]),
        )]);
        let mut left = Machine::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);
        right.outcome = Some(Ok(()));

        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        assert_eq!(report.divergences.len(), 1);
        let d = &report.divergences[0];
        assert_eq!(d.detail, Detail::Verdict);
        assert_eq!(d.subject, "fails on both, honestly");
        assert_eq!(d.right, "passed");
        assert!(d.left.contains("E0501"), "{}", d.left);
    }

    #[test]
    fn a_divergence_only_in_a_note_is_caught_and_the_note_is_named() {
        let (program, resolved) = standalone(vec![test_def(
            "assertion",
            callv("assert_eq", vec![int(1), int(2)]),
        )]);
        let mut left = Machine::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);

        let mut drifted = Machine::for_program(&program, &resolved)
            .eval_test(0)
            .unwrap_err();
        drifted.notes[1] = "actual: 1".to_string();
        right.outcome = Some(Err(drifted));

        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        let d = &report.divergences[0];
        assert_eq!(
            d.detail,
            Detail::Diagnostic {
                field: "notes[1]".to_string()
            }
        );
        assert_eq!(d.left, "actual:   1");
        assert_eq!(d.right, "actual: 1");
    }

    #[test]
    fn a_divergence_only_in_a_label_span_is_caught() {
        let (program, resolved) = standalone(vec![test_def(
            "assertion",
            spanned(callv("assert_eq", vec![int(1), int(2)]), at(88, 100)),
        )]);
        let mut left = Machine::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);

        let mut drifted = Machine::for_program(&program, &resolved)
            .eval_test(0)
            .unwrap_err();
        drifted.labels[0].span = at(1, 2);
        right.outcome = Some(Err(drifted));

        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        assert_eq!(
            report.divergences[0].detail,
            Detail::Diagnostic {
                field: "labels[0]".to_string()
            }
        );
        assert!(report.divergences[0].left.contains("88..100"));
    }

    /// The case a verdict comparison alone would miss entirely: both sides pass, and one of them
    /// left its cells somewhere else.
    #[test]
    fn an_arena_that_differs_after_a_passing_test_is_caught_at_the_cell() {
        let (program, resolved) = standalone(corpus());
        let mut left = Machine::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);
        right.extra_cell = Some(Value::Int(99));

        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        let d = report
            .divergences
            .first()
            .expect("an extra cell is a divergence");
        assert!(
            matches!(&d.detail, Detail::Cells { at } if at == "0"),
            "{:?}",
            d.detail
        );
        assert_eq!(d.left, "no such cell");
        assert_eq!(d.right, "99");
    }

    #[test]
    fn an_arena_whose_contents_differ_names_the_cell_and_both_values() {
        let (program, resolved) = standalone(corpus());
        let mut left = Machine::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);

        let seeded = Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(0))));

        // `compare_tests` re-seeds both from its own base, so the divergence has to be injected
        // through the engine rather than through the fixture.
        right.extra_cell = Some(Value::Int(7));
        let report = compare_tests(&mut left, &mut right, &seeded);
        let d = &report.divergences[0];
        assert!(matches!(&d.detail, Detail::Cells { .. }), "{:?}", d.detail);
    }

    #[test]
    fn footprints_are_compared_only_when_both_sides_traced_one() {
        let (program, resolved) = standalone(corpus());
        let mut left = Machine::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);

        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        assert!(report.is_clean(), "{report}");
        assert_eq!(report.footprints_compared, 0);
    }

    #[test]
    fn a_corpus_the_two_sides_disagree_on_the_size_of_stops_immediately() {
        let (program, resolved) = standalone(corpus());
        let (smaller, smaller_resolved) = standalone(vec![test_def("only one", int(1))]);
        let mut left = Machine::for_program(&program, &resolved);
        let mut right = Perturbed::new(&smaller, &smaller_resolved);

        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        assert_eq!(report.compared, 0);
        assert_eq!(report.divergences.len(), 1);
        assert_eq!(report.divergences[0].left, "2 tests");
    }

    #[test]
    fn an_expression_comparison_reports_the_two_values() {
        let (program, resolved) = standalone(Vec::new());
        let mut left = Machine::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);

        let agree = bin(BinOp::Add, int(1), int(2));
        assert!(compare_expr(&mut left, &mut right, "sum", &agree).is_none());
    }

    #[test]
    fn a_divergence_becomes_a_failing_diagnostic_naming_both_sides() {
        let (program, resolved) = standalone(vec![test_def("t", int(1))]);
        let mut left = Machine::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);
        right.outcome = Some(Err(Diagnostic::error(codes::RUNTIME_ERROR, "boom")));

        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        let d = report.into_result().unwrap_err();
        assert_eq!(d.code, codes::ENGINE_DIVERGENCE);
        assert!(d.message.contains("backend"), "{}", d.message);
        assert!(d.message.contains("machine"), "{}", d.message);
        assert!(d.notes.iter().any(|n| n.contains("boom")), "{:?}", d.notes);
    }

    /// Exercises the shapes a backend is most likely to get wrong: a handler answering
    /// a perform, a cell written from a clause, all three higher-order builtins driving a closure
    /// that itself performs, and four distinct failures whose diagnostics must match label for
    /// label.
    fn mixed_corpus() -> Vec<Item> {
        let state = effect_def("state", &[("get", Mode::Read, false)]);
        let handled = |body: Expr, answer: Expr| {
            handle(body, vec![clause("state", "get", None, &[], answer)])
        };
        vec![
            state,
            fn_def("twice", &["x"], bin(BinOp::Mul, var("x"), int(2))),
            test_def(
                "arithmetic and calls",
                callv("assert_eq", vec![callv("twice", vec![int(21)]), int(42)]),
            ),
            test_def(
                "map over a performing closure",
                handled(
                    callv(
                        "assert_eq",
                        vec![
                            callv(
                                "map",
                                vec![
                                    list(vec![int(1), int(2)]),
                                    lam(
                                        &["x"],
                                        bin(
                                            BinOp::Add,
                                            var("x"),
                                            perform("state", "get", None, vec![]),
                                        ),
                                    ),
                                ],
                            ),
                            list(vec![int(11), int(12)]),
                        ],
                    ),
                    int(10),
                ),
            ),
            test_def(
                "filter and fold agree",
                callv(
                    "assert_eq",
                    vec![
                        callv(
                            "fold",
                            vec![
                                callv(
                                    "filter",
                                    vec![
                                        callv("range", vec![int(0), int(6)]),
                                        lam(
                                            &["x"],
                                            bin(
                                                BinOp::Eq,
                                                bin(BinOp::Rem, var("x"), int(2)),
                                                int(0),
                                            ),
                                        ),
                                    ],
                                ),
                                int(0),
                                lam(&["acc", "x"], bin(BinOp::Add, var("acc"), var("x"))),
                            ],
                        ),
                        int(6),
                    ],
                ),
            ),
            test_def(
                "a cell written from a clause",
                with_cell(
                    "s",
                    int(0),
                    "c",
                    block(
                        vec![discard(handle(
                            block(
                                vec![
                                    discard(perform("state", "get", None, vec![])),
                                    discard(perform("state", "get", None, vec![])),
                                ],
                                None,
                            ),
                            vec![clause(
                                "state",
                                "get",
                                None,
                                &[],
                                callv(
                                    "cell_set",
                                    vec![
                                        var("c"),
                                        bin(BinOp::Add, callv("cell_get", vec![var("c")]), int(1)),
                                    ],
                                ),
                            )],
                        ))],
                        Some(callv(
                            "assert_eq",
                            vec![callv("cell_get", vec![var("c")]), int(2)],
                        )),
                    ),
                ),
            ),
            test_def(
                "a failing assertion",
                spanned(
                    callv(
                        "assert_eq",
                        vec![list(vec![int(1), int(2)]), list(vec![int(1), int(3)])],
                    ),
                    at(88, 100),
                ),
            ),
            test_def("an unhandled effect", perform("state", "get", None, vec![])),
            test_def("a panic", callv("panic", vec![string("boom")])),
            test_def("an arity mismatch", callv("twice", vec![int(1), int(2)])),
        ]
    }

    #[test]
    fn two_real_machines_agree_over_a_mixed_corpus() {
        let (program, resolved) = standalone(mixed_corpus());
        let mut plain = Machine::for_program(&program, &resolved);
        let mut machine = Machine::for_program(&program, &resolved);

        let report = compare_tests(&mut plain, &mut machine, &Fixture::empty());
        assert_eq!(report.compared, 8);
        assert!(report.is_clean(), "{report}");
    }

    /// The corpus is only evidence if some of it actually failed on both sides; eight agreeing
    /// passes would prove nothing about diagnostic equality.
    #[test]
    fn the_mixed_corpus_really_does_fail_four_of_its_tests() {
        let (program, resolved) = standalone(mixed_corpus());
        let mut plain = Machine::for_program(&program, &resolved);
        let codes: Vec<&str> = (0..plain.test_count())
            .filter_map(|i| Evaluator::eval_test(&mut plain, i).err().map(|d| d.code))
            .collect();
        assert_eq!(
            codes,
            [
                super::codes::ASSERTION_FAILED,
                super::codes::UNHANDLED_EFFECT,
                super::codes::RUNTIME_ERROR,
                super::codes::ARITY_MISMATCH,
            ]
        );
    }

    /// The harness must still bite when both sides are honest machines.
    #[test]
    fn a_machine_that_answered_differently_would_be_caught() {
        let (program, resolved) = standalone(vec![test_def(
            "a failing assertion",
            callv("assert_eq", vec![int(1), int(2)]),
        )]);
        let mut plain = Machine::for_program(&program, &resolved);
        let mut machine = Machine::for_program(&program, &resolved);
        assert!(compare_tests(&mut plain, &mut machine, &Fixture::empty()).is_clean());

        let mut perturbed = Perturbed::new(&program, &resolved);
        perturbed.outcome = Some(Err(Diagnostic::error(
            super::codes::ASSERTION_FAILED,
            "assertion failed: expected 2, found 1",
        )));
        let report = compare_tests(&mut plain, &mut perturbed, &Fixture::empty());
        assert_eq!(
            report.divergences[0].detail,
            Detail::Diagnostic {
                field: "labels".to_string()
            }
        );
    }

    #[test]
    fn a_seeded_fixture_reaches_both_sides() {
        let (program, resolved) = standalone(vec![test_def("t", int(1))]);
        let mut left = Machine::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);

        let seeded = Fixture::build(|r| Value::Cell(r.alloc_cell(Value::str("fixture"))));
        let cell = seeded
            .handle()
            .as_cell(Span::DUMMY, "the fixture handle")
            .expect("a cell");

        let report = compare_tests(&mut left, &mut right, &seeded);
        assert!(report.is_clean(), "{report}");
        assert_eq!(left.cells().get(cell).unwrap().render(), "\"fixture\"");
        assert_eq!(right.cells().get(cell).unwrap().render(), "\"fixture\"");
    }
}

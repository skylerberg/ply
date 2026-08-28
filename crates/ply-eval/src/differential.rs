//! Running one program on both engines and comparing what they did.
//!
//! A divergence between two evaluators of one language is the most expensive
//! defect this project can have, because the result cache makes it sticky: the
//! wrong answer is recorded as a `Pass` and never recomputed. So a mismatch is
//! a failure and never a warning, and the comparison is by equality on every
//! field rather than by "both failed" — a weaker check hides exactly the defect
//! this exists to catch.
//!
//! What is compared, per test: the `Result<(), Diagnostic>` field by field
//! (code, severity, message, every label with its span, every note); the
//! observed footprint, when both engines traced one; and the final cell arena
//! as the ordered `(Slot, rendered value)` sequence. Each answer names the
//! first place the two disagreed, so a report says *where* rather than *that*.

use crate::Engine;
use crate::arena::Arena;
use crate::interp::Interp;
use crate::machine::Machine;
use crate::task_regions::Fixture;
use crate::value::Value;
use ply_core::ty::Footprint;
use ply_span::{Diagnostic, Label, Severity, Span, Symbol, codes};
use ply_syntax::ast::{Expr, ExprKind, Item, Program, Stmt};
use std::fmt;

/// What the harness needs of an engine. Both `Interp` and the machine expose
/// these already; the trait exists so the comparison is written once and so a
/// deliberately-wrong engine can be substituted to prove the harness bites.
pub trait Evaluator {
    fn engine(&self) -> Engine;
    fn test_count(&self) -> usize;
    fn test_name(&self, index: usize) -> Option<&str>;
    fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic>;
    /// A position in a `CheckOutput` is not a position in the AST an engine
    /// holds, because the incremental front end reports tests from modules it
    /// never parsed. A runner that has the module addresses uses this instead.
    fn eval_test_in(&mut self, module: &Symbol, ordinal: usize) -> Result<(), Diagnostic>;
    fn eval_expr(&mut self, e: &Expr) -> Result<Value, Diagnostic>;
    /// The run's cells, ascending by slot: the state the two engines must agree
    /// on once a test has run.
    fn cells(&self) -> &Arena;
    /// The same arena, so the harness can ask it to journal what it reclaims.
    fn cells_mut(&mut self) -> &mut Arena;
    fn set_fixture(&mut self, fixture: &Fixture);

    /// The atoms actually performed, for an engine that traces. `None` means
    /// "not traced", which is not the same as "traced and empty": a consumer
    /// that cannot tell them apart acts on the wrong one, so the harness counts
    /// how many footprint comparisons it was actually able to make.
    fn observed_footprint(&self) -> Option<Footprint> {
        None
    }

    /// How many atoms were performed in total, for an engine that traces.
    ///
    /// A footprint is a set and has never been a count, so two engines that
    /// performed one atom three times and once agree on it. This is the axis
    /// that separates them, and it is reported beside the footprint rather than
    /// instead of it because a set is what a scheduler reads.
    fn observed_performs(&self) -> Option<u64> {
        None
    }
}

impl Evaluator for Interp<'_> {
    fn engine(&self) -> Engine {
        Engine::Treewalk
    }

    fn test_count(&self) -> usize {
        Interp::test_count(self)
    }

    fn test_name(&self, index: usize) -> Option<&str> {
        Interp::test_name(self, index)
    }

    fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic> {
        Interp::eval_test(self, index)
    }

    fn eval_test_in(&mut self, module: &Symbol, ordinal: usize) -> Result<(), Diagnostic> {
        Interp::eval_test_in(self, module, ordinal)
    }

    fn eval_expr(&mut self, e: &Expr) -> Result<Value, Diagnostic> {
        self.eval_expr_for_test(e)
    }

    fn cells(&self) -> &Arena {
        Interp::cells(self)
    }

    fn cells_mut(&mut self) -> &mut Arena {
        Interp::cells_mut(self)
    }

    fn set_fixture(&mut self, fixture: &Fixture) {
        let (regions, _) = fixture.open();
        Interp::set_regions(self, regions);
    }

    fn observed_footprint(&self) -> Option<Footprint> {
        Some(self.trace().footprint().clone())
    }

    fn observed_performs(&self) -> Option<u64> {
        Some(self.trace().performs())
    }
}

impl Evaluator for Machine<'_> {
    fn engine(&self) -> Engine {
        Engine::Machine
    }

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
    /// Both refused it, but not identically. `field` is the first field that
    /// differs, in the order a reader scans a diagnostic.
    Diagnostic {
        field: String,
    },
    /// Both accepted it and produced different values.
    Value,
    Footprint,
    /// The arenas differ. `at` names the first cell they disagree on.
    Cells {
        at: String,
    },
    /// The two runs reclaimed different cells, or reclaimed them in a different
    /// order. `at` is the position in each run's reclamation journal.
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

/// A single disagreement, carrying both sides so the reader never has to re-run
/// anything to see what happened.
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
    /// Fails the run. `left` and `right` name the engines in the order the
    /// fields hold them.
    pub fn to_diagnostic(&self, left: Engine, right: Engine, span: Span) -> Diagnostic {
        Diagnostic::error(
            codes::ENGINE_DIVERGENCE,
            format!(
                "`{}` and `{}` disagree on `{}`",
                left.as_str(),
                right.as_str(),
                self.subject
            ),
        )
        .primary(span, format!("the two engines' {} differ", self.detail.what()))
        .note(format!("{}: {}", left.as_str(), self.left))
        .note(format!("{}: {}", right.as_str(), self.right))
        .note("a divergence is never a warning: the result cache would record whichever engine ran first")
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

#[derive(Clone, Debug)]
pub struct Report {
    pub left: Engine,
    pub right: Engine,
    /// Subjects both engines ran and the harness compared.
    pub compared: usize,
    /// Of those, how many carried a footprint from both engines. A zero here
    /// with a non-zero `compared` means the footprint half of the audit did not
    /// happen, which is worth knowing before trusting a green run.
    pub footprints_compared: usize,
    /// Subjects one engine refused, so there was nothing to compare. Counted
    /// apart from `compared` so a corpus only one engine can run cannot pass for
    /// an audited one.
    pub machine_only: usize,
    pub divergences: Vec<Divergence>,
}

impl Report {
    pub fn new(left: Engine, right: Engine) -> Report {
        Report {
            left,
            right,
            compared: 0,
            footprints_compared: 0,
            machine_only: 0,
            divergences: Vec::new(),
        }
    }

    pub fn is_clean(&self) -> bool {
        self.divergences.is_empty()
    }

    /// The first divergence as a diagnostic, so a caller can fail a run without
    /// deciding how to summarize the rest.
    pub fn into_result(self) -> Result<Report, Diagnostic> {
        match self.divergences.first() {
            Some(d) => Err(d.to_diagnostic(self.left, self.right, Span::DUMMY)),
            None => Ok(self),
        }
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} vs {}: {} compared, {} footprints, {} machine-only, {} divergences",
            self.left.as_str(),
            self.right.as_str(),
            self.compared,
            self.footprints_compared,
            self.machine_only,
            self.divergences.len()
        )?;
        for d in &self.divergences {
            writeln!(f, "  {d}")?;
        }
        Ok(())
    }
}

/// Both engines are stepped even when the first has already diverged: a run
/// that stops at the first disagreement leaves the two evaluators at different
/// points in the corpus and every later comparison becomes meaningless.
pub fn compare_test(left: &mut dyn Evaluator, right: &mut dyn Evaluator, index: usize) -> Compared {
    let subject = left
        .test_name(index)
        .or_else(|| right.test_name(index))
        .unwrap_or("<unnamed>")
        .to_string();

    audit_state(left, right);
    let l = left.eval_test(index);
    let r = right.eval_test(index);
    if refused(&l) || refused(&r) {
        return Compared::Refused;
    }
    match compare_outcomes(left, right, &subject, Some(index), &l, &r) {
        Some(d) => Compared::Diverged(d),
        None => Compared::Agreed,
    }
}

/// Asks both arenas to record what their closes reclaim.
///
/// Without it the state half of this oracle is vacuous: a region hands its
/// cells back at its lexical close, so a run that behaved leaves nothing to
/// compare and two engines that disagreed about every write still agree about
/// the empty arena afterwards. Cheap enough to leave on for an audit and off
/// everywhere else — it clones each reclaimed value.
pub fn audit_state(left: &mut dyn Evaluator, right: &mut dyn Evaluator) {
    left.cells_mut().journal();
    right.cells_mut().journal();
}

/// What one subject's audit produced. `Refused` is not agreement: only one
/// engine ran, so the pair proved nothing and must not be counted as if it had.
pub enum Compared {
    Agreed,
    Diverged(Divergence),
    Refused,
}

fn refused(outcome: &Result<(), Diagnostic>) -> bool {
    matches!(outcome, Err(d) if is_machine_only(d))
}

/// Compares two engines that have each already answered the same question.
///
/// Separate from [`compare_test`] so a caller that needs the answer itself —
/// the test runner, which has to report a verdict as well as audit it — does
/// not have to evaluate anything twice.
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

/// The two evaluators must have been built over the same program, so that an
/// index means the same test on both sides. `base` seeds each engine's region
/// stack before the run and every test resets to it.
pub fn compare_tests(
    left: &mut dyn Evaluator,
    right: &mut dyn Evaluator,
    base: &Fixture,
) -> Report {
    let mut report = Report::new(left.engine(), right.engine());

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
            Compared::Refused => {
                report.machine_only += 1;
                continue;
            }
            Compared::Agreed => report.compared += 1,
            Compared::Diverged(d) => {
                report.compared += 1;
                report.divergences.push(d);
            }
        }
        // After the run, not before: before it, a footprint is the *previous*
        // test's and counting it would claim a comparison that never happened.
        if left.observed_footprint().is_some() && right.observed_footprint().is_some() {
            report.footprints_compared += 1;
        }
    }
    report
}

/// The first field of two outcomes that differs, in the order a reader scans a
/// diagnostic: whether it failed at all, then code, severity, message, labels,
/// notes.
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

/// The atoms performed, and how many were performed in total. The count is part
/// of this axis rather than its own: an engine that traces reports both or
/// neither, and a reader comparing two runs wants the pair.
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

/// Two arenas agree when they hold the same cells in the same order with the
/// same rendered contents. `Arena::slots` is ascending by index, so the walk is
/// deterministic and the first disagreement is a stable answer.
///
/// **By index, never by whole slot.** A slot's generation counts how often its
/// position has been reclaimed, which is the engine's own history: two engines
/// reach one state through different numbers of entry points — the tree-walker
/// refuses a machine-only test before it resets — and comparing generations
/// makes that a divergence in every program with a cell in it. What a program
/// means is which cell holds what, and that is the index and the value.
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
///
/// This is the half of the state oracle that survives reclamation: it compares
/// every cell that ever existed, at the value it held when its region closed,
/// rather than the ones a run happened to leave behind. Both journals are empty
/// when nobody asked for one, and then this decides nothing — which is why
/// [`audit_state`] is not optional for a caller that wants the oracle.
fn reclaimed_divergence(left: &Arena, right: &Arena) -> Option<(Detail, String, String)> {
    let (a, b) = (left.journalled(), right.journalled());
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        // By index and value, never by generation: a generation counts how many
        // entry points a *position* has been through, and the tree-walker
        // refuses a machine-only test before it resets, so the two engines reach
        // the same state having reclaimed a different number of times.
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

/// The tree-walker's refusal of a `simulate` region.
///
/// A task is a suspended machine state, so scheduling one needs the explicit
/// control stack. Running the region's body straight through would be a
/// plausible wrong answer — one interleaving, unnamed by any seed — and the
/// result cache would keep it.
pub fn machine_only_region(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::MACHINE_ONLY_CLAUSE,
        "a `simulate` region installs a scheduler over captured continuations",
    )
    .primary(span, "this region needs an explicit control stack")
    .note(format!(
        "run this with `--engine {}`",
        Engine::Machine.as_str()
    ))
}

/// The tree-walker's refusal of a clause it cannot run.
///
/// A general clause reifies a continuation, which needs an explicit control
/// stack. Approximating it as tail-resumptive would produce a plausible wrong
/// answer that the result cache would then keep, so the tree-walker must refuse
/// the clause by name and say which engine runs it.
pub fn machine_only_clause(span: Span, effect: &str, op: &str) -> Diagnostic {
    Diagnostic::error(
        codes::MACHINE_ONLY_CLAUSE,
        format!("the handler clause for `{effect}.{op}` binds a continuation"),
    )
    .primary(span, "this clause needs an explicit control stack")
    .note(format!(
        "run this with `--engine {}`",
        Engine::Machine.as_str()
    ))
}

/// A refusal means this engine declined to start, so the other engine's answer
/// is the only one there is and comparing them would report a divergence where
/// there is no disagreement — only one participant.
pub fn is_machine_only(d: &Diagnostic) -> bool {
    d.code == codes::MACHINE_ONLY_CLAUSE
}

/// Every clause in a program the tree-walker would refuse.
///
/// Reported all at once rather than one per failing run, so that `ply check`
/// can answer "which engine does this program need" before anything is
/// evaluated. The walk is over the AST rather than over a run, because a clause
/// on a path no test reaches is still a clause this engine cannot express.
pub fn machine_only_clauses(program: &Program) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for module in &program.modules {
        for item in &module.items {
            match item {
                Item::Fn(d) => {
                    walk_expr(&d.body, &mut out);
                    for clause in &d.spec {
                        walk_expr(&clause.expr, &mut out);
                    }
                }
                Item::Test(t) => walk_expr(&t.body, &mut out),
                // A concurrency law's body *is* a `simulate` region, which is
                // machine-only, so a law is exactly where this walk pays.
                Item::Law(l) => {
                    if let Some(guard) = &l.guard {
                        walk_expr(guard, &mut out);
                    }
                    walk_expr(&l.body, &mut out);
                }
                // A `derive` has no body of its own; its generated
                // definitions are `Item::Fn`s and are walked above.
                Item::Type(_) | Item::Effect(_) | Item::Derive(_) | Item::EffectSet(_) => {}
            }
        }
    }
    out
}

fn walk_expr(e: &Expr, out: &mut Vec<Diagnostic>) {
    let mut children: Vec<&Expr> = Vec::new();
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Var(_) => {}
        ExprKind::Unary { operand, .. } => children.push(operand),
        ExprKind::Binary { lhs, rhs, .. } => children.extend([lhs.as_ref(), rhs.as_ref()]),
        ExprKind::Lambda { body, .. } => children.push(body),
        ExprKind::App { func, args } => {
            children.push(func);
            children.extend(args);
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => children.extend([cond.as_ref(), then_branch.as_ref(), else_branch.as_ref()]),
        ExprKind::Match { scrutinee, arms } => {
            children.push(scrutinee);
            for arm in arms {
                children.extend(arm.guard.iter());
                children.push(&arm.body);
            }
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match s {
                    Stmt::Expr(x) => children.push(x),
                    Stmt::Let { value, .. } => children.push(value),
                }
            }
            children.extend(tail.as_deref());
        }
        ExprKind::Record { fields } => children.extend(fields.iter().map(|(_, v)| v)),
        ExprKind::RecordUpdate { base, fields } => {
            children.push(base);
            children.extend(fields.iter().map(|(_, v)| v));
        }
        ExprKind::Field { base, .. } => children.push(base),
        ExprKind::List { items } => children.extend(items),
        ExprKind::Perform { args, .. } => children.extend(args),
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            children.push(body);
            for c in clauses {
                if c.resume.is_some() {
                    out.push(machine_only_clause(
                        c.span,
                        &c.effect.to_string(),
                        c.op.name.as_str(),
                    ));
                }
                children.push(&c.body);
            }
            children.extend(return_clause.as_deref().map(|r| &r.body));
        }
        ExprKind::WithCell { init, body, .. } => children.extend([init.as_ref(), body.as_ref()]),
        ExprKind::WithRegion { body, .. } => children.push(body),
        ExprKind::Simulate { body } => {
            out.push(machine_only_region(e.span));
            children.push(body);
        }
    }
    for child in children {
        walk_expr(child, out);
    }
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

    /// An engine that is the tree-walker except where a test asks it not to be.
    /// It is how the harness is proved to bite: without an injected divergence
    /// a green report says nothing.
    struct Perturbed<'a> {
        inner: Interp<'a>,
        /// Answers `eval_test` with this instead of running it.
        outcome: Option<Result<(), Diagnostic>>,
        /// Allocated into the arena after every test.
        extra_cell: Option<Value>,
        footprint: Option<Footprint>,
    }

    impl<'a> Perturbed<'a> {
        fn new(program: &'a Program, resolved: &'a Resolved) -> Perturbed<'a> {
            Perturbed {
                inner: Interp::for_program(program, resolved),
                outcome: None,
                extra_cell: None,
                footprint: None,
            }
        }
    }

    impl Evaluator for Perturbed<'_> {
        fn engine(&self) -> Engine {
            Engine::Machine
        }

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
    fn two_honest_engines_over_one_corpus_report_nothing() {
        let (program, resolved) = standalone(corpus());
        let mut left = Interp::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);
        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        assert_eq!(report.compared, 2);
        assert!(report.is_clean(), "{report}");
    }

    #[test]
    fn an_engine_that_passes_what_the_other_failed_is_caught() {
        let (program, resolved) = standalone(vec![test_def(
            "fails on both, honestly",
            callv("assert_eq", vec![int(1), int(2)]),
        )]);
        let mut left = Interp::for_program(&program, &resolved);
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
        let mut left = Interp::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);

        let mut drifted = Interp::for_program(&program, &resolved)
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
        let mut left = Interp::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);

        let mut drifted = Interp::for_program(&program, &resolved)
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

    /// The case a verdict comparison alone would miss entirely: both engines
    /// pass, and one of them left its cells somewhere else.
    #[test]
    fn an_arena_that_differs_after_a_passing_test_is_caught_at_the_cell() {
        let (program, resolved) = standalone(corpus());
        let mut left = Interp::for_program(&program, &resolved);
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
        let mut left = Interp::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);

        let seeded = Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(0))));

        // `compare_tests` re-seeds both from its own base, so the divergence has
        // to be injected through the engine rather than through the fixture.
        right.extra_cell = Some(Value::Int(7));
        let report = compare_tests(&mut left, &mut right, &seeded);
        let d = &report.divergences[0];
        assert!(matches!(&d.detail, Detail::Cells { .. }), "{:?}", d.detail);
    }

    #[test]
    fn footprints_are_compared_only_when_both_engines_traced_one() {
        let (program, resolved) = standalone(corpus());
        let mut left = Interp::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);

        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        assert!(report.is_clean(), "{report}");
        assert_eq!(report.footprints_compared, 0);
    }

    #[test]
    fn a_corpus_the_two_engines_disagree_on_the_size_of_stops_immediately() {
        let (program, resolved) = standalone(corpus());
        let (smaller, smaller_resolved) = standalone(vec![test_def("only one", int(1))]);
        let mut left = Interp::for_program(&program, &resolved);
        let mut right = Perturbed::new(&smaller, &smaller_resolved);

        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        assert_eq!(report.compared, 0);
        assert_eq!(report.divergences.len(), 1);
        assert_eq!(report.divergences[0].left, "2 tests");
    }

    #[test]
    fn an_expression_comparison_reports_the_two_values() {
        let (program, resolved) = standalone(Vec::new());
        let mut left = Interp::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);

        let agree = bin(BinOp::Add, int(1), int(2));
        assert!(compare_expr(&mut left, &mut right, "sum", &agree).is_none());
    }

    #[test]
    fn a_divergence_becomes_a_failing_diagnostic_naming_both_engines() {
        let (program, resolved) = standalone(vec![test_def("t", int(1))]);
        let mut left = Interp::for_program(&program, &resolved);
        let mut right = Perturbed::new(&program, &resolved);
        right.outcome = Some(Err(Diagnostic::error(codes::RUNTIME_ERROR, "boom")));

        let report = compare_tests(&mut left, &mut right, &Fixture::empty());
        let d = report.into_result().unwrap_err();
        assert_eq!(d.code, codes::ENGINE_DIVERGENCE);
        assert!(d.message.contains("treewalk"), "{}", d.message);
        assert!(d.message.contains("machine"), "{}", d.message);
        assert!(d.notes.iter().any(|n| n.contains("boom")), "{:?}", d.notes);
    }

    /// Exercises the shapes the two engines are most likely to disagree about:
    /// a handler answering a perform, a cell written from a clause, all three
    /// higher-order builtins driving a closure that itself performs, and four
    /// distinct failures whose diagnostics must match label for label.
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
                                        callv("range", vec![int(6)]),
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
    fn the_two_real_engines_agree_over_a_mixed_corpus() {
        let (program, resolved) = standalone(mixed_corpus());
        let mut treewalk = Interp::for_program(&program, &resolved);
        let mut machine = Machine::for_program(&program, &resolved);

        let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
        assert_eq!(report.compared, 8);
        assert!(report.is_clean(), "{report}");
    }

    /// The corpus is only evidence if some of it actually failed on both sides;
    /// eight agreeing passes would prove nothing about diagnostic equality.
    #[test]
    fn the_mixed_corpus_really_does_fail_four_of_its_tests() {
        let (program, resolved) = standalone(mixed_corpus());
        let mut treewalk = Interp::for_program(&program, &resolved);
        let codes: Vec<&str> = (0..treewalk.test_count())
            .filter_map(|i| Evaluator::eval_test(&mut treewalk, i).err().map(|d| d.code))
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

    /// The harness must still bite when the honest engine is the machine.
    #[test]
    fn a_machine_that_answered_differently_would_be_caught() {
        let (program, resolved) = standalone(vec![test_def(
            "a failing assertion",
            callv("assert_eq", vec![int(1), int(2)]),
        )]);
        let mut treewalk = Interp::for_program(&program, &resolved);
        let mut machine = Machine::for_program(&program, &resolved);
        assert!(compare_tests(&mut treewalk, &mut machine, &Fixture::empty()).is_clean());

        let mut perturbed = Perturbed::new(&program, &resolved);
        perturbed.outcome = Some(Err(Diagnostic::error(
            super::codes::ASSERTION_FAILED,
            "assertion failed: expected 2, found 1",
        )));
        let report = compare_tests(&mut treewalk, &mut perturbed, &Fixture::empty());
        assert_eq!(
            report.divergences[0].detail,
            Detail::Diagnostic {
                field: "labels".to_string()
            }
        );
    }

    #[test]
    fn a_refused_clause_names_itself_and_the_engine_that_runs_it() {
        let d = machine_only_clause(at(4, 9), "amb", "flip");
        assert!(d.message.contains("`amb.flip`"), "{}", d.message);
        assert!(
            d.notes.iter().any(|n| n.contains("--engine machine")),
            "{:?}",
            d.notes
        );
        assert_eq!(d.primary_span().unwrap(), at(4, 9));
    }

    #[test]
    fn a_seeded_fixture_reaches_both_engines() {
        let (program, resolved) = standalone(vec![test_def("t", int(1))]);
        let mut left = Interp::for_program(&program, &resolved);
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

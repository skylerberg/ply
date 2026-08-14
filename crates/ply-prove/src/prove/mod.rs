//! The static proof tier: a decision procedure for ADR 0007 §5.1's fragment.
//!
//! # What this module answers
//!
//! One question, about one obligation: *is `guard ⟹ body` valid for every
//! value of its binders?* — and it has three answers, of which two are safe by
//! construction:
//!
//! | answer | meaning |
//! | --- | --- |
//! | [`Decision::Proved`] | a static argument covering **every** input satisfying the guard |
//! | [`Decision::GuardUnsatisfiable`] | the guard admits nothing, so the obligation is vacuous and says nothing |
//! | [`Decision::Unknown`] | the fragment did not decide it |
//!
//! **There is no fourth answer, and in particular there is no refutation.** If
//! the negation survives, what survives is a model over uninterpreted symbols,
//! and such a model need not correspond to any Ply value — an uninterpreted `f`
//! in it can be a function no closure computes. Reporting that as a
//! counterexample would be a confidently wrong red, which is the failure
//! symmetric to a wrong `proved`. The static side of this milestone is
//! **refutation-incomplete on purpose**: it either proves or shrugs, and the
//! property tier does the refuting, with a value it actually ran.
//!
//! # The fragment, exactly
//!
//! - **Linear integer arithmetic.** `+`, binary `-`, unary `-`, multiplication
//!   where a factor is an integer literal, and `== != < <= > >=` over `Int`.
//!   `x * y` with both symbolic, `/` and `%` are uninterpreted, so
//!   `x / 2 * 2 == x` is **not** proved.
//! - **Propositional structure.** `&&`, `||`, `!`, `if` and `==` at `Bool`, by
//!   unit propagation and case split over the atoms.
//! - **Case analysis over ADTs.** A `match` splits on the scrutinee's outermost
//!   constructor when the complete constructor list is in hand; the fields
//!   become fresh symbolic constants. Depth 1, which is exactly where induction
//!   would be needed and is not available.
//! - **Structural equality and congruence closure**, with constructors
//!   injective and distinct, record projection reducing over a record, and
//!   every other application of a **pure** head an uninterpreted function
//!   symbol closed under congruence. A call whose row is not known to be empty
//!   gets a fresh symbol per occurrence: two calls that perform may answer
//!   differently, and `f() - f() == 0` is not a theorem about one of them.
//! - **Bounded unfolding of non-recursive definitions**, to
//!   [`crate::UNFOLD_DEPTH`]. A member of a recursive component is never
//!   unfolded, which is why `reverse(reverse(xs)) == xs` is `property` and
//!   should be.
//!
//! # Where it deliberately stops
//!
//! Induction, quantifier alternation, `exists`, division, bit-width — none of
//! them. `Int` is reasoned about as a mathematical integer, which ADR 0007
//! §5.1(a) discloses: a law true over ℤ and false at `i64::MAX` is reported
//! `proved`, and the generator drawing the boundaries on every property run is
//! the named mitigation. Nested constructor patterns, record patterns, list
//! patterns and pattern guards leave their `match` uninterpreted rather than
//! guessed.

mod arith;
mod context;
mod egraph;
mod lower;
mod solve;
mod term;

#[cfg(test)]
mod tests;

pub use context::Context;
pub use lower::Blocker;

use crate::{Certificate, Rule, DEFAULT_PROVE_BUDGET, UNFOLD_DEPTH};
use ply_core::{LawBinder, TyVar, Type};
use ply_span::Symbol;
use ply_syntax::ast::Expr;
use std::collections::BTreeSet;

/// How deep the case analysis nests before the answer becomes `Unknown`. A
/// split always decides something previously undecided, so the search
/// terminates without this; it bounds the cost of a pathological obligation,
/// not the correctness of an ordinary one.
///
/// Deep enough for a case analysis **and** the definedness requirements stacked
/// on top of it, which is a chain of one split per arithmetic operator in the
/// body. The step budget is the bound that matters; this one only has to stop
/// being the thing an ordinary obligation trips over.
pub const SPLIT_DEPTH: u32 = 48;

/// One obligation, as the prover sees it.
///
/// Every binder becomes a fresh symbolic constant, which is what makes the
/// answer a universal statement rather than a statement about one case.
pub struct Goal<'a> {
    /// Index into `Program::modules`: the module the expressions were written
    /// in, which is what their bare names resolve against.
    pub module: usize,
    /// For an `ensures`, the owner's parameters and `result`; for a law, its
    /// `forall` binders.
    pub binders: &'a [LawBinder],
    /// The `requires` clauses beside this one, or a law's `where`. Empty is an
    /// unguarded claim. They are conjoined, and the domain they cut out is what
    /// [`Decision::GuardUnsatisfiable`] reports on.
    pub guards: &'a [&'a Expr],
    /// For an `ensures`: the binder standing for the return value — which must
    /// also appear in `binders`, carrying the declared return type — and the
    /// definition's own body.
    ///
    /// Without this equation `result` is an arbitrary value of its type and no
    /// postcondition mentioning it could ever be valid, so a caller that omits
    /// it gets `Unknown` rather than a wrong answer. The body is lowered in the
    /// same scope as the clause, which is what "`result` is bound beside the
    /// parameters" means operationally.
    pub result: Option<(Symbol, &'a Expr)>,
    pub body: &'a Expr,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    /// Inference steps, charged per obligation.
    pub steps: u32,
    pub unfold_depth: u32,
    pub split_depth: u32,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            steps: DEFAULT_PROVE_BUDGET,
            unfold_depth: UNFOLD_DEPTH,
            split_depth: SPLIT_DEPTH,
        }
    }
}

/// Why an attempt was inconclusive. Neither variant is a claim about the
/// obligation: both mean the prover stopped, not that the obligation is false.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reason {
    /// A branch of the case analysis survived — a term outside the fragment, a
    /// needed unfolding refused because the callee is recursive, or a goal that
    /// is simply not valid. The prover does not distinguish the three, and must
    /// not: telling them apart would mean claiming a model.
    Open,
    /// The step budget or the split depth ran out.
    BudgetSpent,
}

/// A static argument, and the rules it rests on.
///
/// Holding one is *not* enough to report `proved`: [`Proof::certify`] refuses
/// to build a [`Certificate`] until something has established that the guard
/// admits a value, because `guard ⟹ body` over an empty domain is valid and
/// says nothing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Proof {
    /// In application order, deduplicated. Every entry is a fragment rule by
    /// construction: [`Rule`] is a closed enum.
    pub rules: Vec<Rule>,
    pub steps: u32,
    /// Type variables the proof left as uninterpreted sorts, so a proved
    /// polymorphic law is genuinely polymorphic.
    pub sorts: Vec<Symbol>,
    /// Whether the **prover** established that the guard admits a value: there
    /// is no guard over an inhabited domain, or the guard was itself proved
    /// valid. A property run that kept a case establishes the same thing, which
    /// is what the argument to [`Proof::certify`] is for.
    pub guard_satisfiable: bool,
}

impl Proof {
    pub fn certify(&self, guard_witnessed: bool) -> Option<Certificate> {
        if !self.guard_satisfiable && !guard_witnessed {
            return None;
        }
        Some(Certificate {
            rules: self.rules.clone(),
            steps: self.steps,
            guard_satisfiable: true,
            sorts: self.sorts.clone(),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Decision {
    Proved(Proof),
    /// The prover showed the guard unsatisfiable within the fragment. The
    /// obligation is `Vacuous` — `E0420`, an error — and never `proved`: a
    /// system that reported it proved would turn a typo in a guard into a proof
    /// of everything.
    GuardUnsatisfiable { steps: u32 },
    Unknown { reason: Reason, steps: u32 },
}

impl Decision {
    pub fn steps(&self) -> u32 {
        match self {
            Decision::Proved(p) => p.steps,
            Decision::GuardUnsatisfiable { steps } | Decision::Unknown { steps, .. } => *steps,
        }
    }
}

/// Attempts one obligation.
///
/// The order is not an optimization. Vacuity is decided **first**, so an
/// obligation whose guard admits nothing can never be reported as a proof of
/// its body.
pub fn decide(ctx: &Context<'_>, goal: &Goal<'_>, limits: &Limits) -> Decision {
    decide_and_diagnose(ctx, goal, limits).0
}

/// [`decide`], and where the obligation left the fragment on the way.
///
/// The second half is a measurement of the prover's reach and is not part of
/// any answer: the decision is computed identically either way, and a
/// [`Blocker`] is never consulted. It exists so that "what would extend this
/// prover" is a number somebody can read rather than a guess.
pub fn decide_and_diagnose(
    ctx: &Context<'_>,
    goal: &Goal<'_>,
    limits: &Limits,
) -> (Decision, Vec<Blocker>) {
    let mut rules = RuleLog::default();
    let mut lowering = lower::Lowering::new(ctx, &mut rules, goal.module, limits.unfold_depth);
    let mut bound: Vec<(Symbol, term::TermId)> = Vec::new();
    for binder in goal.binders {
        bound.push((
            binder.name.clone(),
            lowering.bind_symbolic(&binder.name, &binder.ty),
        ));
    }
    let mut guards: Vec<term::TermId> = Vec::with_capacity(goal.guards.len());
    for guard in goal.guards {
        let lowered = lowering.lower(guard);
        // A later `requires` is only reached when the earlier ones held, so it
        // may lean on them — exactly as the evaluator does, which stops at the
        // first clause that answers `false`.
        lowering.assume(lowered);
        guards.push(lowered);
    }
    // Everything after this point is evaluated under a guard that already held;
    // everything before it is evaluated to decide whether it holds, so it may
    // not assume it.
    let guard_requirements = lowering.requirement_mark();
    let definition = goal.result.as_ref().and_then(|(name, body)| {
        let value = lowering.lower(body);
        let symbol = bound.iter().find(|(n, _)| n == name)?.1;
        Some((symbol, value))
    });
    let body = lowering.lower(goal.body);
    let blockers = lowering.blockers().to_vec();
    let requirements = lowering.requirements().to_vec();
    let mut terms = lowering.finish();
    let result_symbol = definition.map(|(symbol, _)| symbol);
    let definition = definition.map(|(symbol, value)| terms.eq(symbol, value));

    let mut spent = 0u32;
    let mut budget = limits.steps;
    let guarded: Vec<(term::TermId, bool)> = guards.iter().map(|g| (*g, true)).collect();
    let ranges = int_ranges(&mut terms, result_symbol);

    // The guard is evaluated before anything knows whether it holds, so a guard
    // that raises at some input has no domain to speak of and the obligation is
    // not decided here at any tier.
    let (guard_needs, body_needs) = requirements.split_at(guard_requirements);
    if let Some(conjoined) = conjunction(&mut terms, guard_needs) {
        let mut assertions = ranges.clone();
        assertions.push((conjoined, false));
        let (answer, left) = run(&mut terms, ctx, &mut rules, budget, limits, &assertions);
        spent += budget - left;
        budget = left;
        if let Some(reason) = inconclusive(answer) {
            return (Decision::Unknown { reason, steps: spent }, blockers);
        }
    }

    // Vacuity first, and over the guard **alone**. An obligation whose guard
    // admits nothing is `Vacuous` — a defect in the spec — and reporting it as
    // a proof of its body would turn a typo in a guard into a proof of
    // everything.
    if !guarded.is_empty() {
        let mut vacuity = RuleLog::default();
        let (answer, left) = run(&mut terms, ctx, &mut vacuity, budget, limits, &guarded);
        spent += budget - left;
        budget = left;
        if answer == solve::Answer::Closed {
            return (Decision::GuardUnsatisfiable { steps: spent }, blockers);
        }
    }

    // The goal decides `guard ⟹ body` over ℤ and over total function symbols.
    // That is a statement about *Ply* only where those agree with what the
    // evaluator does, which is exactly what the body's requirements say — so the
    // claim is `body ∧ requirements` and not `body`, in **one** run rather than
    // two. One run is not an optimization: the case analysis a body needs is the
    // case analysis its arithmetic needs, and paying for it twice would put an
    // obligation that decides today over its own budget.
    let claim = match conjunction(&mut terms, body_needs) {
        Some(conjoined) => terms.mk(term::Node::And(body, conjoined), Some(Type::bool())),
        None => body,
    };
    let mut assertions = ranges.clone();
    assertions.extend(guarded.iter().copied());
    assertions.extend(definition.map(|d| (d, true)));
    assertions.push((claim, false));

    let (answer, left) = run(&mut terms, ctx, &mut rules, budget, limits, &assertions);
    spent += budget - left;
    budget = left;

    if let Some(reason) = inconclusive(answer) {
        return (Decision::Unknown { reason, steps: spent }, blockers);
    }

    let guard_satisfiable = domain_inhabited(ctx, goal.binders)
        && match conjunction(&mut terms, &guards) {
            None => true,
            // A guard that holds of *every* input admits one, given the domain
            // is not empty. Anything weaker than that the prover does not
            // establish, and the caller must produce a kept case instead.
            Some(all) => {
                let mut ignored = RuleLog::default();
                let (answer, left) =
                    run(&mut terms, ctx, &mut ignored, budget, limits, &[(all, false)]);
                spent += budget - left;
                answer == solve::Answer::Closed
            }
        };

    (
        Decision::Proved(Proof {
            rules: rules.into_rules(),
            steps: spent,
            sorts: uninterpreted_sorts(ctx, goal.binders),
            guard_satisfiable,
        }),
        blockers,
    )
}

/// Why an attempt stopped, or `None` when it closed.
fn inconclusive(answer: solve::Answer) -> Option<Reason> {
    match answer {
        solve::Answer::Closed => None,
        solve::Answer::Open => Some(Reason::Open),
        solve::Answer::Exhausted => Some(Reason::BudgetSpent),
    }
}

/// `MIN ≤ t ≤ MAX` for every term that denotes a Ply `Int` whenever it is
/// defined.
///
/// Every one of them is a true statement about every Ply value, so asserting
/// them can only decide more goals — and it is what puts `-x` under `x >= 0`,
/// or `x + 1` under `x < 100`, back inside the fragment once the definedness
/// requirements are being asked for.
///
/// A [`term::Node::Lin`] is deliberately absent: it is the mathematical value
/// whose range is the question. So is any term that got its symbol by *leaving*
/// `Int` — `MAX + MAX` is not a value, and assuming it were one would let a
/// requirement close on a falsehood.
///
/// **And so is `result`, with everything built on it.** `result` is a Ply value
/// only if the definition returned one, which is the very thing the requirements
/// decide; assuming its width alongside `result == <body>` would let the goal
/// prove its own definedness — `ensures result > x` on `x + 1` reported `proved`
/// is what that looks like from outside.
fn int_ranges(
    terms: &mut term::Terms,
    computed: Option<term::TermId>,
) -> Vec<(term::TermId, bool)> {
    let derived = derived_from(terms, computed);
    let mut atoms: Vec<term::TermId> = Vec::new();
    for (id, node) in terms.nodes() {
        if derived[id] {
            continue;
        }
        let atom = match node {
            term::Node::Sym(_) | term::Node::Field { .. } => true,
            term::Node::App { head, .. } => !matches!(
                terms.node(*head),
                term::Node::Opaque(name) if is_operator_symbol(name.as_str())
            ),
            _ => false,
        };
        if atom && terms.sort(id).is_some_and(term::is_int_type) {
            atoms.push(id);
        }
    }

    let min = terms.int_lit(i64::MIN);
    let max = terms.int_lit(i64::MAX);
    let mut out = Vec::with_capacity(atoms.len() * 2);
    for atom in atoms {
        let low = terms.mk(
            term::Node::Cmp {
                op: term::CmpOp::Ge,
                lhs: atom,
                rhs: min,
            },
            Some(Type::bool()),
        );
        let high = terms.mk(
            term::Node::Cmp {
                op: term::CmpOp::Le,
                lhs: atom,
                rhs: max,
            },
            Some(Type::bool()),
        );
        out.push((low, true));
        out.push((high, true));
    }
    out
}

/// Every term reachable from `root`, by one pass in construction order: the
/// interner never builds a node before its children, so a parent's flag is
/// settled by the time it is read.
fn derived_from(terms: &term::Terms, root: Option<term::TermId>) -> Vec<bool> {
    let mut out = vec![false; terms.len()];
    let Some(root) = root else {
        return out;
    };
    out[root] = true;
    for (id, node) in terms.nodes() {
        if !out[id] {
            out[id] = children(node).into_iter().any(|child| out[child]);
        }
    }
    out
}

fn children(node: &term::Node) -> Vec<term::TermId> {
    match node {
        term::Node::Int(_)
        | term::Node::Bool(_)
        | term::Node::Str(_)
        | term::Node::Unit
        | term::Node::Sym(_)
        | term::Node::Opaque(_) => Vec::new(),
        term::Node::Lin(poly) => poly.monomials.iter().map(|(t, _)| *t).collect(),
        term::Node::App { head, args } => {
            let mut out = vec![*head];
            out.extend(args);
            out
        }
        term::Node::Ctor { args, .. } | term::Node::List(args) => args.clone(),
        term::Node::Record(fields) => fields.iter().map(|(_, v)| *v).collect(),
        term::Node::Field { base, .. } => vec![*base],
        term::Node::Not(a) => vec![*a],
        term::Node::And(a, b) | term::Node::Or(a, b) => vec![*a, *b],
        term::Node::Cmp { lhs, rhs, .. } | term::Node::Eq { lhs, rhs } => vec![*lhs, *rhs],
        term::Node::If {
            cond,
            then_branch,
            else_branch,
        } => vec![*cond, *then_branch, *else_branch],
        term::Node::Match { scrutinee, arms } => {
            let mut out = vec![*scrutinee];
            for arm in arms {
                out.extend(&arm.binds);
                out.push(arm.body);
                if let term::ArmTest::Lit(literal) = arm.test {
                    out.push(literal);
                }
            }
            out
        }
    }
}

fn is_operator_symbol(name: &str) -> bool {
    matches!(
        name,
        term::ADD | term::SUB | term::MUL | term::DIV | term::REM | term::CONCAT
    )
}

fn conjunction(terms: &mut term::Terms, guards: &[term::TermId]) -> Option<term::TermId> {
    let mut out: Option<term::TermId> = None;
    for guard in guards {
        out = Some(match out {
            None => *guard,
            Some(previous) => terms.mk(
                term::Node::And(previous, *guard),
                Some(ply_core::Type::bool()),
            ),
        });
    }
    out
}

fn run(
    terms: &mut term::Terms,
    ctx: &Context<'_>,
    rules: &mut RuleLog,
    budget: u32,
    limits: &Limits,
    assertions: &[(term::TermId, bool)],
) -> (solve::Answer, u32) {
    let mut solver = solve::Solver::new(terms, ctx, rules, budget, limits.split_depth);
    let answer = solver.refute(assertions);
    let left = solver.budget();
    (answer, left)
}

/// Whether every binder's type has at least one value. A guard that holds of
/// every input still admits nothing if there is no input, and a proof over an
/// empty domain is the vacuity this milestone reports as a defect.
fn domain_inhabited(ctx: &Context<'_>, binders: &[LawBinder]) -> bool {
    binders.iter().all(|b| ctx.inhabited(&b.ty))
}

fn uninterpreted_sorts(ctx: &Context<'_>, binders: &[LawBinder]) -> Vec<Symbol> {
    let mut vars: BTreeSet<TyVar> = BTreeSet::new();
    for binder in binders {
        collect_vars(&binder.ty, &mut vars);
    }
    vars.into_iter().map(|v| ctx.sort_name(v)).collect()
}

fn collect_vars(ty: &Type, out: &mut BTreeSet<TyVar>) {
    match ty {
        Type::Var(v) => {
            out.insert(*v);
        }
        Type::Con(_, args) => args.iter().for_each(|a| collect_vars(a, out)),
        Type::Fn { params, ret, .. } => {
            params.iter().for_each(|p| collect_vars(p, out));
            collect_vars(ret, out);
        }
        Type::Record(fields) => fields.values().for_each(|t| collect_vars(t, out)),
    }
}

/// The rules a proof used, in application order and without repeats.
///
/// [`Rule`] is a closed enum, so a prover that grew a rule nobody sanctioned
/// stops compiling before it fails ADR 0007 §11's certificate audit.
#[derive(Default)]
pub(crate) struct RuleLog {
    rules: Vec<Rule>,
    unfolds: Vec<(Symbol, u32)>,
}

impl RuleLog {
    pub(crate) fn note(&mut self, rule: Rule) {
        if !self.rules.contains(&rule) {
            self.rules.push(rule);
        }
    }

    /// One entry per definition, at the deepest inlining it received.
    pub(crate) fn unfolded(&mut self, def: Symbol, depth: u32) {
        match self.unfolds.iter_mut().find(|(name, _)| *name == def) {
            Some((_, deepest)) => *deepest = (*deepest).max(depth),
            None => self.unfolds.push((def, depth)),
        }
    }

    fn into_rules(mut self) -> Vec<Rule> {
        for (def, depth) in self.unfolds {
            self.rules.push(Rule::Unfold { def, depth });
        }
        self.rules
    }
}

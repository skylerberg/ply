//! The static proof tier: a decision procedure for ADR 0007 §5.1's fragment.

mod arith;
mod context;
mod egraph;
mod lower;
mod solve;
mod term;

#[cfg(test)]
mod bits;
#[cfg(test)]
mod numerics;
#[cfg(test)]
mod tests;

pub use context::Context;
pub use lower::Blocker;

use crate::{Certificate, DEFAULT_PROVE_BUDGET, Rule, UNFOLD_DEPTH};
use ply_core::{LawBinder, TyVar, Type};
use ply_span::Symbol;
use ply_syntax::ast::Expr;
use std::collections::BTreeSet;

/// How deep the case analysis nests before the answer becomes `Unknown`.
pub const SPLIT_DEPTH: u32 = 48;

/// One obligation, as the prover sees it.
pub struct Goal<'a> {
    /// Index into `Program::modules`: the module the expressions were written in, which is what
    /// their bare names resolve against.
    pub module: usize,
    /// For an `ensures`, the owner's parameters and `result`; for a law, its `forall` binders.
    pub binders: &'a [LawBinder],
    /// The `requires` clauses beside this one, or a law's `where`.
    pub guards: &'a [&'a Expr],
    /// For an `ensures`: the binder standing for the return value — which must also appear in
    /// `binders`, carrying the declared return type — and the definition's own body.
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

/// Why an attempt was inconclusive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reason {
    /// A branch of the case analysis survived — a term outside the fragment, a needed unfolding
    /// refused because the callee is recursive, or a goal that is simply not valid.
    Open,
    /// The step budget or the split depth ran out.
    BudgetSpent,
}

/// A static argument, and the rules it rests on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Proof {
    /// In application order, deduplicated.
    pub rules: Vec<Rule>,
    pub steps: u32,
    /// Type variables the proof left as uninterpreted sorts, so a proved polymorphic law is
    /// genuinely polymorphic.
    pub sorts: Vec<Symbol>,
    /// Whether the **prover** established that the guard admits a value: there is no guard over an
    /// inhabited domain, or the guard was itself proved valid.
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
    /// The prover showed the guard unsatisfiable within the fragment.
    GuardUnsatisfiable {
        steps: u32,
    },
    Unknown {
        reason: Reason,
        steps: u32,
    },
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
pub fn decide(ctx: &Context<'_>, goal: &Goal<'_>, limits: &Limits) -> Decision {
    decide_and_diagnose(ctx, goal, limits).0
}

/// [`decide`], and where the obligation left the fragment on the way.
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
        // A later `requires` is only reached when the earlier ones held, so it may lean on them —
        // exactly as the evaluator does, which stops at the first clause that answers `false`.
        lowering.assume(lowered);
        guards.push(lowered);
    }
    // Everything after this point is evaluated under a guard that already held; everything before
    // it is evaluated to decide whether it holds, so it may not assume it.
    let guard_requirements = lowering.requirement_mark();
    let definition = goal.result.as_ref().and_then(|(name, body)| {
        let value = lowering.lower(body);
        let symbol = bound.iter().find(|(n, _)| n == name)?.1;
        Some((symbol, value))
    });
    let body = lowering.lower(goal.body);
    let blockers = lowering.blockers().to_vec();
    let requirements = lowering.requirements().to_vec();
    let unsupported = lowering.unsupported();
    let mut terms = lowering.finish();

    // The `Float` refusal, and it is deliberately here rather than at the end: no solver runs, so
    // no `Proof` is constructed, so no `Certificate` can be built out of one.
    if unsupported || float_in(ctx, &terms) {
        let mut blockers = blockers;
        if !blockers.contains(&Blocker::FloatTerm) {
            blockers.push(Blocker::FloatTerm);
        }
        return (
            Decision::Unknown {
                reason: Reason::Open,
                steps: 0,
            },
            blockers,
        );
    }

    let result_symbol = definition.map(|(symbol, _)| symbol);
    let definition = definition.map(|(symbol, value)| terms.eq(symbol, value));

    let mut spent = 0u32;
    let mut budget = limits.steps;
    let guarded: Vec<(term::TermId, bool)> = guards.iter().map(|g| (*g, true)).collect();
    let ranges = int_ranges(&mut terms, result_symbol);

    // The guard is evaluated before anything knows whether it holds, so a guard that raises at some
    // input has no domain to speak of and the obligation is not decided here at any tier.
    let (guard_needs, body_needs) = requirements.split_at(guard_requirements);
    if let Some(conjoined) = conjunction(&mut terms, guard_needs) {
        let mut assertions = ranges.clone();
        assertions.push((conjoined, false));
        let (answer, left) = run(&mut terms, ctx, &mut rules, budget, limits, &assertions);
        spent += budget - left;
        budget = left;
        if let Some(reason) = inconclusive(answer) {
            return (
                Decision::Unknown {
                    reason,
                    steps: spent,
                },
                blockers,
            );
        }
    }

    // Vacuity first, and over the guard **alone**.
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
        return (
            Decision::Unknown {
                reason,
                steps: spent,
            },
            blockers,
        );
    }

    let guard_satisfiable = domain_inhabited(ctx, goal.binders)
        && match conjunction(&mut terms, &guards) {
            None => true,
            // A guard that holds of *every* input admits one, given the domain is not empty.
            Some(all) => {
                let mut ignored = RuleLog::default();
                let (answer, left) = run(
                    &mut terms,
                    ctx,
                    &mut ignored,
                    budget,
                    limits,
                    &[(all, false)],
                );
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

/// Whether any term in the graph has a sort mentioning `Float`.
fn float_in(ctx: &Context<'_>, terms: &term::Terms) -> bool {
    (0..terms.len()).any(|t| terms.sort(t).is_some_and(|s| ctx.reaches_float(s)))
}

/// Why an attempt stopped, or `None` when it closed.
fn inconclusive(answer: solve::Answer) -> Option<Reason> {
    match answer {
        solve::Answer::Closed => None,
        solve::Answer::Open => Some(Reason::Open),
        solve::Answer::Exhausted => Some(Reason::BudgetSpent),
    }
}

/// `MIN ≤ t ≤ MAX` for every term that denotes a Ply `Int` whenever it is defined.
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

/// Every term reachable from `root`, by one pass in construction order: the interner never builds a
/// node before its children, so a parent's flag is settled by the time it is read.
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
        | term::Node::Decimal { .. }
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

/// The heads [`int_ranges`] declines to call atoms.
///
/// The bit operators are here for the same reason `(/)` is rather than the
/// reason `(+)` is: a shift is a value only where its count is a bit position,
/// so `MIN <= x << n <= MAX` is a theorem about the shifts that have an answer
/// and not about every one that can be written. `(&)`, `(|)`, `(^)` and `(~)`
/// do have the unconditional width and lose a little reach by sitting here,
/// which is the price of one rule over an operator set instead of a rule with
/// four exceptions in it.
fn is_operator_symbol(name: &str) -> bool {
    matches!(
        name,
        term::ADD
            | term::SUB
            | term::MUL
            | term::DIV
            | term::REM
            | term::CONCAT
            | term::BIT_AND
            | term::BIT_OR
            | term::BIT_XOR
            | term::BIT_NOT
            | term::SHL
            | term::SHR
            | term::USHR
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

/// Whether every binder's type has at least one value.
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

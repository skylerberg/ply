//! Induction over an `Int` binder, for a claim about self-recursive definitions.
//!
//! Each recursive definition the claim reaches is first shown to terminate: some `Int` parameter
//! is non-negative and strictly smaller at every self call, under the conditions on the way to
//! it. Such a definition is total, so its calls are values and its body may be unrolled with the
//! equation `f(x̄) == body(x̄)` in hand. The claim is then proved for a binder `n` at `n <= 0`, and
//! at `n > 0` from its own instance at `n - 1`, whose recursive calls are left as they are so
//! the unrolled conclusion meets them by congruence.

use super::context::Context;
use super::lower::{Blocker, Lowering, Measure};
use super::term::{self, Node, TermId, is_int_type, list_elem};
use super::{
    Goal, Limits, Proof, RuleLog, conjunction, domain_inhabited, int_ranges, ranges_of, run,
};
use super::{solve, uninterpreted_sorts};
use crate::Rule;
use ply_span::Symbol;
use ply_ty::Type;
use std::collections::BTreeSet;

pub(super) fn attempt(
    ctx: &Context<'_>,
    goal: &Goal<'_>,
    limits: &Limits,
    budget: u32,
    spent: u32,
    blockers: &mut Vec<Blocker>,
) -> Option<Proof> {
    let recursive: BTreeSet<Symbol> = blockers
        .iter()
        .filter_map(|b| match b {
            Blocker::RecursiveCall(name) => Some(name.clone()),
            _ => None,
        })
        .collect();
    if recursive.is_empty() {
        return None;
    }
    let mut budget = budget;
    let mut spent = spent;
    for name in &recursive {
        let Some((terminates, used)) = terminating(ctx, name, limits, budget) else {
            blockers.push(Blocker::Induction(format!(
                "`{name}` is not a pure definition calling only itself"
            )));
            return None;
        };
        spent += used;
        budget -= used;
        if !terminates {
            blockers.push(Blocker::Induction(format!(
                "no `Int` argument of `{name}` is non-negative and smaller at every self call"
            )));
            return None;
        }
    }
    let result_slot = goal.result.map(|_| goal.binders.len().saturating_sub(1));
    for (slot, binder) in goal.binders.iter().enumerate() {
        let over_list = list_elem(&binder.ty).is_some();
        if !(is_int_type(&binder.ty) || over_list) || Some(slot) == result_slot {
            continue;
        }
        let (proved, used) = if over_list {
            induct_list(ctx, goal, limits, budget, &recursive, slot, blockers)
        } else {
            induct_on(ctx, goal, limits, budget, &recursive, slot, blockers)
        };
        spent += used;
        budget -= used;
        if let Some(mut proof) = proved {
            proof.steps = spent;
            for def in &recursive {
                proof.rules.push(Rule::Induction {
                    binder: binder.name.clone(),
                    def: def.clone(),
                });
            }
            return Some(proof);
        }
        if budget == 0 {
            return None;
        }
    }
    None
}

/// Whether `name` terminates by some `Int` parameter decreasing at every self call, and the budget
/// it took to decide; `None` when the definition is not one induction unrolls.
fn terminating(
    ctx: &Context<'_>,
    name: &Symbol,
    limits: &Limits,
    budget: u32,
) -> Option<(bool, u32)> {
    let def = ctx.self_recursive(name)?;
    let Type::Fn { params, .. } = &ctx.scheme(name)?.ty else {
        return None;
    };
    if params.len() != def.params {
        return None;
    }
    let mut used = 0u32;
    for (slot, ty) in params.iter().enumerate() {
        if !(is_int_type(ty) || list_elem(ty).is_some()) || budget <= used {
            continue;
        }
        let mut rules = RuleLog::default();
        let mut lowering = Lowering::new(ctx, &mut rules, limits.unfold_depth);
        let bound: Vec<TermId> = params.iter().map(|ty| lowering.bind_symbolic(ty)).collect();
        lowering.set_measure(Measure {
            def: name.clone(),
            slot,
            bound: bound[slot],
        });
        lowering.lower_root(&def.body, &bound);
        // Only the measure: whether the body raises is judged where it is unrolled.
        let measures = lowering.measures().to_vec();
        if lowering.unsupported() || measures.is_empty() {
            return Some((false, used));
        }
        let mut terms = lowering.finish();
        let Some(all) = conjunction(&mut terms, &measures) else {
            continue;
        };
        let mut assertions = int_ranges(&mut terms, None, &BTreeSet::new());
        assertions.push((all, false));
        let (answer, left) = run(
            &mut terms,
            ctx,
            &mut rules,
            budget - used,
            limits,
            &assertions,
        );
        used += budget - used - left;
        if answer == solve::Answer::Closed {
            return Some((true, used));
        }
    }
    Some((false, used))
}

/// The claim at `n <= 0`, then at `n > 0` from its instance at `n - 1`.
fn induct_on(
    ctx: &Context<'_>,
    goal: &Goal<'_>,
    limits: &Limits,
    budget: u32,
    recursive: &BTreeSet<Symbol>,
    slot: usize,
    blockers: &mut Vec<Blocker>,
) -> (Option<Proof>, u32) {
    let binder = goal.binders[slot].name.clone();
    let declined = |blockers: &mut Vec<Blocker>, what: &str| {
        blockers.push(Blocker::Induction(format!("on `{binder}`: {what}")));
    };
    let mut rules = RuleLog::default();
    let mut lowering = Lowering::new(ctx, &mut rules, limits.unfold_depth);
    lowering.set_total(recursive.clone());
    lowering.set_unrolling(recursive.clone());
    let bound: Vec<TermId> = goal
        .binders
        .iter()
        .map(|binder| lowering.bind_symbolic(&binder.ty))
        .collect();

    let mut guards: Vec<TermId> = Vec::with_capacity(goal.guards.len());
    for guard in goal.guards {
        let lowered = lowering.lower_root(guard, &bound);
        lowering.assume(lowered);
        guards.push(lowered);
    }
    let guard_mark = lowering.requirement_mark();
    let definition = goal.result.and_then(|body| {
        let (&symbol, parameters) = bound.split_last()?;
        Some((symbol, lowering.lower_root(body, parameters)))
    });
    let body = lowering.lower_root(goal.body, &bound);
    let body_mark = lowering.requirement_mark();
    let claim_calls = lowering.calls().to_vec();

    // The hypothesis: the same claim one step down, its recursive calls left unrolled.
    lowering.drop_assumptions();
    lowering.set_unrolling(BTreeSet::new());
    let one = lowering.terms.int_lit(1);
    let Some(previous) = lowering.terms.sub(bound[slot], one) else {
        declined(blockers, "no term for the step down");
        return (None, 0);
    };
    let mut prior = bound.clone();
    prior[slot] = previous;
    let prior_guards: Vec<TermId> = goal
        .guards
        .iter()
        .map(|guard| lowering.lower_root(guard, &prior))
        .collect();
    if let Some(body) = goal.result
        && !prior.is_empty()
    {
        let value = lowering.lower_root(body, &prior[..prior.len() - 1]);
        if let Some(last) = prior.last_mut() {
            *last = value;
        }
    }
    let prior_body = lowering.lower_root(goal.body, &prior);
    let hypothesis_needs = lowering.requirements_since(body_mark).to_vec();
    // The hypothesis makes values of the calls it reaches; a call it does not reach, and that no
    // equation defines, is one this induction cannot speak for.
    let covered: BTreeSet<TermId> = lowering.calls()[claim_calls.len()..]
        .iter()
        .copied()
        .collect();
    let defined: BTreeSet<TermId> = lowering.defined().iter().copied().collect();
    if claim_calls
        .iter()
        .any(|c| !covered.contains(c) && !defined.contains(c))
    {
        declined(
            blockers,
            "a remaining call is not one the hypothesis reaches",
        );
        return (None, 0);
    }

    let requirements = lowering.requirements().to_vec();
    let equations = lowering.equations().to_vec();
    if lowering.unsupported() {
        declined(blockers, "a Float");
        return (None, 0);
    }
    let mut terms = lowering.finish();

    let result_symbol = definition.map(|(symbol, _)| symbol);
    let definition = definition.map(|(symbol, value)| terms.eq(symbol, value));
    let in_question: BTreeSet<TermId> = covered.iter().chain(&defined).copied().collect();
    let ranges = int_ranges(&mut terms, result_symbol, &in_question);
    let (guard_needs, body_needs) = requirements.split_at(guard_mark);
    let body_needs = &body_needs[..body_mark - guard_mark];

    let mut spent = 0u32;
    let mut budget = budget;

    if let Some(conjoined) = conjunction(&mut terms, guard_needs) {
        let mut assertions = ranges.clone();
        assertions.push((conjoined, false));
        if !settle(
            &mut terms,
            ctx,
            &mut rules,
            limits,
            &mut budget,
            &mut spent,
            &assertions,
        ) {
            declined(blockers, "the guard can raise");
            return (None, spent);
        }
    }

    let claim = match conjunction(&mut terms, body_needs) {
        Some(conjoined) => terms.mk(term::Node::And(body, conjoined), Some(Type::bool())),
        None => body,
    };
    let hypothesis = {
        let held = match conjunction(&mut terms, &hypothesis_needs) {
            Some(needs) => terms.mk(term::Node::And(prior_body, needs), Some(Type::bool())),
            None => prior_body,
        };
        match conjunction(&mut terms, &prior_guards) {
            Some(guard) => {
                let unguarded = terms.not(guard);
                terms.mk(term::Node::Or(unguarded, held), Some(Type::bool()))
            }
            None => held,
        }
    };
    let zero = terms.int_lit(0);
    let base = terms.mk(
        term::Node::Cmp {
            op: term::CmpOp::Le,
            lhs: bound[slot],
            rhs: zero,
        },
        Some(Type::bool()),
    );
    let step = terms.mk(
        term::Node::Cmp {
            op: term::CmpOp::Gt,
            lhs: bound[slot],
            rhs: zero,
        },
        Some(Type::bool()),
    );

    let mut common = ranges.clone();
    common.extend(guards.iter().map(|g| (*g, true)));
    common.extend(equations.iter().map(|e| (*e, true)));
    common.extend(definition.map(|d| (d, true)));

    let mut assertions = common.clone();
    assertions.push((base, true));
    assertions.push((claim, false));
    if !settle(
        &mut terms,
        ctx,
        &mut rules,
        limits,
        &mut budget,
        &mut spent,
        &assertions,
    ) {
        declined(
            blockers,
            &format!("the base case is open{}", ran_out(budget)),
        );
        return (None, spent);
    }
    let mut assertions = common;
    assertions.push((step, true));
    assertions.push((hypothesis, true));
    assertions.extend(ranges_of(&mut terms, covered.iter().copied()));
    assertions.push((claim, false));
    if !settle(
        &mut terms,
        ctx,
        &mut rules,
        limits,
        &mut budget,
        &mut spent,
        &assertions,
    ) {
        declined(blockers, &format!("the step is open{}", ran_out(budget)));
        return (None, spent);
    }

    let guard_satisfiable = domain_inhabited(ctx, goal.binders)
        && match conjunction(&mut terms, &guards) {
            None => true,
            Some(all) => {
                let mut ignored = RuleLog::default();
                settle(
                    &mut terms,
                    ctx,
                    &mut ignored,
                    limits,
                    &mut budget,
                    &mut spent,
                    &[(all, false)],
                )
            }
        };
    (
        Some(Proof {
            rules: rules.into_rules(),
            steps: spent,
            sorts: uninterpreted_sorts(ctx, goal.binders),
            guard_satisfiable,
        }),
        spent,
    )
}

/// The claim at `[]`, then at `[h, ..t]` from its instance at `t`. Each instance is its own
/// lowering, so `len` and the matches in reach reduce over the spine they are given.
fn induct_list(
    ctx: &Context<'_>,
    goal: &Goal<'_>,
    limits: &Limits,
    budget: u32,
    recursive: &BTreeSet<Symbol>,
    slot: usize,
    blockers: &mut Vec<Blocker>,
) -> (Option<Proof>, u32) {
    let binder = goal.binders[slot].name.clone();
    let declined = |blockers: &mut Vec<Blocker>, what: &str| {
        blockers.push(Blocker::Induction(format!("on `{binder}`: {what}")));
    };
    if goal.result.is_some() {
        declined(
            blockers,
            "a clause with a `result` is not inducted over a list",
        );
        return (None, 0);
    }
    let mut rules = RuleLog::default();
    let mut lowering = Lowering::new(ctx, &mut rules, limits.unfold_depth);
    lowering.set_total(recursive.clone());
    lowering.set_unrolling(recursive.clone());
    let bound: Vec<TermId> = goal
        .binders
        .iter()
        .map(|binder| lowering.bind_symbolic(&binder.ty))
        .collect();
    let sort = goal.binders[slot].ty.clone();
    let elem = list_elem(&sort).cloned();
    let nil = lowering.terms.nil(Some(sort.clone()));
    let head = lowering.terms.sym(elem);
    let tail = lowering.terms.sym(Some(sort.clone()));
    let cons = lowering.terms.cons(head, tail, Some(sort));

    let base = instance(&mut lowering, goal, &bound, slot, nil);
    let step = instance(&mut lowering, goal, &bound, slot, cons);
    let claim_calls = lowering.calls().to_vec();
    // A remaining call over a tail the patterns took off the instance is one the hypothesis speaks for.
    let structural: BTreeSet<TermId> = claim_calls
        .iter()
        .copied()
        .filter(|c| match lowering.terms.node(*c) {
            Node::App { args, .. } => args
                .iter()
                .any(|a| lowering.smaller_than(*a, cons) || lowering.smaller_than(*a, nil)),
            _ => false,
        })
        .collect();

    lowering.set_unrolling(BTreeSet::new());
    let hyp = instance(&mut lowering, goal, &bound, slot, tail);
    let covered: BTreeSet<TermId> = lowering.calls()[claim_calls.len()..]
        .iter()
        .copied()
        .collect();
    let defined: BTreeSet<TermId> = lowering.defined().iter().copied().collect();
    if claim_calls
        .iter()
        .any(|c| !covered.contains(c) && !defined.contains(c) && !structural.contains(c))
    {
        declined(
            blockers,
            "a remaining call is not one the hypothesis reaches",
        );
        return (None, 0);
    }
    let equations = lowering.equations().to_vec();
    if lowering.unsupported() {
        declined(blockers, "a Float");
        return (None, 0);
    }
    let mut terms = lowering.finish();
    let in_question: BTreeSet<TermId> = covered
        .iter()
        .chain(&defined)
        .chain(&structural)
        .copied()
        .collect();
    let ranges = int_ranges(&mut terms, None, &in_question);

    let mut spent = 0u32;
    let mut budget = budget;
    for case in [&base, &step] {
        if let Some(conjoined) = conjunction(&mut terms, &case.guard_needs) {
            let mut assertions = ranges.clone();
            assertions.push((conjoined, false));
            if !settle(
                &mut terms,
                ctx,
                &mut rules,
                limits,
                &mut budget,
                &mut spent,
                &assertions,
            ) {
                declined(blockers, "the guard can raise");
                return (None, spent);
            }
        }
    }
    let base_claim = base.claim(&mut terms);
    let step_claim = step.claim(&mut terms);
    let hypothesis = {
        let held = hyp.claim(&mut terms);
        match conjunction(&mut terms, &hyp.guards) {
            Some(guard) => {
                let unguarded = terms.not(guard);
                terms.mk(term::Node::Or(unguarded, held), Some(Type::bool()))
            }
            None => held,
        }
    };
    let mut common = ranges;
    common.extend(equations.iter().map(|e| (*e, true)));

    let mut assertions = common.clone();
    assertions.extend(base.guards.iter().map(|g| (*g, true)));
    assertions.push((base_claim, false));
    if !settle(
        &mut terms,
        ctx,
        &mut rules,
        limits,
        &mut budget,
        &mut spent,
        &assertions,
    ) {
        declined(
            blockers,
            &format!("the base case is open{}", ran_out(budget)),
        );
        return (None, spent);
    }
    let mut assertions = common;
    assertions.extend(step.guards.iter().map(|g| (*g, true)));
    assertions.push((hypothesis, true));
    assertions.extend(ranges_of(&mut terms, covered.iter().copied()));
    assertions.push((step_claim, false));
    if !settle(
        &mut terms,
        ctx,
        &mut rules,
        limits,
        &mut budget,
        &mut spent,
        &assertions,
    ) {
        declined(blockers, &format!("the step is open{}", ran_out(budget)));
        return (None, spent);
    }

    let guard_satisfiable = domain_inhabited(ctx, goal.binders)
        && match conjunction(&mut terms, &step.guards) {
            None => true,
            Some(all) => {
                let mut ignored = RuleLog::default();
                settle(
                    &mut terms,
                    ctx,
                    &mut ignored,
                    limits,
                    &mut budget,
                    &mut spent,
                    &[(all, false)],
                )
            }
        };
    (
        Some(Proof {
            rules: rules.into_rules(),
            steps: spent,
            sorts: uninterpreted_sorts(ctx, goal.binders),
            guard_satisfiable,
        }),
        spent,
    )
}

/// The claim lowered with the binder at `at`: its guards, what they owe, its body and what it owes.
struct Instance {
    guards: Vec<TermId>,
    guard_needs: Vec<TermId>,
    body: TermId,
    needs: Vec<TermId>,
}

impl Instance {
    fn claim(&self, terms: &mut term::Terms) -> TermId {
        match conjunction(terms, &self.needs) {
            Some(needs) => terms.mk(term::Node::And(self.body, needs), Some(Type::bool())),
            None => self.body,
        }
    }
}

fn instance(
    lowering: &mut Lowering<'_, '_>,
    goal: &Goal<'_>,
    bound: &[TermId],
    slot: usize,
    at: TermId,
) -> Instance {
    let mut bound = bound.to_vec();
    bound[slot] = at;
    lowering.drop_assumptions();
    let before = lowering.requirement_mark();
    let guards: Vec<TermId> = goal
        .guards
        .iter()
        .map(|guard| {
            let lowered = lowering.lower_root(guard, &bound);
            lowering.assume(lowered);
            lowered
        })
        .collect();
    let guard_mark = lowering.requirement_mark();
    let body = lowering.lower_root(goal.body, &bound);
    let after = lowering.requirement_mark();
    let requirements = lowering.requirements();
    Instance {
        guards,
        guard_needs: requirements[before..guard_mark].to_vec(),
        body,
        needs: requirements[guard_mark..after].to_vec(),
    }
}

/// One refutation on the shared budget; `true` when the assertions are contradictory.
fn settle(
    terms: &mut term::Terms,
    ctx: &Context<'_>,
    rules: &mut RuleLog,
    limits: &Limits,
    budget: &mut u32,
    spent: &mut u32,
    assertions: &[(TermId, bool)],
) -> bool {
    let (answer, left) = run(terms, ctx, rules, *budget, limits, assertions);
    *spent += *budget - left;
    *budget = left;
    answer == solve::Answer::Closed
}

fn ran_out(budget: u32) -> &'static str {
    if budget == 0 {
        ", the budget spent"
    } else {
        ""
    }
}

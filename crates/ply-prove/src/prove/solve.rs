//! The decision procedure: case analysis over a saturating congruence closure, with linear integer
//! arithmetic at every leaf.

use super::context::Context;
use super::egraph::{Classes, conflict, shape_of};
use super::term::{Arm, ArmTest, CmpOp, Node, Poly, TermId, Terms};
use super::{RuleLog, arith};
use crate::Rule;
use ply_core::Type;
use ply_span::Symbol;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    /// Every branch contradicted.
    Closed,
    /// A branch survived.
    Open,
    /// The step budget or the split depth ran out.
    Exhausted,
}

#[derive(Clone)]
struct Branch {
    classes: Classes,
    /// `a < b` over `Int`, from a disequality split.
    int_lt: Vec<(TermId, TermId)>,
    /// Disequalities already split, so a branch cannot split one twice.
    split: BTreeSet<(TermId, TermId)>,
}

enum Split {
    Boolean(TermId),
    Constructor {
        scrutinee: TermId,
        type_name: Symbol,
        ctors: Vec<(Symbol, Vec<Option<Type>>)>,
    },
    Literal(TermId, TermId),
    Disequality(TermId, TermId),
}

pub struct Solver<'a, 'p> {
    pub terms: &'a mut Terms,
    ctx: &'a Context<'p>,
    rules: &'a mut RuleLog,
    budget: u32,
    split_depth: u32,
}

impl<'a, 'p> Solver<'a, 'p> {
    pub fn new(
        terms: &'a mut Terms,
        ctx: &'a Context<'p>,
        rules: &'a mut RuleLog,
        budget: u32,
        split_depth: u32,
    ) -> Solver<'a, 'p> {
        Solver {
            terms,
            ctx,
            rules,
            budget,
            split_depth,
        }
    }

    pub fn budget(&self) -> u32 {
        self.budget
    }

    /// `Closed` iff the assertions are contradictory — which is what makes the goal they negate
    /// valid.
    pub fn refute(&mut self, assertions: &[(TermId, bool)]) -> Answer {
        let mut branch = Branch {
            classes: Classes::new(self.terms.len()),
            int_lt: Vec::new(),
            split: BTreeSet::new(),
        };
        for (term, polarity) in assertions {
            let value = if *polarity {
                self.terms.true_id
            } else {
                self.terms.false_id
            };
            branch.classes.union(*term, value);
        }
        self.explore(branch, 0)
    }

    fn explore(&mut self, mut branch: Branch, depth: u32) -> Answer {
        match self.saturate(&mut branch) {
            Answer::Closed => return Answer::Closed,
            Answer::Exhausted => return Answer::Exhausted,
            Answer::Open => {}
        }
        let Some(split) = self.choose_split(&branch) else {
            return Answer::Open;
        };
        if depth >= self.split_depth || !self.charge(1) {
            return Answer::Exhausted;
        }
        self.branch_on(split, branch, depth)
    }

    fn branch_on(&mut self, split: Split, branch: Branch, depth: u32) -> Answer {
        let mut exhausted = false;
        let mut children: Vec<Branch> = Vec::new();

        match split {
            Split::Boolean(term) => {
                self.rules.note(Rule::Propositional);
                for value in [self.terms.true_id, self.terms.false_id] {
                    let mut child = branch.clone();
                    child.classes.union(term, value);
                    children.push(child);
                }
            }
            Split::Constructor {
                scrutinee,
                type_name,
                ctors,
            } => {
                self.rules.note(Rule::CaseSplit {
                    ty: type_name,
                    arms: ctors.len() as u32,
                });
                for (name, fields) in ctors {
                    let args: Vec<TermId> = fields
                        .into_iter()
                        .map(|sort| self.terms.sym(sort))
                        .collect();
                    let sort = self.terms.sort(scrutinee).cloned();
                    let applied = self.terms.mk(Node::Ctor { name, args }, sort);
                    let mut child = branch.clone();
                    child.classes.grow(self.terms.len());
                    child.classes.union(scrutinee, applied);
                    children.push(child);
                }
            }
            Split::Literal(scrutinee, literal) => {
                self.rules.note(Rule::Propositional);
                let mut equal = branch.clone();
                equal.classes.union(scrutinee, literal);
                let mut distinct = branch.clone();
                distinct.classes.distinguish(scrutinee, literal);
                children.push(equal);
                children.push(distinct);
            }
            Split::Disequality(a, b) => {
                self.rules.note(Rule::LinearArithmetic);
                for (lo, hi) in [(a, b), (b, a)] {
                    let mut child = branch.clone();
                    child.split.insert((a.min(b), a.max(b)));
                    child.int_lt.push((lo, hi));
                    children.push(child);
                }
            }
        }

        for child in children {
            match self.explore(child, depth + 1) {
                Answer::Open => return Answer::Open,
                Answer::Exhausted => exhausted = true,
                Answer::Closed => {}
            }
        }
        if exhausted {
            Answer::Exhausted
        } else {
            Answer::Closed
        }
    }

    fn saturate(&mut self, branch: &mut Branch) -> Answer {
        loop {
            if !self.charge(1) {
                return Answer::Exhausted;
            }
            branch.classes.grow(self.terms.len());
            let mut changed = self.congruence(branch);
            if branch.classes.contradiction {
                return Answer::Closed;
            }
            changed |= self.propagate(branch);
            branch.classes.check_diseqs();
            if branch.classes.contradiction {
                // Two terms asserted distinct were proved equal, which is what the closure is for.
                self.rules.note(Rule::Congruence);
                return Answer::Closed;
            }
            if !changed {
                break;
            }
        }
        match self.arithmetic(branch) {
            arith::Feasibility::Infeasible => {
                self.rules.note(Rule::LinearArithmetic);
                Answer::Closed
            }
            arith::Feasibility::Unknown if self.budget == 0 => Answer::Exhausted,
            arith::Feasibility::Unknown => Answer::Open,
        }
    }

    /// Merges every pair of terms whose arguments are already equal, then applies the two rules a
    /// constructor obeys within each class.
    fn congruence(&mut self, branch: &mut Branch) -> bool {
        let n = self.terms.len();
        let mut signatures: HashMap<Node, TermId> = HashMap::with_capacity(n);
        let mut changed = false;
        for term in 0..n {
            // A term whose signature does not exist is one no congruence may merge.
            let Some(signature) = self.canonical(branch, term) else {
                continue;
            };
            match signatures.get(&signature) {
                Some(&other) => {
                    if branch.classes.union(term, other) {
                        changed = true;
                        self.rules.note(Rule::Congruence);
                    }
                }
                None => {
                    signatures.insert(signature, term);
                }
            }
        }
        changed |= self.structural(branch, n);
        changed
    }

    /// A node with every argument replaced by its class representative.
    fn canonical(&self, branch: &Branch, term: TermId) -> Option<Node> {
        let find = |t: TermId| branch.classes.find(t);
        Some(match self.terms.node(term) {
            Node::Lin(poly) => {
                let mut monomials: BTreeMap<TermId, i128> = BTreeMap::new();
                for (t, c) in &poly.monomials {
                    let slot = monomials.entry(find(*t)).or_insert(0);
                    *slot = slot.checked_add(*c)?;
                }
                monomials.retain(|_, c| *c != 0);
                Node::Lin(Poly {
                    monomials: monomials.into_iter().collect(),
                    konst: poly.konst,
                })
            }
            Node::App { head, args } => Node::App {
                head: find(*head),
                args: args.iter().map(|a| find(*a)).collect(),
            },
            Node::Ctor { name, args } => Node::Ctor {
                name: name.clone(),
                args: args.iter().map(|a| find(*a)).collect(),
            },
            Node::List(items) => Node::List(items.iter().map(|i| find(*i)).collect()),
            Node::Record(fields) => {
                Node::Record(fields.iter().map(|(n, v)| (n.clone(), find(*v))).collect())
            }
            Node::Field { base, field } => Node::Field {
                base: find(*base),
                field: field.clone(),
            },
            Node::Not(a) => Node::Not(find(*a)),
            Node::And(a, b) => Node::And(find(*a), find(*b)),
            Node::Or(a, b) => Node::Or(find(*a), find(*b)),
            Node::Cmp { op, lhs, rhs } => Node::Cmp {
                op: *op,
                lhs: find(*lhs),
                rhs: find(*rhs),
            },
            Node::Eq { lhs, rhs } => {
                let (a, b) = (find(*lhs), find(*rhs));
                Node::Eq {
                    lhs: a.min(b),
                    rhs: a.max(b),
                }
            }
            Node::If {
                cond,
                then_branch,
                else_branch,
            } => Node::If {
                cond: find(*cond),
                then_branch: find(*then_branch),
                else_branch: find(*else_branch),
            },
            Node::Match { scrutinee, arms } => Node::Match {
                scrutinee: find(*scrutinee),
                arms: arms
                    .iter()
                    .map(|arm| Arm {
                        test: arm.test.clone(),
                        binds: arm.binds.iter().map(|b| find(*b)).collect(),
                        body: find(arm.body),
                    })
                    .collect(),
            },
            leaf => leaf.clone(),
        })
    }

    /// Constructor injectivity and disjointness, and the same for list literals and record
    /// literals.
    fn structural(&mut self, branch: &mut Branch, n: usize) -> bool {
        let mut changed = false;
        for (_, members) in branch.classes.groups(n) {
            let mut witness: Option<TermId> = None;
            for term in members {
                let Some(shape) = shape_of(self.terms, term) else {
                    continue;
                };
                let Some(previous) = witness else {
                    witness = Some(term);
                    continue;
                };
                let Some(earlier) = shape_of(self.terms, previous) else {
                    continue;
                };
                match conflict(&earlier, &shape) {
                    Some(true) => {
                        branch.classes.contradiction = true;
                        self.rules.note(Rule::Injectivity);
                        return true;
                    }
                    Some(false) => {
                        if self.unify_arguments(branch, previous, term) {
                            changed = true;
                            self.rules.note(Rule::Injectivity);
                        }
                    }
                    None => {}
                }
            }
        }
        changed
    }

    fn unify_arguments(&self, branch: &mut Branch, a: TermId, b: TermId) -> bool {
        let pairs: Vec<(TermId, TermId)> = match (self.terms.node(a), self.terms.node(b)) {
            (Node::Ctor { args: xs, .. }, Node::Ctor { args: ys, .. })
            | (Node::List(xs), Node::List(ys)) => {
                xs.iter().copied().zip(ys.iter().copied()).collect()
            }
            (Node::Record(xs), Node::Record(ys)) => xs
                .iter()
                .map(|(_, v)| *v)
                .zip(ys.iter().map(|(_, v)| *v))
                .collect(),
            _ => Vec::new(),
        };
        let mut changed = false;
        for (x, y) in pairs {
            changed |= branch.classes.union(x, y);
        }
        changed
    }

    fn propagate(&mut self, branch: &mut Branch) -> bool {
        let mut changed = false;
        for term in 0..self.terms.len() {
            let node = self.terms.node(term).clone();
            changed |= match node {
                Node::Not(a) => {
                    let mut c = false;
                    if let Some(v) = self.truth(branch, a) {
                        c |= self.set(branch, term, !v);
                    }
                    if let Some(v) = self.truth(branch, term) {
                        c |= self.set(branch, a, !v);
                    }
                    if c {
                        self.rules.note(Rule::Propositional);
                    }
                    c
                }
                Node::And(a, b) => self.junction(branch, term, a, b, true),
                Node::Or(a, b) => self.junction(branch, term, a, b, false),
                Node::Eq { lhs, rhs } => self.equality(branch, term, lhs, rhs),
                Node::If {
                    cond,
                    then_branch,
                    else_branch,
                } => match self.truth(branch, cond) {
                    Some(true) => {
                        self.rules.note(Rule::Propositional);
                        branch.classes.union(term, then_branch)
                    }
                    Some(false) => {
                        self.rules.note(Rule::Propositional);
                        branch.classes.union(term, else_branch)
                    }
                    None => false,
                },
                Node::Match { scrutinee, arms } => {
                    self.reduce_match(branch, term, scrutinee, &arms)
                }
                Node::Field { base, field } => match self.record_field(branch, base, &field) {
                    Some(value) => {
                        self.rules.note(Rule::Congruence);
                        branch.classes.union(term, value)
                    }
                    None => false,
                },
                _ => false,
            };
        }
        changed | self.propagate_disequalities(branch)
    }

    /// `a != b` at `Bool` decides one side from the other, because `Bool` has exactly two values.
    fn propagate_disequalities(&mut self, branch: &mut Branch) -> bool {
        let pairs: Vec<(TermId, TermId)> = branch.classes.diseqs().to_vec();
        let mut changed = false;
        for (a, b) in pairs {
            for (x, y) in [(a, b), (b, a)] {
                if !self.is_bool(y) {
                    continue;
                }
                if let Some(value) = self.truth(branch, x) {
                    changed |= self.set(branch, y, !value);
                }
            }
        }
        if changed {
            self.rules.note(Rule::Propositional);
        }
        changed
    }

    fn is_bool(&self, term: TermId) -> bool {
        matches!(self.terms.sort(term), Some(Type::Con(name, args))
            if name.as_str() == "Bool" && args.is_empty())
    }

    /// `&&` when `dominant` is `true`, `||` when it is `false`.
    fn junction(
        &mut self,
        branch: &mut Branch,
        term: TermId,
        a: TermId,
        b: TermId,
        conjunction: bool,
    ) -> bool {
        let short = !conjunction;
        let (left, right) = (self.truth(branch, a), self.truth(branch, b));
        let mut changed = false;
        if left == Some(short) || right == Some(short) {
            changed |= self.set(branch, term, short);
        } else if left == Some(!short) && right == Some(!short) {
            changed |= self.set(branch, term, !short);
        }
        match self.truth(branch, term) {
            Some(v) if v != short => {
                changed |= self.set(branch, a, !short);
                changed |= self.set(branch, b, !short);
            }
            Some(_) => {
                if left == Some(!short) {
                    changed |= self.set(branch, b, short);
                }
                if right == Some(!short) {
                    changed |= self.set(branch, a, short);
                }
            }
            None => {}
        }
        if changed {
            self.rules.note(Rule::Propositional);
        }
        changed
    }

    fn equality(&mut self, branch: &mut Branch, term: TermId, lhs: TermId, rhs: TermId) -> bool {
        // Extensionality does not depend on what this equality was *asserted* to be: it establishes
        // that the two records are the same value, which settles a positive occurrence and
        // contradicts a negative one.
        let mut changed = false;
        if !branch.classes.equal(lhs, rhs) && self.fields_all_equal(branch, lhs, rhs) {
            self.rules.note(Rule::Congruence);
            changed |= branch.classes.union(lhs, rhs);
        }
        changed | self.decide_equality(branch, term, lhs, rhs)
    }

    fn decide_equality(
        &mut self,
        branch: &mut Branch,
        term: TermId,
        lhs: TermId,
        rhs: TermId,
    ) -> bool {
        match self.truth(branch, term) {
            Some(true) => branch.classes.union(lhs, rhs),
            Some(false) => {
                if branch.classes.diseqs().contains(&(lhs, rhs)) {
                    return false;
                }
                branch.classes.distinguish(lhs, rhs);
                true
            }
            None => {
                if branch.classes.equal(lhs, rhs) {
                    self.rules.note(Rule::Congruence);
                    self.set(branch, term, true)
                } else if self.distinct(branch, lhs, rhs) {
                    self.rules.note(Rule::Injectivity);
                    self.set(branch, term, false)
                } else {
                    false
                }
            }
        }
    }

    fn reduce_match(
        &mut self,
        branch: &mut Branch,
        term: TermId,
        scrutinee: TermId,
        arms: &[Arm],
    ) -> bool {
        let Taken::Arm(index) = self.taken_arm(branch, scrutinee, arms) else {
            return false;
        };
        let arm = &arms[index];
        let mut changed = branch.classes.union(term, arm.body);
        if let Some(applied) = self.known_constructor(branch, scrutinee)
            && let Node::Ctor { args, .. } = self.terms.node(applied).clone()
        {
            for (bind, arg) in arm.binds.iter().zip(args) {
                changed |= branch.classes.union(*bind, arg);
            }
        }
        changed
    }

    fn taken_arm(&self, branch: &Branch, scrutinee: TermId, arms: &[Arm]) -> Taken {
        for (index, arm) in arms.iter().enumerate() {
            match &arm.test {
                ArmTest::Always => return Taken::Arm(index),
                ArmTest::Ctor(wanted) => match self.known_constructor(branch, scrutinee) {
                    Some(applied) => match self.terms.node(applied) {
                        Node::Ctor { name, .. } if name == wanted => return Taken::Arm(index),
                        _ => continue,
                    },
                    None => return Taken::NeedsConstructor,
                },
                ArmTest::Lit(literal) => {
                    if branch.classes.equal(scrutinee, *literal) {
                        return Taken::Arm(index);
                    }
                    if self.distinct(branch, scrutinee, *literal) {
                        continue;
                    }
                    return Taken::NeedsLiteral(*literal);
                }
                ArmTest::Undecidable => return Taken::Undecidable,
            }
        }
        Taken::Undecidable
    }

    fn known_constructor(&self, branch: &Branch, term: TermId) -> Option<TermId> {
        let rep = branch.classes.find(term);
        (0..self.terms.len()).find(|&t| {
            branch.classes.find(t) == rep && matches!(self.terms.node(t), Node::Ctor { .. })
        })
    }

    /// Record extensionality: two records of one type are equal when every field is.
    fn fields_all_equal(&self, branch: &Branch, a: TermId, b: TermId) -> bool {
        let (Some(Type::Record(left)), Some(Type::Record(right))) =
            (self.terms.sort(a), self.terms.sort(b))
        else {
            return false;
        };
        if left.is_empty() || left.keys().ne(right.keys()) {
            return false;
        }
        left.keys().all(|name| {
            match (
                self.projection(branch, a, name),
                self.projection(branch, b, name),
            ) {
                (Some(x), Some(y)) => branch.classes.equal(x, y),
                _ => false,
            }
        })
    }

    /// The term standing for `base.field`: the field of a record literal in `base`'s class, or an
    /// interned `Field` node over it.
    fn projection(&self, branch: &Branch, base: TermId, field: &Symbol) -> Option<TermId> {
        if let Some(found) = self.record_field(branch, base, field) {
            return Some(found);
        }
        let rep = branch.classes.find(base);
        (0..self.terms.len()).find(|&t| match self.terms.node(t) {
            Node::Field { base: b, field: f } => f == field && branch.classes.find(*b) == rep,
            _ => false,
        })
    }

    fn record_field(&self, branch: &Branch, base: TermId, field: &Symbol) -> Option<TermId> {
        let rep = branch.classes.find(base);
        for term in 0..self.terms.len() {
            if branch.classes.find(term) != rep {
                continue;
            }
            if let Node::Record(fields) = self.terms.node(term)
                && let Some((_, value)) = fields.iter().find(|(n, _)| n == field)
            {
                return Some(*value);
            }
        }
        None
    }

    fn truth(&self, branch: &Branch, term: TermId) -> Option<bool> {
        if branch.classes.equal(term, self.terms.true_id) {
            Some(true)
        } else if branch.classes.equal(term, self.terms.false_id) {
            Some(false)
        } else {
            None
        }
    }

    fn set(&self, branch: &mut Branch, term: TermId, value: bool) -> bool {
        let target = if value {
            self.terms.true_id
        } else {
            self.terms.false_id
        };
        branch.classes.union(term, target)
    }

    /// Whether the two are provably different values.
    fn distinct(&self, branch: &Branch, a: TermId, b: TermId) -> bool {
        let (ra, rb) = (branch.classes.find(a), branch.classes.find(b));
        if ra == rb {
            return false;
        }
        for x in 0..self.terms.len() {
            if branch.classes.find(x) != ra {
                continue;
            }
            let Some(left) = shape_of(self.terms, x) else {
                continue;
            };
            for y in 0..self.terms.len() {
                if branch.classes.find(y) != rb {
                    continue;
                }
                let Some(right) = shape_of(self.terms, y) else {
                    continue;
                };
                if conflict(&left, &right) == Some(true) {
                    return true;
                }
            }
        }
        false
    }

    fn choose_split(&self, branch: &Branch) -> Option<Split> {
        // An `if` or a `match` first: reducing one exposes the structure every other rule works on.
        for term in 0..self.terms.len() {
            match self.terms.node(term).clone() {
                Node::If { cond, .. } if self.truth(branch, cond).is_none() => {
                    return Some(Split::Boolean(cond));
                }
                Node::Match { scrutinee, arms } => match self.taken_arm(branch, scrutinee, &arms) {
                    Taken::NeedsConstructor => {
                        if let Some(split) = self.constructor_split(scrutinee) {
                            return Some(split);
                        }
                    }
                    Taken::NeedsLiteral(literal) => {
                        return Some(Split::Literal(scrutinee, literal));
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        // Then the propositional split: a disjunction that must hold with neither side decided, or
        // a conjunction that must fail.
        for term in 0..self.terms.len() {
            let (a, b, wanted) = match *self.terms.node(term) {
                Node::And(a, b) => (a, b, false),
                Node::Or(a, b) => (a, b, true),
                _ => continue,
            };
            if self.truth(branch, term) == Some(wanted)
                && self.truth(branch, a).is_none()
                && self.truth(branch, b).is_none()
            {
                return Some(Split::Boolean(a));
            }
        }

        // Then an `Int` disequality, which is `<` or `>` and is the only way one reaches the
        // arithmetic at all.
        for (a, b) in branch.classes.diseqs() {
            let key = ((*a).min(*b), (*a).max(*b));
            if branch.split.contains(&key) {
                continue;
            }
            if self.terms.is_int(*a) && self.terms.is_int(*b) {
                return Some(Split::Disequality(*a, *b));
            }
        }

        // Last, a Boolean atom of a formula this branch constrains.
        self.undecided_atom(branch).map(Split::Boolean)
    }

    /// A `Bool`-sorted term that is not itself a connective, is reachable from something this
    /// branch has constrained, and whose truth neither propagation nor an earlier split settled.
    fn undecided_atom(&self, branch: &Branch) -> Option<TermId> {
        let mut stack: Vec<TermId> = (0..self.terms.len())
            .filter(|t| self.truth(branch, *t).is_some())
            .collect();
        for (a, b) in branch.classes.diseqs() {
            stack.push(*a);
            stack.push(*b);
        }

        let mut seen = vec![false; self.terms.len()];
        let mut best: Option<TermId> = None;
        while let Some(term) = stack.pop() {
            if seen[term] {
                continue;
            }
            seen[term] = true;
            match *self.terms.node(term) {
                Node::Not(a) => stack.push(a),
                Node::And(a, b) | Node::Or(a, b) => {
                    stack.push(a);
                    stack.push(b);
                }
                Node::If { cond, .. } => stack.push(cond),
                Node::Eq { lhs, rhs } if self.is_bool(lhs) && self.is_bool(rhs) => {
                    stack.push(lhs);
                    stack.push(rhs);
                }
                _ => {
                    if self.is_bool(term) && self.truth(branch, term).is_none() {
                        best = Some(best.map_or(term, |previous: TermId| previous.min(term)));
                    }
                }
            }
        }
        best
    }

    /// The complete constructor list of the scrutinee's type, with each constructor's field sorts.
    fn constructor_split(&self, scrutinee: TermId) -> Option<Split> {
        let sort = self.terms.sort(scrutinee)?.clone();
        let Type::Con(type_name, _) = &sort else {
            return None;
        };
        let variants = self.ctx.variants(type_name)?;
        let ctors = variants
            .ctors
            .iter()
            .map(|c| (c.name.clone(), super::lower::field_sorts(c, Some(&sort))))
            .collect();
        Some(Split::Constructor {
            scrutinee,
            type_name: variants.type_name,
            ctors,
        })
    }

    /// The linear system this branch asserts.
    fn arithmetic(&mut self, branch: &Branch) -> arith::Feasibility {
        let find = |t: TermId| branch.classes.find(t);
        let mut system = arith::System::default();
        let mut relevant: BTreeSet<TermId> = BTreeSet::new();
        let mut comparisons: Vec<(BTreeMap<TermId, i128>, i128)> = Vec::new();

        for (term, node) in self.terms.nodes() {
            let Node::Cmp { op, lhs, rhs } = node else {
                continue;
            };
            let Some(holds) = self.truth(branch, term) else {
                continue;
            };
            let (a, b, slack) = match (op, holds) {
                (CmpOp::Lt, true) | (CmpOp::Ge, false) => (*lhs, *rhs, 1),
                (CmpOp::Le, true) | (CmpOp::Gt, false) => (*lhs, *rhs, 0),
                (CmpOp::Gt, true) | (CmpOp::Le, false) => (*rhs, *lhs, 1),
                (CmpOp::Ge, true) | (CmpOp::Lt, false) => (*rhs, *lhs, 0),
            };
            comparisons.push((difference(find(a), find(b)), slack));
            relevant.insert(find(a));
            relevant.insert(find(b));
        }
        for (a, b) in &branch.int_lt {
            comparisons.push((difference(find(*a), find(*b)), 1));
            relevant.insert(find(*a));
            relevant.insert(find(*b));
        }
        // Only the definitions a comparison can reach, so an unfolded body full of unrelated
        // literals does not blow the elimination up.
        let mut definitions: Vec<(BTreeMap<TermId, i128>, i128)> = Vec::new();
        for (term, node) in self.terms.nodes() {
            match node {
                Node::Lin(poly) => {
                    let mut coefficients: BTreeMap<TermId, i128> = BTreeMap::new();
                    *coefficients.entry(find(term)).or_insert(0) += 1;
                    let mut ok = true;
                    for (t, c) in &poly.monomials {
                        let slot = coefficients.entry(find(*t)).or_insert(0);
                        match slot.checked_sub(*c) {
                            Some(v) => *slot = v,
                            None => ok = false,
                        }
                    }
                    coefficients.retain(|_, c| *c != 0);
                    if let (true, Some(konst)) = (ok, poly.konst.checked_neg()) {
                        definitions.push((coefficients, konst));
                    }
                }
                Node::Int(k) => {
                    definitions.push((BTreeMap::from([(find(term), 1)]), -(*k as i128)));
                }
                _ => {}
            }
        }

        // A definition whose variables all cancelled is a bare claim about a constant — `x == x +
        // 1` collapses to `1 = 0` — and it reaches nothing, so the closure below would never pick
        // it up.
        let mut used: Vec<bool> = definitions
            .iter()
            .map(|(coefficients, _)| coefficients.is_empty())
            .collect();
        loop {
            let mut grew = false;
            for (index, (coefficients, _)) in definitions.iter().enumerate() {
                if used[index] {
                    continue;
                }
                if coefficients.keys().any(|v| relevant.contains(v)) {
                    used[index] = true;
                    grew = true;
                    for v in coefficients.keys() {
                        relevant.insert(*v);
                    }
                }
            }
            if !grew {
                break;
            }
        }

        for (index, (coefficients, konst)) in definitions.into_iter().enumerate() {
            if used[index] {
                system.eq(coefficients, konst);
            }
        }
        for (coefficients, konst) in comparisons {
            system.leq(coefficients, konst);
        }
        if system.is_empty() {
            return arith::Feasibility::Unknown;
        }
        system.feasibility(&mut self.budget)
    }

    fn charge(&mut self, cost: u32) -> bool {
        match self.budget.checked_sub(cost) {
            Some(left) => {
                self.budget = left;
                true
            }
            None => {
                self.budget = 0;
                false
            }
        }
    }
}

enum Taken {
    Arm(usize),
    NeedsConstructor,
    NeedsLiteral(TermId),
    Undecidable,
}

/// `a - b`, which collapses to nothing when the two are one class.
fn difference(a: TermId, b: TermId) -> BTreeMap<TermId, i128> {
    let mut out: BTreeMap<TermId, i128> = BTreeMap::new();
    *out.entry(a).or_insert(0) += 1;
    *out.entry(b).or_insert(0) -= 1;
    out.retain(|_, c| *c != 0);
    out
}

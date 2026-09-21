//! The port's lowered claims into the prover's terms.

use super::RuleLog;
use super::claims::{self, Code, Pat, Stmt};
use super::context::Context;
use super::term::{self, Arm, ArmTest, CmpOp, Node, TermId, Terms};
use ply_span::Symbol;
use ply_ty::{BinOp, CtorInfo, Lit, Scheme, TyVar, Type, UnOp};
use std::collections::{BTreeMap, BTreeSet};

const MAX_TERMS: usize = 20_000;

/// Prelude functions that cannot raise or diverge, so a call needs no definedness requirement.
const TOTAL_BUILTINS: &[&str] = &[
    "len",
    "push",
    "list_at",
    "int_to_string",
    "min",
    "max",
    "string_concat",
    "bytes_len",
    "bytes_concat",
    "bytes_of_string",
    "bytes_is_utf8",
    "bytes_index_of",
    "bytes_starts_with",
    "bytes_ends_with",
    "string_of_bytes_lossy",
    "string_len",
    "string_trim",
    "string_lower",
    "string_upper",
    "string_starts_with",
    "string_ends_with",
    "string_contains",
    "map_new",
    "map_insert",
    "map_get",
    "map_contains",
    "map_remove",
    "map_len",
    "map_keys",
    "map_values",
    "map_entries",
    "map_of_entries",
    "map_merge",
    "decimal_of_int",
    "decimal_to_string",
    "decimal_of_string",
    "float_of_decimal",
    "decimal_of_float",
    "int_of_decimal",
    "bits_of_float",
    "float_of_bits",
    // Total but still uninterpreted.
    "wrap_add",
    "wrap_sub",
    "wrap_mul",
    "rotr32",
];

/// Where lowering left the decidable fragment.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Blocker {
    RecursiveCall(Symbol),
    /// A call whose row is not known to be empty, so occurrences cannot share a term.
    EffectfulCall(Symbol),
    UnfoldLimit(Symbol),
    /// A builtin, or a name from outside the program.
    OpaqueCall(Symbol),
    Division,
    NonlinearMultiplication,
    CoefficientRange,
    Lambda,
    Concat,
    BitOperator,
    FloatTerm,
    DecimalArithmetic,
    /// `perform`, `handle`, `with_cell` or `simulate`.
    Region,
    UndecidableMatchArm,
    DestructuringLet,
    /// Why induction declined: which definition or case, in words.
    Induction(String),
}

/// A non-`Int` numeric operand type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Numeric {
    Float,
    Decimal,
}

fn is_con(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Con(n, args) if n.as_str() == name && args.is_empty())
}

/// A self call whose argument in `slot` must be non-negative and below `bound`, the parameter it
/// was entered with: what makes a recursive definition terminate.
#[derive(Clone)]
pub struct Measure {
    pub def: Symbol,
    pub slot: usize,
    pub bound: TermId,
}

pub struct Lowering<'a, 'p> {
    pub terms: Terms,
    ctx: &'a Context<'p>,
    rules: &'a mut RuleLog,
    unfold_depth: u32,
    depth: u32,
    window: Vec<Option<TermId>>,
    blockers: Vec<Blocker>,
    /// What must hold for the lowered expressions not to raise or diverge.
    requirements: Vec<TermId>,
    /// Conditions assumed on the way to the expression being lowered.
    path: Vec<TermId>,
    float: bool,
    /// Recursive definitions shown to terminate, so a call of one is a value.
    total: BTreeSet<Symbol>,
    /// Recursive definitions inlined to the depth, each unrolling recorded as an equation.
    unrolled: BTreeSet<Symbol>,
    measure: Option<Measure>,
    /// What each self call under the measure owes: the argument decreases and stays non-negative.
    measures: Vec<TermId>,
    /// `f(x̄) == body(x̄)` for each unrolling of a recursive definition.
    equations: Vec<TermId>,
    /// The `f(x̄)` of each equation.
    defined: Vec<TermId>,
    /// Calls of a total definition left as terms.
    calls: Vec<TermId>,
    /// Each rest binder of a list pattern, with the list it is the tail of.
    smaller: Vec<(TermId, TermId)>,
}

impl<'a, 'p> Lowering<'a, 'p> {
    pub fn new(
        ctx: &'a Context<'p>,
        rules: &'a mut RuleLog,
        unfold_depth: u32,
    ) -> Lowering<'a, 'p> {
        Lowering {
            terms: Terms::new(),
            ctx,
            rules,
            unfold_depth,
            depth: 0,
            window: Vec::new(),
            blockers: Vec::new(),
            requirements: Vec::new(),
            path: Vec::new(),
            float: false,
            total: BTreeSet::new(),
            unrolled: BTreeSet::new(),
            measure: None,
            measures: Vec::new(),
            equations: Vec::new(),
            defined: Vec::new(),
            calls: Vec::new(),
            smaller: Vec::new(),
        }
    }

    pub fn measures(&self) -> &[TermId] {
        &self.measures
    }

    pub fn set_total(&mut self, names: BTreeSet<Symbol>) {
        self.total = names;
    }

    pub fn set_unrolling(&mut self, names: BTreeSet<Symbol>) {
        self.unrolled = names;
    }

    pub fn set_measure(&mut self, measure: Measure) {
        self.measure = Some(measure);
    }

    pub fn equations(&self) -> &[TermId] {
        &self.equations
    }

    pub fn defined(&self) -> &[TermId] {
        &self.defined
    }

    pub fn calls(&self) -> &[TermId] {
        &self.calls
    }

    /// Whether `t` is a tail of `root`, through the list patterns that took it apart.
    pub fn smaller_than(&self, t: TermId, root: TermId) -> bool {
        let mut at = t;
        for _ in 0..=self.smaller.len() {
            let Some((_, from)) = self.smaller.iter().find(|(tail, _)| *tail == at) else {
                return false;
            };
            if *from == root {
                return true;
            }
            at = *from;
        }
        false
    }

    pub fn requirements_since(&self, mark: usize) -> &[TermId] {
        &self.requirements[mark..]
    }

    /// Forgets the conditions assumed so far, before lowering a hypothesis instance.
    pub fn drop_assumptions(&mut self) {
        self.path.clear();
    }

    /// No proof survives a `Float` in the obligation.
    fn float(&mut self) {
        if !self.float {
            self.blocked(Blocker::FloatTerm);
        }
        self.float = true;
    }

    pub fn unsupported(&self) -> bool {
        self.float
    }

    fn blocked(&mut self, blocker: Blocker) {
        self.blockers.push(blocker);
    }

    pub fn blockers(&self) -> &[Blocker] {
        &self.blockers
    }

    pub fn requirements(&self) -> &[TermId] {
        &self.requirements
    }

    /// Separates the requirements a guard owes from the ones its body does.
    pub fn requirement_mark(&self) -> usize {
        self.requirements.len()
    }

    pub fn assume(&mut self, cond: TermId) {
        self.path.push(cond);
    }

    /// Records `p₁ ∧ … ∧ pₙ ⟹ cond` under the current path.
    fn require(&mut self, cond: TermId) {
        if cond == self.terms.true_id {
            return;
        }
        let out = self.under_path(cond);
        self.requirements.push(out);
    }

    fn under_path(&mut self, cond: TermId) -> TermId {
        let mut out = cond;
        for i in (0..self.path.len()).rev() {
            let negated = self.terms.not(self.path[i]);
            out = self.terms.mk(Node::Or(negated, out), Some(Type::bool()));
        }
        out
    }

    fn undefined(&mut self) {
        let never = self.terms.false_id;
        self.require(never);
    }

    fn require_int_range(&mut self, t: TermId) {
        if !matches!(self.terms.node(t), Node::Lin(_)) {
            return;
        }
        let min = self.terms.int_lit(i64::MIN);
        let max = self.terms.int_lit(i64::MAX);
        let low = self.terms.mk(
            Node::Cmp {
                op: CmpOp::Ge,
                lhs: t,
                rhs: min,
            },
            Some(Type::bool()),
        );
        let high = self.terms.mk(
            Node::Cmp {
                op: CmpOp::Le,
                lhs: t,
                rhs: max,
            },
            Some(Type::bool()),
        );
        let both = self.terms.mk(Node::And(low, high), Some(Type::bool()));
        self.require(both);
    }

    /// Zero divisors raise, and `i64::MIN / -1` overflows.
    fn require_divisible(&mut self, lhs: TermId, rhs: TermId) {
        if let Node::Int(k) = *self.terms.node(rhs) {
            if k == 0 {
                self.undefined();
            } else if k == -1 {
                let min = self.terms.int_lit(i64::MIN);
                let is_min = self.terms.eq(lhs, min);
                let not_min = self.terms.not(is_min);
                self.require(not_min);
            }
            return;
        }
        let zero = self.terms.int_lit(0);
        let is_zero = self.terms.eq(rhs, zero);
        let nonzero = self.terms.not(is_zero);
        self.require(nonzero);

        let min = self.terms.int_lit(i64::MIN);
        let minus_one = self.terms.int_lit(-1);
        let lhs_min = self.terms.eq(lhs, min);
        let rhs_minus_one = self.terms.eq(rhs, minus_one);
        let overflows = self
            .terms
            .mk(Node::And(lhs_min, rhs_minus_one), Some(Type::bool()));
        let safe = self.terms.not(overflows);
        self.require(safe);
    }

    /// A shift raises unless its count is a bit position; nothing else makes it raise.
    fn require_shift_count(&mut self, count: TermId) {
        let zero = self.terms.int_lit(0);
        let width = self.terms.int_lit(63);
        let low = self.terms.mk(
            Node::Cmp {
                op: CmpOp::Ge,
                lhs: count,
                rhs: zero,
            },
            Some(Type::bool()),
        );
        let high = self.terms.mk(
            Node::Cmp {
                op: CmpOp::Le,
                lhs: count,
                rhs: width,
            },
            Some(Type::bool()),
        );
        let both = self.terms.mk(Node::And(low, high), Some(Type::bool()));
        self.require(both);
    }

    fn under<T>(&mut self, cond: TermId, f: impl FnOnce(&mut Self) -> T) -> T {
        self.path.push(cond);
        let out = f(self);
        self.path.pop();
        out
    }

    pub fn bind_symbolic(&mut self, ty: &Type) -> TermId {
        if self.ctx.reaches_float(ty) {
            self.float();
        }
        self.terms.sym(Some(ty.clone()))
    }

    pub fn finish(self) -> Terms {
        self.terms
    }

    pub fn lower_root(&mut self, code: &Code, binders: &[TermId]) -> TermId {
        let window = binders.iter().map(|t| Some(*t)).collect();
        self.within(window, |this| this.lower(code))
    }

    fn lower(&mut self, code: &Code) -> TermId {
        // Expressions may nest as deep as the parser accepted.
        stacker::maybe_grow(256 * 1024, 2 * 1024 * 1024, || self.lower_inner(code))
    }

    fn lower_inner(&mut self, code: &Code) -> TermId {
        match code {
            Code::Lit(lit) => self.literal(lit),
            Code::Local(slot) => self.local(*slot),
            Code::Global(name) => self.global(name),
            // Short-circuit: the right operand owes its requirements only under the left's answer.
            Code::Binary(op @ (BinOp::And | BinOp::Or), lhs, rhs) => {
                let l = self.lower(lhs);
                let reached = if *op == BinOp::And {
                    l
                } else {
                    self.terms.not(l)
                };
                let r = self.under(reached, |this| this.lower(rhs));
                self.binary(*op, l, r)
            }
            Code::Binary(op, lhs, rhs) => {
                let l = self.lower(lhs);
                let r = self.lower(rhs);
                self.binary(*op, l, r)
            }
            Code::Unary(op, operand) => {
                let t = self.lower(operand);
                match op {
                    UnOp::Not => self.terms.not(t),
                    // Not folded to `-x - 1`, which raises at `i64::MIN` where `~` does not.
                    UnOp::BitNot => {
                        self.terms.force_int(t);
                        self.blocked(Blocker::BitOperator);
                        let head = self.terms.opaque(term::BIT_NOT, None);
                        let term = self.terms.mk(
                            Node::App {
                                head,
                                args: vec![t],
                            },
                            Some(Type::int()),
                        );
                        self.terms.force_int(term);
                        term
                    }
                    UnOp::Neg => {
                        self.terms.force_int(t);
                        match self.terms.neg(t) {
                            Some(out) => {
                                self.require_int_range(out);
                                out
                            }
                            None => {
                                self.blocked(Blocker::CoefficientRange);
                                self.undefined();
                                self.terms.sym(Some(Type::int()))
                            }
                        }
                    }
                }
            }
            Code::Lambda { .. } => {
                self.blocked(Blocker::Lambda);
                self.terms.sym(None)
            }
            Code::App(func, args) => self.application(func, args),
            Code::If(cond, then_branch, else_branch) => {
                let cond = self.lower(cond);
                let then_branch = self.under(cond, |this| this.lower(then_branch));
                let otherwise = self.terms.not(cond);
                let else_branch = self.under(otherwise, |this| this.lower(else_branch));
                let sort = self
                    .terms
                    .sort(then_branch)
                    .or_else(|| self.terms.sort(else_branch))
                    .cloned();
                self.terms.mk(
                    Node::If {
                        cond,
                        then_branch,
                        else_branch,
                    },
                    sort,
                )
            }
            Code::Match(scrutinee, arms) => self.match_expr(scrutinee, arms),
            Code::Block(stmts, tail) => self.block(stmts, tail.as_deref()),
            Code::Record(fields) => {
                let mut lowered: Vec<(Symbol, TermId)> = fields
                    .iter()
                    .map(|(name, value)| (name.clone(), self.lower(value)))
                    .collect();
                lowered.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
                let sort = self.record_sort(&lowered);
                self.terms.mk(Node::Record(lowered), sort)
            }
            Code::Field(base, field) => {
                let base = self.lower(base);
                self.terms.field(base, field.clone())
            }
            Code::List(items) => {
                let items: Vec<TermId> = items.iter().map(|i| self.lower(i)).collect();
                let sort = items
                    .first()
                    .and_then(|t| self.terms.sort(*t).cloned())
                    .map(Type::list);
                let mut out = self.terms.nil(sort.clone());
                for item in items.into_iter().rev() {
                    out = self.terms.cons(item, out, sort.clone());
                }
                out
            }
            Code::Region => {
                self.blocked(Blocker::Region);
                self.terms.sym(None)
            }
            Code::Unreached => {
                self.blocked(Blocker::UndecidableMatchArm);
                self.terms.sym(None)
            }
        }
    }

    fn literal(&mut self, lit: &Lit) -> TermId {
        match lit {
            Lit::Int(k) => self.terms.int_lit(*k),
            Lit::Bool(b) => self.terms.boolean(*b),
            Lit::Str(s) => self.terms.string(s.clone()),
            // Not `Node::Str`: `b"ab"` and `"ab"` must not be congruent.
            Lit::Bytes(_) => self.terms.sym(Some(Type::bytes())),
            // No shared node: congruence needs a reflexive `==`, which `Float` lacks.
            Lit::Float(_) => {
                self.float();
                self.terms.sym(Some(Type::float()))
            }
            Lit::Decimal { mantissa, scale } => self.terms.decimal(*mantissa, *scale),
            Lit::Fixed { ty, bits } => self.terms.fixed(*ty, *bits),
            Lit::Unit => self.terms.unit(),
        }
    }

    fn local(&mut self, slot: usize) -> TermId {
        match self.window.get(slot).copied().flatten() {
            Some(term) => term,
            None => self.terms.sym(None),
        }
    }

    fn bind(&mut self, slot: usize, term: TermId) {
        if self.window.len() <= slot {
            self.window.resize(slot + 1, None);
        }
        self.window[slot] = Some(term);
    }

    fn within<T>(&mut self, window: Vec<Option<TermId>>, f: impl FnOnce(&mut Self) -> T) -> T {
        let outer = std::mem::replace(&mut self.window, window);
        let out = f(self);
        self.window = outer;
        out
    }

    fn global(&mut self, name: &Symbol) -> TermId {
        if let Some(ctor) = self.ctx.ctor(name) {
            let sort = scheme_sort(&ctor.scheme);
            if ctor.arity == 0 {
                return self.terms.mk(
                    Node::Ctor {
                        name: name.clone(),
                        args: Vec::new(),
                    },
                    sort,
                );
            }
            return self.terms.mk(Node::Opaque(name.clone()), sort);
        }
        match self.ctx.scheme(name) {
            Some(scheme) => self
                .terms
                .mk(Node::Opaque(name.clone()), scheme_sort(scheme)),
            None if TOTAL_BUILTINS.contains(&name.as_str()) => {
                self.terms.opaque(name.as_str(), None)
            }
            None => self.terms.sym(None),
        }
    }

    fn operand_type(&self, lhs: TermId, rhs: TermId) -> Option<Numeric> {
        for side in [lhs, rhs] {
            match self.terms.sort(side) {
                Some(t) if is_con(t, "Float") => return Some(Numeric::Float),
                Some(t) if is_con(t, "Decimal") => return Some(Numeric::Decimal),
                _ => {}
            }
        }
        None
    }

    fn non_int_operator(
        &mut self,
        op: BinOp,
        lhs: TermId,
        rhs: TermId,
        numeric: Numeric,
    ) -> TermId {
        let comparison = matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge);
        match numeric {
            Numeric::Float => self.float(),
            Numeric::Decimal => {
                self.blocked(Blocker::DecimalArithmetic);
                if !comparison {
                    self.undefined();
                }
            }
        }
        let symbol = match op {
            BinOp::Add => term::ADD,
            BinOp::Sub => term::SUB,
            BinOp::Mul => term::MUL,
            BinOp::Div => term::DIV,
            BinOp::Rem => term::REM,
            BinOp::Lt => term::LT,
            BinOp::Le => term::LE,
            BinOp::Gt => term::GT,
            _ => term::GE,
        };
        let sort = if comparison {
            Type::bool()
        } else {
            match numeric {
                Numeric::Float => Type::float(),
                Numeric::Decimal => Type::decimal(),
            }
        };
        let head = self.terms.opaque(symbol, None);
        self.terms.mk(
            Node::App {
                head,
                args: vec![lhs, rhs],
            },
            Some(sort),
        )
    }

    fn binary(&mut self, op: BinOp, lhs: TermId, rhs: TermId) -> TermId {
        if matches!(
            op,
            BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Rem
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
        ) && let Some(numeric) = self.operand_type(lhs, rhs)
        {
            return self.non_int_operator(op, lhs, rhs, numeric);
        }
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                self.terms.force_int(lhs);
                self.terms.force_int(rhs);
                let folded = match op {
                    BinOp::Add => self.terms.add(lhs, rhs),
                    BinOp::Sub => self.terms.sub(lhs, rhs),
                    BinOp::Mul => self.terms.mul(lhs, rhs),
                    // Uninterpreted even by a literal, so `x / 2 * 2 == x` can never be proved.
                    BinOp::Div | BinOp::Rem => None,
                    _ => unreachable!(),
                };
                match folded {
                    Some(term) => {
                        self.require_int_range(term);
                        term
                    }
                    None => {
                        let literal_factor = matches!(self.terms.node(lhs), Node::Int(_))
                            || matches!(self.terms.node(rhs), Node::Int(_));
                        self.blocked(match op {
                            BinOp::Div | BinOp::Rem => Blocker::Division,
                            BinOp::Mul if !literal_factor => Blocker::NonlinearMultiplication,
                            _ => Blocker::CoefficientRange,
                        });
                        match op {
                            // Uninterpreted as values, but their definedness is decidable.
                            BinOp::Div | BinOp::Rem => self.require_divisible(lhs, rhs),
                            // Everything else left `Int`, which the evaluator never produces.
                            _ => self.undefined(),
                        }
                        let symbol = match op {
                            BinOp::Add => term::ADD,
                            BinOp::Sub => term::SUB,
                            BinOp::Div => term::DIV,
                            BinOp::Rem => term::REM,
                            _ => term::MUL,
                        };
                        let head = self.terms.opaque(symbol, None);
                        let term = self.terms.mk(
                            Node::App {
                                head,
                                args: vec![lhs, rhs],
                            },
                            Some(Type::int()),
                        );
                        self.terms.force_int(term);
                        term
                    }
                }
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                self.terms.force_int(lhs);
                self.terms.force_int(rhs);
                let op = match op {
                    BinOp::Lt => CmpOp::Lt,
                    BinOp::Le => CmpOp::Le,
                    BinOp::Gt => CmpOp::Gt,
                    _ => CmpOp::Ge,
                };
                self.terms
                    .mk(Node::Cmp { op, lhs, rhs }, Some(Type::bool()))
            }
            BinOp::And => self.terms.mk(Node::And(lhs, rhs), Some(Type::bool())),
            BinOp::Or => self.terms.mk(Node::Or(lhs, rhs), Some(Type::bool())),
            // `==` on functions is a type error, so it needs no requirement here.
            BinOp::Eq => self.terms.eq(lhs, rhs),
            BinOp::Ne => {
                let eq = self.terms.eq(lhs, rhs);
                self.terms.not(eq)
            }
            // Uninterpreted. Folding `x << 1` into `2·x` is wrong: the evaluator refutes
            // `x << 1 > x` at `x = 2^62`.
            BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor
            | BinOp::Shl
            | BinOp::Shr
            | BinOp::Ushr => {
                self.terms.force_int(lhs);
                self.terms.force_int(rhs);
                self.blocked(Blocker::BitOperator);
                if matches!(op, BinOp::Shl | BinOp::Shr | BinOp::Ushr) {
                    self.require_shift_count(rhs);
                }
                let symbol = match op {
                    BinOp::BitAnd => term::BIT_AND,
                    BinOp::BitOr => term::BIT_OR,
                    BinOp::BitXor => term::BIT_XOR,
                    BinOp::Shl => term::SHL,
                    BinOp::Shr => term::SHR,
                    _ => term::USHR,
                };
                let head = self.terms.opaque(symbol, None);
                let term = self.terms.mk(
                    Node::App {
                        head,
                        args: vec![lhs, rhs],
                    },
                    Some(Type::int()),
                );
                self.terms.force_int(term);
                term
            }
            BinOp::Concat => {
                self.blocked(Blocker::Concat);
                let head = self.terms.opaque(term::CONCAT, None);
                // Both sides share one sort, so either side that has one gives the answer's.
                let sort = match self.terms.sort(lhs).or(self.terms.sort(rhs)) {
                    Some(t) => t.clone(),
                    None => Type::string(),
                };
                self.terms.mk(
                    Node::App {
                        head,
                        args: vec![lhs, rhs],
                    },
                    Some(sort),
                )
            }
        }
    }

    fn application(&mut self, func: &Code, args: &[Code]) -> TermId {
        let lowered: Vec<TermId> = args.iter().map(|a| self.lower(a)).collect();

        if let Code::Lambda {
            params,
            captures,
            body,
        } = func
            && *params == lowered.len()
        {
            let mut window: Vec<Option<TermId>> = lowered.iter().map(|t| Some(*t)).collect();
            for &(outer, inner) in captures {
                let term = self.local(outer);
                if window.len() <= inner {
                    window.resize(inner + 1, None);
                }
                window[inner] = Some(term);
            }
            return self.within(window, |this| this.lower(body));
        }

        let callee = self.callee(func);

        let head = self.lower(func);
        let sort = match self.terms.sort(head) {
            Some(Type::Fn { ret, .. }) => Some((**ret).clone()),
            _ => None,
        };

        // A total builtin is a function of its arguments, so two calls over one argument are one term.
        let total_builtin =
            matches!(&callee, Callee::Unresolved(name) if TOTAL_BUILTINS.contains(&name.as_str()));
        let sort = match &callee {
            Callee::Unresolved(name) if total_builtin => {
                builtin_sort(name.as_str(), &lowered, &self.terms)
            }
            _ => sort,
        };
        let mut pure = total_builtin || self.head_is_pure(head);
        if let Node::Opaque(name) = self.terms.node(head).clone()
            && !total_builtin
        {
            if let Some(ctor) = self.ctx.ctor(&name) {
                if ctor.arity == lowered.len() {
                    let sort = ctor_result_sort(ctor, &lowered, &self.terms);
                    return self.terms.mk(
                        Node::Ctor {
                            name,
                            args: lowered,
                        },
                        sort,
                    );
                }
            } else {
                if let Some(term) = self.try_unfold(&name, head, sort.clone(), &lowered) {
                    return term;
                }
                pure &= self.ctx.is_pure(&name);
                self.note_unfold_refusal(&name);
            }
        }

        if !self.decreasing_call(&callee, &lowered) && !self.callee_is_total(&callee, head) {
            self.undefined();
        }
        if let Callee::Unresolved(name) = &callee
            && let Some(term) = self.list_builtin(name.as_str(), head, &lowered)
        {
            return term;
        }

        // A call not known to be a function of its arguments gets a fresh symbol per occurrence.
        if !pure {
            return self.terms.sym(sort);
        }

        let call = self.terms.mk(
            Node::App {
                head,
                args: lowered,
            },
            sort,
        );
        if matches!(&callee, Callee::Named(name) if self.total.contains(name)) {
            self.calls.push(call);
        }
        call
    }

    /// `len` and `push` over a list whose spine is in view, structurally; where the spine ends in
    /// a symbol the call stays, over that symbol.
    fn list_builtin(&mut self, name: &str, head: TermId, args: &[TermId]) -> Option<TermId> {
        match (name, args) {
            ("len", [xs]) => self.list_len(head, *xs),
            ("push", [xs, x]) => self.list_push(head, *xs, *x),
            _ => None,
        }
    }

    fn list_len(&mut self, head: TermId, xs: TermId) -> Option<TermId> {
        let mut count = 0i64;
        let mut at = xs;
        loop {
            match self.terms.node(at).clone() {
                Node::Nil => return Some(self.terms.int_lit(count)),
                Node::Cons { tail, .. } => {
                    count += 1;
                    at = tail;
                }
                _ => break,
            }
        }
        if count == 0 {
            return None;
        }
        let rest = self.terms.mk(
            Node::App {
                head,
                args: vec![at],
            },
            Some(Type::int()),
        );
        let k = self.terms.int_lit(count);
        let sum = self.terms.add(k, rest)?;
        self.require_int_range(sum);
        Some(sum)
    }

    fn list_push(&mut self, head: TermId, xs: TermId, x: TermId) -> Option<TermId> {
        let sort = self.terms.sort(xs).cloned();
        let mut heads = Vec::new();
        let mut at = xs;
        loop {
            match self.terms.node(at).clone() {
                Node::Nil => break,
                Node::Cons { head, tail } => {
                    heads.push(head);
                    at = tail;
                }
                _ => {
                    if heads.is_empty() {
                        return None;
                    }
                    break;
                }
            }
        }
        let mut out = match self.terms.node(at).clone() {
            Node::Nil => {
                let nil = self.terms.nil(sort.clone());
                self.terms.cons(x, nil, sort.clone())
            }
            _ => self.terms.mk(
                Node::App {
                    head,
                    args: vec![at, x],
                },
                sort.clone(),
            ),
        };
        for h in heads.into_iter().rev() {
            out = self.terms.cons(h, out, sort.clone());
        }
        Some(out)
    }

    /// Decided from the code, not the head's term: a local and a same-named definition lower alike.
    fn callee(&self, func: &Code) -> Callee {
        match func {
            Code::Local(_) => Callee::Local,
            Code::Global(name)
                if self.ctx.ctor(name).is_some() || self.ctx.scheme(name).is_some() =>
            {
                Callee::Named(name.clone())
            }
            // Written `module::name`, and the resolver bound nothing.
            Code::Global(name) if name.as_str().contains("::") => Callee::Other,
            Code::Global(name) => Callee::Unresolved(name.clone()),
            _ => Callee::Other,
        }
    }

    fn callee_is_total(&self, callee: &Callee, head: TermId) -> bool {
        match callee {
            Callee::Local => matches!(
                self.terms.sort(head),
                Some(Type::Fn { effects, .. }) if effects.is_pure()
            ),
            // A definition whose body was not inlined is not known to be total.
            Callee::Named(name) => self.ctx.ctor(name).is_some() || self.total.contains(name),
            Callee::Unresolved(name) => TOTAL_BUILTINS.contains(&name.as_str()),
            Callee::Other => false,
        }
    }

    fn head_is_pure(&self, head: TermId) -> bool {
        matches!(self.terms.sort(head), Some(Type::Fn { effects, .. }) if effects.is_pure())
    }

    /// Why [`Lowering::try_unfold`] declined; keep in its decision order.
    fn note_unfold_refusal(&mut self, name: &Symbol) {
        let blocker = if self.depth >= self.unfold_depth || self.terms.len() >= MAX_TERMS {
            Blocker::UnfoldLimit(name.clone())
        } else if self.ctx.is_recursive(name) {
            Blocker::RecursiveCall(name.clone())
        } else if !self.ctx.is_pure(name) {
            Blocker::EffectfulCall(name.clone())
        } else {
            Blocker::OpaqueCall(name.clone())
        };
        self.blocked(blocker);
    }

    /// The self call of the definition under a termination check owes the measure: its argument
    /// in the measured slot is non-negative and below the parameter the body was entered with.
    fn decreasing_call(&mut self, callee: &Callee, args: &[TermId]) -> bool {
        let Some(measure) = self.measure.clone() else {
            return false;
        };
        if !matches!(callee, Callee::Named(name) if *name == measure.def) {
            return false;
        }
        let Some(&arg) = args.get(measure.slot) else {
            return false;
        };
        if self
            .terms
            .sort(measure.bound)
            .is_some_and(|s| super::term::list_elem(s).is_some())
        {
            let owed = if self.smaller_than(arg, measure.bound) {
                self.terms.true_id
            } else {
                self.terms.false_id
            };
            self.measures.push(owed);
            return true;
        }
        let zero = self.terms.int_lit(0);
        let low = self.terms.mk(
            Node::Cmp {
                op: CmpOp::Ge,
                lhs: arg,
                rhs: zero,
            },
            Some(Type::bool()),
        );
        let high = self.terms.mk(
            Node::Cmp {
                op: CmpOp::Lt,
                lhs: arg,
                rhs: measure.bound,
            },
            Some(Type::bool()),
        );
        let both = self.terms.mk(Node::And(low, high), Some(Type::bool()));
        let owed = self.under_path(both);
        self.measures.push(owed);
        true
    }

    fn try_unfold(
        &mut self,
        name: &Symbol,
        head: TermId,
        sort: Option<Type>,
        args: &[TermId],
    ) -> Option<TermId> {
        if self.depth >= self.unfold_depth || self.terms.len() >= MAX_TERMS {
            return None;
        }
        let ctx = self.ctx;
        let unrolling = self.unrolled.contains(name);
        // One step of a recursive definition: the hypothesis speaks about the call that remains.
        if unrolling && self.depth >= 1 {
            return None;
        }
        let def = match ctx.unfoldable(name) {
            Some(def) => def,
            None if unrolling => ctx.self_recursive(name)?,
            None => return None,
        };
        if def.params != args.len() {
            return None;
        }
        self.rules.unfolded(name.clone(), self.depth + 1);

        let window = args.iter().map(|t| Some(*t)).collect();
        self.depth += 1;
        let out = self.within(window, |this| this.lower(&def.body));
        self.depth -= 1;
        if unrolling {
            let call = self.terms.mk(
                Node::App {
                    head,
                    args: args.to_vec(),
                },
                sort,
            );
            let equation = self.terms.eq(call, out);
            self.equations.push(equation);
            self.defined.push(call);
        }
        Some(out)
    }

    fn block(&mut self, stmts: &[Stmt], tail: Option<&Code>) -> TermId {
        for stmt in stmts {
            match stmt {
                Stmt::Let(slot, value) => {
                    let term = self.lower(value);
                    self.bind(*slot, term);
                }
                Stmt::LetPat(Pat::Wild, _) => {}
                Stmt::LetPat(pat, value) => {
                    let term = self.lower(value);
                    let sort = self.terms.sort(term).cloned();
                    self.blocked(Blocker::DestructuringLet);
                    self.bind_opaque(pat, sort.as_ref());
                }
            }
        }
        match tail {
            Some(code) => self.lower(code),
            None => self.terms.unit(),
        }
    }

    fn bind_opaque(&mut self, pat: &Pat, sort: Option<&Type>) {
        if sort.is_some_and(|s| self.ctx.reaches_float(s)) {
            self.float();
        }
        for slot in pattern_slots(pat) {
            let term = self.terms.sym(None);
            self.bind(slot, term);
        }
    }

    fn match_expr(&mut self, scrutinee: &Code, arms: &[claims::Arm]) -> TermId {
        let scrutinee = self.lower(scrutinee);
        let scrutinee_sort = self.terms.sort(scrutinee).cloned();
        let mut lowered = Vec::with_capacity(arms.len());
        let mut result_sort = None;

        for arm in arms {
            // What a guard admits is not modelled, so a guarded arm is never known to be taken.
            let shape = match arm.guard {
                Some(_) => None,
                None => self.arm_shape(&arm.pat, scrutinee, scrutinee_sort.as_ref()),
            };
            let (test, binds) = match shape {
                Some(shape) => shape,
                None => {
                    self.blocked(Blocker::UndecidableMatchArm);
                    self.bind_opaque(&arm.pat, scrutinee_sort.as_ref());
                    (ArmTest::Undecidable, Vec::new())
                }
            };
            if let Some(guard) = &arm.guard {
                self.lower(guard);
            }
            // What a list arm's body owes is owed only where the arm runs: at `[]`, or off it.
            let taken = match &test {
                ArmTest::List {
                    fixed: 0,
                    rest: false,
                } => {
                    let nil = self.terms.nil(scrutinee_sort.clone());
                    Some(self.terms.eq(scrutinee, nil))
                }
                ArmTest::List { .. } => {
                    let nil = self.terms.nil(scrutinee_sort.clone());
                    let at_nil = self.terms.eq(scrutinee, nil);
                    Some(self.terms.not(at_nil))
                }
                _ => None,
            };
            if let Some(cond) = taken {
                self.path.push(cond);
            }
            let body = self.lower(&arm.body);
            if taken.is_some() {
                self.path.pop();
            }
            if result_sort.is_none() {
                result_sort = self.terms.sort(body).cloned();
            }
            lowered.push(Arm { test, binds, body });
        }

        self.terms.mk(
            Node::Match {
                scrutinee,
                arms: lowered,
            },
            result_sort,
        )
    }

    fn arm_shape(
        &mut self,
        pat: &Pat,
        scrutinee: TermId,
        scrutinee_sort: Option<&Type>,
    ) -> Option<(ArmTest, Vec<TermId>)> {
        match pat {
            Pat::Wild => Some((ArmTest::Always, Vec::new())),
            Pat::Var(slot) => {
                self.bind(*slot, scrutinee);
                Some((ArmTest::Always, Vec::new()))
            }
            Pat::Lit(lit) => {
                let term = self.literal(lit);
                Some((ArmTest::Lit(term), Vec::new()))
            }
            Pat::Ctor(name, args) => {
                let ctor = self.ctx.ctor(name)?;
                if ctor.arity != args.len() {
                    return None;
                }
                if !args.iter().all(|a| matches!(a, Pat::Wild | Pat::Var(_))) {
                    return None;
                }
                let sorts = field_sorts(ctor, scrutinee_sort);
                let mut binds = Vec::with_capacity(args.len());
                for (arg, sort) in args.iter().zip(sorts) {
                    let field = self.terms.sym(sort);
                    if let Pat::Var(slot) = arg {
                        self.bind(*slot, field);
                    }
                    binds.push(field);
                }
                Some((ArmTest::Ctor(name.clone()), binds))
            }
            Pat::List(items, rest) => {
                let inner = items.iter().chain(rest.iter().map(|r| &**r));
                if !inner.clone().all(|a| matches!(a, Pat::Wild | Pat::Var(_))) {
                    return None;
                }
                let elem = scrutinee_sort.and_then(super::term::list_elem).cloned();
                let mut binds = Vec::with_capacity(items.len() + 1);
                // A spine in view binds its own heads and tail, so a call over the tail is the
                // same term wherever the tail is named; a symbol gets fresh fields.
                let mut at = Some(scrutinee);
                for item in items {
                    let field = match at.map(|t| self.terms.node(t).clone()) {
                        Some(Node::Cons { head, tail }) => {
                            at = Some(tail);
                            head
                        }
                        _ => {
                            at = None;
                            self.terms.sym(elem.clone())
                        }
                    };
                    if let Pat::Var(slot) = item {
                        self.bind(*slot, field);
                    }
                    binds.push(field);
                }
                if let Some(rest) = rest {
                    let tail = match at {
                        Some(t) => t,
                        None => self.terms.sym(scrutinee_sort.cloned()),
                    };
                    if tail != scrutinee {
                        self.smaller.push((tail, scrutinee));
                    }
                    if let Pat::Var(slot) = &**rest {
                        self.bind(*slot, tail);
                    }
                    binds.push(tail);
                }
                Some((
                    ArmTest::List {
                        fixed: items.len(),
                        rest: rest.is_some(),
                    },
                    binds,
                ))
            }
            Pat::Nested(_) => None,
        }
    }

    fn record_sort(&self, fields: &[(Symbol, TermId)]) -> Option<Type> {
        let mut out = BTreeMap::new();
        for (name, value) in fields {
            out.insert(name.clone(), self.terms.sort(*value)?.clone());
        }
        Some(Type::Record(out))
    }
}

enum Callee {
    Local,
    Named(Symbol),
    /// A prelude function.
    Unresolved(Symbol),
    Other,
}

fn pattern_slots(pat: &Pat) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack = vec![pat];
    while let Some(p) = stack.pop() {
        match p {
            Pat::Var(slot) => out.push(*slot),
            Pat::Ctor(_, inner) | Pat::Nested(inner) => stack.extend(inner),
            Pat::List(items, rest) => {
                stack.extend(items);
                if let Some(rest) = rest {
                    stack.push(rest);
                }
            }
            Pat::Wild | Pat::Lit(_) => {}
        }
    }
    out
}

fn scheme_sort(scheme: &Scheme) -> Option<Type> {
    Some(scheme.ty.clone())
}

/// The owning sum type's parameters, in argument order.
fn type_parameters(ctor: &CtorInfo) -> Option<Vec<TyVar>> {
    let ret = match &ctor.scheme.ty {
        Type::Fn { ret, .. } => ret.as_ref(),
        other => other,
    };
    let Type::Con(name, args) = ret else {
        return None;
    };
    if *name != ctor.type_name {
        return None;
    }
    args.iter()
        .map(|a| match a {
            Type::Var(v) => Some(*v),
            _ => None,
        })
        .collect()
}

/// Instantiated against the scrutinee's sort when it is known.
/// What a total builtin answers, read off its arguments' sorts where the answer depends on them.
fn builtin_sort(name: &str, args: &[TermId], terms: &Terms) -> Option<Type> {
    let first = args.first().and_then(|a| terms.sort(*a).cloned());
    match name {
        "len" | "min" | "max" | "bytes_len" | "string_len" => Some(Type::int()),
        "int_to_string" => Some(Type::string()),
        "push" => first,
        "list_at" => first
            .as_ref()
            .and_then(super::term::list_elem)
            .map(|elem| Type::Con(Symbol::new("Option"), vec![elem.clone()])),
        _ => None,
    }
}

pub(super) fn field_sorts(ctor: &CtorInfo, sort: Option<&Type>) -> Vec<Option<Type>> {
    let subst = match (sort, type_parameters(ctor)) {
        (Some(Type::Con(name, args)), Some(params))
            if *name == ctor.type_name && args.len() == params.len() =>
        {
            params.into_iter().zip(args.iter().cloned()).collect()
        }
        _ => BTreeMap::new(),
    };
    ctor.fields
        .iter()
        .map(|f| Some(substitute(f, &subst)))
        .collect()
}

fn ctor_result_sort(ctor: &CtorInfo, args: &[TermId], terms: &Terms) -> Option<Type> {
    let params = type_parameters(ctor)?;
    let mut subst: BTreeMap<TyVar, Type> = BTreeMap::new();
    for (field, arg) in ctor.fields.iter().zip(args) {
        if let Some(actual) = terms.sort(*arg) {
            match_type(field, actual, &mut subst);
        }
    }
    let args = params
        .into_iter()
        .map(|p| subst.get(&p).cloned().unwrap_or(Type::Var(p)))
        .collect();
    Some(Type::Con(ctor.type_name.clone(), args))
}

/// One-way matching of `pattern` against `actual`; silently declines where they disagree.
fn match_type(pattern: &Type, actual: &Type, subst: &mut BTreeMap<TyVar, Type>) {
    match (pattern, actual) {
        (Type::Var(v), _) => {
            subst.entry(*v).or_insert_with(|| actual.clone());
        }
        (Type::Con(a, xs), Type::Con(b, ys)) if a == b && xs.len() == ys.len() => {
            for (x, y) in xs.iter().zip(ys) {
                match_type(x, y, subst);
            }
        }
        (
            Type::Fn {
                params: ps, ret: r, ..
            },
            Type::Fn {
                params: qs, ret: s, ..
            },
        ) if ps.len() == qs.len() => {
            for (p, q) in ps.iter().zip(qs) {
                match_type(p, q, subst);
            }
            match_type(r, s, subst);
        }
        (Type::Record(xs), Type::Record(ys)) => {
            for (name, x) in xs {
                if let Some(y) = ys.get(name) {
                    match_type(x, y, subst);
                }
            }
        }
        _ => {}
    }
}

fn substitute(ty: &Type, subst: &BTreeMap<TyVar, Type>) -> Type {
    match ty {
        Type::Var(v) => subst.get(v).cloned().unwrap_or_else(|| ty.clone()),
        Type::Con(name, args) => Type::Con(
            name.clone(),
            args.iter().map(|a| substitute(a, subst)).collect(),
        ),
        Type::Fn {
            params,
            ret,
            effects,
        } => Type::Fn {
            params: params.iter().map(|p| substitute(p, subst)).collect(),
            ret: Box::new(substitute(ret, subst)),
            effects: effects.clone(),
        },
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(n, t)| (n.clone(), substitute(t, subst)))
                .collect(),
        ),
    }
}

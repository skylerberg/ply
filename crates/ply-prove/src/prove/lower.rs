//! Ply expressions into the prover's terms.
//!
//! The rule this file is written to: **every construct the fragment does not
//! interpret becomes a fresh symbol, and never a guess.** A `perform`, a
//! `handle`, a `simulate`, a lambda, a destructuring `let`, an arithmetic
//! expression whose coefficients left range — each is replaced by a constant
//! about which nothing is known, so a proof over the lowered term is a proof
//! for every value the original could have taken.
//!
//! The one place that direction is easy to get backwards is name lookup: a
//! binder the lowering fails to record would let its occurrences resolve to a
//! *top-level definition of the same name*, which is a different value, not an
//! unknown one. So every pattern variable, including one in a pattern the
//! fragment declines to reduce, is bound to a fresh symbol before the body it
//! scopes over is lowered.
//!
//! # Definedness
//!
//! Replacing a construct by a symbol is enough to keep the prover from claiming
//! a wrong *value*. It is not enough to keep it from claiming a wrong *tier*,
//! because the prover reasons over mathematical integers and total functions
//! while Ply evaluates over `i64` with checked arithmetic and has no termination
//! checker. `x + 1 > x` is valid over ℤ and **raises** at `i64::MAX`;
//! `spin(x) == spin(x)` is valid for a total uninterpreted symbol and never
//! returns for `fn spin(x) = spin(x)`. Neither is an input at which the
//! obligation holds, so neither may sit under a tier whose claim is "every input
//! satisfying the guard".
//!
//! So every lowered expression also records a **requirement**: a Boolean term
//! that must be valid for the expression to evaluate to a value at all. An
//! arithmetic operator requires its mathematical result to fit in `Int`; a call
//! this module could not look inside requires `false`, which nothing proves. The
//! requirements are conditioned on the path they were reached under, so an arm
//! of an `if` that the guard rules out costs nothing.
//!
//! [`super::decide_and_diagnose`] discharges them alongside the goal, and no
//! [`super::Proof`] is issued until it has. Failing to discharge one is
//! `Unknown` — the weaker tier — never a refutation.

use super::RuleLog;
use super::context::{Context, Unfoldable};
use super::term::{self, Arm, ArmTest, CmpOp, Node, TermId, Terms};
use ply_core::{CtorInfo, Scheme, TyVar, Type};
use ply_span::Symbol;
use ply_syntax::ast::{BinOp, Expr, ExprKind, Lit, Param, Pattern, PatternKind, QName, Stmt, UnOp};
use std::collections::BTreeMap;

/// The size past which unfolding stops. A bound on the term graph rather than
/// on time, so two runs of the prover agree whatever the machine was doing.
const MAX_TERMS: usize = 20_000;

/// The prelude functions whose evaluation cannot raise and cannot diverge, so a
/// call to one needs no definedness requirement.
///
/// Deliberately short. `map`, `filter` and `fold` apply a function this module
/// never sees the body of; `range` builds a list whose length is an argument;
/// `assert`, `assert_eq` and `panic` exist to raise. Every one of those is
/// absent, so a proof cannot rest on it terminating.
///
/// `list_at` is present: an index outside the list answers `None` rather than
/// refusing, so it has no input it declines and a call to it is a value.
///
/// Absent for the same reason: `bytes_at`, `bytes_slice`, `string_slice`,
/// `string_split`, `string_find` and `string_of_bytes` all have inputs they
/// refuse, so a call to one is not a value until its argument is known. So do
/// `bytes_index_of_from`, `bytes_index_of_byte`, `bytes_split`, `bytes_scan`
/// and `bytes_scan_until`; `bytes_position` applies a function this module
/// never sees the body of.
const TOTAL_BUILTINS: &[&str] = &[
    "len",
    "push",
    // The list index. Total for the reason `map_get` is: an index outside the
    // list is an `Option`'s `None`, never a refusal, so a call is a value
    // whatever its arguments turn out to be.
    //
    // **Correct and, today, inert.** Nothing over a `List` reaches the static
    // tier at all — `law forall (i: Int) { len([10, 20, 30]) == 3 }` is
    // `property`, while `i + 0 == i` beside it is `proved` — so removing this
    // entry changes no tier that can currently be written. It is here because
    // it is true and because a fragment with a list theory would need it, and
    // ADR 0027 §2 records it as an unarmed change rather than as a gate.
    "list_at",
    "int_to_string",
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
    // `Map`. Every one of these is a function of its arguments with no input it
    // refuses: a lookup answers an `Option` rather than raising, and removing an
    // absent key is a no-op. Being total buys congruence and nothing else —
    // there is no theory of arrays here, so `map_get(map_insert(m, k, v), k) ==
    // Some(v)` is still `property` and must be.
    //
    // `map_fold` is absent, with `map`, `filter` and `fold`, because it applies
    // a function this module never sees the body of.
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
    // The `Decimal` conversions with no input they refuse: each answers an
    // `Option` or a total value rather than raising. `decimal_div` and
    // `decimal_round` are deliberately absent — both refuse a scale outside
    // `0..=28` and `decimal_div` refuses a zero divisor, so a call to either is
    // not a value until its arguments are known.
    "decimal_of_int",
    "decimal_to_string",
    "decimal_of_string",
    "float_of_decimal",
    "decimal_of_float",
    "int_of_decimal",
];

/// Where lowering left the decidable fragment, for measurement only.
///
/// Recording one never changes a term, so [`Lowering`] answers the same whether
/// or not anything reads these. `Reason::Open` is all the prover is willing to
/// *claim* about an inconclusive attempt — telling an unprovable goal from an
/// unrepresentable one would mean claiming a model — and this is the separate,
/// weaker question of which construct was replaced by a symbol on the way.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Blocker {
    /// A call to a member of a recursive component. Inlining it needs
    /// induction, which M8 does not have.
    RecursiveCall(Symbol),
    /// A call whose row is not known to be empty: two occurrences may answer
    /// differently, so they cannot share a term.
    EffectfulCall(Symbol),
    /// A non-recursive call refused because [`crate::UNFOLD_DEPTH`] or the term
    /// limit was already reached.
    UnfoldLimit(Symbol),
    /// A call to something this crate cannot see the body of: a builtin, or a
    /// name from outside the program.
    OpaqueCall(Symbol),
    Division,
    /// `x * y` with both factors symbolic.
    NonlinearMultiplication,
    /// A linear combination whose coefficients left range.
    CoefficientRange,
    Lambda,
    StringConcat,
    /// A `Float`-typed term anywhere in the graph. Not a completeness cost like
    /// the others: it is the refusal that makes `Float` outside `proved`
    /// structural rather than a rule somebody has to remember.
    FloatTerm,
    /// `Decimal` arithmetic or a `Decimal` ordering. The type's `==` is an
    /// equivalence relation, so congruence over it is sound and `f(x) == f(x)`
    /// is provable — but there is no theory of `+` here, so `x + 0m == x` is
    /// `property`.
    DecimalArithmetic,
    /// A `perform`, `handle`, `with_cell` or `simulate`. Only reachable in a
    /// concurrency law, whose row may carry `sim.read`.
    Region,
    /// A parse-time-only node the parser is supposed to have expanded away —
    /// today only `{..b, f: e}`, whose expansion runs inside
    /// `ply_syntax::parse_module`.
    ///
    /// **Unreachable, and therefore never seen in a report.** It is `blocked`
    /// rather than `unreachable!` because a prover's safe answer is "I did not
    /// reason about this" and never a term it guessed; it is its own variant
    /// rather than `Region` because a record update is not a region, and a
    /// blocker a report prints has to say what it means the day the arm becomes
    /// reachable. What keeps it unreachable is
    /// `ply_syntax::tests::no_record_update_survives_parse_module_anywhere_in_the_tree`.
    UnexpandedSugar,
    /// A pattern the fragment declines to reduce, or a pattern guard.
    UndecidableMatchArm,
    DestructuringLet,
}

/// The numeric type an operator was applied at, when it was not `Int`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Numeric {
    Float,
    Decimal,
}

fn is_con(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Con(n, args) if n.as_str() == name && args.is_empty())
}

pub struct Lowering<'a, 'p> {
    pub terms: Terms,
    ctx: &'a Context<'p>,
    rules: &'a mut RuleLog,
    module: usize,
    unfold_depth: u32,
    depth: u32,
    /// `(name, term)` in scope order. `barriers` marks where an unfolded body's
    /// scope begins, so a callee never sees a caller's locals.
    frames: Vec<(Symbol, TermId)>,
    barriers: Vec<usize>,
    blockers: Vec<Blocker>,
    /// What must hold for the lowered expressions to evaluate to a value rather
    /// than raise or diverge. Every entry is already conditioned on the path it
    /// was reached under.
    requirements: Vec<TermId>,
    /// Conditions assumed true on the way to the expression being lowered: the
    /// branch of an `if`, the right operand of a short-circuiting operator, and
    /// the guards a caller has already lowered.
    path: Vec<TermId>,
    /// Whether a `Float` was met. See [`Lowering::float`].
    float: bool,
}

impl<'a, 'p> Lowering<'a, 'p> {
    pub fn new(
        ctx: &'a Context<'p>,
        rules: &'a mut RuleLog,
        module: usize,
        unfold_depth: u32,
    ) -> Lowering<'a, 'p> {
        Lowering {
            terms: Terms::new(),
            ctx,
            rules,
            module,
            unfold_depth,
            depth: 0,
            frames: Vec::new(),
            barriers: Vec::new(),
            blockers: Vec::new(),
            requirements: Vec::new(),
            path: Vec::new(),
            float: false,
        }
    }

    /// Records that a `Float` entered the obligation, which no proof survives.
    ///
    /// `==` on `Float` is not reflexive — `NaN != NaN` — and congruence closure
    /// over a relation that is not reflexive is unsound, so there is no honest
    /// way to run this fragment over the type at all. The refusal is
    /// **structural**: lowering answers "unsupported" and
    /// [`super::decide_and_diagnose`] returns before a solver runs, so the
    /// certificate cannot be constructed rather than being constructed and then
    /// discarded. A tier is computed from the evidence a discharge carries, and
    /// this is what makes that sentence true for `Float`.
    fn float(&mut self) {
        if !self.float {
            self.blocked(Blocker::FloatTerm);
        }
        self.float = true;
    }

    /// Whether anything lowered here puts a `Float` in the obligation.
    pub fn unsupported(&self) -> bool {
        self.float
    }

    fn blocked(&mut self, blocker: Blocker) {
        self.blockers.push(blocker);
    }

    /// Every fragment boundary this lowering crossed, in encounter order.
    pub fn blockers(&self) -> &[Blocker] {
        &self.blockers
    }

    /// What must hold for everything lowered so far to evaluate to a value.
    pub fn requirements(&self) -> &[TermId] {
        &self.requirements
    }

    /// How many requirements have been recorded, so a caller can tell the ones a
    /// guard owes from the ones its body does. A guard is evaluated *before* it
    /// is known to hold, so its own requirements may not assume it.
    pub fn requirement_mark(&self) -> usize {
        self.requirements.len()
    }

    /// Assumes a condition for everything lowered after it. Used for a guard,
    /// which the evaluator has already established by the time it reaches the
    /// body.
    pub fn assume(&mut self, cond: TermId) {
        self.path.push(cond);
    }

    /// Records a condition the evaluation depends on, under the path it was
    /// reached at: `p₁ ∧ … ∧ pₙ ⟹ cond`.
    fn require(&mut self, cond: TermId) {
        if cond == self.terms.true_id {
            return;
        }
        let mut out = cond;
        for i in (0..self.path.len()).rev() {
            let negated = self.terms.not(self.path[i]);
            out = self.terms.mk(Node::Or(negated, out), Some(Type::bool()));
        }
        self.requirements.push(out);
    }

    /// The evaluation cannot be shown to produce a value at all. `false` is
    /// discharged by nothing, so an obligation reaching this is never proved —
    /// unless the path it sits on is itself impossible, which the same term
    /// says.
    fn undefined(&mut self) {
        let never = self.terms.false_id;
        self.require(never);
    }

    /// An arithmetic result must be an `Int`. Only a [`Node::Lin`] can fail:
    /// every other outcome of folding is a literal the interner already fitted
    /// into `i64`, or an operand that was a Ply value to begin with.
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

    /// `a / b` and `a % b` raise on a zero divisor, and `i64::MIN / -1` is the
    /// one quotient that leaves `Int`.
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

    /// Lowers under one more assumed condition.
    fn under<T>(&mut self, cond: TermId, f: impl FnOnce(&mut Self) -> T) -> T {
        self.path.push(cond);
        let out = f(self);
        self.path.pop();
        out
    }

    /// Introduces the obligation's binders as symbolic constants, which is what
    /// makes the answer a statement about every input rather than about one.
    pub fn bind_symbolic(&mut self, name: &Symbol, ty: &Type) -> TermId {
        if self.ctx.reaches_float(ty) {
            self.float();
        }
        let term = self.terms.sym(Some(ty.clone()));
        self.frames.push((name.clone(), term));
        term
    }

    pub fn finish(self) -> Terms {
        self.terms
    }

    pub fn lower(&mut self, expr: &Expr) -> TermId {
        // Ply admits expressions as deep as the parser accepted, and this walk
        // is recursive for the same reason inference's is.
        stacker::maybe_grow(256 * 1024, 2 * 1024 * 1024, || self.lower_inner(expr))
    }

    fn lower_inner(&mut self, expr: &Expr) -> TermId {
        match &expr.kind {
            ExprKind::Lit(lit) => self.literal(lit),
            ExprKind::Var(q) => self.variable(q),
            // `&&` and `||` short-circuit, so the right operand is only ever
            // evaluated under the left one's answer and owes its requirements
            // only there.
            ExprKind::Binary {
                op: op @ (BinOp::And | BinOp::Or),
                lhs,
                rhs,
            } => {
                let l = self.lower(lhs);
                let reached = if *op == BinOp::And {
                    l
                } else {
                    self.terms.not(l)
                };
                let r = self.under(reached, |this| this.lower(rhs));
                self.binary(*op, l, r)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.lower(lhs);
                let r = self.lower(rhs);
                self.binary(*op, l, r)
            }
            ExprKind::Unary { op, operand } => {
                let t = self.lower(operand);
                match op {
                    UnOp::Not => self.terms.not(t),
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
            // A function value the fragment does not look inside. Two lambdas
            // are two symbols, which costs reach and cannot cost truth.
            ExprKind::Lambda { .. } => {
                self.blocked(Blocker::Lambda);
                self.terms.sym(None)
            }
            ExprKind::App { func, args } => self.application(func, args),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
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
            ExprKind::Match { scrutinee, arms } => self.match_expr(scrutinee, arms),
            ExprKind::Block { stmts, tail } => self.block(stmts, tail.as_deref()),
            ExprKind::Record { fields } => {
                let mut lowered: Vec<(Symbol, TermId)> = fields
                    .iter()
                    .map(|(name, value)| (name.name.clone(), self.lower(value)))
                    .collect();
                lowered.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
                let sort = self.record_sort(&lowered);
                self.terms.mk(Node::Record(lowered), sort)
            }
            ExprKind::Field { base, field } => {
                let base = self.lower(base);
                self.terms.field(base, field.name.clone())
            }
            ExprKind::RecordUpdate { .. } => {
                self.blocked(Blocker::UnexpandedSugar);
                self.terms.sym(None)
            }
            ExprKind::List { items } => {
                let items: Vec<TermId> = items.iter().map(|i| self.lower(i)).collect();
                self.terms.mk(Node::List(items), None)
            }
            // Everything below performs, handles or schedules. None of it is a
            // term the fragment reasons about, and a spec expression's row is
            // empty so none of it can appear in one that type-checked.
            ExprKind::Perform { .. }
            | ExprKind::Handle { .. }
            | ExprKind::WithCell { .. }
            | ExprKind::WithRegion { .. }
            | ExprKind::Simulate { .. } => {
                self.blocked(Blocker::Region);
                self.terms.sym(None)
            }
        }
    }

    fn literal(&mut self, lit: &Lit) -> TermId {
        match lit {
            Lit::Int(k) => self.terms.int_lit(*k),
            Lit::Bool(b) => self.terms.boolean(*b),
            Lit::Str(s) => self.terms.string(s.clone()),
            // A fresh symbol rather than a reuse of `Node::Str`: sharing that
            // node would make `b"ab"` and `"ab"` congruent, which is a wrong
            // answer wearing a certificate. Costing completeness — two
            // occurrences of one literal do not unify, so a claim about them
            // reports `property` — is the safe direction.
            Lit::Bytes(_) => self.terms.sym(Some(Type::bytes())),
            // No `Node::Float`, deliberately: two occurrences of `0.0` sharing
            // a node would make them congruent, and congruence needs a
            // reflexive `==`, which this type does not have. The obligation is
            // refused whole, so what this term is does not matter.
            Lit::Float(_) => {
                self.float();
                self.terms.sym(Some(Type::float()))
            }
            Lit::Decimal { mantissa, scale } => self.terms.decimal(*mantissa, *scale),
            Lit::Unit => self.terms.unit(),
        }
    }

    fn variable(&mut self, q: &QName) -> TermId {
        if q.is_bare()
            && let Some(term) = self.lookup(q.symbol())
        {
            return term;
        }
        let Some(name) = self.ctx.resolve_value(self.module, q) else {
            return self.terms.sym(None);
        };
        if let Some(ctor) = self.ctx.ctor(&name) {
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
            return self.terms.mk(Node::Opaque(name), sort);
        }
        let sort = self.ctx.scheme(&name).and_then(scheme_sort);
        self.terms.mk(Node::Opaque(name), sort)
    }

    /// Which non-`Int` numeric type an operator's operands have, by their
    /// sorts. `None` is the `Int` path, which is what every operand had before
    /// this milestone.
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

    /// An operator at `Float` or `Decimal`: an uninterpreted symbol, and no
    /// theory.
    ///
    /// `Decimal` arithmetic is additionally **undefined** here, and that is not
    /// conservatism for its own sake: `+` on `Decimal` raises on mantissa
    /// overflow, so `x + x == x + x` is not true of every input the way a
    /// statement about a total function would be. Congruence over `Decimal`
    /// *values* stays available — `f(x) == f(x)` is provable — which is exactly
    /// the line ADR 0012 §4 draws.
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

    fn lookup(&self, name: &Symbol) -> Option<TermId> {
        let floor = self.barriers.last().copied().unwrap_or(0);
        self.frames[floor..]
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, t)| *t)
    }

    fn binary(&mut self, op: BinOp, lhs: TermId, rhs: TermId) -> TermId {
        // `+`, `-`, `*`, `%`, `<` and friends are defined at three numeric
        // types now, and everything below this point is the theory of exactly
        // one of them. Folding a `Float` sum into a `Node::Lin` would put
        // `x + y == y + x` over binary64 inside the fragment, which is a wrong
        // `proved` assembled out of a correct rule applied at the wrong sort.
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
                    // Division is outside the fragment at all, including by a
                    // literal: an `x / 2 * 2 == x` reported `proved` is exactly
                    // the defect this milestone must not ship, and an
                    // uninterpreted `/` makes a wrong division rule impossible
                    // to have.
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
                            // `/` and `%` are uninterpreted as *values* and
                            // still have a definedness condition the fragment
                            // can decide, which is what keeps a division under
                            // a guard that establishes its divisor in reach.
                            BinOp::Div | BinOp::Rem => self.require_divisible(lhs, rhs),
                            // Everything else got here by leaving `Int`, and a
                            // product or a sum that left `Int` is not a value
                            // the evaluator ever produces.
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
            // Comparing functions raises, and needs no requirement here: the
            // type system rejects `==` at any type containing one (E0201), so a
            // spec that could ask never reaches the prover.
            BinOp::Eq => self.terms.eq(lhs, rhs),
            BinOp::Ne => {
                let eq = self.terms.eq(lhs, rhs);
                self.terms.not(eq)
            }
            BinOp::Concat => {
                self.blocked(Blocker::StringConcat);
                let head = self.terms.opaque(term::CONCAT, None);
                self.terms.mk(
                    Node::App {
                        head,
                        args: vec![lhs, rhs],
                    },
                    Some(Type::string()),
                )
            }
        }
    }

    fn application(&mut self, func: &Expr, args: &[Expr]) -> TermId {
        let lowered: Vec<TermId> = args.iter().map(|a| self.lower(a)).collect();

        if let ExprKind::Lambda { params, body } = &func.kind
            && params.len() == lowered.len()
        {
            return self.with_frame(params_frame(params, &lowered), |this| this.lower(body));
        }

        // Asked before the head is lowered: a local binder and a top-level
        // definition of one name lower to the same shape and are not the same
        // callee.
        let callee = self.callee(func);

        let head = self.lower(func);
        let sort = match self.terms.sort(head) {
            Some(Type::Fn { ret, .. }) => Some((**ret).clone()),
            _ => None,
        };

        let mut pure = self.head_is_pure(head);
        if let Node::Opaque(name) = self.terms.node(head).clone() {
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
                if let Some(term) = self.try_unfold(&name, &lowered) {
                    return term;
                }
                pure &= self.ctx.is_pure(&name);
                self.note_unfold_refusal(&name);
            }
        }

        if !self.callee_is_total(&callee, head) {
            self.undefined();
        }

        // A call the fragment cannot establish is a function of its arguments
        // gets a fresh symbol per occurrence. Sharing one would assert that two
        // calls answer equally, which is false of anything that performs — and
        // `f() - f() == 0` reported `proved` is the shape of that mistake.
        if !pure {
            return self.terms.sym(sort);
        }

        self.terms.mk(
            Node::App {
                head,
                args: lowered,
            },
            sort,
        )
    }

    /// What is being applied, decided from the source rather than from the
    /// lowered head.
    fn callee(&self, func: &Expr) -> Callee {
        let ExprKind::Var(q) = &func.kind else {
            return Callee::Other;
        };
        if q.is_bare() && self.lookup(q.symbol()).is_some() {
            return Callee::Local;
        }
        match self.ctx.resolve_value(self.module, q) {
            Some(name) => Callee::Named(name),
            None if q.is_bare() => Callee::Unresolved(q.symbol().clone()),
            None => Callee::Other,
        }
    }

    /// Whether applying this callee produces a value, rather than raising or
    /// never returning.
    ///
    /// Reached only for a call that was **not** inlined — an inlined body is
    /// lowered, so its own requirements are recorded and this question does not
    /// arise. Everything left is a body this module has not read, and the honest
    /// answer for one of those is no: M8 has no termination checker (ADR 0007
    /// §12), so `fn spin(x: Int) -> Int = spin(x)` is a total uninterpreted
    /// symbol to the congruence closure and a non-terminating program to the
    /// evaluator, and the tier must follow the evaluator.
    fn callee_is_total(&self, callee: &Callee, head: TermId) -> bool {
        match callee {
            // A quantified function value. ADR 0007 §8.1's family is pure,
            // total and extensionally deterministic, and §3.1 rejects a binder
            // whose row is not empty, so applying one is a value.
            Callee::Local => matches!(
                self.terms.sort(head),
                Some(Type::Fn { effects, .. }) if effects.is_pure()
            ),
            // Constructing a value is always a value; a definition whose body
            // was not inlined is not.
            Callee::Named(name) => self.ctx.ctor(name).is_some(),
            Callee::Unresolved(name) => TOTAL_BUILTINS.contains(&name.as_str()),
            Callee::Other => false,
        }
    }

    /// Whether applying this head is a function of its arguments, by the row the
    /// type system already inferred for it. A head whose row is not *known* to
    /// be empty reads as impure, which costs congruence over a call nobody
    /// declared and cannot cost truth.
    fn head_is_pure(&self, head: TermId) -> bool {
        matches!(self.terms.sort(head), Some(Type::Fn { effects, .. }) if effects.is_pure())
    }

    /// Why [`Lowering::try_unfold`] declined, in the order it decides. The
    /// classification is for a measurement and nothing reads it back: an
    /// unfolding refused is `Reason::Open` either way.
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

    /// Inlines a non-recursive, pure definition. A member of a recursive
    /// component is never unfolded: reaching a general statement about one needs
    /// induction, which M8 does not have, so a claim resting on it belongs at
    /// `property` and this returns `None`.
    fn try_unfold(&mut self, name: &Symbol, args: &[TermId]) -> Option<TermId> {
        if self.depth >= self.unfold_depth || self.terms.len() >= MAX_TERMS {
            return None;
        }
        let Unfoldable { def, module, .. } = self.ctx.unfoldable(name)?;
        if def.params.len() != args.len() {
            return None;
        }
        self.rules.unfolded(name.clone(), self.depth + 1);

        let frame = params_frame(&def.params, args);
        let saved_module = std::mem::replace(&mut self.module, module);
        self.depth += 1;
        self.barriers.push(self.frames.len());
        let out = self.with_frame(frame, |this| this.lower(&def.body));
        self.barriers.pop();
        self.depth -= 1;
        self.module = saved_module;
        Some(out)
    }

    fn with_frame<T>(&mut self, frame: Vec<(Symbol, TermId)>, f: impl FnOnce(&mut Self) -> T) -> T {
        let mark = self.frames.len();
        self.frames.extend(frame);
        let out = f(self);
        self.frames.truncate(mark);
        out
    }

    fn block(&mut self, stmts: &[Stmt], tail: Option<&Expr>) -> TermId {
        let mark = self.frames.len();
        for stmt in stmts {
            let Stmt::Let { pat, value, .. } = stmt else {
                // A statement in a pure block computes a value nothing reads.
                continue;
            };
            match &pat.kind {
                PatternKind::Var(name) => {
                    let term = self.lower(value);
                    self.frames.push((name.name.clone(), term));
                }
                PatternKind::Wildcard => {}
                // A destructuring bind is not in the fragment, so every name it
                // introduces stands for an unknown value rather than resolving
                // past the binder to a definition of the same name.
                _ => {
                    let term = self.lower(value);
                    let sort = self.terms.sort(term).cloned();
                    self.blocked(Blocker::DestructuringLet);
                    self.bind_opaque(pat, sort.as_ref());
                }
            }
        }
        let out = match tail {
            Some(expr) => self.lower(expr),
            None => self.terms.unit(),
        };
        self.frames.truncate(mark);
        out
    }

    /// Binds every name a pattern introduces to a fresh symbol.
    ///
    /// `sort` is what the pattern was matched *against*, and it is taken only to
    /// ask whether a `Float` is being let in without one: a binder pulled out of
    /// a destructuring has no sort of its own, so it would otherwise reach the
    /// arithmetic below as an operand of unknown type and be folded as an `Int`.
    /// That is the one way a `Float` can enter the graph carrying no sort for
    /// [`super::float_in`] to find.
    fn bind_opaque(&mut self, pat: &Pattern, sort: Option<&Type>) {
        if sort.is_some_and(|s| self.ctx.reaches_float(s)) {
            self.float();
        }
        for name in pattern_vars(pat) {
            let term = self.terms.sym(None);
            self.frames.push((name, term));
        }
    }

    fn match_expr(&mut self, scrutinee: &Expr, arms: &[ply_syntax::ast::MatchArm]) -> TermId {
        let scrutinee = self.lower(scrutinee);
        let scrutinee_sort = self.terms.sort(scrutinee).cloned();
        let mut lowered = Vec::with_capacity(arms.len());
        let mut result_sort = None;

        for arm in arms {
            let mark = self.frames.len();
            // A pattern guard is a second condition on top of the constructor
            // test. Deciding it is not in the fragment, so the arm is left
            // undecidable and the whole `match` stays uninterpreted.
            let shape = if arm.guard.is_some() {
                None
            } else {
                self.arm_shape(&arm.pat, scrutinee, scrutinee_sort.as_ref())
            };
            let (test, binds) = match shape {
                Some(shape) => shape,
                None => {
                    self.blocked(Blocker::UndecidableMatchArm);
                    self.bind_opaque(&arm.pat, scrutinee_sort.as_ref());
                    (ArmTest::Undecidable, Vec::new())
                }
            };
            let body = self.lower(&arm.body);
            self.frames.truncate(mark);
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

    /// The test an arm reduces to, and the symbols its constructor's fields are
    /// exposed as. `None` for every pattern outside the fragment.
    fn arm_shape(
        &mut self,
        pat: &Pattern,
        scrutinee: TermId,
        scrutinee_sort: Option<&Type>,
    ) -> Option<(ArmTest, Vec<TermId>)> {
        match &pat.kind {
            PatternKind::Wildcard => Some((ArmTest::Always, Vec::new())),
            PatternKind::Var(name) => {
                self.frames.push((name.name.clone(), scrutinee));
                Some((ArmTest::Always, Vec::new()))
            }
            PatternKind::Lit(lit) => {
                let term = self.literal(lit);
                Some((ArmTest::Lit(term), Vec::new()))
            }
            PatternKind::Ctor { name, args } => {
                let qualified = self.ctx.resolve_value(self.module, name)?;
                let ctor = self.ctx.ctor(&qualified)?;
                if ctor.arity != args.len() {
                    return None;
                }
                // Only a flat pattern. A nested constructor would need the
                // field's own outermost constructor decided as well, and
                // guessing it is exactly what this milestone must not do.
                if !args
                    .iter()
                    .all(|a| matches!(a.kind, PatternKind::Wildcard | PatternKind::Var(_)))
                {
                    return None;
                }
                let sorts = field_sorts(ctor, scrutinee_sort);
                let mut binds = Vec::with_capacity(args.len());
                for (arg, sort) in args.iter().zip(sorts) {
                    let field = self.terms.sym(sort);
                    if let PatternKind::Var(name) = &arg.kind {
                        self.frames.push((name.name.clone(), field));
                    }
                    binds.push(field);
                }
                let _ = scrutinee;
                Some((ArmTest::Ctor(qualified), binds))
            }
            PatternKind::Record { .. } | PatternKind::List { .. } => None,
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

/// What an application is applying, for [`Lowering::callee_is_total`].
enum Callee {
    /// A local binder: a parameter, a `forall` binder, or a `let`.
    Local,
    /// A name that resolves to a top-level definition or a constructor.
    Named(Symbol),
    /// A bare name nothing in the program declares — a prelude function.
    Unresolved(Symbol),
    /// A computed function: a projection, another application, a lambda that
    /// was not applied in place.
    Other,
}

fn params_frame(params: &[Param], args: &[TermId]) -> Vec<(Symbol, TermId)> {
    params
        .iter()
        .zip(args)
        .map(|(p, t)| (p.name.name.clone(), *t))
        .collect()
}

fn pattern_vars(pat: &Pattern) -> Vec<Symbol> {
    let mut out = Vec::new();
    let mut stack = vec![pat];
    while let Some(p) = stack.pop() {
        match &p.kind {
            PatternKind::Var(name) => out.push(name.name.clone()),
            PatternKind::Ctor { args, .. } => stack.extend(args),
            PatternKind::Record { fields, .. } => stack.extend(fields.iter().map(|(_, p)| p)),
            PatternKind::List { items, rest } => {
                stack.extend(items);
                stack.extend(rest.as_deref());
            }
            PatternKind::Wildcard | PatternKind::Lit(_) => {}
        }
    }
    out
}

fn scheme_sort(scheme: &Scheme) -> Option<Type> {
    Some(scheme.ty.clone())
}

/// The type parameters of the sum type a constructor belongs to, in the order
/// its arguments are written.
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

/// The declared types of a constructor's fields, instantiated against the
/// scrutinee's sort when that sort is known. An unsolved parameter stays a type
/// variable, which is an uninterpreted sort and never `Int` and never
/// splittable — the conservative reading.
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

/// The sort a constructor application has, solved from the sorts of the
/// arguments that were supplied.
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

/// One-way matching: solves the variables of `pattern` against `actual`, and
/// silently declines wherever the two disagree. Used only to sharpen a sort,
/// never to conclude anything.
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

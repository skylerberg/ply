//! The AST, lowered so that a subexpression can be held by a continuation frame.
//!
//! Two passes meet here. The forward pass — [`crate::slots`] — assigns every binding a slot in
//! its barrier's window and resolves every occurrence to one. The backward pass — [`Live`] —
//! computes each occurrence's ownership: the last use of a binding moves the value out of its
//! slot rather than cloning it, which is ADR 0034's whole mechanism.

use crate::rc::{Live, Own, Use};
use crate::value::Value;
use ply_span::{Span, Symbol};
use ply_syntax::ast::{
    BinOp, Expr, ExprKind, HandleClause, Ident, Lit, MatchArm, Pattern, PatternKind, Program,
    QName, ReturnClause, Stmt as AstStmt, UnOp,
};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;

pub type Code = Rc<Node>;

pub struct Node {
    pub kind: NodeKind,
    pub span: Span,
    /// How a `Var` takes its value, and on a `Field` node whether the projection takes the field
    /// out of the record in place ([`Own::OwnedField`]).
    pub own: Own,
}

/// What entering a barrier copies in from the enclosing activation: for each free variable, the
/// slot it is read from outside, the slot it is written to inside, and how the read takes it.
#[derive(Debug, Default)]
pub struct Captures {
    pub src: Vec<u32>,
    pub dst: Vec<u32>,
    pub owns: Vec<Own>,
    /// The names, parallel to the slots, for diagnostics and escape reporting.
    pub names: Vec<Symbol>,
}

impl Captures {
    pub fn len(&self) -> usize {
        self.src.len()
    }

    pub fn is_empty(&self) -> bool {
        self.src.is_empty()
    }
}

/// The shared empty capture set, so a barrier with no free variables allocates nothing per node.
pub fn no_captures() -> Rc<Captures> {
    thread_local! {
        static EMPTY: Rc<Captures> = Rc::new(Captures::default());
    }
    EMPTY.with(Rc::clone)
}

pub enum NodeKind {
    /// The literal, and the [`Value`] it denotes, built once here rather than per evaluation.
    Lit(Lit, Value),
    Var {
        name: QName,
        /// The slot of the current activation this occurrence reads, or `None` for a name no
        /// binder in scope introduces — a definition, a constructor or a builtin.
        slot: Option<u32>,
    },
    Unary {
        op: UnOp,
        operand: Code,
    },
    Binary {
        op: BinOp,
        lhs: Code,
        rhs: Code,
    },
    Lambda {
        params: Rc<Vec<Symbol>>,
        body: Code,
        /// The window size an activation of the body needs.
        size: u32,
        captures: Rc<Captures>,
    },
    App {
        func: Code,
        args: Rc<Vec<Code>>,
    },
    If {
        cond: Code,
        then_branch: Code,
        else_branch: Code,
    },
    Match {
        scrutinee: Code,
        arms: Rc<Vec<Arm>>,
    },
    Block {
        stmts: Rc<Vec<Stmt>>,
        tail: Option<Code>,
    },
    Record {
        fields: Rc<Vec<(Symbol, Code)>>,
    },
    Field {
        base: Code,
        field: Ident,
    },
    List {
        items: Rc<Vec<Code>>,
    },
    Perform {
        effect: QName,
        op: Symbol,
        resource: Option<Symbol>,
        args: Rc<Vec<Code>>,
    },
    Handle {
        body: Code,
        clauses: Rc<Vec<Clause>>,
        ret: Option<Rc<ReturnArm>>,
    },
    WithCell {
        resource: Symbol,
        init: Code,
        binder: Symbol,
        /// The binder's slot in the current activation.
        slot: Option<u32>,
        body: Code,
    },
    Simulate {
        body: Code,
        /// The body is a barrier of its own — a region's tasks interleave, so its control copies
        /// in what it reads at the region's entry.
        size: u32,
        captures: Rc<Captures>,
    },
    WithRegion {
        body: Code,
    },
}

/// A pattern with its binders resolved to slots, which is what lets a match write bindings
/// straight into the current activation's window.
#[derive(Clone)]
pub enum Pat {
    Wildcard,
    /// May really be a nullary constructor — the AST cannot tell — so the machine still consults
    /// the constructor table before binding, and a constructor's "slot" is simply never written.
    Var {
        name: Ident,
        slot: Option<u32>,
    },
    Lit(Lit),
    Ctor {
        name: QName,
        args: Vec<Pat>,
    },
    Record {
        fields: Vec<(Ident, Pat)>,
        rest: bool,
    },
    List {
        items: Vec<Pat>,
        rest: Option<Box<Pat>>,
    },
}

impl Pat {
    /// Every name this pattern can bind.
    pub fn binders(&self, out: &mut Vec<Symbol>) {
        crate::limit::grow(|| match self {
            Pat::Wildcard | Pat::Lit(_) => {}
            Pat::Var { name, .. } => out.push(name.name.clone()),
            Pat::Ctor { args, .. } => {
                for a in args {
                    a.binders(out);
                }
            }
            Pat::Record { fields, .. } => {
                for (_, p) in fields {
                    p.binders(out);
                }
            }
            Pat::List { items, rest } => {
                for p in items {
                    p.binders(out);
                }
                if let Some(rest) = rest {
                    rest.binders(out);
                }
            }
        });
    }
}

pub struct Arm {
    pub pat: Pat,
    pub guard: Option<Code>,
    pub body: Code,
    pub span: Span,
}

pub enum Stmt {
    Let { pat: Pat, value: Code, span: Span },
    Expr { code: Code },
}

impl Stmt {
    pub fn code(&self) -> &Code {
        match self {
            Stmt::Let { value, .. } => value,
            Stmt::Expr { code } => code,
        }
    }
}

pub struct Clause {
    pub effect: QName,
    pub op: Symbol,
    pub resource: Option<Symbol>,
    pub params: Rc<Vec<Symbol>>,
    /// The continuation binder of a general clause.
    pub resume: Option<Symbol>,
    pub body: Code,
    /// The clause body's window size.
    pub size: u32,
    /// What the body reads from the scope its handler was installed in — copied into the
    /// [`crate::cont::Prompt`] at handle entry, and nothing else.
    pub captures: Rc<Captures>,
    pub span: Span,
}

pub struct ReturnArm {
    pub binder: Symbol,
    pub body: Code,
    pub size: u32,
    /// As [`Clause::captures`].
    pub captures: Rc<Captures>,
    pub span: Span,
}

/// One lowered barrier: its code, and the window size an activation of it needs.
#[derive(Clone)]
pub struct Lowered {
    pub code: Code,
    pub size: u32,
}

/// Grows the host stack rather than bounding the nesting: the parser, inference and normalization
/// all accept an expression of any depth by growing, and a bound here would refuse — on the machine
/// only — a program `ply check` and `ply run` accept.
pub fn lower(e: &Expr) -> Lowered {
    lower_fn(&[], e)
}

/// A function body, whose parameters take the leading slots of its window.
pub fn lower_fn(params: &[Symbol], e: &Expr) -> Lowered {
    let table = crate::slots::resolve(params, e);
    let mut cx = Cx {
        table: &table,
        barrier: 0,
        live: Live::new(table.barriers[0].names.clone()),
    };
    cx.live.declare(params.len());
    let code = lower_in(e, &mut cx);
    Lowered {
        code,
        size: table.barriers[0].size(),
    }
}

/// A body's parameters, shared with the closure the machine builds from it rather than copied into
/// the cache — the cache's own storage is a cost on a request path that lowers nothing, so it is
/// kept to the map's slots.
pub type Params = Rc<Vec<Symbol>>;

/// Lowered bodies, shared by every machine built from one program.
pub struct Lowering<'a> {
    program: &'a Program,
    bodies: RefCell<FxHashMap<usize, (Params, Lowered)>>,
    /// The parameter list of a test body and of a spec clause, so that the overwhelmingly common
    /// empty one is one allocation for the cache rather than one per entry.
    nullary: Params,
    /// Load-bearing, not decoration: a function pointer is contravariant in its argument and
    /// covariant in its result, so `'a` occurring in both makes this field — and therefore the
    /// whole type — invariant in `'a`.
    invariant: PhantomData<fn(&'a Program) -> &'a Program>,
}

impl<'a> Lowering<'a> {
    pub fn for_program(program: &'a Program) -> Lowering<'a> {
        Lowering {
            program,
            bodies: RefCell::new(FxHashMap::default()),
            nullary: Rc::new(Vec::new()),
            invariant: PhantomData,
        }
    }

    /// Whether this cache was taken over `program`.
    pub fn describes(&self, program: &Program) -> bool {
        std::ptr::eq(self.program, program)
    }

    /// [`lower_fn`] over a body that takes no parameters: a test, a law, a spec clause.
    pub fn body(&self, body: &'a Expr) -> Lowered {
        self.of(&self.nullary, body)
    }

    /// [`lower_fn`], skipped when this body has been lowered before.
    pub fn of(&self, params: &Params, body: &'a Expr) -> Lowered {
        let key = std::ptr::from_ref(body) as usize;
        let hit = self
            .bodies
            .borrow()
            .get(&key)
            .filter(|(cached, _)| cached == params)
            .map(|(_, lowered)| lowered.clone());
        if let Some(lowered) = hit {
            return lowered;
        }
        let lowered = lower_fn(params, body);
        self.bodies
            .borrow_mut()
            .insert(key, (Rc::clone(params), lowered.clone()));
        lowered
    }

    /// How many bodies this has lowered.
    pub fn len(&self) -> usize {
        self.bodies.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The lowered body of the last unlowered closure the machine applied.
#[derive(Default)]
pub struct ClosureCode {
    last: Option<(Arc<Expr>, Vec<Symbol>, Lowered)>,
}

impl ClosureCode {
    /// [`lower_fn`], skipped when this is the same body and the same parameters as the previous
    /// call. `pre` is any external bindings the closure carries, lowered as leading parameters so
    /// their occurrences resolve to slots like anything else.
    pub fn of(&mut self, pre: &[Symbol], params: &[Symbol], body: &Arc<Expr>) -> Lowered {
        let combined: Vec<Symbol> = pre.iter().chain(params.iter()).cloned().collect();
        if let Some((held, cached, lowered)) = &self.last
            && Arc::ptr_eq(held, body)
            && cached.as_slice() == combined.as_slice()
        {
            return lowered.clone();
        }
        let lowered = lower_fn(&combined, body);
        self.last = Some((Arc::clone(body), combined, lowered.clone()));
        lowered
    }
}

/// The lowering context: the forward pass's answers, the barrier being lowered, and the backward
/// liveness state.
struct Cx<'t> {
    table: &'t crate::slots::Slots,
    barrier: u32,
    live: Live,
}

fn lower_in(e: &Expr, cx: &mut Cx) -> Code {
    crate::limit::grow(|| lower_node(e, cx))
}

fn node(kind: NodeKind, span: Span) -> Code {
    Rc::new(Node {
        kind,
        span,
        own: Own::Borrowed,
    })
}

/// Lowers a bare-variable occurrence, asking the forward pass for its slot and the backward pass
/// for its ownership.
fn lower_var(e: &Expr, q: &QName, cx: &mut Cx) -> Code {
    let resolved = if q.is_bare() { cx.table.var(e) } else { None };
    let (own, slot) = match resolved {
        Some((barrier, slot)) => {
            debug_assert_eq!(
                barrier, cx.barrier,
                "an occurrence resolved into a barrier it is not being lowered in"
            );
            (cx.live.use_of(q.symbol()), Some(slot))
        }
        None => (Own::Borrowed, None),
    };
    Rc::new(Node {
        kind: NodeKind::Var {
            name: q.clone(),
            slot,
        },
        span: e.span,
        own,
    })
}

/// Children are visited in **reverse** evaluation order throughout, so that `live` holds what the
/// rest of the activation still reads by the time an occurrence is reached.
fn lower_node(e: &Expr, cx: &mut Cx) -> Code {
    let kind = match &e.kind {
        ExprKind::Lit(lit) => NodeKind::Lit(lit.clone(), crate::semantics::literal(lit)),
        ExprKind::Var(q) => return lower_var(e, q, cx),
        ExprKind::Unary { op, operand } => NodeKind::Unary {
            op: *op,
            operand: lower_in(operand, cx),
        },
        ExprKind::Binary { op, lhs, rhs } => {
            let rhs = lower_in(rhs, cx);
            let lhs = lower_in(lhs, cx);
            NodeKind::Binary { op: *op, lhs, rhs }
        }
        ExprKind::Lambda { params, body } => {
            let params: Vec<Symbol> = params.iter().map(|p| p.name.name.clone()).collect();
            let (body, size, captures) = lower_barrier(&params, body, cx, true);
            NodeKind::Lambda {
                params: Rc::new(params),
                body,
                size,
                captures,
            }
        }
        ExprKind::App { func, args, .. } => {
            let args = lower_all(args, cx);
            let func = lower_in(func, cx);
            NodeKind::App { func, args }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            let after = cx.live.snapshot();
            let else_branch = lower_in(else_branch, cx);
            let from_else = cx.live.snapshot();
            cx.live.restore(after);
            let then_branch = lower_in(then_branch, cx);
            cx.live.union(from_else);
            let cond = lower_in(cond, cx);
            NodeKind::If {
                cond,
                then_branch,
                else_branch,
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            let after = cx.live.snapshot();
            let mut lowered: Vec<Arm> = Vec::with_capacity(arms.len());
            let mut merged: Vec<Use> = Vec::new();
            for arm in arms.iter().rev() {
                cx.live.restore(after.clone());
                lowered.push(lower_arm(arm, cx));
                merged.extend(cx.live.snapshot());
            }
            lowered.reverse();
            // A `match` with no arms reads nothing and answers nothing, so the union over its arms
            // is empty and restoring it would forget every binding the enclosing activation still
            // reads.
            cx.live.restore(if lowered.is_empty() {
                after
            } else {
                Vec::new()
            });
            cx.live.union(merged);
            let scrutinee = lower_in(scrutinee, cx);
            NodeKind::Match {
                scrutinee,
                arms: Rc::new(lowered),
            }
        }
        ExprKind::Block { stmts, tail } => {
            let (stmts, tail) = lower_block(stmts, tail.as_deref(), cx);
            NodeKind::Block {
                stmts: Rc::new(stmts),
                tail,
            }
        }
        ExprKind::Record { fields } => {
            let mut lowered: Vec<(Symbol, Code)> = Vec::with_capacity(fields.len());
            for (name, value) in fields.iter().rev() {
                lowered.push((name.name.clone(), lower_in(value, cx)));
            }
            lowered.reverse();
            NodeKind::Record {
                fields: Rc::new(lowered),
            }
        }
        // Unreachable: the sugar is gone before any module reaches this crate, and lowering it as
        // if it were a plain record would drop the base's untouched fields — a wrong value,
        // silently.
        ExprKind::RecordUpdate { .. } => unreachable!(
            "`{{..b, f: e}}` is expanded away by `ply_syntax::parse_module`; the guard is \
             `no_record_update_survives_parse_module_anywhere_in_the_tree`"
        ),
        // Unreachable for the same reason and with the same guard: `?` is a `match` before it
        // leaves the parser, so the machine has no early-exit node to get wrong at a `handle`
        // boundary.
        ExprKind::Try { .. } => unreachable!(
            "`e?` is expanded away by `ply_syntax::parse_module`; the guard is \
             `no_try_survives_parse_module_anywhere_in_the_tree`"
        ),
        ExprKind::Field { base, field } => {
            // A projection of a slot variable is where field-granular liveness lands: the last
            // use of *this field* may take it out of the record in place even while other fields
            // are still read later — the shape no release keyed by a name can free.
            if let ExprKind::Var(q) = &base.kind
                && q.is_bare()
                && let Some((barrier, slot)) = cx.table.var(base)
            {
                debug_assert_eq!(barrier, cx.barrier);
                let own = cx.live.use_field(q.symbol(), &field.name);
                let (base_own, field_own) = match own {
                    Own::Owned => (Own::Owned, Own::Borrowed),
                    Own::OwnedField => (Own::Borrowed, Own::OwnedField),
                    Own::Borrowed => (Own::Borrowed, Own::Borrowed),
                };
                let base = Rc::new(Node {
                    kind: NodeKind::Var {
                        name: q.clone(),
                        slot: Some(slot),
                    },
                    span: base.span,
                    own: base_own,
                });
                return Rc::new(Node {
                    kind: NodeKind::Field {
                        base,
                        field: field.clone(),
                    },
                    span: e.span,
                    own: field_own,
                });
            }
            NodeKind::Field {
                base: lower_in(base, cx),
                field: field.clone(),
            }
        }
        ExprKind::List { items } => NodeKind::List {
            items: lower_all(items, cx),
        },
        ExprKind::Perform {
            effect,
            op,
            resource,
            args,
        } => NodeKind::Perform {
            effect: effect.clone(),
            op: op.name.clone(),
            resource: resource.as_ref().map(|r| r.name.clone()),
            args: lower_all(args, cx),
        },
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            let ret = return_clause.as_deref().map(|rc| lower_return(rc, cx));
            let mut lowered: Vec<Clause> = Vec::with_capacity(clauses.len());
            for clause in clauses.iter().rev() {
                lowered.push(lower_clause(clause, cx));
            }
            lowered.reverse();
            let body = lower_in(body, cx);
            NodeKind::Handle {
                body,
                clauses: Rc::new(lowered),
                ret,
            }
        }
        ExprKind::WithCell {
            resource,
            init,
            binder,
            body,
        } => {
            let slot = cx.table.binder_of_ident(binder);
            let binder = binder.name.clone();
            let shadowed = cx.live.shadow(std::slice::from_ref(&binder));
            cx.live.declare(1);
            let body = lower_in(body, cx);
            cx.live.kill(&binder);
            cx.live.union(shadowed);
            let init = lower_in(init, cx);
            NodeKind::WithCell {
                resource: resource.name.clone(),
                init,
                binder,
                slot,
                body,
            }
        }
        // A barrier: a region's tasks interleave, so a binding in scope may be read by any of them
        // at any point. What the body reads from the enclosing scope it copies in at the region's
        // entry, always by clone — the region runs after the capture.
        ExprKind::Simulate { body } => {
            let (body, size, captures) = lower_barrier(&[], body, cx, false);
            NodeKind::Simulate {
                body,
                size,
                captures,
            }
        }
        // Kept as a node rather than lowered away: the machine opens an arena scope here and closes
        // it at the body's end, and the span is the key `region_kind` filed its decision under.
        ExprKind::WithRegion { body, .. } => NodeKind::WithRegion {
            body: lower_in(body, cx),
        },
    };
    node(kind, e.span)
}

/// A construct whose body may run more than once, or later, or beside another task: a lambda, a
/// handler clause, a `return` clause, a `simulate` region. Lowers the body in its own barrier and
/// answers its window size and capture set.
///
/// `movable` says whether the capture happens at the construct's own position in evaluation order
/// — a lambda — as against at an enclosing construct's entry, before code to its left has run.
fn lower_barrier(
    params: &[Symbol],
    body: &Expr,
    cx: &mut Cx,
    movable: bool,
) -> (Code, u32, Rc<Captures>) {
    let table = cx.table;
    let at = table
        .barrier_of(body)
        .expect("the forward pass walked every barrier body");
    let info = &table.barriers[at as usize];
    let outer = cx.live.open(info.names.clone());
    cx.live.declare(params.len());
    let saved = cx.barrier;
    cx.barrier = at;
    let code = lower_in(body, cx);
    cx.barrier = saved;
    let names: Vec<Symbol> = info
        .captures
        .iter()
        .map(|(_, to)| info.names[*to as usize].clone())
        .collect();
    let owns = cx.live.close_with_owns(outer, &names, movable);
    let captures = if names.is_empty() {
        no_captures()
    } else {
        Rc::new(Captures {
            src: info.captures.iter().map(|(from, _)| *from).collect(),
            dst: info.captures.iter().map(|(_, to)| *to).collect(),
            owns,
            names,
        })
    };
    (code, info.size(), captures)
}

fn lower_all(exprs: &[Expr], cx: &mut Cx) -> Rc<Vec<Code>> {
    let mut out: Vec<Code> = Vec::with_capacity(exprs.len());
    for e in exprs.iter().rev() {
        out.push(lower_in(e, cx));
    }
    out.reverse();
    Rc::new(out)
}

fn lower_pat(pat: &Pattern, cx: &Cx) -> Pat {
    crate::limit::grow(|| match &pat.kind {
        PatternKind::Wildcard => Pat::Wildcard,
        PatternKind::Lit(lit) => Pat::Lit(lit.clone()),
        PatternKind::Var(id) => Pat::Var {
            name: id.clone(),
            slot: cx.table.binder_of_pattern(pat),
        },
        PatternKind::Ctor { name, args } => Pat::Ctor {
            name: name.clone(),
            args: args.iter().map(|a| lower_pat(a, cx)).collect(),
        },
        PatternKind::Record { fields, rest } => Pat::Record {
            fields: fields
                .iter()
                .map(|(n, p)| (n.clone(), lower_pat(p, cx)))
                .collect(),
            rest: *rest,
        },
        PatternKind::List { items, rest } => Pat::List {
            items: items.iter().map(|p| lower_pat(p, cx)).collect(),
            rest: rest.as_ref().map(|r| Box::new(lower_pat(r, cx))),
        },
    })
}

fn lower_arm(arm: &MatchArm, cx: &mut Cx) -> Arm {
    let mut bound = Vec::new();
    pattern_binders(&arm.pat, &mut bound);
    let shadowed = cx.live.shadow(&bound);
    cx.live.declare(bound.len());
    let body = lower_in(&arm.body, cx);
    let guard = arm.guard.as_ref().map(|g| lower_in(g, cx));
    for name in &bound {
        cx.live.kill(name);
    }
    cx.live.union(shadowed);
    Arm {
        pat: lower_pat(&arm.pat, cx),
        guard,
        body,
        span: arm.span,
    }
}

/// The statements and tail of a block.
fn lower_block(stmts: &[AstStmt], tail: Option<&Expr>, cx: &mut Cx) -> (Vec<Stmt>, Option<Code>) {
    let bound: Vec<Vec<Symbol>> = stmts.iter().map(stmt_binders).collect();
    let flat: Vec<Symbol> = bound.iter().flatten().cloned().collect();
    let shadowed = cx.live.shadow(&flat);
    cx.live.declare(bound.iter().map(Vec::len).sum());

    let tail = tail.map(|t| lower_in(t, cx));

    let mut lowered: Vec<Stmt> = Vec::with_capacity(stmts.len());
    for (i, stmt) in stmts.iter().enumerate().rev() {
        lowered.push(match stmt {
            AstStmt::Let {
                pat, value, span, ..
            } => {
                for name in &bound[i] {
                    cx.live.kill(name);
                }
                // Left of this binder the name is the outer binding's again, and it is live exactly
                // when the enclosing activation still reads it.
                cx.live.union(
                    shadowed
                        .iter()
                        .filter(|u| bound[i].contains(&u.name))
                        .cloned()
                        .collect(),
                );
                Stmt::Let {
                    pat: lower_pat(pat, cx),
                    value: lower_in(value, cx),
                    span: *span,
                }
            }
            AstStmt::Expr(e) => Stmt::Expr {
                code: lower_in(e, cx),
            },
        });
    }
    lowered.reverse();
    // Every name here was handed back at the binder that shadowed it; saying so unconditionally is
    // what makes the invariant hold for a binder the walk never crossed rather than only for the
    // ones it did.
    cx.live.union(shadowed);
    (lowered, tail)
}

fn lower_clause(c: &HandleClause, cx: &mut Cx) -> Clause {
    let params: Vec<Symbol> = c.params.iter().map(|p| p.name.clone()).collect();
    let resume = c.resume.as_ref().map(|r| r.name.clone());
    let mut bound = params.clone();
    bound.extend(resume.clone());
    // A clause's captures are copied at handle entry, before the handled body runs, so they are
    // never a move.
    let (body, size, captures) = lower_barrier(&bound, &c.body, cx, false);
    Clause {
        effect: c.effect.clone(),
        op: c.op.name.clone(),
        resource: c.resource.as_ref().map(|r| r.name.clone()),
        params: Rc::new(params),
        resume,
        body,
        size,
        captures,
        span: c.span,
    }
}

fn lower_return(rc: &ReturnClause, cx: &mut Cx) -> Rc<ReturnArm> {
    let binder = rc.binder.name.clone();
    let (body, size, captures) = lower_barrier(std::slice::from_ref(&binder), &rc.body, cx, false);
    Rc::new(ReturnArm {
        binder,
        body,
        size,
        captures,
        span: rc.span,
    })
}

fn stmt_binders(stmt: &AstStmt) -> Vec<Symbol> {
    let mut out = Vec::new();
    if let AstStmt::Let { pat, .. } = stmt {
        pattern_binders(pat, &mut out);
    }
    out
}

/// Every name a pattern can bind.
fn pattern_binders(p: &Pattern, out: &mut Vec<Symbol>) {
    crate::limit::grow(|| match &p.kind {
        PatternKind::Wildcard | PatternKind::Lit(_) => {}
        PatternKind::Var(id) => out.push(id.name.clone()),
        PatternKind::Ctor { args, .. } => {
            for arg in args {
                pattern_binders(arg, out);
            }
        }
        PatternKind::Record { fields, .. } => {
            for (_, pat) in fields {
                pattern_binders(pat, out);
            }
        }
        PatternKind::List { items, rest } => {
            for item in items {
                pattern_binders(item, out);
            }
            if let Some(rest) = rest {
                pattern_binders(rest, out);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{
        at, bin, block, callv, discard, fn_def, int, lam, record, spanned, standalone, var,
    };
    use ply_syntax::ast::Item;

    #[test]
    fn lowering_preserves_spans() {
        let e = spanned(int(3), at(10, 11));
        assert_eq!(lower(&e).code.span, at(10, 11));
    }

    #[test]
    fn a_lambda_body_is_shared_rather_than_cloned_per_reference() {
        let e = lam(&["x"], bin(BinOp::Add, var("x"), int(1)));
        let code = lower(&e).code;
        let NodeKind::Lambda { body, .. } = &code.kind else {
            panic!("expected a lambda");
        };
        let held = body.clone();
        assert!(Rc::ptr_eq(body, &held));
    }

    #[test]
    fn a_block_lowers_its_statements_and_tail() {
        let e = block(vec![discard(int(1))], Some(callv("len", vec![var("xs")])));
        let NodeKind::Block { stmts, tail } = &lower(&e).code.kind else {
            panic!("expected a block");
        };
        assert_eq!(stmts.len(), 1);
        assert!(tail.is_some());
    }

    #[test]
    fn a_record_keeps_its_fields_in_source_order() {
        let e = record(vec![("b", int(2)), ("a", int(1))]);
        let NodeKind::Record { fields, .. } = &lower(&e).code.kind else {
            panic!("expected a record");
        };
        let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["b", "a"]);
    }

    /// The whole mechanism in one shape: a free variable of a lambda takes a slot of the lambda's
    /// own window, copied in at the capture.
    #[test]
    fn a_lambda_captures_its_free_variable_into_its_own_window() {
        let e = block(
            vec![crate::build::letv("n", int(1))],
            Some(lam(&["y"], bin(BinOp::Add, var("y"), var("n")))),
        );
        let lowered = lower(&e);
        let NodeKind::Block { tail, .. } = &lowered.code.kind else {
            panic!("expected a block");
        };
        let NodeKind::Lambda { captures, size, .. } = &tail.as_ref().unwrap().kind else {
            panic!("expected a lambda");
        };
        assert_eq!(captures.len(), 1, "`n` is free in the lambda");
        assert_eq!(*size, 2, "the window holds the parameter and the capture");
    }

    fn body_of<'a>(program: &'a ply_syntax::ast::Program, name: &str) -> &'a Expr {
        program.modules[0]
            .items
            .iter()
            .find_map(|item| match item {
                Item::Fn(f) if f.name.name.as_str() == name => Some(&f.body),
                _ => None,
            })
            .expect("the program declares it")
    }

    #[test]
    fn a_body_lowered_twice_is_lowered_once() {
        let (program, _) = standalone(vec![fn_def("f", &["x"], bin(BinOp::Add, var("x"), int(1)))]);
        let lowering = Lowering::for_program(&program);
        let params: Params = Rc::new(vec![Symbol::new("x")]);
        let first = lowering.of(&params, body_of(&program, "f"));
        let second = lowering.of(&params, body_of(&program, "f"));
        assert!(
            Rc::ptr_eq(&first.code, &second.code),
            "the same body lowered twice produced two different trees"
        );
        assert_eq!(lowering.len(), 1, "one body, {} entries", lowering.len());
    }

    /// The parameter list is what makes a name ownable inside the body, so a cache that answered
    /// for one from the other would hand out a liveness analysis of a different function.
    #[test]
    fn one_body_under_two_parameter_lists_is_lowered_twice() {
        let (program, _) = standalone(vec![fn_def("f", &["x"], var("x"))]);
        let lowering = Lowering::for_program(&program);
        let body = body_of(&program, "f");
        let owned = lowering.of(&Rc::new(vec![Symbol::new("x")]), body);
        let borrowed = lowering.body(body);
        assert!(matches!(owned.code.own, Own::Owned));
        assert!(
            matches!(borrowed.code.own, Own::Borrowed),
            "a body lowered under no parameters was answered from the entry that had one"
        );
    }

    #[test]
    fn a_cache_taken_over_another_program_does_not_describe_this_one() {
        let (one, _) = standalone(vec![fn_def("f", &[], int(1))]);
        let (two, _) = standalone(vec![fn_def("f", &[], int(2))]);
        let lowering = Lowering::for_program(&one);
        assert!(lowering.describes(&one));
        assert!(
            !lowering.describes(&two),
            "a cache over one program claimed to describe another, so a bisection's \
             rebuilt body could be answered from the body it replaced"
        );
    }

    #[test]
    fn the_last_closure_body_is_lowered_once_however_often_it_is_applied() {
        let body = Arc::new(bin(BinOp::Add, var("x"), int(1)));
        let params = [Symbol::new("x")];
        let mut cache = ClosureCode::default();
        let first = cache.of(&[], &params, &body);
        let second = cache.of(&[], &params, &body);
        assert!(Rc::ptr_eq(&first.code, &second.code));

        let other = Arc::new(bin(BinOp::Sub, var("x"), int(1)));
        let third = cache.of(&[], &params, &other);
        assert!(
            !Rc::ptr_eq(&first.code, &third.code),
            "a different body was answered from the previous one's entry"
        );
        assert!(Rc::ptr_eq(
            &third.code,
            &cache.of(&[], &params, &other).code
        ));
    }
}

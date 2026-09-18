//! The AST lowered so a subexpression can be held by a continuation frame, with every binding
//! resolved to a slot and every occurrence marked as a move (last use) or a clone.

use crate::rc::{Live, Own};
use crate::value::Value;
use ply_span::{Span, Symbol};
use ply_syntax::ast::{
    Expr, ExprKind, HandleClause, Ident, MatchArm, Pattern, PatternKind, Program, QName,
    ReturnClause, Stmt as AstStmt,
};
use ply_ty::{BinOp, Lit, UnOp};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;

pub type Code = Rc<Node>;

pub struct Node {
    pub kind: NodeKind,
    pub span: Span,
    /// How a `Var` takes its value; every other node is [`Own::Borrowed`].
    pub own: Own,
}

/// What entering a barrier copies in: per free variable, the outer slot, the inner slot, and how.
#[derive(Debug, Default)]
pub struct Captures {
    pub src: Vec<u32>,
    pub dst: Vec<u32>,
    pub owns: Vec<Own>,
    /// Parallel to the slots.
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

/// The shared empty capture set, so a barrier with no free variables allocates nothing.
pub fn no_captures() -> Rc<Captures> {
    thread_local! {
        static EMPTY: Rc<Captures> = Rc::new(Captures::default());
    }
    EMPTY.with(Rc::clone)
}

pub fn no_captured() -> Rc<[Value]> {
    thread_local! {
        static EMPTY: Rc<[Value]> = Rc::from(Vec::new());
    }
    EMPTY.with(Rc::clone)
}

pub enum NodeKind {
    /// The [`Value`] is built once here rather than per evaluation.
    Lit(Lit, Value),
    Var {
        name: QName,
        /// `None` for a definition, a constructor or a builtin.
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
    /// A literal copying fields from one slot variable (what `{..b, f: e}` expands to); the base
    /// is updated in place when uniquely owned and its fields are exactly `copies` plus `sets`.
    RecordUpdate {
        base: Code,
        copies: Rc<Vec<Ident>>,
        sets: Rc<Vec<(Symbol, Code)>>,
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
        slot: Option<u32>,
        body: Code,
    },
    Simulate {
        body: Code,
        /// The body is its own barrier because a region's tasks interleave.
        size: u32,
        captures: Rc<Captures>,
    },
    WithRegion {
        body: Code,
    },
}

#[derive(Clone)]
pub enum Pat {
    Wildcard,
    /// May be a nullary constructor, so the machine checks the constructor table before binding.
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
    pub resume: Option<Symbol>,
    pub body: Code,
    pub size: u32,
    pub captures: Rc<Captures>,
    pub span: Span,
}

pub struct ReturnArm {
    pub binder: Symbol,
    pub body: Code,
    pub size: u32,
    pub captures: Rc<Captures>,
    pub span: Span,
}

#[derive(Clone)]
pub struct Lowered {
    pub code: Code,
    pub size: u32,
}

/// Grows the host stack rather than bounding nesting, matching the parser and checker.
pub fn lower(e: &Expr) -> Lowered {
    lower_fn(&[], e)
}

/// Parameters take the leading slots of the window.
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

/// Shared with the closure built from the body rather than copied into the cache.
pub type Params = Rc<Vec<Symbol>>;

/// Lowered bodies, shared by every machine built from one program.
pub struct Lowering<'a> {
    program: &'a Program,
    bodies: RefCell<FxHashMap<usize, (Params, Lowered)>>,
    nullary: Params,
    /// Makes the type invariant in `'a`; do not remove.
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

    /// Whether this cache was built for `program`, by pointer identity.
    pub fn describes(&self, program: &Program) -> bool {
        std::ptr::eq(self.program, program)
    }

    pub fn body(&self, body: &'a Expr) -> Lowered {
        self.of(&self.nullary, body)
    }

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
    /// `pre` is the closure's external bindings, lowered as leading parameters.
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

/// Children are visited in reverse evaluation order, so `live` holds what is still read later.
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
        ExprKind::Lambda { params, body, .. } => {
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
            let mut merged: Vec<Symbol> = Vec::new();
            for arm in arms.iter().rev() {
                cx.live.restore(after.clone());
                lowered.push(lower_arm(arm, cx));
                merged.extend(cx.live.snapshot());
            }
            lowered.reverse();
            // With no arms the union is empty, and restoring it would forget every live binding.
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
        ExprKind::Record { fields } => match lower_record_update(fields, cx) {
            Some(update) => update,
            None => {
                let mut lowered: Vec<(Symbol, Code)> = Vec::with_capacity(fields.len());
                for (name, value) in fields.iter().rev() {
                    lowered.push((name.name.clone(), lower_in(value, cx)));
                }
                lowered.reverse();
                NodeKind::Record {
                    fields: Rc::new(lowered),
                }
            }
        },
        ExprKind::RecordUpdate { .. } => unreachable!(
            "`{{..b, f: e}}` is expanded away by `ply_syntax::parse_module`; the guard is \
             `no_record_update_survives_parse_module_anywhere_in_the_tree`"
        ),
        ExprKind::Try { .. } => unreachable!(
            "`e?` is expanded away by `ply_syntax::parse_module`; the guard is \
             `no_try_survives_parse_module_anywhere_in_the_tree`"
        ),
        ExprKind::Field { base, field } => {
            // A projection of a slot variable reads the whole record; its last read moves it.
            if let ExprKind::Var(q) = &base.kind
                && q.is_bare()
                && let Some((barrier, slot)) = cx.table.var(base)
            {
                debug_assert_eq!(barrier, cx.barrier);
                let base = Rc::new(Node {
                    kind: NodeKind::Var {
                        name: q.clone(),
                        slot: Some(slot),
                    },
                    span: base.span,
                    own: cx.live.use_of(q.symbol()),
                });
                return Rc::new(Node {
                    kind: NodeKind::Field {
                        base,
                        field: field.clone(),
                    },
                    span: e.span,
                    own: Own::Borrowed,
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
        // A barrier whose captures are always clones: the region runs after the capture.
        ExprKind::Simulate { body } => {
            let (body, size, captures) = lower_barrier(&[], body, cx, false);
            NodeKind::Simulate {
                body,
                size,
                captures,
            }
        }
        // Kept as a node: the machine opens an arena scope here, keyed by this span.
        ExprKind::WithRegion { body, .. } => NodeKind::WithRegion {
            body: lower_in(body, cx),
        },
    };
    node(kind, e.span)
}

/// A body that may run again, later, or beside another task. `movable` is true when the capture
/// happens at the construct's own position (a lambda), so a capture can be a move.
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

/// A literal whose copied fields all come from one slot variable `b`, read once and last.
fn lower_record_update(fields: &[(Ident, Expr)], cx: &mut Cx) -> Option<NodeKind> {
    let mut base: Option<(&Expr, &QName, (u32, u32))> = None;
    let mut copies: Vec<Ident> = Vec::new();
    let mut sets: Vec<&(Ident, Expr)> = Vec::new();
    for entry in fields {
        let (name, value) = entry;
        if let ExprKind::Field { base: b, field } = &value.kind
            && field.name == name.name
            && let ExprKind::Var(q) = &b.kind
            && q.is_bare()
            && let Some(slot) = cx.table.var(b)
        {
            match base {
                None => base = Some((b, q, slot)),
                Some((_, _, seen)) if seen == slot => {}
                Some(_) => return None,
            }
            copies.push(field.clone());
            continue;
        }
        sets.push(entry);
    }
    // With no copies, a record projected inside a written field can still be reused; the
    // machine's exact-shape check makes any candidate safe.
    let base = base.or_else(|| {
        sets.iter()
            .find_map(|(_, value)| projected_slot_var(value, cx))
    });
    let (b, q, (_, slot)) = base?;
    // Liveness first: the base is read after every written field.
    let own = cx.live.use_of(q.symbol());
    let base = Rc::new(Node {
        kind: NodeKind::Var {
            name: q.clone(),
            slot: Some(slot),
        },
        span: b.span,
        own,
    });
    let mut lowered: Vec<(Symbol, Code)> = Vec::with_capacity(sets.len());
    for (name, value) in sets.into_iter().rev() {
        lowered.push((name.name.clone(), lower_in(value, cx)));
    }
    lowered.reverse();
    Some(NodeKind::RecordUpdate {
        base,
        copies: Rc::new(copies),
        sets: Rc::new(lowered),
    })
}

/// The first outer slot variable projected in `e`, outside barriers and `e`'s own binders.
fn projected_slot_var<'e>(e: &'e Expr, cx: &Cx) -> Option<(&'e Expr, &'e QName, (u32, u32))> {
    let mut inner = Vec::new();
    projected_outer_var(e, cx, &mut inner)
}

fn projected_outer_var<'e>(
    e: &'e Expr,
    cx: &Cx,
    inner: &mut Vec<Symbol>,
) -> Option<(&'e Expr, &'e QName, (u32, u32))> {
    crate::limit::grow(|| {
        if let ExprKind::Field { base, .. } = &e.kind
            && let ExprKind::Var(q) = &base.kind
            && q.is_bare()
            && !inner.contains(q.symbol())
            && let Some(slot) = cx.table.var(base)
        {
            return Some((&**base, q, slot));
        }
        let children: Vec<&Expr> = match &e.kind {
            ExprKind::Lit(_) | ExprKind::Var(_) | ExprKind::Lambda { .. } => Vec::new(),
            ExprKind::Unary { operand, .. } => vec![operand],
            ExprKind::Binary { lhs, rhs, .. } => vec![lhs, rhs],
            ExprKind::App { func, args, .. } => {
                std::iter::once(&**func).chain(args.iter()).collect()
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => vec![cond, then_branch, else_branch],
            ExprKind::Match { scrutinee, .. } => vec![scrutinee],
            ExprKind::Block { stmts, tail } => {
                let mark = inner.len();
                let found = stmts
                    .iter()
                    .find_map(|s| match s {
                        AstStmt::Let { pat, value, .. } => {
                            let found = projected_outer_var(value, cx, inner);
                            pattern_binders(pat, inner);
                            found
                        }
                        AstStmt::Expr(e) => projected_outer_var(e, cx, inner),
                    })
                    .or_else(|| {
                        tail.as_ref()
                            .and_then(|t| projected_outer_var(t, cx, inner))
                    });
                inner.truncate(mark);
                return found;
            }
            ExprKind::Record { fields } => fields.iter().map(|(_, v)| v).collect(),
            ExprKind::RecordUpdate { base, fields } => std::iter::once(&**base)
                .chain(fields.iter().map(|(_, v)| v))
                .collect(),
            ExprKind::Field { base, .. } => vec![base],
            ExprKind::Try { operand } => vec![operand],
            ExprKind::List { items } => items.iter().collect(),
            ExprKind::Perform { args, .. } => args.iter().collect(),
            ExprKind::Handle { body, .. } => vec![body],
            ExprKind::WithCell {
                init, binder, body, ..
            } => {
                let found = projected_outer_var(init, cx, inner);
                if found.is_some() {
                    return found;
                }
                inner.push(binder.name.clone());
                let found = projected_outer_var(body, cx, inner);
                inner.pop();
                return found;
            }
            ExprKind::WithRegion { body, .. } => vec![body],
            ExprKind::Simulate { .. } => Vec::new(),
        };
        children
            .into_iter()
            .find_map(|c| projected_outer_var(c, cx, inner))
    })
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
                // Before this binder the name is the outer binding again, live if still read.
                cx.live.union(
                    shadowed
                        .iter()
                        .filter(|name| bound[i].contains(name))
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
    // Unconditional, so the invariant also holds for binders the walk never crossed.
    cx.live.union(shadowed);
    (lowered, tail)
}

fn lower_clause(c: &HandleClause, cx: &mut Cx) -> Clause {
    let params: Vec<Symbol> = c.params.iter().map(|p| p.name.clone()).collect();
    let resume = c.resume.as_ref().map(|r| r.name.clone());
    let mut bound = params.clone();
    bound.extend(resume.clone());
    // Captures are copied at handle entry, before the body runs, so they are never a move.
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

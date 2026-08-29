//! The AST, lowered so that a subexpression can be held by a continuation
//! frame.
//!
//! A frame has to name the expression it will evaluate next, and a closure has
//! to name its body. Neither can be `&Expr` without a lifetime on [`Value`],
//! which would spread through `Env` and every crate that holds an
//! evaluated value; and neither can be an owned `Expr`, because a frame is
//! pushed per node and cloning a subtree per push is quadratic. `Rc` on every
//! node is the representation that is cheap in both directions: [`Lowering`]
//! runs `lower` once per body per *program*, and after it every handle to a
//! subexpression is one pointer.
//!
//! The shape mirrors `ply_syntax::ast::ExprKind` one to one. Anything the
//! machine never has to suspend inside — patterns, names, literals, operators —
//! is reused from the AST rather than mirrored.
//!
//! Lowering also **is** the reference-counting pass ([`crate::rc`]): it visits
//! children in reverse evaluation order, so at every occurrence it already knows
//! what the rest of the activation still reads. One traversal does both jobs
//! rather than a second traversal of every definition body.

use crate::rc::{Dead, Live, Own};
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
    /// How a `Var` takes its value. [`Own::Borrowed`] on every other node.
    pub own: Own,
}

pub enum NodeKind {
    /// The literal, and the [`Value`] it denotes, built once here rather than
    /// per evaluation.
    ///
    /// A literal is a compile-time constant, so `Str` and `Bytes` were the only
    /// two that ever reached the allocator and they reached it on every
    /// evaluation; cloning the value instead is a refcount bump. Sharing is
    /// unobservable for the same reason [`Value::builtin`] is: no Ply
    /// expression reads an address, and nothing mutates a `Str` or a `Bytes` in
    /// place — the one in-place path, `builtins::push`, is `List`-only and no
    /// literal builds a `List`.
    ///
    /// The `Lit` stays because `crates/ply-codegen-spike` dispatches on it to
    /// choose a Cranelift type; it is outside the workspace, so nothing in
    /// `cargo test --workspace` would catch its loss.
    Lit(Lit, Value),
    Var(QName),
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
        body: Code,
    },
    Simulate {
        body: Code,
    },
    WithRegion {
        body: Code,
    },
}

pub struct Arm {
    pub pat: Pattern,
    pub guard: Option<Code>,
    pub body: Code,
    pub span: Span,
}

pub enum Stmt {
    Let {
        pat: Pattern,
        value: Code,
        span: Span,
        dead: Dead,
    },
    Expr {
        code: Code,
        dead: Dead,
    },
}

impl Stmt {
    pub fn code(&self) -> &Code {
        match self {
            Stmt::Let { value, .. } => value,
            Stmt::Expr { code, .. } => code,
        }
    }

    /// The bindings this statement's end kills.
    ///
    /// The machine drops them out of the scope it hands its *continuation*,
    /// while the statement itself still evaluates under the full scope. That
    /// ordering is the whole of what makes a uniquely-owned value reachable: at
    /// `let ys = push(xs, 1)` the frame waiting for `ys` no longer holds `xs`,
    /// so once the argument evaluation's scope goes the list in hand is the only
    /// one left and `push` rewrites it rather than copying it.
    pub fn dead(&self) -> &Dead {
        match self {
            Stmt::Let { dead, .. } | Stmt::Expr { dead, .. } => dead,
        }
    }
}

pub struct Clause {
    pub effect: QName,
    pub op: Symbol,
    pub resource: Option<Symbol>,
    pub params: Rc<Vec<Symbol>>,
    /// The continuation binder of a general clause. `None` is the
    /// tail-resumptive form, whose body's value goes straight back to the
    /// perform site and which therefore needs no capture at all.
    pub resume: Option<Symbol>,
    pub body: Code,
    pub span: Span,
}

pub struct ReturnArm {
    pub binder: Symbol,
    pub body: Code,
    pub span: Span,
}

/// Grows the host stack rather than bounding the nesting: the parser, inference
/// and normalization all accept an expression of any depth by growing, and a
/// bound here would refuse — on the machine only — a program `ply check` and
/// `ply run` accept. That is an `E0503` divergence on every corpus with a long
/// operator chain in it.
pub fn lower(e: &Expr) -> Code {
    lower_fn(&[], e)
}

/// A function body, whose parameters are bindings of its own scope and are
/// therefore ownable inside it.
pub fn lower_fn(params: &[Symbol], e: &Expr) -> Code {
    let mut ownable: Vec<Symbol> = params.to_vec();
    barrier_binders(e, &mut ownable);
    let mut live = Live::new(ownable);
    live.declare(params.len());
    lower_in(e, &mut live)
}

/// A body's parameters, shared with the closure the machine builds from it
/// rather than copied into the cache — the cache's own storage is a cost on a
/// request path that lowers nothing, so it is kept to the map's slots.
pub type Params = Rc<Vec<Symbol>>;

/// Lowered bodies, shared by every machine built from one program.
///
/// A body is lowered once per *machine* without this, and a machine is built
/// per pool thread per concurrency group, per interleaving of a search, and per
/// sampled case of a spec — so the same traversal runs hundreds of times over
/// one program. Lowering depends on the syntax and on nothing else a machine
/// holds, so one result serves all of them.
///
/// The key is the body's address, and what makes an address an identity is the
/// lifetime rather than the map: everything keyed here is borrowed for `'a` and
/// this cache cannot outlive `'a`, so nothing it has keyed can be freed and
/// something else allocated in its place while it is still readable. That is
/// the reuse a bare pointer key would otherwise admit.
///
/// **That argument holds only because `invariant` below makes `Lowering<'a>`
/// invariant in `'a`, and it is false without it.** With the type covariant —
/// which it was until a regression audit, its only `'a`-carrying field being
/// `&'a Program` — `&Lowering<'long>` coerces to `&Lowering<'short>`, and
/// [`Lowering::of`] takes `&self`, so a shared reference to a long-lived cache
/// accepts a body borrowed for any shorter lifetime at all. Keying a `Box<Expr>`
/// through that coercion and dropping it leaves an entry under a dangling
/// address, and the next allocation to land there is answered with the freed
/// expression's code. Reproduced on the first attempt of a thousand before the
/// field was added. The refusal is machine-checked by the `compile_fail` example
/// below, which runs under `cargo test --workspace` as a doc-test — a variance
/// property is a compile-time property and a `#[test]` cannot observe it.
///
/// ```compile_fail
/// use ply_eval::Lowering;
/// // Covariance in `'a` is what would let a body outlived by the cache be keyed
/// // in it, so this coercion must not compile.
/// fn shrink<'long: 'short, 'short>(c: &'short Lowering<'long>) -> &'short Lowering<'short> {
///     c
/// }
/// ```
///
/// [`Lowering::describes`] is the second guard and it is about intent rather
/// than safety: a cache filled from one program tells a machine over another
/// nothing — two live programs have distinct addresses, so every lookup would
/// miss — and refusing it at the point it is handed over says so once instead of
/// leaving a reader to re-derive it. `params` is stored and compared on a hit
/// rather than assumed, so a caller that lowers one body under two parameter
/// lists gets two answers instead of the first one twice.
pub struct Lowering<'a> {
    program: &'a Program,
    bodies: RefCell<FxHashMap<usize, (Params, Code)>>,
    /// The parameter list of a test body and of a spec clause, so that the
    /// overwhelmingly common empty one is one allocation for the cache rather
    /// than one per entry.
    nullary: Params,
    /// Load-bearing, not decoration: a function pointer is contravariant in its
    /// argument and covariant in its result, so `'a` occurring in both makes
    /// this field — and therefore the whole type — invariant in `'a`. See the
    /// type's own note for what that buys. `PhantomData<&'a mut Program>` does
    /// **not** work here and was the first attempt: `&'a mut T` is invariant in
    /// `T` but still covariant in `'a`.
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

    /// Whether this cache was taken over `program`. See the type's own note for
    /// why this is intent rather than safety.
    pub fn describes(&self, program: &Program) -> bool {
        std::ptr::eq(self.program, program)
    }

    /// [`lower_fn`] over a body that takes no parameters: a test, a law, a spec
    /// clause.
    pub fn body(&self, body: &'a Expr) -> Code {
        self.of(&self.nullary, body)
    }

    /// [`lower_fn`], skipped when this body has been lowered before.
    pub fn of(&self, params: &Params, body: &'a Expr) -> Code {
        let key = std::ptr::from_ref(body) as usize;
        let hit = self
            .bodies
            .borrow()
            .get(&key)
            .filter(|(cached, _)| cached == params)
            .map(|(_, code)| Rc::clone(code));
        if let Some(code) = hit {
            return code;
        }
        let code = lower_fn(params, body);
        self.bodies
            .borrow_mut()
            .insert(key, (Rc::clone(params), Rc::clone(&code)));
        code
    }

    /// How many bodies this has lowered.
    pub fn len(&self) -> usize {
        self.bodies.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The lowered body of the last closure the tree-walker made that the machine
/// applied.
///
/// Such a body is a deep clone rather than a node of the program, so [`Lowering`]
/// cannot hold it: its address is an identity only for as long as something owns
/// it, which is why the `Arc` is kept beside the code. One slot rather than a
/// map, because the shape this exists for is a closure applied in a loop and a
/// map keyed on a value the program can mint would grow without a bound.
#[derive(Default)]
pub struct ClosureCode {
    last: Option<(Arc<Expr>, Vec<Symbol>, Code)>,
}

impl ClosureCode {
    /// [`lower_fn`], skipped when this is the same body and the same parameters
    /// as the previous call.
    pub fn of(&mut self, params: &[Symbol], body: &Arc<Expr>) -> Code {
        if let Some((held, cached, code)) = &self.last
            && Arc::ptr_eq(held, body)
            && cached.as_slice() == params
        {
            return Rc::clone(code);
        }
        let code = lower_fn(params, body);
        self.last = Some((Arc::clone(body), params.to_vec(), Rc::clone(&code)));
        code
    }
}

fn lower_in(e: &Expr, live: &mut Live) -> Code {
    crate::limit::grow(|| lower_node(e, live))
}

fn node(kind: NodeKind, span: Span) -> Code {
    Rc::new(Node {
        kind,
        span,
        own: Own::Borrowed,
    })
}

/// Children are visited in **reverse** evaluation order throughout, so that
/// `live` holds what the rest of the activation still reads by the time an
/// occurrence is reached. Construction order is irrelevant; this order is not.
fn lower_node(e: &Expr, live: &mut Live) -> Code {
    let kind = match &e.kind {
        ExprKind::Lit(lit) => NodeKind::Lit(lit.clone(), crate::interp::literal(lit)),
        ExprKind::Var(q) => {
            let own = if q.is_bare() {
                live.use_of(q.symbol())
            } else {
                Own::Borrowed
            };
            return Rc::new(Node {
                kind: NodeKind::Var(q.clone()),
                span: e.span,
                own,
            });
        }
        ExprKind::Unary { op, operand } => NodeKind::Unary {
            op: *op,
            operand: lower_in(operand, live),
        },
        ExprKind::Binary { op, lhs, rhs } => {
            let rhs = lower_in(rhs, live);
            let lhs = lower_in(lhs, live);
            NodeKind::Binary { op: *op, lhs, rhs }
        }
        ExprKind::Lambda { params, body } => {
            let params: Vec<Symbol> = params.iter().map(|p| p.name.name.clone()).collect();
            let body = lower_barrier(&params, body, live);
            NodeKind::Lambda {
                params: Rc::new(params),
                body,
            }
        }
        ExprKind::App { func, args } => {
            let args = lower_all(args, live);
            let func = lower_in(func, live);
            NodeKind::App { func, args }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            let after = live.snapshot();
            let else_branch = lower_in(else_branch, live);
            let from_else = live.snapshot();
            live.restore(after);
            let then_branch = lower_in(then_branch, live);
            live.union(from_else);
            let cond = lower_in(cond, live);
            NodeKind::If {
                cond,
                then_branch,
                else_branch,
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            let after = live.snapshot();
            let mut lowered: Vec<Arm> = Vec::with_capacity(arms.len());
            let mut merged: Vec<Symbol> = Vec::new();
            for arm in arms.iter().rev() {
                live.restore(after.clone());
                lowered.push(lower_arm(arm, live));
                merged.extend(live.snapshot());
            }
            lowered.reverse();
            // A `match` with no arms reads nothing and answers nothing, so the
            // union over its arms is empty and restoring it would forget every
            // binding the enclosing activation still reads.
            live.restore(if lowered.is_empty() {
                after
            } else {
                Vec::new()
            });
            live.union(merged);
            let scrutinee = lower_in(scrutinee, live);
            NodeKind::Match {
                scrutinee,
                arms: Rc::new(lowered),
            }
        }
        ExprKind::Block { stmts, tail } => {
            let (stmts, tail) = lower_block(stmts, tail.as_deref(), live);
            NodeKind::Block {
                stmts: Rc::new(stmts),
                tail,
            }
        }
        ExprKind::Record { fields } => {
            let mut lowered: Vec<(Symbol, Code)> = Vec::with_capacity(fields.len());
            for (name, value) in fields.iter().rev() {
                lowered.push((name.name.clone(), lower_in(value, live)));
            }
            lowered.reverse();
            NodeKind::Record {
                fields: Rc::new(lowered),
            }
        }
        // Unreachable: the sugar is gone before any module reaches this crate,
        // and lowering it as if it were a plain record would drop the base's
        // untouched fields — a wrong value, silently.
        ExprKind::RecordUpdate { .. } => unreachable!(
            "`{{..b, f: e}}` is expanded away by `ply_syntax::parse_module`; the guard is \
             `no_record_update_survives_parse_module_anywhere_in_the_tree`"
        ),
        ExprKind::Field { base, field } => NodeKind::Field {
            base: lower_in(base, live),
            field: field.clone(),
        },
        ExprKind::List { items } => NodeKind::List {
            items: lower_all(items, live),
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
            args: lower_all(args, live),
        },
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            let ret = return_clause.as_deref().map(|rc| lower_return(rc, live));
            let mut lowered: Vec<Clause> = Vec::with_capacity(clauses.len());
            for clause in clauses.iter().rev() {
                lowered.push(lower_clause(clause, live));
            }
            lowered.reverse();
            let body = lower_in(body, live);
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
            let binder = binder.name.clone();
            let shadowed = live.shadow(std::slice::from_ref(&binder));
            live.declare(1);
            let body = lower_in(body, live);
            live.kill(&binder);
            live.union(shadowed);
            let init = lower_in(init, live);
            NodeKind::WithCell {
                resource: resource.name.clone(),
                init,
                binder,
                body,
            }
        }
        // A barrier: a region's tasks interleave, so a binding in scope may be
        // read by any of them at any point and no occurrence inside is the last
        // use of anything outside.
        ExprKind::Simulate { body } => NodeKind::Simulate {
            body: lower_barrier(&[], body, live),
        },
        // Kept as a node rather than lowered away: the machine opens an arena
        // scope here and closes it at the body's end, and the span is the key
        // `region_kind` filed its decision under.
        ExprKind::WithRegion { body, .. } => NodeKind::WithRegion {
            body: lower_in(body, live),
        },
    };
    node(kind, e.span)
}

/// A construct whose body may run more than once, or later, or beside another
/// task: a lambda, a handler clause, a `return` clause, a `simulate` region.
///
/// Its free variables become reads *at* the construct, and never last ones: what
/// captured the body holds them for as long as it lives, which is not something
/// an analysis of this body can bound.
fn lower_barrier(params: &[Symbol], body: &Expr, live: &mut Live) -> Code {
    let mut ownable: Vec<Symbol> = params.to_vec();
    barrier_binders(body, &mut ownable);
    let outer = live.open(ownable);
    live.declare(params.len());
    let code = lower_in(body, live);
    for p in params {
        live.kill(p);
    }
    live.close(outer);
    code
}

fn lower_all(exprs: &[Expr], live: &mut Live) -> Rc<Vec<Code>> {
    let mut out: Vec<Code> = Vec::with_capacity(exprs.len());
    for e in exprs.iter().rev() {
        out.push(lower_in(e, live));
    }
    out.reverse();
    Rc::new(out)
}

fn lower_arm(arm: &MatchArm, live: &mut Live) -> Arm {
    let mut bound = Vec::new();
    pattern_binders(&arm.pat, &mut bound);
    let shadowed = live.shadow(&bound);
    live.declare(bound.len());
    let body = lower_in(&arm.body, live);
    let guard = arm.guard.as_ref().map(|g| lower_in(g, live));
    for name in &bound {
        live.kill(name);
    }
    live.union(shadowed);
    Arm {
        pat: arm.pat.clone(),
        guard,
        body,
        span: arm.span,
    }
}

/// The statements and tail of a block, with the bindings each statement's end
/// kills.
fn lower_block(
    stmts: &[AstStmt],
    tail: Option<&Expr>,
    live: &mut Live,
) -> (Vec<Stmt>, Option<Code>) {
    let bound: Vec<Vec<Symbol>> = stmts.iter().map(stmt_binders).collect();
    let flat: Vec<Symbol> = bound.iter().flatten().cloned().collect();
    let shadowed = live.shadow(&flat);
    live.declare(bound.iter().map(Vec::len).sum());

    let tail = tail.map(|t| lower_in(t, live));

    let n = stmts.len();
    let mut lowered: Vec<Option<Stmt>> = (0..n).map(|_| None).collect();
    // `after[i]` is what is still read once statement `i` has finished.
    let mut after: Vec<Vec<Symbol>> = (0..n).map(|_| Vec::new()).collect();
    for i in (0..n).rev() {
        after[i] = live.snapshot();
        lowered[i] = Some(match &stmts[i] {
            AstStmt::Let {
                pat, value, span, ..
            } => {
                for name in &bound[i] {
                    live.kill(name);
                }
                // Left of this binder the name is the outer binding's again, and
                // it is live exactly when the enclosing activation still reads
                // it.
                live.union(
                    shadowed
                        .iter()
                        .filter(|n| bound[i].contains(n))
                        .cloned()
                        .collect(),
                );
                Stmt::Let {
                    pat: pat.clone(),
                    value: lower_in(value, live),
                    span: *span,
                    dead: crate::rc::no_dead(),
                }
            }
            AstStmt::Expr(e) => Stmt::Expr {
                code: lower_in(e, live),
                dead: crate::rc::no_dead(),
            },
        });
    }
    let entry = live.snapshot();

    let mut cumulative: Vec<Symbol> = Vec::new();
    let mut out: Vec<Stmt> = Vec::with_capacity(n);
    for i in 0..n {
        for name in &bound[i] {
            if !cumulative.contains(name) {
                cumulative.push(name.clone());
            }
        }
        let before = if i == 0 { &entry } else { &after[i - 1] };
        // Bound at or before this statement, not read after it, and either
        // introduced by it or still read up to it — so a binding is named by
        // exactly the statement that kills it and by no other.
        let dead = live.released(
            cumulative
                .iter()
                .filter(|name| !after[i].contains(name))
                .filter(|name| bound[i].contains(name) || before.contains(name))
                .cloned()
                .collect(),
        );
        out.push(
            match lowered[i].take().expect("every statement was lowered") {
                Stmt::Let {
                    pat, value, span, ..
                } => Stmt::Let {
                    pat,
                    value,
                    span,
                    dead,
                },
                Stmt::Expr { code, .. } => Stmt::Expr { code, dead },
            },
        );
    }
    // Every name here was handed back at the binder that shadowed it; saying so
    // unconditionally is what makes the invariant hold for a binder the walk
    // never crossed rather than only for the ones it did.
    live.union(shadowed);
    (out, tail)
}

fn lower_clause(c: &HandleClause, live: &mut Live) -> Clause {
    let params: Vec<Symbol> = c.params.iter().map(|p| p.name.clone()).collect();
    let resume = c.resume.as_ref().map(|r| r.name.clone());
    let mut bound = params.clone();
    bound.extend(resume.clone());
    let body = lower_barrier(&bound, &c.body, live);
    Clause {
        effect: c.effect.clone(),
        op: c.op.name.clone(),
        resource: c.resource.as_ref().map(|r| r.name.clone()),
        params: Rc::new(params),
        resume,
        body,
        span: c.span,
    }
}

fn lower_return(rc: &ReturnClause, live: &mut Live) -> Rc<ReturnArm> {
    let binder = rc.binder.name.clone();
    let body = lower_barrier(std::slice::from_ref(&binder), &rc.body, live);
    Rc::new(ReturnArm {
        binder,
        body,
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
///
/// A bare `Var` pattern naming a nullary constructor is a constructor pattern
/// and binds nothing, and this cannot tell the two apart — resolution needs the
/// module, which lowering does not have. Over-approximating is safe in both
/// directions that matter: releasing a name no scope holds does nothing, and
/// ownership is a hint the machine re-checks against the scope itself.
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

/// Every name bound anywhere inside one barrier, not crossing into another.
fn barrier_binders(e: &Expr, out: &mut Vec<Symbol>) {
    crate::limit::grow(|| match &e.kind {
        ExprKind::Lit(_) | ExprKind::Var(_) | ExprKind::Lambda { .. } => {}
        ExprKind::Unary { operand, .. } => barrier_binders(operand, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            barrier_binders(lhs, out);
            barrier_binders(rhs, out);
        }
        ExprKind::App { func, args } => {
            barrier_binders(func, out);
            for a in args {
                barrier_binders(a, out);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            barrier_binders(cond, out);
            barrier_binders(then_branch, out);
            barrier_binders(else_branch, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            barrier_binders(scrutinee, out);
            for arm in arms {
                pattern_binders(&arm.pat, out);
                if let Some(g) = &arm.guard {
                    barrier_binders(g, out);
                }
                barrier_binders(&arm.body, out);
            }
        }
        ExprKind::Block { stmts, tail } => {
            for stmt in stmts {
                match stmt {
                    AstStmt::Let { pat, value, .. } => {
                        pattern_binders(pat, out);
                        barrier_binders(value, out);
                    }
                    AstStmt::Expr(e) => barrier_binders(e, out),
                }
            }
            if let Some(t) = tail {
                barrier_binders(t, out);
            }
        }
        ExprKind::Record { fields } => {
            for (_, value) in fields {
                barrier_binders(value, out);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            barrier_binders(base, out);
            for (_, value) in fields {
                barrier_binders(value, out);
            }
        }
        ExprKind::Field { base, .. } => barrier_binders(base, out),
        ExprKind::List { items } => {
            for item in items {
                barrier_binders(item, out);
            }
        }
        ExprKind::Perform { args, .. } => {
            for a in args {
                barrier_binders(a, out);
            }
        }
        ExprKind::Handle { body, .. } => barrier_binders(body, out),
        ExprKind::WithCell {
            init, binder, body, ..
        } => {
            barrier_binders(init, out);
            out.push(binder.name.clone());
            barrier_binders(body, out);
        }
        ExprKind::WithRegion { body, .. } => barrier_binders(body, out),
        ExprKind::Simulate { .. } => {}
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
        assert_eq!(lower(&e).span, at(10, 11));
    }

    #[test]
    fn a_lambda_body_is_shared_rather_than_cloned_per_reference() {
        let e = lam(&["x"], bin(BinOp::Add, var("x"), int(1)));
        let code = lower(&e);
        let NodeKind::Lambda { body, .. } = &code.kind else {
            panic!("expected a lambda");
        };
        let held = body.clone();
        assert!(Rc::ptr_eq(body, &held));
    }

    #[test]
    fn a_block_lowers_its_statements_and_tail() {
        let e = block(vec![discard(int(1))], Some(callv("len", vec![var("xs")])));
        let NodeKind::Block { stmts, tail } = &lower(&e).kind else {
            panic!("expected a block");
        };
        assert_eq!(stmts.len(), 1);
        assert!(tail.is_some());
    }

    #[test]
    fn a_record_keeps_its_fields_in_source_order() {
        let e = record(vec![("b", int(2)), ("a", int(1))]);
        let NodeKind::Record { fields } = &lower(&e).kind else {
            panic!("expected a record");
        };
        let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["b", "a"]);
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
            Rc::ptr_eq(&first, &second),
            "the same body lowered twice produced two different trees"
        );
        assert_eq!(lowering.len(), 1, "one body, {} entries", lowering.len());
    }

    /// The parameter list is what makes a name ownable inside the body, so a
    /// cache that answered for one from the other would hand out a liveness
    /// analysis of a different function.
    #[test]
    fn one_body_under_two_parameter_lists_is_lowered_twice() {
        let (program, _) = standalone(vec![fn_def("f", &["x"], var("x"))]);
        let lowering = Lowering::for_program(&program);
        let body = body_of(&program, "f");
        let owned = lowering.of(&Rc::new(vec![Symbol::new("x")]), body);
        let borrowed = lowering.body(body);
        assert!(matches!(owned.own, Own::Owned));
        assert!(
            matches!(borrowed.own, Own::Borrowed),
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
        let first = cache.of(&params, &body);
        let second = cache.of(&params, &body);
        assert!(Rc::ptr_eq(&first, &second));

        let other = Arc::new(bin(BinOp::Sub, var("x"), int(1)));
        let third = cache.of(&params, &other);
        assert!(
            !Rc::ptr_eq(&first, &third),
            "a different body was answered from the previous one's entry"
        );
        assert!(Rc::ptr_eq(&third, &cache.of(&params, &other)));
    }
}

//! Rewrites over a definition's syntax before it is lowered (ADR 0035, sequence step 5). A small
//! pure callee in the caller's own module is inlined at its call, with every name the callee
//! binds made fresh so nothing in the caller is shadowed or captured; a block that a `let` binds
//! is flattened into the block around it; and a record a `let` binds that is only ever read by
//! field becomes one binding per field. Together they take a record built and read within one
//! function — the mixing step of a hash, a state threaded through a helper — off the heap and
//! into registers. Nothing here changes what a body means: the checker saw the original, the
//! interpreter runs the original, and the differential corpus holds the two engines together.

use crate::source::Source;
use ply_span::Symbol;
use ply_syntax::ast::{
    Expr, ExprKind, FnDef, HandleClause, Ident, Item, MatchArm, Module, Pattern, PatternKind,
    QName, ReturnClause, Stmt,
};
use std::collections::HashSet;

/// The most syntax nodes a callee may have to be inlined.
const INLINE_BUDGET: usize = 64;
/// How many times a callee's own calls are inlined in turn.
const INLINE_DEPTH: usize = 2;

/// `def`'s body, rewritten.
pub fn optimize(loaded: &Source, module_index: usize, def: &FnDef) -> Expr {
    let module = &loaded.program.modules[module_index];
    let mut cx = Cx { module, fresh: 0 };
    let mut scope: Vec<Symbol> = def.params.iter().map(|p| p.name.name.clone()).collect();
    let body = cx.inline(&def.body, &def.name.name, INLINE_DEPTH, &mut scope);
    let body = scalarize(body);
    if std::env::var("PLY_OPT_DUMP").is_ok_and(|want| want == def.name.name.as_str()) {
        eprintln!("optimized `{}`:\n{body:#?}", def.name.name);
    }
    body
}

struct Cx<'a> {
    module: &'a Module,
    fresh: u32,
}

impl<'a> Cx<'a> {
    fn fresh(&mut self, base: &Symbol) -> Symbol {
        self.fresh += 1;
        Symbol::new(format!("{base}${}", self.fresh))
    }

    /// The function `name` denotes in this module, if it is one this pass may inline.
    fn callee(&self, name: &Symbol, caller: &Symbol, arity: usize) -> Option<&'a FnDef> {
        for item in &self.module.items {
            if let Item::Fn(def) = item
                && def.name.name == *name
            {
                let plain = def.spec.is_empty()
                    && def.params.len() == arity
                    && def.params.iter().all(|p| p.default.is_none())
                    && def.name.name != *caller
                    && size(&def.body) <= INLINE_BUDGET
                    && inlinable(&def.body);
                return plain.then_some(def);
            }
        }
        None
    }

    fn inline(&mut self, e: &Expr, caller: &Symbol, depth: usize, scope: &mut Vec<Symbol>) -> Expr {
        let span = e.span;
        let site = match &e.kind {
            ExprKind::App { func, args, named } if named.is_empty() && depth > 0 => {
                match &func.kind {
                    ExprKind::Var(q) if q.is_bare() && !scope.contains(&q.name.name) => self
                        .callee(&q.name.name, caller, args.len())
                        .map(|def| (def, args)),
                    _ => None,
                }
            }
            _ => None,
        };
        let kind = match site {
            Some((def, args)) => {
                // Refused when a name the callee reads from its module is a local here, since
                // inside the caller that name would mean the local.
                let params: Vec<Symbol> = def.params.iter().map(|p| p.name.name.clone()).collect();
                let free = free_names(&def.body, &params);
                if free.iter().any(|n| scope.contains(n)) {
                    return self.inline_children(e, caller, depth, scope);
                }
                let def = def.clone();
                let args: Vec<Expr> = args
                    .iter()
                    .map(|a| self.inline(a, caller, depth, scope))
                    .collect();
                // Every name the callee binds is made fresh, so its parameters cannot be read by
                // an argument and its `let`s cannot shadow anything of the caller's.
                let mut env: Vec<(Symbol, Symbol)> = Vec::new();
                let mut stmts = Vec::with_capacity(args.len());
                let mut inner_scope: Vec<Symbol> = Vec::new();
                for (param, arg) in def.params.iter().zip(args) {
                    let fresh = self.fresh(&param.name.name);
                    stmts.push(Stmt::Let {
                        pat: Pattern {
                            kind: PatternKind::Var(Ident {
                                name: fresh.clone(),
                                span: param.name.span,
                            }),
                            span: param.name.span,
                        },
                        ty: param.ty.clone(),
                        value: Box::new(arg),
                        span: param.span,
                    });
                    env.push((param.name.name.clone(), fresh.clone()));
                    inner_scope.push(fresh);
                }
                let body = self.freshen(&def.body, &mut env);
                let body = self.inline(&body, &def.name.name, depth - 1, &mut inner_scope);
                ExprKind::Block {
                    stmts,
                    tail: Some(Box::new(body)),
                }
            }
            _ => return self.inline_children(e, caller, depth, scope),
        };
        Expr { kind, span }
    }

    /// `inline` over an expression's children, with the scope kept.
    fn inline_children(
        &mut self,
        e: &Expr,
        caller: &Symbol,
        depth: usize,
        scope: &mut Vec<Symbol>,
    ) -> Expr {
        let mut go = |e: &Expr, scope: &mut Vec<Symbol>| self.inline(e, caller, depth, scope);
        let kind = match &e.kind {
            ExprKind::Lit(_) | ExprKind::Var(_) => e.kind.clone(),
            ExprKind::Binary { op, lhs, rhs } => ExprKind::Binary {
                op: *op,
                lhs: Box::new(go(lhs, scope)),
                rhs: Box::new(go(rhs, scope)),
            },
            ExprKind::Unary { op, operand } => ExprKind::Unary {
                op: *op,
                operand: Box::new(go(operand, scope)),
            },
            ExprKind::Lambda { params, body, ret } => {
                let mark = scope.len();
                scope.extend(params.iter().map(|p| p.name.name.clone()));
                let body = go(body, scope);
                scope.truncate(mark);
                ExprKind::Lambda {
                    params: params.clone(),
                    body: Box::new(body),
                    ret: ret.clone(),
                }
            }
            ExprKind::App { func, args, named } => ExprKind::App {
                func: Box::new(go(func, scope)),
                args: args.iter().map(|a| go(a, scope)).collect(),
                named: named.clone(),
            },
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => ExprKind::If {
                cond: Box::new(go(cond, scope)),
                then_branch: Box::new(go(then_branch, scope)),
                else_branch: Box::new(go(else_branch, scope)),
            },
            ExprKind::Match { scrutinee, arms } => ExprKind::Match {
                scrutinee: Box::new(go(scrutinee, scope)),
                arms: arms
                    .iter()
                    .map(|arm| {
                        let mark = scope.len();
                        binders(&arm.pat, scope);
                        let guard = arm.guard.as_ref().map(|g| go(g, scope));
                        let body = go(&arm.body, scope);
                        scope.truncate(mark);
                        MatchArm {
                            pat: arm.pat.clone(),
                            guard,
                            body,
                            span: arm.span,
                        }
                    })
                    .collect(),
            },
            ExprKind::Block { stmts, tail } => {
                let mark = scope.len();
                let stmts = stmts
                    .iter()
                    .map(|s| match s {
                        Stmt::Let {
                            pat,
                            ty,
                            value,
                            span,
                        } => {
                            let value = go(value, scope);
                            binders(pat, scope);
                            Stmt::Let {
                                pat: pat.clone(),
                                ty: ty.clone(),
                                value: Box::new(value),
                                span: *span,
                            }
                        }
                        Stmt::Expr(x) => Stmt::Expr(go(x, scope)),
                    })
                    .collect();
                let tail = tail.as_ref().map(|t| Box::new(go(t, scope)));
                scope.truncate(mark);
                ExprKind::Block { stmts, tail }
            }
            ExprKind::Record { fields } => ExprKind::Record {
                fields: fields
                    .iter()
                    .map(|(n, x)| (n.clone(), go(x, scope)))
                    .collect(),
            },
            ExprKind::RecordUpdate { base, fields } => ExprKind::RecordUpdate {
                base: Box::new(go(base, scope)),
                fields: fields
                    .iter()
                    .map(|(n, x)| (n.clone(), go(x, scope)))
                    .collect(),
            },
            ExprKind::Field { base, field } => ExprKind::Field {
                base: Box::new(go(base, scope)),
                field: field.clone(),
            },
            ExprKind::Try { operand } => ExprKind::Try {
                operand: Box::new(go(operand, scope)),
            },
            ExprKind::List { items } => ExprKind::List {
                items: items.iter().map(|x| go(x, scope)).collect(),
            },
            // Effects and the constructs that scope them are refused by the fragment; they are
            // carried through untouched.
            other => other.clone(),
        };
        Expr { kind, span: e.span }
    }

    /// The expression with every name it binds replaced by a fresh one, consistently, with the
    /// scope's shadowing kept: a name bound twice is two fresh names.
    fn freshen(&mut self, e: &Expr, env: &mut Vec<(Symbol, Symbol)>) -> Expr {
        let lookup = |env: &Vec<(Symbol, Symbol)>, name: &Symbol| {
            env.iter()
                .rev()
                .find(|(o, _)| o == name)
                .map(|(_, f)| f.clone())
        };
        let kind = match &e.kind {
            ExprKind::Lit(l) => ExprKind::Lit(l.clone()),
            ExprKind::Var(q) => match (q.is_bare(), lookup(env, &q.name.name)) {
                (true, Some(fresh)) => ExprKind::Var(QName::bare(Ident {
                    name: fresh,
                    span: q.name.span,
                })),
                _ => ExprKind::Var(q.clone()),
            },
            ExprKind::Binary { op, lhs, rhs } => ExprKind::Binary {
                op: *op,
                lhs: Box::new(self.freshen(lhs, env)),
                rhs: Box::new(self.freshen(rhs, env)),
            },
            ExprKind::Unary { op, operand } => ExprKind::Unary {
                op: *op,
                operand: Box::new(self.freshen(operand, env)),
            },
            ExprKind::Lambda { params, body, ret } => {
                let mark = env.len();
                let params = params
                    .iter()
                    .map(|p| {
                        let fresh = self.fresh(&p.name.name);
                        env.push((p.name.name.clone(), fresh.clone()));
                        let mut p = p.clone();
                        p.name.name = fresh;
                        p
                    })
                    .collect();
                let body = self.freshen(body, env);
                env.truncate(mark);
                ExprKind::Lambda {
                    params,
                    body: Box::new(body),
                    ret: ret.clone(),
                }
            }
            ExprKind::App { func, args, named } => ExprKind::App {
                func: Box::new(self.freshen(func, env)),
                args: args.iter().map(|a| self.freshen(a, env)).collect(),
                named: named.clone(),
            },
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => ExprKind::If {
                cond: Box::new(self.freshen(cond, env)),
                then_branch: Box::new(self.freshen(then_branch, env)),
                else_branch: Box::new(self.freshen(else_branch, env)),
            },
            ExprKind::Match { scrutinee, arms } => ExprKind::Match {
                scrutinee: Box::new(self.freshen(scrutinee, env)),
                arms: arms
                    .iter()
                    .map(|arm| {
                        let mark = env.len();
                        let pat = self.freshen_pat(&arm.pat, env);
                        let guard = arm.guard.as_ref().map(|g| self.freshen(g, env));
                        let body = self.freshen(&arm.body, env);
                        env.truncate(mark);
                        MatchArm {
                            pat,
                            guard,
                            body,
                            span: arm.span,
                        }
                    })
                    .collect(),
            },
            ExprKind::Block { stmts, tail } => {
                let mark = env.len();
                let stmts = stmts
                    .iter()
                    .map(|s| match s {
                        Stmt::Let {
                            pat,
                            ty,
                            value,
                            span,
                        } => {
                            let value = self.freshen(value, env);
                            let pat = self.freshen_pat(pat, env);
                            Stmt::Let {
                                pat,
                                ty: ty.clone(),
                                value: Box::new(value),
                                span: *span,
                            }
                        }
                        Stmt::Expr(x) => Stmt::Expr(self.freshen(x, env)),
                    })
                    .collect();
                let tail = tail.as_ref().map(|t| Box::new(self.freshen(t, env)));
                env.truncate(mark);
                ExprKind::Block { stmts, tail }
            }
            ExprKind::Record { fields } => ExprKind::Record {
                fields: fields
                    .iter()
                    .map(|(n, x)| (n.clone(), self.freshen(x, env)))
                    .collect(),
            },
            ExprKind::RecordUpdate { base, fields } => ExprKind::RecordUpdate {
                base: Box::new(self.freshen(base, env)),
                fields: fields
                    .iter()
                    .map(|(n, x)| (n.clone(), self.freshen(x, env)))
                    .collect(),
            },
            ExprKind::Field { base, field } => ExprKind::Field {
                base: Box::new(self.freshen(base, env)),
                field: field.clone(),
            },
            ExprKind::Try { operand } => ExprKind::Try {
                operand: Box::new(self.freshen(operand, env)),
            },
            ExprKind::List { items } => ExprKind::List {
                items: items.iter().map(|x| self.freshen(x, env)).collect(),
            },
            other => other.clone(),
        };
        Expr { kind, span: e.span }
    }

    /// The pattern with its binders made fresh and entered into `env`. A name that is a
    /// constructor is not a binder, and the pattern keeps it: this pass cannot tell, so a
    /// bare name that begins with an upper-case letter is left alone, which is the language's
    /// own convention for a constructor.
    fn freshen_pat(&mut self, p: &Pattern, env: &mut Vec<(Symbol, Symbol)>) -> Pattern {
        let kind = match &p.kind {
            PatternKind::Wildcard => PatternKind::Wildcard,
            PatternKind::Lit(l) => PatternKind::Lit(l.clone()),
            PatternKind::Var(id) => {
                if id
                    .name
                    .as_str()
                    .starts_with(|c: char| c.is_ascii_uppercase())
                {
                    PatternKind::Var(id.clone())
                } else {
                    let fresh = self.fresh(&id.name);
                    env.push((id.name.clone(), fresh.clone()));
                    PatternKind::Var(Ident {
                        name: fresh,
                        span: id.span,
                    })
                }
            }
            PatternKind::Ctor { name, args } => PatternKind::Ctor {
                name: name.clone(),
                args: args.iter().map(|a| self.freshen_pat(a, env)).collect(),
            },
            PatternKind::Record { fields, rest } => PatternKind::Record {
                fields: fields
                    .iter()
                    .map(|(n, sub)| (n.clone(), self.freshen_pat(sub, env)))
                    .collect(),
                rest: *rest,
            },
            PatternKind::List { items, rest } => PatternKind::List {
                items: items.iter().map(|a| self.freshen_pat(a, env)).collect(),
                rest: rest.as_ref().map(|r| Box::new(self.freshen_pat(r, env))),
            },
        };
        Pattern { kind, span: p.span }
    }
}

/// Whether a body is one this pass inlines: no effect, handler, cell, region or simulation, which
/// the fragment refuses in any case, and no lambda, whose captures the lowering resolves by
/// position.
fn inlinable(e: &Expr) -> bool {
    let mut ok = true;
    walk(e, &mut |x| {
        if matches!(
            x.kind,
            ExprKind::Perform { .. }
                | ExprKind::Handle { .. }
                | ExprKind::Lambda { .. }
                | ExprKind::WithCell { .. }
                | ExprKind::Simulate { .. }
                | ExprKind::WithRegion { .. }
        ) {
            ok = false;
        }
    });
    ok
}

fn size(e: &Expr) -> usize {
    let mut n = 0;
    walk(e, &mut |_| n += 1);
    n
}

/// Every expression under `e`, `e` included, in no particular order.
fn walk(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Var(_) => {}
        ExprKind::Binary { lhs, rhs, .. } => {
            walk(lhs, f);
            walk(rhs, f);
        }
        ExprKind::Unary { operand, .. } => walk(operand, f),
        ExprKind::Lambda { body, .. } => walk(body, f),
        ExprKind::App { func, args, named } => {
            walk(func, f);
            args.iter().for_each(|a| walk(a, f));
            named.iter().for_each(|a| walk(&a.value, f));
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            walk(cond, f);
            walk(then_branch, f);
            walk(else_branch, f);
        }
        ExprKind::Match { scrutinee, arms } => {
            walk(scrutinee, f);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk(g, f);
                }
                walk(&arm.body, f);
            }
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match s {
                    Stmt::Let { value, .. } => walk(value, f),
                    Stmt::Expr(x) => walk(x, f),
                }
            }
            if let Some(t) = tail {
                walk(t, f);
            }
        }
        ExprKind::Record { fields } | ExprKind::RecordUpdate { fields, .. } => {
            if let ExprKind::RecordUpdate { base, .. } = &e.kind {
                walk(base, f);
            }
            fields.iter().for_each(|(_, x)| walk(x, f));
        }
        ExprKind::Field { base, .. } => walk(base, f),
        ExprKind::Try { operand } => walk(operand, f),
        ExprKind::List { items } => items.iter().for_each(|x| walk(x, f)),
        ExprKind::Perform { args, .. } => args.iter().for_each(|x| walk(x, f)),
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            walk(body, f);
            for HandleClause { body, .. } in clauses {
                walk(body, f);
            }
            if let Some(ReturnClause { body, .. }) = return_clause.as_deref() {
                walk(body, f);
            }
        }
        ExprKind::WithCell { init, body, .. } => {
            walk(init, f);
            walk(body, f);
        }
        ExprKind::Simulate { body, .. } | ExprKind::WithRegion { body, .. } => walk(body, f),
    }
}

/// Whether every name these statements bind was made by `Cx::fresh`.
fn all_fresh(stmts: &[Stmt]) -> bool {
    let mut bound = Vec::new();
    for s in stmts {
        if let Stmt::Let { pat, .. } = s {
            binders(pat, &mut bound);
        }
    }
    bound.iter().all(|n| n.as_str().contains('$'))
}

/// The names a pattern binds, pushed onto `scope`.
fn binders(p: &Pattern, scope: &mut Vec<Symbol>) {
    match &p.kind {
        PatternKind::Wildcard | PatternKind::Lit(_) => {}
        PatternKind::Var(id) => scope.push(id.name.clone()),
        PatternKind::Ctor { args, .. } => args.iter().for_each(|a| binders(a, scope)),
        PatternKind::Record { fields, .. } => fields.iter().for_each(|(_, a)| binders(a, scope)),
        PatternKind::List { items, rest } => {
            items.iter().for_each(|a| binders(a, scope));
            if let Some(r) = rest {
                binders(r, scope);
            }
        }
    }
}

/// The bare names `e` reads that it does not bind itself, `bound` excluded: what the body means
/// by reference to its module.
fn free_names(e: &Expr, bound: &[Symbol]) -> HashSet<Symbol> {
    fn go(e: &Expr, scope: &mut Vec<Symbol>, out: &mut HashSet<Symbol>) {
        match &e.kind {
            ExprKind::Var(q) if q.is_bare() => {
                if !scope.contains(&q.name.name) {
                    out.insert(q.name.name.clone());
                }
            }
            ExprKind::Lambda { params, body, .. } => {
                let mark = scope.len();
                scope.extend(params.iter().map(|p| p.name.name.clone()));
                go(body, scope, out);
                scope.truncate(mark);
            }
            ExprKind::Match { scrutinee, arms } => {
                go(scrutinee, scope, out);
                for arm in arms {
                    let mark = scope.len();
                    binders(&arm.pat, scope);
                    if let Some(g) = &arm.guard {
                        go(g, scope, out);
                    }
                    go(&arm.body, scope, out);
                    scope.truncate(mark);
                }
            }
            ExprKind::Block { stmts, tail } => {
                let mark = scope.len();
                for s in stmts {
                    match s {
                        Stmt::Let { pat, value, .. } => {
                            go(value, scope, out);
                            binders(pat, scope);
                        }
                        Stmt::Expr(x) => go(x, scope, out),
                    }
                }
                if let Some(t) = tail {
                    go(t, scope, out);
                }
                scope.truncate(mark);
            }
            _ => {
                let mut children = Vec::new();
                walk_children(e, &mut |x| children.push(x));
                for c in children {
                    go(c, scope, out);
                }
            }
        }
    }
    let mut scope = bound.to_vec();
    let mut out = HashSet::new();
    go(e, &mut scope, &mut out);
    out
}

/// The direct children of an expression.
fn walk_children<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Var(_) => {}
        ExprKind::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        ExprKind::Unary { operand, .. } => f(operand),
        ExprKind::Lambda { body, .. } => f(body),
        ExprKind::App { func, args, named } => {
            f(func);
            args.iter().for_each(&mut *f);
            named.iter().for_each(|a| f(&a.value));
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            f(cond);
            f(then_branch);
            f(else_branch);
        }
        ExprKind::Match { scrutinee, arms } => {
            f(scrutinee);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    f(g);
                }
                f(&arm.body);
            }
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match s {
                    Stmt::Let { value, .. } => f(value),
                    Stmt::Expr(x) => f(x),
                }
            }
            if let Some(t) = tail {
                f(t);
            }
        }
        ExprKind::Record { fields } => fields.iter().for_each(|(_, x)| f(x)),
        ExprKind::RecordUpdate { base, fields } => {
            f(base);
            fields.iter().for_each(|(_, x)| f(x));
        }
        ExprKind::Field { base, .. } => f(base),
        ExprKind::Try { operand } => f(operand),
        ExprKind::List { items } => items.iter().for_each(&mut *f),
        ExprKind::Perform { args, .. } => args.iter().for_each(&mut *f),
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            f(body);
            for HandleClause { body, .. } in clauses {
                f(body);
            }
            if let Some(ReturnClause { body, .. }) = return_clause.as_deref() {
                f(body);
            }
        }
        ExprKind::WithCell { init, body, .. } => {
            f(init);
            f(body);
        }
        ExprKind::Simulate { body, .. } | ExprKind::WithRegion { body, .. } => f(body),
    }
}

// --- Scalar replacement -------------------------------------------------------------------------

/// Every block under `e` with its `let`-bound blocks flattened and its field-only records
/// replaced by their fields.
fn scalarize(e: Expr) -> Expr {
    let span = e.span;
    let kind = match e.kind {
        ExprKind::Block { stmts, tail } => {
            let mut flat: Vec<Stmt> = Vec::with_capacity(stmts.len());
            for s in stmts {
                match s {
                    Stmt::Let {
                        pat,
                        ty,
                        value,
                        span,
                    } => {
                        let value = scalarize(*value);
                        let value_span = value.span;
                        // A block bound by a `let` opens into the block around it when every
                        // name it binds is the inliner's, since those shadow nothing outside.
                        match value.kind {
                            ExprKind::Block {
                                stmts: inner,
                                tail: Some(inner_tail),
                            } if matches!(pat.kind, PatternKind::Var(_)) && all_fresh(&inner) => {
                                flat.extend(inner);
                                flat.push(Stmt::Let {
                                    pat,
                                    ty,
                                    value: inner_tail,
                                    span,
                                });
                            }
                            kind => flat.push(Stmt::Let {
                                pat,
                                ty,
                                value: Box::new(Expr {
                                    kind,
                                    span: value_span,
                                }),
                                span,
                            }),
                        }
                    }
                    Stmt::Expr(x) => flat.push(Stmt::Expr(scalarize(x))),
                }
            }
            let tail = tail.map(|t| Box::new(scalarize(*t)));
            replace_records(flat, tail)
        }
        other => {
            let mut e = Expr { kind: other, span };
            map_children(&mut e, scalarize);
            return e;
        }
    };
    Expr { kind, span }
}

/// A block's statements with every `let x = {f: .., ..}` whose `x` is only ever read as `x.f`
/// afterwards replaced by one `let` per field, and those reads by the field's binding; a `let y =
/// x` of such an `x` is an alias and goes the same way.
fn replace_records(stmts: Vec<Stmt>, tail: Option<Box<Expr>>) -> ExprKind {
    let mut out: Vec<Stmt> = Vec::with_capacity(stmts.len());
    // The records replaced so far, by name, with the binding each field went to.
    let mut split: Vec<(Symbol, Vec<(Symbol, Symbol)>)> = Vec::new();
    let mut rest: Vec<Stmt> = stmts;
    let mut tail = tail;
    while !rest.is_empty() {
        let s = rest.remove(0);
        match s {
            Stmt::Let {
                pat,
                ty,
                value,
                span,
            } => {
                let name = match &pat.kind {
                    PatternKind::Var(id) => Some(id.name.clone()),
                    _ => None,
                };
                // An alias of a split record reads through it.
                if let Some(name) = &name
                    && let ExprKind::Var(q) = &value.kind
                    && q.is_bare()
                    && let Some((_, fields)) = split.iter().find(|(r, _)| r == &q.name.name)
                    && only_field_reads(name, &rest, tail.as_deref(), fields)
                {
                    let fields = fields.clone();
                    substitute_reads(name, &fields, &mut rest, tail.as_deref_mut());
                    split.push((name.clone(), fields));
                    continue;
                }
                if let Some(name) = &name
                    && let ExprKind::Record { fields } = &value.kind
                    && fields
                        .iter()
                        .all(|(f, _)| fields.iter().filter(|(g, _)| g.name == f.name).count() == 1)
                    && only_field_reads(
                        name,
                        &rest,
                        tail.as_deref(),
                        &fields
                            .iter()
                            .map(|(f, _)| (f.name.clone(), f.name.clone()))
                            .collect::<Vec<_>>(),
                    )
                {
                    let ExprKind::Record { fields } = value.kind else {
                        unreachable!()
                    };
                    let mut bound = Vec::with_capacity(fields.len());
                    for (field, x) in fields {
                        let fresh = Symbol::new(format!("{name}${}", field.name));
                        out.push(Stmt::Let {
                            pat: Pattern {
                                kind: PatternKind::Var(Ident {
                                    name: fresh.clone(),
                                    span: field.span,
                                }),
                                span: field.span,
                            },
                            ty: None,
                            value: Box::new(x),
                            span,
                        });
                        bound.push((field.name.clone(), fresh));
                    }
                    substitute_reads(name, &bound, &mut rest, tail.as_deref_mut());
                    split.push((name.clone(), bound));
                    continue;
                }
                out.push(Stmt::Let {
                    pat,
                    ty,
                    value,
                    span,
                });
            }
            other => out.push(other),
        }
    }
    ExprKind::Block { stmts: out, tail }
}

/// Whether every mention of `name` in what follows is a read of one of `fields` through it.
fn only_field_reads(
    name: &Symbol,
    rest: &[Stmt],
    tail: Option<&Expr>,
    fields: &[(Symbol, Symbol)],
) -> bool {
    fn ok(e: &Expr, name: &Symbol, fields: &[(Symbol, Symbol)]) -> bool {
        match &e.kind {
            ExprKind::Field { base, field } if matches!(&base.kind, ExprKind::Var(q) if q.is_bare() && q.name.name == *name) => {
                fields.iter().any(|(f, _)| f == &field.name)
            }
            ExprKind::Var(q) if q.is_bare() && q.name.name == *name => false,
            // A rebinding of the name below shadows it; what is under that is not a read of ours,
            // but telling the two apart is not worth it: refuse.
            ExprKind::Lambda { params, .. } if params.iter().any(|p| p.name.name == *name) => false,
            ExprKind::Match { arms, .. }
                if arms.iter().any(|arm| {
                    let mut b = Vec::new();
                    binders(&arm.pat, &mut b);
                    b.contains(name)
                }) =>
            {
                false
            }
            ExprKind::Block { stmts, .. }
                if stmts.iter().any(|s| match s {
                    Stmt::Let { pat, .. } => {
                        let mut b = Vec::new();
                        binders(pat, &mut b);
                        b.contains(name)
                    }
                    Stmt::Expr(_) => false,
                }) =>
            {
                false
            }
            _ => {
                let mut fine = true;
                walk_children(e, &mut |c| {
                    if !ok(c, name, fields) {
                        fine = false;
                    }
                });
                fine
            }
        }
    }
    let rebinding = rest.iter().any(|s| match s {
        Stmt::Let { pat, .. } => {
            let mut b = Vec::new();
            binders(pat, &mut b);
            b.contains(name)
        }
        Stmt::Expr(_) => false,
    });
    if rebinding {
        return false;
    }
    rest.iter().all(|s| match s {
        Stmt::Let { value, .. } => ok(value, name, fields),
        Stmt::Expr(x) => ok(x, name, fields),
    }) && tail.is_none_or(|t| ok(t, name, fields))
}

/// Every `name.f` in what follows becomes the binding `f` went to.
fn substitute_reads(
    name: &Symbol,
    fields: &[(Symbol, Symbol)],
    rest: &mut [Stmt],
    tail: Option<&mut Expr>,
) {
    fn go(e: &mut Expr, name: &Symbol, fields: &[(Symbol, Symbol)]) {
        if let ExprKind::Field { base, field } = &e.kind
            && let ExprKind::Var(q) = &base.kind
            && q.is_bare()
            && q.name.name == *name
            && let Some((_, to)) = fields.iter().find(|(f, _)| f == &field.name)
        {
            e.kind = ExprKind::Var(QName::bare(Ident {
                name: to.clone(),
                span: field.span,
            }));
            return;
        }
        map_children_mut(e, &mut |c| go(c, name, fields));
    }
    for s in rest.iter_mut() {
        match s {
            Stmt::Let { value, .. } => go(value, name, fields),
            Stmt::Expr(x) => go(x, name, fields),
        }
    }
    if let Some(t) = tail {
        go(t, name, fields);
    }
}

/// `f` over each direct child, in place.
fn map_children_mut(e: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    match &mut e.kind {
        ExprKind::Lit(_) | ExprKind::Var(_) => {}
        ExprKind::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        ExprKind::Unary { operand, .. } => f(operand),
        ExprKind::Lambda { body, .. } => f(body),
        ExprKind::App { func, args, named } => {
            f(func);
            args.iter_mut().for_each(&mut *f);
            named.iter_mut().for_each(|a| f(&mut a.value));
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            f(cond);
            f(then_branch);
            f(else_branch);
        }
        ExprKind::Match { scrutinee, arms } => {
            f(scrutinee);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    f(g);
                }
                f(&mut arm.body);
            }
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match s {
                    Stmt::Let { value, .. } => f(value),
                    Stmt::Expr(x) => f(x),
                }
            }
            if let Some(t) = tail {
                f(t);
            }
        }
        ExprKind::Record { fields } => fields.iter_mut().for_each(|(_, x)| f(x)),
        ExprKind::RecordUpdate { base, fields } => {
            f(base);
            fields.iter_mut().for_each(|(_, x)| f(x));
        }
        ExprKind::Field { base, .. } => f(base),
        ExprKind::Try { operand } => f(operand),
        ExprKind::List { items } => items.iter_mut().for_each(&mut *f),
        ExprKind::Perform { args, .. } => args.iter_mut().for_each(&mut *f),
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            f(body);
            for HandleClause { body, .. } in clauses {
                f(body);
            }
            if let Some(rc) = return_clause.as_deref_mut() {
                f(&mut rc.body);
            }
        }
        ExprKind::WithCell { init, body, .. } => {
            f(init);
            f(body);
        }
        ExprKind::Simulate { body, .. } | ExprKind::WithRegion { body, .. } => f(body),
    }
}

/// `f` over each direct child, replacing it.
fn map_children(e: &mut Expr, f: fn(Expr) -> Expr) {
    map_children_mut(e, &mut |c| {
        let taken = std::mem::replace(
            c,
            Expr {
                kind: ExprKind::Lit(ply_syntax::ast::Lit::Bool(false)),
                span: c.span,
            },
        );
        *c = f(taken);
    });
}

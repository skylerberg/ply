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
    BinOp, Expr, ExprKind, FnDef, HandleClause, Ident, Item, Lit, MatchArm, Module, Pattern,
    PatternKind, QName, ReturnClause, Stmt,
};
use std::collections::HashSet;

/// How hard the inliner works. It differs by tier, and the difference is measured rather than
/// assumed: what a wider budget exposes is a chain of records built and read in one body, and
/// whether that is a win depends entirely on what the code generator downstream does with it.
#[derive(Clone, Copy)]
pub struct Inlining {
    /// The most syntax nodes a callee may have to be inlined.
    pub budget: usize,
    /// How many times a callee's own calls are inlined in turn.
    pub depth: usize,
}

impl Inlining {
    /// The in-process tier's. Wider was measured there and was **worse** -- the register allocator
    /// spills more than the calls cost -- so this is the ceiling rather than a default nobody
    /// tried.
    pub const IN_PROCESS: Inlining = Inlining {
        budget: 64,
        depth: 2,
    };

    /// The emitted tier's. Wide enough to fold BLAKE3's rounds into its compression, because the
    /// C compiler forwards the record chain that exposes instead of spilling it: about a fifth off
    /// the integer kernel, for about twice the compile. That trade is only available to a tier
    /// that is already off the loop's path.
    pub const EMITTED: Inlining = Inlining {
        budget: 2000,
        depth: 6,
    };

    /// `PLY_INLINE_BUDGET` and `PLY_INLINE_DEPTH` override, for a measurement.
    fn overridden(self) -> Inlining {
        let n = |k: &str, d: usize| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d)
        };
        Inlining {
            budget: n("PLY_INLINE_BUDGET", self.budget),
            depth: n("PLY_INLINE_DEPTH", self.depth),
        }
    }
}
/// A callee no larger than this that calls nothing is inlined however deep the site: a mask or
/// a shift written as a function costs a call at the depth the budget runs out otherwise.
const TINY_LEAF: usize = 8;

/// `def`'s body, rewritten.
pub fn optimize(loaded: &Source, module_index: usize, def: &FnDef, how: Inlining) -> Expr {
    let how = how.overridden();
    let module = &loaded.program.modules[module_index];
    let mut cx = Cx {
        module,
        fresh: 0,
        how,
    };
    let mut scope: Vec<Symbol> = def.params.iter().map(|p| p.name.name.clone()).collect();
    let body = cx.inline(&def.body, &def.name.name, how.depth, &mut scope);
    let body = scalarize(body);
    if std::env::var("PLY_OPT_DUMP").is_ok_and(|want| want == def.name.name.as_str()) {
        let mut text = String::new();
        render(&body, &mut text, 1);
        eprintln!("optimized `{}`:\n{text}", def.name.name);
    }
    body
}

/// The expression as Ply-like text, one statement per line, for `PLY_OPT_DUMP`.
fn render(e: &Expr, out: &mut String, depth: usize) {
    use std::fmt::Write;
    let pad = |out: &mut String, depth: usize| out.push_str(&"  ".repeat(depth));
    match &e.kind {
        ExprKind::Lit(l) => write!(out, "{l:?}").unwrap(),
        ExprKind::Var(q) => out.push_str(q.name.name.as_str()),
        ExprKind::Binary { op, lhs, rhs } => {
            out.push('(');
            render(lhs, out, depth);
            write!(out, " {op:?} ").unwrap();
            render(rhs, out, depth);
            out.push(')');
        }
        ExprKind::Unary { op, operand } => {
            write!(out, "{op:?}(").unwrap();
            render(operand, out, depth);
            out.push(')');
        }
        ExprKind::App { func, args, .. } => {
            render(func, out, depth);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render(a, out, depth);
            }
            out.push(')');
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            out.push_str("if ");
            render(cond, out, depth);
            out.push_str(" then ");
            render(then_branch, out, depth);
            out.push_str(" else ");
            render(else_branch, out, depth);
        }
        ExprKind::Block { stmts, tail } => {
            out.push_str("{\n");
            for s in stmts {
                pad(out, depth);
                match s {
                    Stmt::Let { pat, value, .. } => {
                        write!(out, "let {} = ", render_pat(pat)).unwrap();
                        render(value, out, depth + 1);
                    }
                    Stmt::Expr(x) => render(x, out, depth + 1),
                }
                out.push_str(";\n");
            }
            if let Some(t) = tail {
                pad(out, depth);
                render(t, out, depth + 1);
                out.push('\n');
            }
            pad(out, depth - 1);
            out.push('}');
        }
        ExprKind::Record { fields } => {
            out.push('{');
            for (i, (n, x)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write!(out, "{}: ", n.name).unwrap();
                render(x, out, depth);
            }
            out.push('}');
        }
        ExprKind::RecordUpdate { base, fields } => {
            out.push_str("{..");
            render(base, out, depth);
            for (n, x) in fields {
                write!(out, ", {}: ", n.name).unwrap();
                render(x, out, depth);
            }
            out.push('}');
        }
        ExprKind::Field { base, field } => {
            render(base, out, depth);
            write!(out, ".{}", field.name).unwrap();
        }
        ExprKind::List { items } => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render(x, out, depth);
            }
            out.push(']');
        }
        ExprKind::Match { scrutinee, arms } => {
            out.push_str("match ");
            render(scrutinee, out, depth);
            out.push_str(" {\n");
            for arm in arms {
                pad(out, depth);
                write!(out, "{} -> ", render_pat(&arm.pat)).unwrap();
                render(&arm.body, out, depth + 1);
                out.push_str(";\n");
            }
            pad(out, depth - 1);
            out.push('}');
        }
        other => write!(out, "<{}>", std::any::type_name_of_val(other)).unwrap(),
    }
}

fn render_pat(p: &Pattern) -> String {
    match &p.kind {
        PatternKind::Wildcard => "_".to_string(),
        PatternKind::Lit(l) => format!("{l:?}"),
        PatternKind::Var(id) => id.name.to_string(),
        PatternKind::Ctor { name, args } => format!(
            "{}({})",
            name.name.name,
            args.iter().map(render_pat).collect::<Vec<_>>().join(", ")
        ),
        PatternKind::Record { fields, rest } => format!(
            "{{{}{}}}",
            fields
                .iter()
                .map(|(n, sub)| format!("{}: {}", n.name, render_pat(sub)))
                .collect::<Vec<_>>()
                .join(", "),
            if *rest { ", .." } else { "" }
        ),
        PatternKind::List { items, rest } => format!(
            "[{}{}]",
            items.iter().map(render_pat).collect::<Vec<_>>().join(", "),
            rest.as_ref()
                .map_or(String::new(), |r| format!(", ..{}", render_pat(r)))
        ),
    }
}

struct Cx<'a> {
    module: &'a Module,
    fresh: u32,
    how: Inlining,
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
                    && size(&def.body) <= self.how.budget
                    && inlinable(&def.body);
                return plain.then_some(def);
            }
        }
        None
    }

    fn inline(&mut self, e: &Expr, caller: &Symbol, depth: usize, scope: &mut Vec<Symbol>) -> Expr {
        let span = e.span;
        let site = match &e.kind {
            ExprKind::App { func, args, named } if named.is_empty() => match &func.kind {
                ExprKind::Var(q) if q.is_bare() && !scope.contains(&q.name.name) => self
                    .callee(&q.name.name, caller, args.len())
                    .filter(|def| depth > 0 || tiny_leaf(&def.body))
                    .map(|def| (def, args)),
                _ => None,
            },
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
                let body = self.inline(
                    &body,
                    &def.name.name,
                    depth.saturating_sub(1),
                    &mut inner_scope,
                );
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

fn tiny_leaf(e: &Expr) -> bool {
    let mut calls = false;
    walk(e, &mut |x| calls |= matches!(x.kind, ExprKind::App { .. }));
    !calls && size(e) <= TINY_LEAF
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
    match e.kind {
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
                        let mut value = scalarize(*value);
                        let var = matches!(pat.kind, PatternKind::Var(_));
                        // A block bound by a `let` opens into the block around it when every
                        // name it binds is the inliner's, since those shadow nothing outside —
                        // and so does its tail, an inlined callee's own block under the block
                        // that bound its parameters.
                        loop {
                            let value_span = value.span;
                            match value.kind {
                                ExprKind::Block {
                                    stmts: inner,
                                    tail: Some(inner_tail),
                                } if var && all_fresh(&inner) => {
                                    flat.extend(inner);
                                    value = *inner_tail;
                                }
                                kind => {
                                    value = Expr {
                                        kind,
                                        span: value_span,
                                    };
                                    break;
                                }
                            }
                        }
                        flat.push(Stmt::Let {
                            pat,
                            ty,
                            value: Box::new(value),
                            span,
                        });
                    }
                    Stmt::Expr(x) => flat.push(Stmt::Expr(scalarize(x))),
                }
            }
            let tail = tail.map(|t| Box::new(scalarize(*t)));
            let mut e = Expr {
                kind: replace_records(flat, tail),
                span,
            };
            fold_literals(&mut e);
            e
        }
        other => {
            let mut e = Expr { kind: other, span };
            map_children(&mut e, scalarize);
            fold_literals(&mut e);
            e
        }
    }
}

/// Every operator over two `Int` literals folded to its answer, where the answer is what the
/// operator would give at evaluation: an overflow or a shift count outside `0..64` is left for
/// the evaluator to raise.
fn fold_literals(e: &mut Expr) {
    map_children_mut(e, &mut fold_literals);
    let folded = match &e.kind {
        ExprKind::Binary { op, lhs, rhs } => match (&lhs.kind, &rhs.kind) {
            (ExprKind::Lit(Lit::Int(a)), ExprKind::Lit(Lit::Int(b))) => match op {
                BinOp::Add => a.checked_add(*b),
                BinOp::Sub => a.checked_sub(*b),
                BinOp::Mul => a.checked_mul(*b),
                BinOp::BitAnd => Some(a & b),
                BinOp::BitOr => Some(a | b),
                BinOp::BitXor => Some(a ^ b),
                BinOp::Shl if (0..64).contains(b) => Some(a << b),
                BinOp::Shr if (0..64).contains(b) => Some(a >> b),
                BinOp::Ushr if (0..64).contains(b) => Some(((*a as u64) >> b) as i64),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    };
    if let Some(n) = folded {
        e.kind = ExprKind::Lit(Lit::Int(n));
    }
}

/// Whether anything in what follows binds `name` again — a `let`, a lambda parameter or a
/// pattern — under which a read would mean the new binding.
fn rebinds(name: &Symbol, rest: &[Stmt], tail: Option<&Expr>) -> bool {
    fn binds(e: &Expr, name: &Symbol) -> bool {
        let mut hit = false;
        walk(e, &mut |x| match &x.kind {
            ExprKind::Lambda { params, .. } if params.iter().any(|p| p.name.name == *name) => {
                hit = true;
            }
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    let mut b = Vec::new();
                    binders(&arm.pat, &mut b);
                    if b.contains(name) {
                        hit = true;
                    }
                }
            }
            ExprKind::Block { stmts, .. } => {
                for s in stmts {
                    if let Stmt::Let { pat, .. } = s {
                        let mut b = Vec::new();
                        binders(pat, &mut b);
                        if b.contains(name) {
                            hit = true;
                        }
                    }
                }
            }
            _ => {}
        });
        hit
    }
    rest.iter().any(|s| match s {
        Stmt::Let { pat, value, .. } => {
            let mut b = Vec::new();
            binders(pat, &mut b);
            b.contains(name) || binds(value, name)
        }
        Stmt::Expr(x) => binds(x, name),
    }) || tail.is_some_and(|t| binds(t, name))
}

/// Every bare read of `name` in what follows becomes `with`.
fn substitute_var(name: &Symbol, with: &Expr, rest: &mut [Stmt], tail: Option<&mut Expr>) {
    fn go(e: &mut Expr, name: &Symbol, with: &Expr) {
        if let ExprKind::Var(q) = &e.kind
            && q.is_bare()
            && q.name.name == *name
        {
            e.kind = with.kind.clone();
            return;
        }
        map_children_mut(e, &mut |c| go(c, name, with));
    }
    for s in rest.iter_mut() {
        match s {
            Stmt::Let { value, .. } => go(value, name, with),
            Stmt::Expr(x) => go(x, name, with),
        }
    }
    if let Some(t) = tail {
        go(t, name, with);
    }
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
                // A `let` of a scalar literal is the literal at every read, so a count a callee
                // named is a constant where it is used — and a shift by one needs no check.
                if let Some(name) = &name
                    && matches!(&value.kind, ExprKind::Lit(Lit::Int(_) | Lit::Bool(_)))
                    && !rebinds(name, &rest, tail.as_deref())
                {
                    substitute_var(name, &value, &mut rest, tail.as_deref_mut());
                    continue;
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use ply_syntax::ast::{ModuleName, Program};

    fn optimized(src: &str, name: &str) -> String {
        let src: &'static str = Box::leak(src.to_string().into_boxed_str());
        let mut sources = ply_span::SourceMap::new();
        let id = sources.add("m.ply", src);
        let mut program = ply_syntax::parse_program(vec![(id, ModuleName::from_dotted("m"), src)])
            .expect("parses");
        let resolved = ply_syntax::resolve::resolve(&mut program).expect("resolves");
        let check = ply_core::check_program(&program, &resolved).expect("checks");
        let program: &'static Program = Box::leak(Box::new(program));
        let source = Source::new(
            program,
            Box::leak(Box::new(resolved)),
            Box::leak(Box::new(check)),
        );
        let def = program.modules[0]
            .items
            .iter()
            .find_map(|item| match item {
                Item::Fn(def) if def.name.name.as_str() == name => Some(&**def),
                _ => None,
            })
            .expect("the function is defined");
        let mut text = String::new();
        render(
            &optimize(&source, 0, def, Inlining::IN_PROCESS),
            &mut text,
            1,
        );
        text
    }

    /// An inlined callee's block opens into the caller's, and then the record it answers is
    /// split into its fields, so a body over small records is a body over scalars.
    #[test]
    fn an_inlined_callee_flattens_and_its_record_splits_into_scalars() {
        let text = optimized(
            "type Q = { a: Int, b: Int }\n\
             fn mask(x: Int) -> Int = x & 255\n\
             fn g(q: Q, m: Int) -> Q = { let a = mask(q.a + m); let b = mask(q.b + a); {a: a, b: b} }\n\
             fn round(p: Q) -> Int = { let c = g({a: p.a, b: p.b}, 3); let d = g({a: c.b, b: c.a}, 5); d.a + d.b }\n",
            "round",
        );
        assert!(
            !text.contains("= {\n"),
            "a let still binds a block:\n{text}"
        );
        assert!(!text.contains("{a:"), "a record literal survived:\n{text}");
        assert!(
            !text.contains("mask("),
            "a tiny leaf was left as a call:\n{text}"
        );
        assert!(
            !text.contains("g("),
            "the callee was left as a call:\n{text}"
        );
    }

    /// A count a callee named is the literal where it is used, and an operator over two
    /// literals is its answer: a shift by `32 - n` with `n` known is a shift by a literal.
    #[test]
    fn a_literal_let_is_propagated_and_folded() {
        let text = optimized(
            "fn turn(x: Int, n: Int) -> Int = ((x >>> n) | (x << (32 - n))) & 255\n\
             fn twice(x: Int) -> Int = turn(turn(x, 7), 12)\n",
            "twice",
        );
        assert!(!text.contains("Sub"), "`32 - n` was not folded:\n{text}");
        assert!(
            text.contains("Shl Int(25)") && text.contains("Shl Int(20)"),
            "{text}"
        );
        assert!(!text.contains("let n"), "a literal let survived:\n{text}");
    }
}

//! Every span in a generated definition becomes the `derive` item's.
//!
//! The alternative is an offset into text the user never wrote, which is the
//! difference between an error a reader can act on and one that sends them
//! looking for a line that does not exist. It also makes `DefEntry::span` — the
//! `derive` line — agree with every span inside the body it stands for.

use ply_span::Span;
use ply_syntax::ast::*;

pub fn fn_def(def: &mut FnDef, span: Span) {
    def.span = span;
    def.name.span = span;
    for g in def
        .generics
        .types
        .iter_mut()
        .chain(&mut def.generics.effects)
    {
        g.span = span;
    }
    for p in &mut def.params {
        p.span = span;
        p.name.span = span;
        if let Some(t) = &mut p.ty {
            ty(t, span);
        }
    }
    if let Some(t) = &mut def.ret {
        ty(t, span);
    }
    if let Some(r) = &mut def.effects {
        row(r, span);
    }
    for c in &mut def.constraints {
        c.span = span;
        c.deriver_span = span;
        c.param.span = span;
    }
    for clause in &mut def.spec {
        clause.span = span;
        expr(&mut clause.expr, span);
    }
    expr(&mut def.body, span);
}

fn ty(t: &mut TypeExpr, span: Span) {
    match t {
        TypeExpr::Var(i) => i.span = span,
        TypeExpr::Con {
            name,
            args,
            span: s,
        } => {
            *s = span;
            qname(name, span);
            for a in args {
                ty(a, span);
            }
        }
        TypeExpr::Fn {
            params,
            ret,
            effects,
            span: s,
        } => {
            *s = span;
            for p in params {
                ty(p, span);
            }
            ty(ret, span);
            if let Some(r) = effects {
                row(r, span);
            }
        }
        TypeExpr::Record { fields, span: s } => {
            *s = span;
            for (n, t) in fields {
                n.span = span;
                ty(t, span);
            }
        }
        TypeExpr::Unit { span: s } => *s = span,
    }
}

fn row(r: &mut RowExpr, span: Span) {
    r.span = span;
    for a in &mut r.atoms {
        a.span = span;
        qname(&mut a.effect, span);
        if let Some(res) = &mut a.resource {
            res.span = span;
        }
    }
    if let Some(tail) = &mut r.tail {
        tail.span = span;
    }
}

fn qname(q: &mut QName, span: Span) {
    q.span = span;
    q.name.span = span;
    if let Some(m) = &mut q.module {
        m.span = span;
    }
}

fn expr(e: &mut Expr, span: Span) {
    e.span = span;
    match &mut e.kind {
        ExprKind::Lit(_) => {}
        ExprKind::Var(q) => qname(q, span),
        ExprKind::Binary { lhs, rhs, .. } => {
            expr(lhs, span);
            expr(rhs, span);
        }
        ExprKind::Unary { operand, .. } => expr(operand, span),
        ExprKind::Lambda { params, body } => {
            for p in params {
                p.span = span;
                p.name.span = span;
                if let Some(t) = &mut p.ty {
                    ty(t, span);
                }
            }
            expr(body, span);
        }
        ExprKind::App { func, args, named } => {
            expr(func, span);
            for a in args {
                expr(a, span);
            }
            for n in named {
                expr(&mut n.value, span);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            expr(cond, span);
            expr(then_branch, span);
            expr(else_branch, span);
        }
        ExprKind::Match { scrutinee, arms } => {
            expr(scrutinee, span);
            for arm in arms {
                arm.span = span;
                pattern(&mut arm.pat, span);
                if let Some(g) = &mut arm.guard {
                    expr(g, span);
                }
                expr(&mut arm.body, span);
            }
        }
        ExprKind::Block { stmts, tail } => {
            for stmt in stmts {
                match stmt {
                    Stmt::Let {
                        pat,
                        ty: t,
                        value,
                        span: s,
                    } => {
                        *s = span;
                        pattern(pat, span);
                        if let Some(t) = t {
                            ty(t, span);
                        }
                        expr(value, span);
                    }
                    Stmt::Expr(e) => expr(e, span),
                }
            }
            if let Some(t) = tail {
                expr(t, span);
            }
        }
        ExprKind::Record { fields } => {
            for (n, e) in fields {
                n.span = span;
                expr(e, span);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            expr(base, span);
            for (n, e) in fields {
                n.span = span;
                expr(e, span);
            }
        }
        ExprKind::Field { base, field } => {
            expr(base, span);
            field.span = span;
        }
        ExprKind::Try { operand } => expr(operand, span),
        ExprKind::List { items } => {
            for i in items {
                expr(i, span);
            }
        }
        ExprKind::Perform {
            effect,
            op,
            resource,
            args,
        } => {
            qname(effect, span);
            op.span = span;
            if let Some(r) = resource {
                r.span = span;
            }
            for a in args {
                expr(a, span);
            }
        }
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            expr(body, span);
            for c in clauses {
                c.span = span;
                qname(&mut c.effect, span);
                c.op.span = span;
                if let Some(r) = &mut c.resource {
                    r.span = span;
                }
                for p in &mut c.params {
                    p.span = span;
                }
                if let Some(k) = &mut c.resume {
                    k.span = span;
                }
                expr(&mut c.body, span);
            }
            if let Some(r) = return_clause {
                r.span = span;
                r.binder.span = span;
                expr(&mut r.body, span);
            }
        }
        ExprKind::WithCell {
            resource,
            init,
            binder,
            body,
        } => {
            resource.span = span;
            expr(init, span);
            binder.span = span;
            expr(body, span);
        }
        ExprKind::WithRegion { region, body } => {
            region.span = span;
            expr(body, span);
        }
        ExprKind::Simulate { body } => expr(body, span),
    }
}

fn pattern(p: &mut Pattern, span: Span) {
    p.span = span;
    match &mut p.kind {
        PatternKind::Wildcard | PatternKind::Lit(_) => {}
        PatternKind::Var(i) => i.span = span,
        PatternKind::Ctor { name, args } => {
            qname(name, span);
            for a in args {
                pattern(a, span);
            }
        }
        PatternKind::Record { fields, .. } => {
            for (n, p) in fields {
                n.span = span;
                pattern(p, span);
            }
        }
        PatternKind::List { items, rest } => {
            for i in items {
                pattern(i, span);
            }
            if let Some(r) = rest {
                pattern(r, span);
            }
        }
    }
}

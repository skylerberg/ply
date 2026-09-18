//! The tree back as source, for programs reconstructed from stored bodies with no text.
//! Faithful rather than pretty: each form is spelled the one way the parser reads back unchanged.

use crate::ast::*;
use crate::lexer::render_decimal;

pub fn program(p: &Program) -> Vec<(String, String)> {
    p.modules
        .iter()
        .map(|m| (m.name.to_string(), module(m)))
        .collect()
}

pub fn module(m: &Module) -> String {
    let mut p = Printer {
        out: String::new(),
        depth: 0,
    };
    for i in &m.imports {
        p.import(i);
        p.push("\n");
    }
    for it in &m.items {
        p.push("\n");
        p.item(it);
        p.push("\n");
    }
    p.out
}

pub fn expr(e: &Expr) -> String {
    let mut p = Printer {
        out: String::new(),
        depth: 0,
    };
    p.expr(e);
    p.out
}

struct Printer {
    out: String,
    depth: usize,
}

impl Printer {
    fn push(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn line(&mut self) {
        self.out.push('\n');
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
    }

    fn open(&mut self) {
        self.push("{");
        self.depth += 1;
    }

    fn close(&mut self) {
        self.depth -= 1;
        self.line();
        self.push("}");
    }

    fn sep<T>(&mut self, xs: &[T], mut each: impl FnMut(&mut Self, &T)) {
        for (i, x) in xs.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            each(self, x);
        }
    }

    fn import(&mut self, i: &ImportDecl) {
        self.push("import ");
        let path: Vec<&str> = i.path.iter().map(|s| s.name.as_str()).collect();
        self.push(&path.join("."));
        match &i.kind {
            ImportKind::Module => {}
            ImportKind::Alias(a) => {
                self.push(" as ");
                self.push(a.name.as_str());
            }
            ImportKind::Names(ns) => {
                self.push(" (");
                self.sep(ns, |p, n| p.push(n.name.as_str()));
                self.push(")");
            }
        }
    }

    fn item(&mut self, it: &Item) {
        if it.visibility().is_public() {
            self.push("pub ");
        }
        match it {
            Item::Fn(f) => self.fn_def(f),
            Item::Type(t) => self.type_def(t),
            Item::Effect(e) => self.effect_def(e),
            Item::Test(t) => {
                self.push(if t.nondet { "test/nondet " } else { "test " });
                self.str_lit(&t.name);
                self.push(" ");
                self.block(&t.body);
            }
            Item::Law(l) => {
                self.push(if l.host { "law/host " } else { "law " });
                self.str_lit(&l.name);
                if !l.binders.is_empty() {
                    self.push(" forall (");
                    self.sep(&l.binders, |p, b| {
                        p.push(b.name.name.as_str());
                        p.push(": ");
                        p.ty(&b.ty);
                    });
                    self.push(")");
                }
                if let Some(g) = &l.guard {
                    self.push(" where ");
                    self.operand(g);
                }
                self.push(" ");
                self.block(&l.body);
            }
            Item::Derive(d) => {
                self.push("derive ");
                self.push(d.deriver.as_str());
                self.push(" for ");
                self.push(d.target.name.as_str());
            }
            Item::EffectSet(s) => {
                self.push("effect set ");
                self.push(s.name.name.as_str());
                self.push(" = {");
                let mut first = true;
                for a in &s.atoms {
                    if !first {
                        self.push(", ");
                    }
                    first = false;
                    self.atom(a);
                }
                for q in &s.includes {
                    if !first {
                        self.push(", ");
                    }
                    first = false;
                    self.push(&q.to_string());
                }
                self.push("}");
            }
        }
    }

    fn fn_def(&mut self, f: &FnDef) {
        if f.reuse.is_some() {
            self.push("reuse ");
        }
        self.push("fn ");
        self.push(f.name.name.as_str());
        self.generics(&f.generics);
        self.push("(");
        self.params(&f.params);
        self.push(")");
        if let Some(r) = &f.ret {
            self.push(" -> ");
            self.ret_ty(r);
        }
        if let Some(row) = &f.effects {
            self.push(" / ");
            self.row(row);
        }
        if !f.constraints.is_empty() {
            self.push(" where ");
            self.sep(&f.constraints, |p, c| {
                p.push("derivable(");
                p.push(c.deriver.as_str());
                p.push(", ");
                p.push(c.param.name.as_str());
                p.push(")");
            });
        }
        for clause in &f.spec {
            self.line();
            self.push("  ");
            self.push(clause.kind.as_str());
            self.push(" ");
            self.operand(&clause.expr);
        }
        self.push(" =");
        self.line();
        self.push("  ");
        self.expr(&f.body);
    }

    fn generics(&mut self, g: &Generics) {
        if g.types.is_empty() && g.effects.is_empty() {
            return;
        }
        self.push("<");
        self.sep(&g.types, |p, t| p.push(t.name.as_str()));
        if !g.effects.is_empty() {
            self.push(" | ");
            self.sep(&g.effects, |p, e| p.push(e.name.as_str()));
        }
        self.push(">");
    }

    fn params(&mut self, ps: &[Param]) {
        self.sep(ps, |p, x| {
            p.push(x.name.name.as_str());
            if let Some(t) = &x.ty {
                p.push(": ");
                p.ty(t);
            }
            if let Some(d) = &x.default {
                p.push(" = ");
                p.expr(d);
            }
        });
    }

    fn type_def(&mut self, t: &TypeDef) {
        self.push("type ");
        self.push(t.name.name.as_str());
        if !t.params.is_empty() {
            self.push("<");
            self.sep(&t.params, |p, x| p.push(x.name.as_str()));
            self.push(">");
        }
        self.push(" = ");
        match &t.body {
            TypeDefBody::Alias(a) => self.ty(a),
            TypeDefBody::Sum(vs) => {
                // Every variant keeps its `|`: a single-variant sum without one reads as an alias.
                for v in vs {
                    self.push("| ");
                    self.push(v.name.name.as_str());
                    if !v.fields.is_empty() {
                        self.push("(");
                        self.sep(&v.fields, |p, f| p.ty(f));
                        self.push(")");
                    }
                    self.push(" ");
                }
            }
        }
    }

    fn effect_def(&mut self, e: &EffectDef) {
        if e.nondet {
            self.push("nondet ");
        }
        self.push("effect ");
        self.push(e.name.name.as_str());
        self.push(" ");
        self.open();
        for op in &e.ops {
            self.line();
            self.push(op.mode.as_str());
            self.push(" ");
            self.push(op.name.name.as_str());
            if op.resource_param {
                self.push("[r]");
            }
            self.push("(");
            self.sep(&op.params, |p, t| p.ty(t));
            self.push(") -> ");
            self.ty(&op.ret);
        }
        self.close();
    }

    fn ty(&mut self, t: &TypeExpr) {
        match t {
            TypeExpr::Var(i) => self.push(i.name.as_str()),
            TypeExpr::Con { name, args, .. } => {
                self.push(&name.to_string());
                // A bare lowercase name reads back as a type variable, so it keeps an empty `<>`.
                if !args.is_empty() || (name.is_bare() && !is_ctor_name(name.symbol())) {
                    self.push("<");
                    self.sep(args, |p, a| p.ty(a));
                    self.push(">");
                }
            }
            TypeExpr::Fn {
                params,
                ret,
                effects,
                ..
            } => {
                self.push("(");
                self.sep(params, |p, a| p.ty(a));
                self.push(") -> ");
                self.ret_ty(ret);
                if let Some(r) = effects {
                    self.push(" / ");
                    self.row(r);
                }
            }
            TypeExpr::Record { fields, .. } => {
                self.push("{");
                self.sep(fields, |p, (n, t)| {
                    p.push(n.name.as_str());
                    p.push(": ");
                    p.ty(t);
                });
                self.push("}");
            }
            TypeExpr::Unit { .. } => self.push("()"),
        }
    }

    /// Parenthesizes a function type after `->`, or a row after it would bind to the inner arrow.
    fn ret_ty(&mut self, t: &TypeExpr) {
        if matches!(t, TypeExpr::Fn { .. }) {
            self.push("(");
            self.ty(t);
            self.push(")");
        } else {
            self.ty(t);
        }
    }

    fn row(&mut self, r: &RowExpr) {
        if r.atoms.is_empty()
            && r.aliases.is_empty()
            && let Some(t) = &r.tail
        {
            self.push(t.name.as_str());
            return;
        }
        self.push("{");
        let mut first = true;
        for a in &r.atoms {
            if !first {
                self.push(", ");
            }
            first = false;
            self.atom(a);
        }
        for q in &r.aliases {
            if !first {
                self.push(", ");
            }
            first = false;
            self.push(&q.to_string());
        }
        if let Some(t) = &r.tail {
            self.push(" | ");
            self.push(t.name.as_str());
        }
        self.push("}");
    }

    fn atom(&mut self, a: &AtomExpr) {
        self.push(&a.effect.to_string());
        self.push(".");
        self.push(a.mode.as_str());
        if let Some(r) = &a.resource {
            self.push("[");
            self.push(r.name.as_str());
            self.push("]");
        }
    }

    /// A primary the parser reads whole, so it may stand bare as an operand or scrutinee.
    fn atomic(e: &Expr) -> bool {
        matches!(
            e.kind,
            ExprKind::Lit(_)
                | ExprKind::Var(_)
                | ExprKind::App { .. }
                | ExprKind::Field { .. }
                | ExprKind::List { .. }
                | ExprKind::Perform { .. }
                | ExprKind::Try { .. }
        )
    }

    fn operand(&mut self, e: &Expr) {
        if Self::atomic(e) {
            self.expr(e);
        } else {
            self.push("(");
            self.expr(e);
            self.push(")");
        }
    }

    /// `r.f(x)` performs an effect, so a call through anything but a name is parenthesized.
    fn callee(&mut self, e: &Expr) {
        if matches!(e.kind, ExprKind::Var(_)) {
            self.expr(e);
        } else {
            self.push("(");
            self.expr(e);
            self.push(")");
        }
    }

    fn base(&mut self, e: &Expr) {
        if matches!(
            e.kind,
            ExprKind::Var(_)
                | ExprKind::App { .. }
                | ExprKind::Field { .. }
                | ExprKind::Perform { .. }
                | ExprKind::Try { .. }
        ) {
            self.expr(e);
        } else {
            self.push("(");
            self.expr(e);
            self.push(")");
        }
    }

    /// The body positions the grammar spells with braces.
    fn block(&mut self, e: &Expr) {
        if matches!(e.kind, ExprKind::Block { .. }) {
            self.expr(e);
        } else {
            self.open();
            self.line();
            self.expr(e);
            self.close();
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Lit(l) => self.lit(l),
            ExprKind::Var(q) => self.push(&q.to_string()),
            ExprKind::Binary { op, lhs, rhs } => {
                self.operand(lhs);
                self.push(" ");
                self.push(op.text());
                self.push(" ");
                self.operand(rhs);
            }
            ExprKind::Unary { op, operand } => {
                self.push(match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
                    UnOp::BitNot => "~",
                });
                self.operand(operand);
            }
            ExprKind::Lambda { params, body, ret } => {
                if params.is_empty() {
                    self.push("||");
                } else {
                    self.push("|");
                    self.params(params);
                    self.push("|");
                }
                match ret {
                    Some(t) => {
                        self.push(" -> ");
                        self.ret_ty(t);
                        self.push(" ");
                        self.block(body);
                    }
                    None => {
                        self.push(" ");
                        self.expr(body);
                    }
                }
            }
            ExprKind::App { func, args, named } => {
                self.callee(func);
                self.push("(");
                let mut first = true;
                for a in args {
                    if !first {
                        self.push(", ");
                    }
                    first = false;
                    self.expr(a);
                }
                for n in named {
                    if !first {
                        self.push(", ");
                    }
                    first = false;
                    self.push(n.name.name.as_str());
                    self.push(": ");
                    self.expr(&n.value);
                }
                self.push(")");
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.if_expr(cond, then_branch, else_branch),
            ExprKind::Match { scrutinee, arms } => {
                self.push("match ");
                self.operand(scrutinee);
                self.push(" ");
                self.open();
                for arm in arms {
                    self.line();
                    self.pat(&arm.pat);
                    if let Some(g) = &arm.guard {
                        self.push(" if ");
                        self.expr(g);
                    }
                    self.push(" -> ");
                    self.expr(&arm.body);
                    self.push(",");
                }
                self.close();
            }
            ExprKind::Block { stmts, tail } => {
                self.open();
                for s in stmts {
                    self.line();
                    self.stmt(s);
                }
                if let Some(t) = tail {
                    self.line();
                    self.expr(t);
                }
                self.close();
            }
            ExprKind::Record { fields } => {
                self.push("{");
                self.sep(fields, |p, (n, v)| {
                    p.push(n.name.as_str());
                    p.push(": ");
                    p.expr(v);
                });
                self.push("}");
            }
            ExprKind::RecordUpdate { base, fields } => {
                self.push("{..");
                self.expr(base);
                for (n, v) in fields {
                    self.push(", ");
                    self.push(n.name.as_str());
                    self.push(": ");
                    self.expr(v);
                }
                self.push("}");
            }
            ExprKind::Field { base, field } => {
                self.base(base);
                self.push(".");
                self.push(field.name.as_str());
            }
            ExprKind::Try { operand } => {
                self.base(operand);
                self.push("?");
            }
            ExprKind::List { items } => {
                self.push("[");
                self.sep(items, |p, i| p.expr(i));
                self.push("]");
            }
            ExprKind::Perform {
                effect,
                op,
                resource,
                args,
            } => {
                self.push(&effect.to_string());
                self.push(".");
                self.push(op.name.as_str());
                if let Some(r) = resource {
                    self.push("[");
                    self.push(r.name.as_str());
                    self.push("]");
                }
                self.push("(");
                self.sep(args, |p, a| p.expr(a));
                self.push(")");
            }
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => {
                self.push("handle ");
                self.expr(body);
                self.push(" with ");
                self.open();
                for c in clauses {
                    self.line();
                    self.push(&c.effect.to_string());
                    self.push(".");
                    self.push(c.op.name.as_str());
                    if let Some(r) = &c.resource {
                        self.push("[");
                        self.push(r.name.as_str());
                        self.push("]");
                    }
                    self.push("(");
                    self.sep(&c.params, |p, x| p.push(x.name.as_str()));
                    self.push(")");
                    if let Some(k) = &c.resume {
                        self.push(" resume ");
                        self.push(k.name.as_str());
                    }
                    self.push(" -> ");
                    self.expr(&c.body);
                    self.push(",");
                }
                if let Some(r) = return_clause {
                    self.line();
                    self.push("return ");
                    self.push(r.binder.name.as_str());
                    self.push(" -> ");
                    self.expr(&r.body);
                    self.push(",");
                }
                self.close();
            }
            ExprKind::WithCell {
                resource,
                init,
                binder,
                body,
            } => {
                self.push("with_cell[");
                self.push(resource.name.as_str());
                self.push("](");
                self.expr(init);
                self.push(") ");
                self.open();
                self.line();
                self.push(binder.name.as_str());
                self.push(" -> ");
                self.expr(body);
                self.close();
            }
            ExprKind::WithRegion { region, body } => {
                self.push("with_region[");
                self.push(region.name.as_str());
                self.push("] ");
                self.block(body);
            }
            ExprKind::Simulate { body } => {
                self.push("simulate ");
                self.block(body);
            }
        }
    }

    fn if_expr(&mut self, cond: &Expr, then_branch: &Expr, else_branch: &Expr) {
        self.push("if ");
        self.operand(cond);
        self.push(" ");
        self.block(then_branch);
        match &else_branch.kind {
            // The parser's own spelling of "no `else`".
            ExprKind::Lit(Lit::Unit) => {}
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.push(" else ");
                self.if_expr(cond, then_branch, else_branch);
            }
            _ => {
                self.push(" else ");
                self.block(else_branch);
            }
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { pat, ty, value, .. } => {
                self.push("let ");
                self.pat(pat);
                if let Some(t) = ty {
                    self.push(": ");
                    self.ty(t);
                }
                self.push(" = ");
                self.expr(value);
                self.push(";");
            }
            Stmt::Expr(e) => {
                self.expr(e);
                self.push(";");
            }
        }
    }

    fn pat(&mut self, p: &Pattern) {
        match &p.kind {
            PatternKind::Wildcard => self.push("_"),
            PatternKind::Var(i) => self.push(i.name.as_str()),
            PatternKind::Lit(l) => self.lit(l),
            PatternKind::Ctor { name, args } => {
                self.push(&name.to_string());
                if !args.is_empty() {
                    self.push("(");
                    self.sep(args, |p, a| p.pat(a));
                    self.push(")");
                }
            }
            PatternKind::Record { fields, rest } => {
                self.push("{");
                self.sep(fields, |p, (n, q)| {
                    p.push(n.name.as_str());
                    p.push(": ");
                    p.pat(q);
                });
                if *rest {
                    if !fields.is_empty() {
                        self.push(", ");
                    }
                    self.push("..");
                }
                self.push("}");
            }
            PatternKind::List { items, rest } => {
                self.push("[");
                self.sep(items, |p, i| p.pat(i));
                if let Some(r) = rest {
                    if !items.is_empty() {
                        self.push(", ");
                    }
                    self.push("..");
                    if let PatternKind::Var(i) = &r.kind {
                        self.push(i.name.as_str());
                    }
                }
                self.push("]");
            }
        }
    }

    fn lit(&mut self, l: &Lit) {
        match l {
            Lit::Int(v) => self.push(&v.to_string()),
            Lit::Fixed { ty, bits } => {
                self.push(&ty.value(*bits).to_string());
                self.push(&ty.name().to_lowercase());
            }
            Lit::Bool(b) => self.push(if *b { "true" } else { "false" }),
            Lit::Str(s) => self.str_lit(s),
            Lit::Bytes(b) => self.bytes_lit(b),
            Lit::Float(f) => self.push(&render_float(*f)),
            Lit::Decimal { mantissa, scale } => {
                self.push(&render_decimal(*mantissa, *scale));
                self.push("m");
            }
            Lit::Unit => self.push("()"),
        }
    }

    fn str_lit(&mut self, s: &str) {
        self.out.push('"');
        for c in s.chars() {
            match c {
                '"' => self.out.push_str("\\\""),
                '\\' => self.out.push_str("\\\\"),
                '\n' => self.out.push_str("\\n"),
                '\t' => self.out.push_str("\\t"),
                '\r' => self.out.push_str("\\r"),
                '\0' => self.out.push_str("\\0"),
                c => self.out.push(c),
            }
        }
        self.out.push('"');
    }

    fn bytes_lit(&mut self, b: &[u8]) {
        self.out.push_str("b\"");
        for &x in b {
            match x {
                b'"' => self.out.push_str("\\\""),
                b'\\' => self.out.push_str("\\\\"),
                b'\n' => self.out.push_str("\\n"),
                b'\t' => self.out.push_str("\\t"),
                b'\r' => self.out.push_str("\\r"),
                0 => self.out.push_str("\\0"),
                0x20..=0x7e => self.out.push(x as char),
                _ => self.out.push_str(&format!("\\x{x:02x}")),
            }
        }
        self.out.push('"');
    }
}

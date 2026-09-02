//! The reference side of the parser spike's differential: `ply_syntax`'s tree, written out in the
//! same flat dump grammar `spikes/ply-parser/*.ply` emits.

use ply_span::{Diagnostic, SourceId, Span};
use ply_syntax::ast::*;
use ply_syntax::parse_unexpanded;

/// **What the differential compares, and what it structurally cannot see.**
pub mod dumper_boundaries {}

/// The whole answer for one file: the tree, then every diagnostic in the order the parser raised
/// them.
pub fn reference_dump(text: &str) -> String {
    let (module, diags) = parse_unexpanded(SourceId(0), ModuleName::anonymous(), text);
    // The one observable difference between the two entry points that is a *field* rather than a
    // node: `effect_set::expand`'s `write_back` fills this in, so an empty one is evidence the pass
    // did not run.
    for item in &module.items {
        if let Item::EffectSet(d) = item {
            assert!(
                d.expansion.is_empty(),
                "`parse_unexpanded` handed back an effect set whose `expansion` is filled \
                 in, so `effect_set::expand` has run and this is no longer the tree the \
                 grammar built"
            );
        }
    }
    dump_of(text, &module, &diags)
}

/// The dump of one already-parsed module.
fn dump_of(text: &str, module: &Module, diags: &[Diagnostic]) -> String {
    let mut d = Dumper {
        text,
        out: String::new(),
    };
    d.list(&module.imports, Dumper::import);
    d.list(&module.items, Dumper::item);
    d.diags(diags);
    d.out
}

/// The reference dump of `text` **after** the three rewrites — `parse_recovering`'s tree — for
/// the third differential, against `rewrite.ply`.
pub fn reference_dump_expanded(text: &str) -> String {
    let (module, diags) = ply_syntax::parse_recovering(SourceId(0), ModuleName::anonymous(), text);
    dump_of(text, &module, &diags)
}

/// The reference's derive expansion of each module on its own, for the fifth differential: the
/// source every derivation generates, byte for byte, and the diagnostics expansion raises.
pub fn reference_derive_dump(modules: &[(String, String)]) -> String {
    let mut out = String::new();
    for (i, (name, text)) in modules.iter().enumerate() {
        let (mut module, _) =
            ply_syntax::parse_recovering(SourceId(i as u32), ModuleName::from_dotted(name), text);
        let sources = ply_derive::preview(&module);
        let diags = ply_derive::expand_module(&mut module);
        out.push_str(&format!("M;{};", sources.len()));
        for s in &sources {
            out.push_str(&format!("S;{};{s}", s.len()));
        }
        resolve_diags(&mut out, &diags);
    }
    out
}

/// The reference's check of a program given as `(module name, source)` pairs: the rewrites, the
/// derive expansion, the resolver and `check_program` in the driver's order, dumped for the
/// fourth differential.
pub fn reference_check_dump(modules: &[(String, String)]) -> String {
    let mut program = Program {
        modules: Vec::new(),
    };
    for (i, (name, text)) in modules.iter().enumerate() {
        let (module, _) =
            ply_syntax::parse_recovering(SourceId(i as u32), ModuleName::from_dotted(name), text);
        program.modules.push(module);
    }
    let mut out = String::new();
    out.push_str(&format!("K;{};", modules.len()));
    // Before anything is resolved, as the driver and `check_module` expand: what resolution
    // and inference see is ordinary definitions.
    let expansion = ply_derive::expand_program(&mut program);
    if !expansion.is_empty() {
        out.push_str("X;");
        resolve_diags(&mut out, &expansion);
        return out;
    }
    let resolved = match ply_syntax::resolve::resolve(&mut program) {
        Ok(r) => r,
        Err(diags) => {
            out.push_str("X;");
            resolve_diags(&mut out, &diags);
            return out;
        }
    };
    match ply_core::check_program(&program, &resolved) {
        Ok(check) => {
            for (name, def) in &check.defs {
                out.push_str(&format!(
                    "F;{name};{};{};{};{};{};",
                    ply_core::print_scheme(&def.scheme),
                    footprint_text(&def.footprint),
                    footprint_text(&def.performed),
                    def.constraints
                        .iter()
                        .map(|c| format!("{}{}", c.deriver, c.param))
                        .collect::<Vec<_>>()
                        .join(","),
                    if def.internally_effectful { 1 } else { 0 }
                ));
            }
            for t in &check.tests {
                out.push_str(&format!(
                    "T;{};{};{};",
                    t.key,
                    if t.nondet { 1 } else { 0 },
                    footprint_text(&t.footprint)
                ));
            }
            for l in &check.laws {
                let binders: Vec<String> = l
                    .binders
                    .iter()
                    .map(|b| format!("{}:{}", b.name, ply_core::print_type(&b.ty)))
                    .collect();
                out.push_str(&format!(
                    "L;{};{};{};{};{};",
                    l.key,
                    binders.join(","),
                    if l.has_guard { 1 } else { 0 },
                    if l.host { 1 } else { 0 },
                    footprint_text(&l.footprint)
                ));
            }
            for (name, e) in &check.effects {
                if e.module.is_anonymous() {
                    continue;
                }
                let ops: Vec<String> = e
                    .ops
                    .values()
                    .map(|o| {
                        let params: Vec<String> =
                            o.params.iter().map(ply_core::print_type).collect();
                        format!(
                            "{}:{}:{}:{}:{}",
                            o.name,
                            o.mode.as_str(),
                            if o.resource_param { 1 } else { 0 },
                            params.join("+"),
                            ply_core::print_type(&o.ret)
                        )
                    })
                    .collect();
                out.push_str(&format!(
                    "E;{name};{};{};",
                    if e.nondet { 1 } else { 0 },
                    ops.join(",")
                ));
            }
            for (name, c) in &check.ctors {
                if c.module.is_anonymous() {
                    continue;
                }
                out.push_str(&format!(
                    "C;{name};{};{};{};{};",
                    c.type_name,
                    c.index,
                    c.arity,
                    ply_core::print_scheme(&c.scheme)
                ));
            }
        }
        Err(diags) => {
            out.push_str("X;");
            resolve_diags(&mut out, &diags);
        }
    }
    out
}

fn footprint_text(f: &ply_core::Footprint) -> String {
    f.atoms()
        .map(|a| a.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// **The tree half of the same cost: how many nodes the three rewrites add.** Signed, because
/// two rewrites remove nodes: `try_op` unwraps a `?` it refused (one node fewer, and a
/// diagnostic the other half counts), and `record_update` drops the base of an update that
/// writes every field, since nothing is left to copy from it.
pub fn nodes_the_rewrites_add(text: &str) -> isize {
    let (before, bd) = parse_unexpanded(SourceId(0), ModuleName::anonymous(), text);
    let (after, ad) = ply_syntax::parse_recovering(SourceId(0), ModuleName::anonymous(), text);
    let b = node_count(&dump_of(text, &before, &bd));
    let a = node_count(&dump_of(text, &after, &ad));
    a as isize - b as isize
}

// > **WITHDRAWN 2026-08-30 — the projection is gone, not merely unused.**

/// **What the pre-expansion comparison gives up, as data rather than as prose.**
pub fn diagnostics_the_rewrites_add(text: &str) -> Vec<String> {
    let (_, before) = parse_unexpanded(SourceId(0), ModuleName::anonymous(), text);
    let (_, after) = ply_syntax::parse_recovering(SourceId(0), ModuleName::anonymous(), text);
    assert!(
        after.len() >= before.len(),
        "the rewrites removed a diagnostic, which no pass can do: {} before, {} after",
        before.len(),
        after.len()
    );
    after[before.len()..]
        .iter()
        .map(|d| d.code.to_string())
        .collect()
}

/// Whether this file would have had `effect_set::expand` run over it, had the comparison entered at
/// `parse_recovering`.
pub fn uses_effect_sets(text: &str) -> bool {
    let (module, _) = parse_unexpanded(SourceId(0), ModuleName::anonymous(), text);
    let mut found = module.items.iter().any(|i| matches!(i, Item::EffectSet(_)));
    walk_rows(&module, &mut |r: &RowExpr| {
        if !r.aliases.is_empty() {
            found = true;
        }
    });
    found
}

/// Every `RowExpr` the module holds, in the order the dump reaches them.
fn walk_rows(module: &Module, f: &mut impl FnMut(&RowExpr)) {
    let mut d = Dumper {
        text: "",
        out: String::new(),
    };
    d.rows_of_module(module, f);
}

struct Dumper<'a> {
    text: &'a str,
    out: String,
}

impl<'a> Dumper<'a> {
    // --- the encoder's five terminals ---------------------------------------

    fn rec(&mut self, span: Span, tag: &str) {
        self.out
            .push_str(&format!("{}:{}:{};", span.start, span.end, tag));
    }

    fn nlist(&mut self, k: usize) {
        self.out.push_str(&format!("#{k};"));
    }

    fn opt_flag(&mut self, present: bool) {
        self.out.push_str(if present { "?1;" } else { "?0;" });
    }

    fn word(&mut self, w: &str) {
        self.out.push('%');
        self.out.push_str(w);
        self.out.push(';');
    }

    fn payload(&mut self, bytes: &[u8]) {
        self.out.push('@');
        for b in bytes {
            self.out.push_str(&format!("{b:02x}"));
        }
        self.out.push(';');
    }

    fn list<T>(&mut self, xs: &[T], f: impl Fn(&mut Self, &T)) {
        self.nlist(xs.len());
        for x in xs {
            f(self, x);
        }
    }

    fn opt<T>(&mut self, o: Option<&T>, f: impl Fn(&mut Self, &T)) {
        match o {
            None => self.opt_flag(false),
            Some(x) => {
                self.opt_flag(true);
                f(self, x);
            }
        }
    }

    /// The bytes of the source under a node's own span, which is what the Ply side's `src_over`
    /// answers.
    fn src_over(&self, span: Span) -> &'a str {
        &self.text[span.start as usize..span.end as usize]
    }

    // --- leaves -------------------------------------------------------------

    fn ident(&mut self, i: &Ident) {
        self.rec(i.span, "ident");
        self.payload(i.name.as_str().as_bytes());
    }

    fn qname(&mut self, q: &QName) {
        self.rec(q.span, "qname");
        self.opt(q.module.as_ref(), Self::ident);
        self.ident(&q.name);
    }

    fn boolean(&mut self, b: bool) {
        self.word(if b { "true" } else { "false" });
    }

    fn vis(&mut self, v: Visibility) {
        match v {
            Visibility::Public => self.word("pub"),
            Visibility::Private => self.word("priv"),
        }
    }

    fn mode(&mut self, m: Mode) {
        match m {
            Mode::Read => self.word("read"),
            Mode::Write => self.word("write"),
        }
    }

    fn deriver(&mut self, d: Deriver) {
        match d {
            Deriver::Json => self.word("json"),
            Deriver::Eq => self.word("eq"),
            Deriver::Ord => self.word("ord"),
        }
    }

    fn spec_kind(&mut self, k: SpecKind) {
        match k {
            SpecKind::Requires => self.word("requires"),
            SpecKind::Ensures => self.word("ensures"),
        }
    }

    /// `span` is the *literal node's* span, not the token's: a negative literal in a pattern is one
    /// `PatternKind::Lit` covering the `-` as well, and `patterns.ply` dumps the source over that
    /// wider span.
    fn lit(&mut self, l: &Lit, span: Span) {
        match l {
            Lit::Int(v) => {
                self.word("int");
                self.payload(v.to_string().as_bytes());
            }
            Lit::Bool(b) => {
                self.word("bool");
                self.payload(if *b { b"1" } else { b"0" });
            }
            Lit::Str(s) => {
                self.word("str");
                self.payload(s.as_bytes());
            }
            Lit::Bytes(b) => {
                self.word("bytes");
                self.payload(b);
            }
            Lit::Float(_) => {
                self.word("float");
                let raw = self.src_over(span).as_bytes().to_vec();
                self.payload(&raw);
            }
            Lit::Decimal { .. } => {
                self.word("dec");
                let raw = self.src_over(span).as_bytes().to_vec();
                self.payload(&raw);
            }
            Lit::Unit => self.word("unit"),
        }
    }

    // --- types --------------------------------------------------------------

    fn ty(&mut self, t: &TypeExpr) {
        match t {
            TypeExpr::Var(i) => {
                self.rec(i.span, "tvar");
                self.ident(i);
            }
            TypeExpr::Con { name, args, span } => {
                self.rec(*span, "tcon");
                self.qname(name);
                self.list(args, Self::ty);
            }
            TypeExpr::Fn {
                params,
                ret,
                effects,
                span,
            } => {
                self.rec(*span, "tfn");
                self.list(params, Self::ty);
                self.ty(ret);
                self.opt(effects.as_ref(), Self::row);
            }
            TypeExpr::Record { fields, span } => {
                self.rec(*span, "trec");
                self.list(fields, |d, (n, t)| {
                    d.ident(n);
                    d.ty(t);
                });
            }
            TypeExpr::Unit { span } => self.rec(*span, "tuni"),
        }
    }

    /// Every atom the row **wrote**, which is every atom it holds: no set has been spliced into it,
    /// because `effect_set::expand` did not run.
    fn row(&mut self, r: &RowExpr) {
        self.rec(r.span, "row");
        self.list(&r.atoms, Self::atom);
        self.list(&r.aliases, Self::qname);
        self.opt(r.tail.as_ref(), Self::ident);
    }

    fn atom(&mut self, a: &AtomExpr) {
        self.rec(a.span, "atm");
        self.qname(&a.effect);
        self.mode(a.mode);
        self.opt(a.resource.as_ref(), Self::ident);
    }

    /// `Generics` carries no span in `ast.rs`, so this leads with a word rather than a record.
    fn generics(&mut self, g: &Generics) {
        self.word("gen");
        self.list(&g.types, Self::ident);
        self.list(&g.effects, Self::ident);
    }

    /// The fallback expression is dumped like any other `Option`, and that is a change: it arrived
    /// with default arguments and nothing emitted it until `../GAPS.md` §11R.D moved this comparison to the
    /// pre-rewrite tree.
    fn param(&mut self, p: &Param) {
        self.rec(p.span, "prm");
        self.ident(&p.name);
        self.opt(p.ty.as_ref(), Self::ty);
        self.opt(p.default.as_ref(), Self::expr);
    }

    // --- patterns -----------------------------------------------------------

    fn pattern(&mut self, p: &Pattern) {
        match &p.kind {
            PatternKind::Wildcard => self.rec(p.span, "pwld"),
            PatternKind::Var(i) => {
                self.rec(p.span, "pvar");
                self.ident(i);
            }
            PatternKind::Lit(l) => {
                self.rec(p.span, "plit");
                self.lit(l, p.span);
            }
            PatternKind::Ctor { name, args } => {
                self.rec(p.span, "pctr");
                self.qname(name);
                self.list(args, Self::pattern);
            }
            PatternKind::Record { fields, rest } => {
                self.rec(p.span, "prec");
                self.opt_flag(*rest);
                self.list(fields, |d, (n, p)| {
                    d.ident(n);
                    d.pattern(p);
                });
            }
            PatternKind::List { items, rest } => {
                self.rec(p.span, "plst");
                self.list(items, Self::pattern);
                self.opt(rest.as_deref(), Self::pattern);
            }
        }
    }

    // --- expressions --------------------------------------------------------

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Lit(l) => {
                self.rec(e.span, "elit");
                self.lit(l, e.span);
            }
            ExprKind::Var(q) => {
                self.rec(e.span, "evar");
                self.qname(q);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                self.rec(e.span, "ebin");
                self.word(bin_op_name(*op));
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Unary { op, operand } => {
                self.rec(e.span, "eun");
                self.word(un_op_name(*op));
                self.expr(operand);
            }
            ExprKind::Lambda { params, body, ret } => {
                self.rec(e.span, "elam");
                self.list(params, Self::param);
                self.opt(ret.as_ref(), Self::ty);
                self.expr(body);
            }
            // Every `name: value` argument, with its own span, its name and its value — and the
            // list's length, so a call that dropped one could not be absorbed.
            ExprKind::App { func, args, named } => {
                self.rec(e.span, "eapp");
                self.expr(func);
                self.list(args, Self::expr);
                self.list(named, Self::named_arg);
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.rec(e.span, "eif");
                self.expr(cond);
                self.expr(then_branch);
                self.expr(else_branch);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.rec(e.span, "emat");
                self.expr(scrutinee);
                self.list(arms, Self::arm);
            }
            ExprKind::Block { stmts, tail } => {
                self.rec(e.span, "eblk");
                self.list(stmts, Self::stmt);
                self.opt(tail.as_deref(), Self::expr);
            }
            ExprKind::Record { fields } => {
                self.rec(e.span, "erec");
                self.list(fields, |d, (n, v)| {
                    d.ident(n);
                    d.expr(v);
                });
            }
            ExprKind::Field { base, field } => {
                self.rec(e.span, "efld");
                self.expr(base);
                self.ident(field);
            }
            ExprKind::List { items } => {
                self.rec(e.span, "elst");
                self.list(items, Self::expr);
            }
            ExprKind::Perform {
                effect,
                op,
                resource,
                args,
            } => {
                self.rec(e.span, "eprf");
                self.qname(effect);
                self.ident(op);
                self.opt(resource.as_ref(), Self::ident);
                self.list(args, Self::expr);
            }
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => {
                self.rec(e.span, "ehnd");
                self.expr(body);
                self.list(clauses, Self::handle_clause);
                self.opt(return_clause.as_deref(), Self::return_clause);
            }
            ExprKind::WithCell {
                resource,
                init,
                binder,
                body,
            } => {
                self.rec(e.span, "ecel");
                self.ident(resource);
                self.expr(init);
                self.ident(binder);
                self.expr(body);
            }
            ExprKind::WithRegion { region, body } => {
                self.rec(e.span, "ergn");
                self.ident(region);
                self.expr(body);
            }
            ExprKind::Simulate { body } => {
                self.rec(e.span, "esim");
                self.expr(body);
            }

            ExprKind::RecordUpdate { base, fields } => {
                self.rec(e.span, "erup");
                self.expr(base);
                self.list(fields, |d, (n, v)| {
                    d.ident(n);
                    d.expr(v);
                });
            }
            ExprKind::Try { operand } => {
                self.rec(e.span, "etry");
                self.expr(operand);
            }
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let {
                pat,
                ty,
                value,
                span,
            } => {
                self.rec(*span, "slet");
                self.pattern(pat);
                self.opt(ty.as_ref(), Self::ty);
                self.expr(value);
            }
            Stmt::Expr(e) => {
                self.rec(e.span, "sexp");
                self.expr(e);
            }
        }
    }

    /// A named argument carries **its own span** as well as its name and its value: `E0123` and
    /// `E0124` both point at exactly that span, and nothing else in the dump would pin it.
    fn named_arg(&mut self, n: &NamedArg) {
        self.rec(n.span, "narg");
        self.ident(&n.name);
        self.expr(&n.value);
    }

    fn arm(&mut self, m: &MatchArm) {
        self.rec(m.span, "arm");
        self.pattern(&m.pat);
        self.opt(m.guard.as_ref(), Self::expr);
        self.expr(&m.body);
    }

    fn handle_clause(&mut self, h: &HandleClause) {
        self.rec(h.span, "hcl");
        self.qname(&h.effect);
        self.ident(&h.op);
        self.opt(h.resource.as_ref(), Self::ident);
        self.list(&h.params, Self::ident);
        self.opt(h.resume.as_ref(), Self::ident);
        self.expr(&h.body);
    }

    fn return_clause(&mut self, r: &ReturnClause) {
        self.rec(r.span, "rcl");
        self.ident(&r.binder);
        self.expr(&r.body);
    }

    // --- items --------------------------------------------------------------

    fn import(&mut self, d: &ImportDecl) {
        self.rec(d.span, "imp");
        self.list(&d.path, Self::ident);
        match &d.kind {
            ImportKind::Module => self.word("mod"),
            ImportKind::Alias(a) => {
                self.word("alias");
                self.ident(a);
            }
            ImportKind::Names(ns) => {
                self.word("names");
                self.list(ns, Self::ident);
            }
        }
    }

    fn constraint(&mut self, c: &Constraint) {
        self.rec(c.span, "cst");
        self.deriver(c.deriver);
        self.rec(c.deriver_span, "dsp");
        self.ident(&c.param);
    }

    fn spec(&mut self, s: &SpecClause) {
        self.rec(s.span, "spc");
        self.spec_kind(s.kind);
        self.expr(&s.expr);
    }

    fn fn_def(&mut self, d: &FnDef) {
        // `FnDef::derived` is written `None` at `parser.rs:723` and nothing in the parser can
        // produce anything else, so `items.ply` does not carry it.
        assert!(
            d.derived.is_none(),
            "the parser filled in `FnDef::derived`, which the Ply port does not carry; \
             the dump grammar has to gain a field before this comparison means anything"
        );
        self.rec(d.span, "fn");
        self.vis(d.vis);
        self.opt(d.reuse.as_ref(), |s, span| s.rec(*span, "reu"));
        self.ident(&d.name);
        self.generics(&d.generics);
        self.list(&d.params, Self::param);
        self.opt(d.ret.as_ref(), Self::ty);
        self.opt(d.effects.as_ref(), Self::row);
        self.list(&d.constraints, Self::constraint);
        self.list(&d.spec, Self::spec);
        self.expr(&d.body);
    }

    fn binder(&mut self, b: &Binder) {
        self.rec(b.span, "bnd");
        self.ident(&b.name);
        self.ty(&b.ty);
    }

    fn law(&mut self, d: &LawDef) {
        self.rec(d.span, "law");
        self.rec(d.name_span, "lnm");
        self.payload(d.name.as_bytes());
        self.boolean(d.host);
        self.list(&d.binders, Self::binder);
        self.opt(d.guard.as_ref(), Self::expr);
        self.expr(&d.body);
    }

    fn variant(&mut self, v: &VariantDef) {
        self.rec(v.span, "var");
        self.ident(&v.name);
        self.list(&v.fields, Self::ty);
    }

    fn type_def(&mut self, d: &TypeDef) {
        self.rec(d.span, "ty");
        self.vis(d.vis);
        self.ident(&d.name);
        self.list(&d.params, Self::ident);
        match &d.body {
            TypeDefBody::Alias(t) => {
                self.word("alias");
                self.ty(t);
            }
            TypeDefBody::Sum(vs) => {
                self.word("sum");
                self.list(vs, Self::variant);
            }
        }
    }

    fn op_def(&mut self, o: &OpDef) {
        self.rec(o.span, "op");
        self.ident(&o.name);
        self.mode(o.mode);
        self.boolean(o.resource_param);
        self.list(&o.params, Self::ty);
        self.ty(&o.ret);
    }

    fn effect_def(&mut self, d: &EffectDef) {
        self.rec(d.span, "eff");
        self.vis(d.vis);
        self.ident(&d.name);
        self.boolean(d.nondet);
        self.list(&d.ops, Self::op_def);
    }

    fn test_def(&mut self, d: &TestDef) {
        self.rec(d.span, "tst");
        self.rec(d.name_span, "tnm");
        self.payload(d.name.as_bytes());
        self.boolean(d.nondet);
        self.expr(&d.body);
    }

    fn derive_def(&mut self, d: &DeriveDef) {
        self.rec(d.span, "der");
        self.deriver(d.deriver);
        self.rec(d.deriver_span, "dsp");
        self.ident(&d.target);
    }

    /// The `expansion` list is `effect_set::expand`'s own output, written back into the tree by
    /// `write_back`, so entering at `parse_unexpanded` makes it always empty.
    fn effect_set(&mut self, d: &EffectSetDef) {
        self.rec(d.span, "set");
        self.ident(&d.name);
        self.list(&d.atoms, Self::atom);
        self.list(&d.includes, Self::qname);
        self.list(&d.expansion, Self::atom);
    }

    fn item(&mut self, i: &Item) {
        match i {
            Item::Fn(d) => self.fn_def(d),
            Item::Type(d) => self.type_def(d),
            Item::Effect(d) => self.effect_def(d),
            Item::Test(d) => self.test_def(d),
            Item::Law(d) => self.law(d),
            Item::Derive(d) => self.derive_def(d),
            Item::EffectSet(d) => self.effect_set(d),
        }
    }

    // --- diagnostics --------------------------------------------------------

    fn diags(&mut self, ds: &[Diagnostic]) {
        self.nlist(ds.len());
        for d in ds {
            let s = d.primary_span().unwrap_or(Span::DUMMY);
            self.out.push_str(&format!(
                "!{}:{}:{}:{}:{};",
                d.code,
                s.start,
                s.end,
                d.labels.len(),
                d.notes.len()
            ));
            for l in &d.labels {
                self.out.push_str(&format!(
                    "={}:{}:{};",
                    l.span.start,
                    l.span.end,
                    if l.primary { 1 } else { 0 }
                ));
            }
        }
    }

    // --- the row walk, for `uses_effect_sets` --------------------------------

    fn rows_of_module(&mut self, m: &Module, f: &mut impl FnMut(&RowExpr)) {
        for i in &m.items {
            match i {
                Item::Fn(d) => {
                    if let Some(r) = &d.effects {
                        f(r);
                    }
                    for p in &d.params {
                        if let Some(t) = &p.ty {
                            self.rows_of_ty(t, f);
                        }
                    }
                    if let Some(t) = &d.ret {
                        self.rows_of_ty(t, f);
                    }
                }
                Item::Type(d) => match &d.body {
                    TypeDefBody::Alias(t) => self.rows_of_ty(t, f),
                    TypeDefBody::Sum(vs) => {
                        for v in vs {
                            for t in &v.fields {
                                self.rows_of_ty(t, f);
                            }
                        }
                    }
                },
                Item::Effect(d) => {
                    for o in &d.ops {
                        for t in &o.params {
                            self.rows_of_ty(t, f);
                        }
                        self.rows_of_ty(&o.ret, f);
                    }
                }
                Item::Test(_) | Item::Law(_) | Item::Derive(_) | Item::EffectSet(_) => {}
            }
        }
    }

    fn rows_of_ty(&mut self, t: &TypeExpr, f: &mut impl FnMut(&RowExpr)) {
        match t {
            TypeExpr::Var(_) | TypeExpr::Unit { .. } => {}
            TypeExpr::Con { args, .. } => {
                for a in args {
                    self.rows_of_ty(a, f);
                }
            }
            TypeExpr::Fn {
                params,
                ret,
                effects,
                ..
            } => {
                for p in params {
                    self.rows_of_ty(p, f);
                }
                self.rows_of_ty(ret, f);
                if let Some(r) = effects {
                    f(r);
                }
            }
            TypeExpr::Record { fields, .. } => {
                for (_, ft) in fields {
                    self.rows_of_ty(ft, f);
                }
            }
        }
    }
}

fn bin_op_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Rem => "rem",
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => "lt",
        BinOp::Le => "le",
        BinOp::Gt => "gt",
        BinOp::Ge => "ge",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::Concat => "concat",
        BinOp::BitAnd => "bitand",
        BinOp::BitOr => "bitor",
        BinOp::BitXor => "bitxor",
        BinOp::Shl => "shl",
        BinOp::Shr => "shr",
        BinOp::Ushr => "ushr",
    }
}

fn un_op_name(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "neg",
        UnOp::Not => "not",
        UnOp::BitNot => "bitnot",
    }
}

/// The dump as a list of records, for a diff that names the first disagreement instead of printing
/// two multi-megabyte strings.
pub fn records(dump: &str) -> Vec<&str> {
    dump.split_terminator(';').collect()
}

/// How many `S:E:TAG;` records the dump holds — the node count, which is what a corpus figure has
/// to state alongside the byte count.
pub fn node_count(dump: &str) -> usize {
    records(dump)
        .iter()
        .filter(|r| !r.is_empty() && !r.starts_with(['#', '?', '%', '@', '!', '=']))
        .count()
}

/// Every distinct node tag the dump reached, sorted — the tag-coverage statistic, so "agrees on the
/// corpus" can be read next to "and the corpus reaches these 30 of the 44 tags this grammar can
/// emit".
pub fn tags(dump: &str) -> Vec<String> {
    let mut v: Vec<String> = records(dump)
        .iter()
        .filter(|r| !r.is_empty() && !r.starts_with(['#', '?', '@', '!', '=']))
        .map(|r| {
            if let Some(w) = r.strip_prefix('%') {
                format!("%{w}")
            } else {
                r.rsplit(':').next().unwrap_or("").to_string()
            }
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The fixtures in a bundle file, in order.
pub fn bundle(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in text.split_inclusive('\n') {
        if line.trim_end_matches('\n') == "%%" {
            if let Some(c) = cur.take() {
                out.push(strip_one_newline(c));
            }
            cur = Some(String::new());
        } else if let Some(c) = cur.as_mut() {
            c.push_str(line);
        }
    }
    if let Some(c) = cur {
        out.push(strip_one_newline(c));
    }
    out
}

fn strip_one_newline(mut s: String) -> String {
    if s.ends_with('\n') {
        s.pop();
    }
    s
}

/// One diagnostic, read back out of a dump: code, primary span, every label, and the note count.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DumpedDiag {
    pub code: String,
    pub span: (u32, u32),
    pub notes: usize,
    pub labels: Vec<(u32, u32, bool)>,
}

/// A dump split into its tree and its diagnostics.
pub fn split_diags(dump: &str) -> Option<(&str, Vec<DumpedDiag>)> {
    let mut starts: Vec<usize> = Vec::new();
    for (i, _) in dump.match_indices('#') {
        starts.push(i);
    }
    for &at in starts.iter().rev() {
        let rest = &dump[at..];
        let Some(semi) = rest.find(';') else { continue };
        let Ok(k) = rest[1..semi].parse::<usize>() else {
            continue;
        };
        if let Some(ds) = read_diags(&rest[semi + 1..], k) {
            return Some((&dump[..at], ds));
        }
    }
    None
}

fn read_diags(mut tail: &str, k: usize) -> Option<Vec<DumpedDiag>> {
    let mut out = Vec::with_capacity(k);
    for _ in 0..k {
        let body = tail.strip_prefix('!')?;
        let end = body.find(';')?;
        let f: Vec<&str> = body[..end].split(':').collect();
        if f.len() != 5 {
            return None;
        }
        let nlabels: usize = f[3].parse().ok()?;
        let mut d = DumpedDiag {
            code: f[0].to_string(),
            span: (f[1].parse().ok()?, f[2].parse().ok()?),
            notes: f[4].parse().ok()?,
            labels: Vec::with_capacity(nlabels),
        };
        tail = &body[end + 1..];
        for _ in 0..nlabels {
            let lb = tail.strip_prefix('=')?;
            let e = lb.find(';')?;
            let g: Vec<&str> = lb[..e].split(':').collect();
            if g.len() != 3 {
                return None;
            }
            d.labels
                .push((g[0].parse().ok()?, g[1].parse().ok()?, g[2] == "1"));
            tail = &lb[e + 1..];
        }
        out.push(d);
    }
    if tail.is_empty() { Some(out) } else { None }
}

/// A `b"..."` literal holding exactly these bytes, for embedding a source file in a generated Ply
/// program.
pub fn byte_literal(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 16);
    out.push_str("b\"");
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('"');
    out
}

// --- The resolve phase -------------------------------------------------------
//
// The second phase under comparison: `ply_syntax::resolve` over a whole program, and the
// `defaults` pass it ends with. One record-based dump of the resolved tables, the load order, the
// diagnostics with their module index, and the post-defaults trees — the same encoding the Ply
// port's `resolve.ply` writes.

/// The reference's resolution of a program given as `(module name, source)` pairs, in the same
/// order and with the same `SourceId`s the harness hands the Ply side.
pub fn reference_resolve_dump(modules: &[(String, String)]) -> String {
    let mut program = Program {
        modules: Vec::new(),
    };
    for (i, (name, text)) in modules.iter().enumerate() {
        let (module, _) = parse_unexpanded(SourceId(i as u32), ModuleName::from_dotted(name), text);
        program.modules.push(module);
    }
    let mut out = String::new();
    out.push_str(&format!("R;{};", modules.len()));
    match ply_syntax::resolve::resolve(&mut program) {
        Ok(resolved) => {
            for (i, scope) in resolved.scopes.iter().enumerate() {
                out.push_str(&format!("M;{i};{};", scope.module));
                for (binder, (target, span)) in &scope.modules {
                    out.push_str(&format!("B;{binder};{target};{}:{};", span.start, span.end));
                }
                for (binder, (target, span)) in &scope.selective {
                    out.push_str(&format!("S;{binder};{target};{}:{};", span.start, span.end));
                }
                for (tag, space) in [
                    ("V", &scope.values),
                    ("T", &scope.types),
                    ("E", &scope.effects),
                ] {
                    for (name, b) in space {
                        out.push_str(&format!(
                            "{tag};{name};{};{};{}:{};",
                            b.qualified, b.owner, b.span.start, b.span.end
                        ));
                    }
                }
            }
            let order: Vec<String> = resolved.order.iter().map(|i| i.to_string()).collect();
            out.push_str(&format!("O;{};", order.join(",")));
            resolve_diags(&mut out, &[]);
            for (i, module) in program.modules.iter().enumerate() {
                out.push_str(&format!("P;{i};"));
                out.push_str(&dump_of(&modules[i].1, module, &[]));
            }
        }
        Err(diags) => {
            out.push_str("X;");
            resolve_diags(&mut out, &diags);
        }
    }
    out
}

/// `Dumper::diags` with each label's module in front of its span, since a program has many.
fn resolve_diags(out: &mut String, ds: &[Diagnostic]) {
    out.push_str(&format!("D;{};", ds.len()));
    for d in ds {
        let s = d.primary_span().unwrap_or(Span::DUMMY);
        out.push_str(&format!(
            "!{}:{}:{}:{}:{}:{};",
            d.code,
            s.source.0,
            s.start,
            s.end,
            d.labels.len(),
            d.notes.len()
        ));
        for l in &d.labels {
            out.push_str(&format!(
                "={}:{}:{}:{};",
                l.span.source.0,
                l.span.start,
                l.span.end,
                if l.primary { 1 } else { 0 }
            ));
        }
    }
}

/// A program bundle: programs separated by a line holding exactly `%%%`, modules within one by a
/// line holding exactly `%%`, and each module's first line its dotted name.
pub fn programs(text: &str) -> Vec<Vec<(String, String)>> {
    let mut out = Vec::new();
    // Everything before the first separator is the bundle's header, not a program.
    for chunk in text.split("\n%%%\n").skip(1) {
        let chunk = chunk.trim_start_matches('\n');
        if chunk.trim().is_empty() {
            continue;
        }
        let mut modules = Vec::new();
        for m in chunk.split("\n%%\n") {
            let (name, src) = m.split_once('\n').unwrap_or((m, ""));
            modules.push((name.trim().to_string(), src.to_string()));
        }
        out.push(modules);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dump_is_printable_ascii_with_no_quote_and_no_backslash() {
        let dump = reference_dump(
            "import a.b (c)\n\
             fn f(x: Int) -> Str / {db.read[users]} = \"a\\nb\" ++ b\"\\xff\"\n\
             test \"t\" { 1.5e-3 + 2.50m }\n\
             type T = | A(Int) | B\n",
        );
        for c in dump.chars() {
            assert!(
                c.is_ascii_graphic() && c != '"' && c != '\\',
                "the dump holds {c:?}, which `ply run --json` would escape:\n{dump}"
            );
        }
    }

    #[test]
    fn a_float_is_dumped_as_the_source_under_its_own_span() {
        // Not as `f64` bits: Ply cannot build one.
        let dump = reference_dump("fn f() = match 0 { -1.5e-3 -> 1 }\n");
        assert!(dump.contains("%float;@2d312e35652d33;"), "{dump}");
    }

    #[test]
    fn a_decimal_is_dumped_as_the_source_under_its_own_span() {
        let dump = reference_dump("fn f() = 2.50m\n");
        assert!(dump.contains("%dec;@322e35306d;"), "{dump}");
    }

    #[test]
    fn every_list_carries_its_length_and_every_option_its_presence() {
        // `fn f()` with no return type, no row, no constraints, no spec.
        let dump = reference_dump("fn f() = 1\n");
        assert_eq!(
            dump,
            "#0;#1;0:10:fn;%priv;?0;3:4:ident;@66;%gen;#0;#0;#0;?0;?0;#0;#0;9:10:elit;%int;@31;#0;"
        );
    }

    #[test]
    fn a_diagnostic_carries_every_label_and_the_note_count() {
        let dump = reference_dump("derive frobnicate for Order\n");
        let tail = &dump[dump.find("!E0207").expect("the unknown-deriver code")..];
        assert!(tail.starts_with("!E0207:7:17:1:1;=7:17:1;"), "{tail}");
    }

    #[test]
    fn node_count_counts_records_and_not_structure() {
        // one `fn`, one `ident`, one `elit`.
        assert_eq!(node_count(&reference_dump("fn f() = 1\n")), 3);
    }

    #[test]
    fn a_file_that_names_no_set_is_not_expanded() {
        assert!(!uses_effect_sets("fn f() -> Int / {db.read} = 1\n"));
        assert!(uses_effect_sets("effect set S = {a.read}\n"));
        assert!(uses_effect_sets("fn f() -> Int / {Web} = 1\n"));
    }

    #[test]
    fn a_dump_splits_into_its_tree_and_its_diagnostics() {
        let dump = reference_dump("derive frobnicate for Order\nfn f() = 1\n");
        let (tree, ds) = split_diags(&dump).expect("a well-formed dump");
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].code, "E0207");
        assert_eq!(ds[0].labels, vec![(7, 17, true)]);
        assert!(tree.ends_with("@31;"), "{tree}");
        // A tree holding its own `#K;` runs must not be mistaken for the block: the split is the
        // last suffix that parses, so an empty diagnostic list after a tree full of lists still
        // lands in the right place.
        let clean = reference_dump("fn f() = [1, 2, 3]\n");
        let (_, none) = split_diags(&clean).expect("a well-formed dump");
        assert!(none.is_empty());
    }

    #[test]
    fn a_bundle_gives_back_exactly_the_fixtures_that_were_written() {
        // The three cases the separator rule exists for: no trailing newline, one trailing newline,
        // and empty.
        let text = "header\nlines\n%%\nfn f() = 1\n%%\nfn g() = 2\n\n%%\n\n";
        assert_eq!(bundle(text), vec!["fn f() = 1", "fn g() = 2\n", ""]);
    }

    #[test]
    fn a_byte_literal_round_trips_every_byte_through_the_real_lexer() {
        let all: Vec<u8> = (0u8..=255).collect();
        let source = byte_literal(&all);
        let (tokens, diags) = ply_syntax::lexer::lex(SourceId(0), &source);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(tokens[0].kind, ply_syntax::lexer::TokenKind::Bytes(all));
    }
}

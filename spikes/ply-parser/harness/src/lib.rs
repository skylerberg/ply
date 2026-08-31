//! The reference side of the parser spike's differential: `ply_syntax`'s tree,
//! written out in the same flat dump grammar `spikes/ply-parser/*.ply` emits.
//!
//! The grammar is specified once, in `spine.ply`'s "The dump encoder" comment,
//! and this file is the other half of it:
//!
//! ```text
//!   S:E:TAG;        one node, span S..E; TAG fixes how many children follow
//!   #K;             a list of exactly K children follows
//!   ?0; / ?1;       an absent / present Option; ?1 is followed by one child
//!   @HEX;           an inline scalar payload, hex-encoded
//!   %WORD;          an enum arm
//!   !CODE:S:E:L:N;  a diagnostic: code, primary span, label count, note count
//!   =S:E:F;         one of that diagnostic's labels; F is 1 for primary
//! ```
//!
//! Three properties make it structural rather than a bag of nodes, and each is
//! a corruption `../arm-harness.sh` performs and watches go red: every list
//! emits its length, so a dropped element shifts every record after it; every
//! `Option` emits presence, so a dropped `Option` cannot be absorbed; and every
//! node leads with **its own span**, which is the hazard the lexer spike names
//! first — a parser with every tag right and every offset wrong would agree
//! with a span-blind comparator perfectly.
//!
//! Every character is printable ASCII and neither `"` nor `\` occurs, because
//! `ply run --json` renders a `String` through `ply_eval::value::escape` and a
//! dump holding either would make the harness compare its own unescaper against
//! the parser. `the_dump_is_printable_ascii_with_no_quote_and_no_backslash`
//! pins that of this side.
//!
//! **Not one `match` here has a `_` arm.** That is the whole reason a tree
//! differential is worth more than a diagnostics-only one: a variant added to
//! `ast.rs` has to be given a dump here or this file stops compiling, where a
//! wildcard would silently emit nothing and the comparison would stay green
//! over a field nobody reached.

use ply_span::{Diagnostic, SourceId, Span, Symbol};
use ply_syntax::ast::*;
use ply_syntax::parse_recovering;
use std::collections::HashMap;

/// The whole answer for one file: the tree, then every diagnostic in the order
/// the parser raised them.
///
/// `text` is kept because two of the dump's leaves are **source slices** rather
/// than values: `Lit::Float` and `Lit::Decimal` are dumped as the raw bytes
/// under the literal node's own span. Ply cannot build an `f64` from digits at
/// all, so a dump carrying the value would be comparing `f64::from_str` against
/// nothing. `patterns.ply`'s header fixes the rule for both sides, and it
/// closes the hole `spikes/ply-lexer/README.md` names by removing the
/// normalisation from the comparison rather than adding a third normaliser.
pub fn reference_dump(text: &str) -> String {
    let (module, diags) = parse_recovering(SourceId(0), ModuleName::anonymous(), text);
    let mut d = Dumper {
        text,
        out: String::new(),
        unexpand: None,
    };
    d.list(&module.imports, Dumper::import);
    d.list(&module.items, Dumper::item);
    d.diags(&diags);
    d.out
}

/// The same dump with `effect_set::expand`'s effect on the **tree** projected
/// back out: every row's atoms truncated to the ones that were written, and
/// every set's `expansion` emitted empty.
///
/// This exists because `items.ply` does not port the expander — the plan ranked
/// it last and the spike did not reach it — and `examples/desk.ply` is 21% of
/// the corpus by bytes and the one file in the tree that uses sets. Without
/// this the honest options are to drop that file or to compare it against a
/// pass the port does not have; this is the third, and it is a projection of
/// the reference's *own output*, not a re-implementation:
///
/// * `expand` appends to `row.atoms`, in order, one set's `expansion` per
///   entry of `row.aliases`. Those expansions are sitting in the tree, filled
///   in by `write_back`, so how many atoms were appended is read off rather
///   than recomputed. A set that was refused, one on a cycle, one named from
///   another module and one that does not exist all contribute an empty
///   expansion, which is exactly the zero atoms `expand` splices for them.
/// * `write_back` gives the expansion to the **first** declaration of a name
///   and to no later one, so the map here is built first-wins.
///
/// It is **not** applied to diagnostics: `expand` raises `E0105`, `E0114` and
/// `E0115` of its own, `items.ply` raises `E0114` for its own reasons, and
/// telling those apart by code would be guessing. A caller comparing an input
/// that uses sets therefore compares trees here and states the diagnostics
/// separately. `../GAPS-harness.md` §H4 carries the numbers.
pub fn reference_dump_unexpanded(text: &str) -> String {
    let (module, diags) = parse_recovering(SourceId(0), ModuleName::anonymous(), text);
    let mut expansions: HashMap<Symbol, usize> = HashMap::new();
    for item in &module.items {
        if let Item::EffectSet(d) = item {
            expansions
                .entry(d.name.name.clone())
                .or_insert(d.expansion.len());
        }
    }
    let mut d = Dumper {
        text,
        out: String::new(),
        unexpand: Some(expansions),
    };
    d.list(&module.imports, Dumper::import);
    d.list(&module.items, Dumper::item);
    d.diags(&diags);
    d.out
}

/// Whether `parse_recovering` ran `effect_set::expand` over this file.
///
/// It is private to `ply-syntax` and it runs *inside* `Parser::run`, so the
/// reference tree for a file that uses sets is post-expansion while
/// `items.ply` — which does not port the expander — answers the written row.
/// The corpus test uses this to state the boundary rather than to hide it: a
/// file it reports `true` for is compared with its rows excluded and the
/// exclusion is counted and printed.
///
/// This mirrors `parser.rs`'s own `uses_effect_sets` from the outside: the flag
/// is set by writing an `effect set` item or by naming one in a row, and after
/// the parse those are exactly a non-empty `EffectSetDef` list and a non-empty
/// `RowExpr::aliases`.
pub fn uses_effect_sets(text: &str) -> bool {
    let (module, _) = parse_recovering(SourceId(0), ModuleName::anonymous(), text);
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
        unexpand: None,
    };
    d.rows_of_module(module, f);
}

struct Dumper<'a> {
    text: &'a str,
    out: String,
    /// `Some(name -> expansion length)` in the projected mode described on
    /// [`reference_dump_unexpanded`]; `None` in the plain one.
    unexpand: Option<HashMap<Symbol, usize>>,
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

    /// The bytes of the source under a node's own span, which is what the Ply
    /// side's `src_over` answers.
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

    /// `span` is the *literal node's* span, not the token's: a negative literal
    /// in a pattern is one `PatternKind::Lit` covering the `-` as well, and
    /// `patterns.ply` dumps the source over that wider span. Passing the token's
    /// span here would agree on every positive number and differ on every
    /// negative one, which is exactly the class of bug the spans exist to catch.
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

    fn row(&mut self, r: &RowExpr) {
        self.rec(r.span, "row");
        let written = match &self.unexpand {
            None => r.atoms.len(),
            Some(map) => {
                let appended: usize = r
                    .aliases
                    .iter()
                    .filter(|q| q.is_bare())
                    .filter_map(|q| map.get(q.symbol()))
                    .sum();
                r.atoms.len().saturating_sub(appended)
            }
        };
        self.list(&r.atoms[..written], Self::atom);
        self.list(&r.aliases, Self::qname);
        self.opt(r.tail.as_ref(), Self::ident);
    }

    fn atom(&mut self, a: &AtomExpr) {
        self.rec(a.span, "atm");
        self.qname(&a.effect);
        self.mode(a.mode);
        self.opt(a.resource.as_ref(), Self::ident);
    }

    /// `Generics` carries no span in `ast.rs`, so this leads with a word rather
    /// than a record. Both lists still carry their length.
    fn generics(&mut self, g: &Generics) {
        self.word("gen");
        self.list(&g.types, Self::ident);
        self.list(&g.effects, Self::ident);
    }

    fn param(&mut self, p: &Param) {
        self.rec(p.span, "prm");
        self.ident(&p.name);
        self.opt(p.ty.as_ref(), Self::ty);
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
            ExprKind::Lambda { params, body } => {
                self.rec(e.span, "elam");
                self.list(params, Self::param);
                self.expr(body);
            }
            // `named` is empty: `defaults::expand` places every named argument
            // in `resolve`, which runs before anything here sees a tree.
            ExprKind::App { func, args, .. } => {
                self.rec(e.span, "eapp");
                self.expr(func);
                self.list(args, Self::expr);
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
        // `FnDef::derived` is written `None` at `parser.rs:723` and nothing in
        // the parser can produce anything else, so `items.ply` does not carry
        // it. **Asserted rather than skipped**: a field reached and not emitted
        // is precisely the survivor a dump-injectivity sweep exists to find,
        // and if a later change makes the parser fill it in, this is what says
        // so instead of the comparison staying quietly green.
        assert!(
            d.derived.is_none(),
            "the parser filled in `FnDef::derived`, which the Ply port does not carry; \
             the dump grammar has to gain a field before this comparison means anything"
        );
        self.rec(d.span, "fn");
        self.vis(d.vis);
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

    fn effect_set(&mut self, d: &EffectSetDef) {
        self.rec(d.span, "set");
        self.ident(&d.name);
        self.list(&d.atoms, Self::atom);
        self.list(&d.includes, Self::qname);
        match self.unexpand {
            None => self.list(&d.expansion, Self::atom),
            Some(_) => self.nlist(0),
        }
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
    //
    // Separate from the dump because it answers a different question and must
    // not be allowed to drift into one: it reaches every `RowExpr` a module
    // holds, and nothing else.

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
    }
}

fn un_op_name(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "neg",
        UnOp::Not => "not",
    }
}

/// The dump as a list of records, for a diff that names the first disagreement
/// instead of printing two multi-megabyte strings.
pub fn records(dump: &str) -> Vec<&str> {
    dump.split_terminator(';').collect()
}

/// How many `S:E:TAG;` records the dump holds — the node count, which is what a
/// corpus figure has to state alongside the byte count. `#K;`, `?F;`, `%W;`,
/// `@HEX;`, `!..;` and `=..;` are structure, not nodes.
pub fn node_count(dump: &str) -> usize {
    records(dump)
        .iter()
        .filter(|r| !r.is_empty() && !r.starts_with(['#', '?', '%', '@', '!', '=']))
        .count()
}

/// Every distinct node tag the dump reached, sorted — the tag-coverage
/// statistic, so "agrees on the corpus" can be read next to "and the corpus
/// reaches these 30 of the 44 tags this grammar can emit".
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
///
/// Everything before the first `%%` line is a header. After that, a line
/// holding exactly `%%` separates fixtures, and the newline immediately before
/// a separator — or before end of file — belongs to the separator rather than
/// to the fixture. `../mine-fixtures.py`'s doc comment says why that rule earns
/// its keep: `fn f() = "oops` unterminated at end of input and the same text
/// followed by a newline are two different arms of `lexer.rs`, and a bundle
/// that appended a newline would quietly test only one of them.
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

/// One diagnostic, read back out of a dump: code, primary span, every label,
/// and the note count.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DumpedDiag {
    pub code: String,
    pub span: (u32, u32),
    pub notes: usize,
    pub labels: Vec<(u32, u32, bool)>,
}

/// A dump split into its tree and its diagnostics.
///
/// The dump ends with `#K;` and then exactly K diagnostics and nothing else, so
/// this takes the **last** `#K;` whose tail parses as exactly that. Scanning
/// from the end rather than walking the tree forwards means this stays right
/// without a second copy of the grammar's arity table — the thing that would
/// have to be kept in step with `spine.ply` by hand and would not be.
///
/// `None` when no suffix parses, which is a malformed dump and must be loud.
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

/// A `b"..."` literal holding exactly these bytes, for embedding a source file
/// in a generated Ply program. Ply is the only way in: there is no file-reading
/// host handler, so a source file reaches a Ply program as a literal or not at
/// all.
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
        // Not as `f64` bits: Ply cannot build one. The negative case is the
        // one that catches a dumper reaching for the token instead of the node.
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
            "#0;#1;0:10:fn;%priv;3:4:ident;@66;%gen;#0;#0;#0;?0;?0;#0;#0;9:10:elit;%int;@31;#0;"
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
        // A tree holding its own `#K;` runs must not be mistaken for the block:
        // the split is the last suffix that parses, so an empty diagnostic list
        // after a tree full of lists still lands in the right place.
        let clean = reference_dump("fn f() = [1, 2, 3]\n");
        let (_, none) = split_diags(&clean).expect("a well-formed dump");
        assert!(none.is_empty());
    }

    #[test]
    fn a_bundle_gives_back_exactly_the_fixtures_that_were_written() {
        // The three cases the separator rule exists for: no trailing newline,
        // one trailing newline, and empty.
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

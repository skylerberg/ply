//! Record update expansion, which happens inside the parser.
//!
//! `{..b, f: e}` is sugar. An unexpanded [`ExprKind::RecordUpdate`] never
//! escapes [`crate::parse_module`]: this pass rewrites every one into the plain
//! [`ExprKind::Record`] a reader would have written by hand, so the two
//! spellings are **one definition with one hash** rather than two definitions
//! computing the same value.
//!
//! That is the whole constraint. Ply is content-addressed — a test re-runs iff
//! its hash is absent from the cache — so a sugar that hashed differently from
//! its expansion would move every definition that adopted it and would split
//! one value into two cache entries.
//!
//! ## Why here, and not in normalization
//!
//! The obvious place is `ply_hash::normalize`, and it cannot work: the driver
//! hashes before it infers (`crates/ply-cli/src/driver.rs`, parse → resolve →
//! hash → gate 2 → infer), which ADR 0002 pins deliberately, so normalization
//! has no types and cannot enumerate the base's fields. Changing normalization's
//! byte stream would also be a cache-format change, which moves every cached
//! result everywhere.
//!
//! ## Why it reads only this module
//!
//! Expansion reads **this module's own `type` items and the type annotations
//! written in this file, and nothing else**, for the reason `effect_set`
//! expansion does: gate 1 (ADR 0002) skips a file whose raw bytes are unchanged.
//! A shape read across a module boundary would let an edit in the declaring
//! module leave a stale expansion behind in a file that never moved — and a
//! stale expansion is a *wrong record*, not merely a stale name.
//!
//! The cost is a real restriction, recorded in `docs/adr/0023-record-update.md`
//! §4 with its lift path: `{..cfg, x: 1}` where `cfg: std::http::Limits` is
//! refused with `E0116`, because `Limits` is declared in another file.
//!
//! ## The canonical order
//!
//! Copies first, **sorted by field name**; then the written fields, in the order
//! written. Both halves are load-bearing and neither is taste:
//!
//! - *Sorted, not declaration order*, because `reordering_the_fields_of_a
//!   _record_type_is_free` (`crates/ply-hash/tests/suite/audit.rs`) is an invariant the
//!   suite asserts. Expanding in the type's declaration order would make
//!   reordering a `type` move the hash of every update against it.
//! - *Written fields last*, because `spikes/ply-lexer/GAPS.md` §1 measures a
//!   growing sub-expression in any but the last position of its enclosing node
//!   as quadratic. Copies are pure field reads and never grow, so putting them
//!   first is free — and it lands `{..s, toks: push(s.toks, t)}` on the linear
//!   spelling rather than the quadratic one.

use crate::ast::*;
use crate::effect_set::grow;
use indexmap::IndexMap;
use ply_span::{Diagnostic, Symbol, codes};

/// How deep an alias chain may be followed before the pass gives up.
///
/// A bound rather than a cycle check because this runs in the parser, before
/// anything has rejected `type A = B` / `type B = A`: a refusal here is a
/// diagnostic, where a loop would be a hang.
const MAX_ALIAS_DEPTH: u32 = 16;

/// Rewrites every `{..b, f: e}` in the module into the record literal it stands
/// for. Only called when the file wrote one, so a program that uses none pays
/// nothing.
pub(crate) fn expand(module: &mut Module, diags: &mut Vec<Diagnostic>) {
    let types = collect_types(module);
    let mut items = std::mem::take(&mut module.items);
    let mut cx = Cx {
        types: &types,
        diags,
        scope: Vec::new(),
    };
    for item in &mut items {
        cx.item(item);
    }
    module.items = items;
}

/// The same expansion for an expression parsed with no module around it
/// ([`crate::parse_expr`]).
///
/// Every base refuses, because there are no `type` items and no signatures to
/// read a shape from. That is the correct outcome and not a gap: what must not
/// happen is an unexpanded [`ExprKind::RecordUpdate`] reaching a crate whose
/// only arm for one is `unreachable!`.
pub(crate) fn expand_bare(e: &mut Expr, diags: &mut Vec<Diagnostic>) {
    let types = IndexMap::new();
    let mut cx = Cx {
        types: &types,
        diags,
        scope: Vec::new(),
    };
    cx.expr(e);
}

/// This module's own `type` items, by simple name, cloned so that the walk can
/// hold the module mutably. First wins: a duplicate is `E0105` from inference,
/// and a module that has one is rejected whichever of the two this picks.
fn collect_types(module: &Module) -> IndexMap<Symbol, TypeDef> {
    let mut out = IndexMap::new();
    for item in &module.items {
        if let Item::Type(d) = item {
            out.entry(d.name.name.clone())
                .or_insert_with(|| (**d).clone());
        }
    }
    out
}

/// Why a base has no shape this pass can name. Each variant carries what the
/// note has to say, because "cannot resolve" on its own tells a reader nothing
/// about which of these rules they hit.
enum Why {
    /// In scope, but nothing wrote its type.
    Unannotated(Symbol),
    /// Not a local binder at all — a function, an import, a prelude name.
    NotALocal(Symbol),
    /// `m::T`, or a base written `m::x`.
    CrossModule,
    /// A type this module does not declare: a builtin, or an imported one.
    Foreign(Symbol),
    /// A generic alias, or a type parameter.
    Generic,
    /// A sum type; there is nothing to copy field-wise.
    Sum(Symbol),
    /// A function type, `Unit`, or an inline non-record.
    NotARecord,
    /// `state.nosuch`.
    NoSuchField(Symbol, Vec<Symbol>),
    /// An alias chain longer than [`MAX_ALIAS_DEPTH`].
    TooDeep,
}

impl Why {
    fn note(&self) -> String {
        match self {
            Why::Unannotated(n) => format!(
                "`{n}` has no written type here; annotate the binder, as in `let {n}: T = ...`"
            ),
            Why::NotALocal(n) => format!(
                "`{n}` is not a local binder with a written type — a record update reads only \
                 annotations written in this file"
            ),
            Why::CrossModule => "the base and its type must both be named in this file; a shape \
                                 read across a module boundary would go stale without this file \
                                 changing"
                .to_string(),
            Why::Foreign(n) => format!(
                "`{n}` is not a `type` declared in this file, so its fields are not known here"
            ),
            Why::Generic => "a generic type has no field list until it is applied; write the \
                             fields out"
                .to_string(),
            Why::Sum(n) => format!("`{n}` is a sum type, and only a record has fields to update"),
            Why::NotARecord => "the base is not a record".to_string(),
            Why::NoSuchField(f, known) => match known.is_empty() {
                true => format!("no field `{f}`; the record has no fields"),
                false => format!(
                    "no field `{f}`; available fields: {}",
                    known
                        .iter()
                        .map(|k| format!("`{k}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
            Why::TooDeep => format!("more than {MAX_ALIAS_DEPTH} type aliases deep"),
        }
    }
}

/// What the innermost binding for a name says about its type.
enum Bound<'a> {
    Typed(&'a TypeExpr),
    Untyped,
    Missing,
}

struct Cx<'a> {
    types: &'a IndexMap<Symbol, TypeDef>,
    diags: &'a mut Vec<Diagnostic>,
    /// Every binder in scope, innermost last. A frame is a mark into this vector
    /// and is popped by truncating back to it.
    ///
    /// An unannotated binder is pushed as `None` rather than skipped. That is
    /// the difference between refusing a shadowed base and expanding it against
    /// the *outer* binder's type, which would be a wrong record that type-checks
    /// nowhere the reader is looking.
    scope: Vec<(Symbol, Option<TypeExpr>)>,
}

impl Cx<'_> {
    fn bind(&mut self, name: &Ident, ty: Option<TypeExpr>) {
        self.scope.push((name.name.clone(), ty));
    }

    /// Every binder a pattern introduces, all untyped: a pattern's binders take
    /// their types from the scrutinee, which this pass does not infer.
    fn bind_pattern(&mut self, p: &Pattern) {
        match &p.kind {
            PatternKind::Wildcard | PatternKind::Lit(_) => {}
            PatternKind::Var(n) => self.bind(n, None),
            PatternKind::Ctor { args, .. } => args.iter().for_each(|a| self.bind_pattern(a)),
            PatternKind::Record { fields, .. } => {
                fields.iter().for_each(|(_, p)| self.bind_pattern(p))
            }
            PatternKind::List { items, rest } => {
                items.iter().for_each(|p| self.bind_pattern(p));
                if let Some(r) = rest {
                    self.bind_pattern(r);
                }
            }
        }
    }

    fn lookup(&self, name: &Symbol) -> Bound<'_> {
        match self.scope.iter().rev().find(|(n, _)| n == name) {
            None => Bound::Missing,
            Some((_, None)) => Bound::Untyped,
            Some((_, Some(t))) => Bound::Typed(t),
        }
    }

    /// The written type of a path, following field projections through this
    /// module's aliases.
    fn type_of_path(&self, e: &Expr) -> Result<TypeExpr, Why> {
        match &e.kind {
            ExprKind::Var(q) if !q.is_bare() => Err(Why::CrossModule),
            ExprKind::Var(q) => match self.lookup(q.symbol()) {
                Bound::Typed(t) => Ok(t.clone()),
                Bound::Untyped => Err(Why::Unannotated(q.symbol().clone())),
                Bound::Missing => Err(Why::NotALocal(q.symbol().clone())),
            },
            ExprKind::Field { base, field } => {
                let outer = self.type_of_path(base)?;
                let fields = self.record_of(&outer, 0)?;
                match fields.iter().find(|(n, _)| n.name == field.name) {
                    Some((_, t)) => Ok(t.clone()),
                    None => Err(Why::NoSuchField(
                        field.name.clone(),
                        fields.iter().map(|(n, _)| n.name.clone()).collect(),
                    )),
                }
            }
            // The parser admits only `x` and `x.f...` as a base, so this is
            // unreachable from source; it is written out rather than
            // `unreachable!` because a refusal is the safe answer either way.
            _ => Err(Why::NotARecord),
        }
    }

    /// The fields of `ty`, following this module's own aliases to a record.
    fn record_of(&self, ty: &TypeExpr, depth: u32) -> Result<Vec<(Ident, TypeExpr)>, Why> {
        if depth > MAX_ALIAS_DEPTH {
            return Err(Why::TooDeep);
        }
        match ty {
            TypeExpr::Record { fields, .. } => Ok(fields.clone()),
            // A lowercase bare name is a type parameter bound by an enclosing
            // `<..>`, never a declared type.
            TypeExpr::Var(_) => Err(Why::Generic),
            TypeExpr::Con { name, args, .. } => {
                if !name.is_bare() {
                    return Err(Why::CrossModule);
                }
                if !args.is_empty() {
                    return Err(Why::Generic);
                }
                let sym = &name.name.name;
                match self.types.get(sym) {
                    None => Err(Why::Foreign(sym.clone())),
                    Some(d) if !d.params.is_empty() => Err(Why::Generic),
                    Some(d) => match &d.body {
                        TypeDefBody::Sum(_) => Err(Why::Sum(sym.clone())),
                        TypeDefBody::Alias(t) => self.record_of(t, depth + 1),
                    },
                }
            }
            TypeExpr::Fn { .. } | TypeExpr::Unit { .. } => Err(Why::NotARecord),
        }
    }

    /// Rewrites one update, or refuses it and leaves the written fields alone.
    ///
    /// On every refusal path the node still becomes an [`ExprKind::Record`]: a
    /// diagnostic has been recorded, and letting the sugar escape the parser
    /// would break the invariant every crate downstream relies on.
    fn expand_update(&mut self, base: Expr, written: Vec<(Ident, Expr)>) -> ExprKind {
        let shape = match self.type_of_path(&base) {
            Ok(ty) => self.record_of(&ty, 0),
            Err(why) => Err(why),
        };
        let shape = match shape {
            Ok(s) => s,
            Err(why) => {
                self.diags.push(
                    Diagnostic::error(
                        codes::RECORD_UPDATE_SHAPE,
                        "the record being updated has no field list this file can name",
                    )
                    .primary(base.span, "no record shape here")
                    .note(why.note()),
                );
                return ExprKind::Record { fields: written };
            }
        };

        let mut bad = false;
        for (name, _) in &written {
            if !shape.iter().any(|(n, _)| n.name == name.name) {
                bad = true;
                let known: Vec<String> =
                    shape.iter().map(|(n, _)| format!("`{}`", n.name)).collect();
                self.diags.push(
                    Diagnostic::error(
                        codes::RECORD_UPDATE_FIELD,
                        format!("the record being updated has no field `{}`", name.name),
                    )
                    .primary(name.span, "not a field of the base")
                    .secondary(base.span, "this is the record being updated")
                    .note(format!(
                        "an update replaces fields, it does not add them; available fields: {}",
                        known.join(", ")
                    )),
                );
            }
        }
        if bad {
            return ExprKind::Record { fields: written };
        }

        let mut copies: Vec<&Ident> = shape
            .iter()
            .map(|(n, _)| n)
            .filter(|n| !written.iter().any(|(w, _)| w.name == n.name))
            .collect();
        // By name, never by length. Every single-letter field set orders the
        // same way under both, and one mixed-length pair only rules out the
        // length direction it disagrees with, so each of these carries a pair
        // each way: `crate::tests::copies_are_sorted_by_name_and_not_by_length`,
        // and `ply-hash`'s `record_update_hashes_as_its_expansion` and
        // `a_projected_base_hashes_as_its_expansion`.
        copies.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));

        let mut fields: Vec<(Ident, Expr)> = Vec::with_capacity(copies.len() + written.len());
        for name in copies {
            let field = Ident::new(name.name.clone(), base.span);
            fields.push((
                field.clone(),
                Expr {
                    span: base.span,
                    kind: ExprKind::Field {
                        base: Box::new(base.clone()),
                        field,
                    },
                },
            ));
        }
        fields.extend(written);
        ExprKind::Record { fields }
    }

    fn item(&mut self, item: &mut Item) {
        match item {
            Item::Fn(d) => {
                let mark = self.scope.len();
                for p in &d.params {
                    let ty = p.ty.clone();
                    self.bind(&p.name, ty);
                }
                for s in &mut d.spec {
                    self.expr(&mut s.expr);
                }
                self.expr(&mut d.body);
                self.scope.truncate(mark);
            }
            Item::Test(d) => self.expr(&mut d.body),
            Item::Law(d) => {
                let mark = self.scope.len();
                for b in &d.binders {
                    let ty = Some(b.ty.clone());
                    self.bind(&b.name, ty);
                }
                if let Some(g) = &mut d.guard {
                    self.expr(g);
                }
                self.expr(&mut d.body);
                self.scope.truncate(mark);
            }
            Item::Type(_) | Item::Effect(_) | Item::Derive(_) | Item::EffectSet(_) => {}
        }
    }

    /// Children first, then this node: an update nested in another's field value
    /// is expanded before the outer one copies it.
    fn expr(&mut self, e: &mut Expr) {
        grow(|| {
            match &mut e.kind {
                ExprKind::Lit(_) | ExprKind::Var(_) => {}
                ExprKind::Binary { lhs, rhs, .. } => {
                    self.expr(lhs);
                    self.expr(rhs);
                }
                ExprKind::Unary { operand, .. } => self.expr(operand),
                ExprKind::Lambda { params, body } => {
                    let mark = self.scope.len();
                    for p in params.iter() {
                        let ty = p.ty.clone();
                        self.bind(&p.name, ty);
                    }
                    self.expr(body);
                    self.scope.truncate(mark);
                }
                ExprKind::App { func, args, named } => {
                    self.expr(func);
                    args.iter_mut().for_each(|a| self.expr(a));
                    named.iter_mut().for_each(|n| self.expr(&mut n.value));
                }
                ExprKind::If {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    self.expr(cond);
                    self.expr(then_branch);
                    self.expr(else_branch);
                }
                ExprKind::Match { scrutinee, arms } => {
                    self.expr(scrutinee);
                    for arm in arms {
                        let mark = self.scope.len();
                        self.bind_pattern(&arm.pat);
                        if let Some(g) = &mut arm.guard {
                            self.expr(g);
                        }
                        self.expr(&mut arm.body);
                        self.scope.truncate(mark);
                    }
                }
                ExprKind::Block { stmts, tail } => {
                    let mark = self.scope.len();
                    for stmt in stmts {
                        match stmt {
                            Stmt::Let { pat, ty, value, .. } => {
                                // The value is elaborated before the binder
                                // exists: `let s = {..s, a: 1}` updates the
                                // outer `s`.
                                self.expr(value);
                                match (&pat.kind, ty) {
                                    (PatternKind::Var(n), Some(t)) => {
                                        let t = t.clone();
                                        self.bind(n, Some(t));
                                    }
                                    _ => self.bind_pattern(pat),
                                }
                            }
                            Stmt::Expr(e) => self.expr(e),
                        }
                    }
                    if let Some(t) = tail {
                        self.expr(t);
                    }
                    self.scope.truncate(mark);
                }
                ExprKind::Record { fields } => fields.iter_mut().for_each(|(_, v)| self.expr(v)),
                ExprKind::RecordUpdate { base, fields } => {
                    self.expr(base);
                    fields.iter_mut().for_each(|(_, v)| self.expr(v));
                }
                ExprKind::Field { base, .. } => self.expr(base),
                // `try_op::expand` runs *after* this pass, so the sugar is
                // still here and this walk goes through it. The reason once
                // given for that order — that a `?` expanded first would have
                // turned `let x: T = e?;` into an untyped `Ok(x)` arm binder —
                // is withdrawn in ADR 0028 Decision 1: that shape is refused,
                // not expanded. This arm is what makes the order free, so it
                // stays whichever way the two are sequenced.
                ExprKind::Try { operand } => self.expr(operand),
                ExprKind::List { items } => items.iter_mut().for_each(|i| self.expr(i)),
                ExprKind::Perform { args, .. } => args.iter_mut().for_each(|a| self.expr(a)),
                ExprKind::Handle {
                    body,
                    clauses,
                    return_clause,
                } => {
                    self.expr(body);
                    for c in clauses {
                        let mark = self.scope.len();
                        for p in c.params.clone() {
                            self.bind(&p, None);
                        }
                        if let Some(k) = c.resume.clone() {
                            self.bind(&k, None);
                        }
                        self.expr(&mut c.body);
                        self.scope.truncate(mark);
                    }
                    if let Some(r) = return_clause {
                        let mark = self.scope.len();
                        let b = r.binder.clone();
                        self.bind(&b, None);
                        self.expr(&mut r.body);
                        self.scope.truncate(mark);
                    }
                }
                ExprKind::WithCell {
                    init, binder, body, ..
                } => {
                    self.expr(init);
                    let mark = self.scope.len();
                    let b = binder.clone();
                    self.bind(&b, None);
                    self.expr(body);
                    self.scope.truncate(mark);
                }
                ExprKind::WithRegion { body, .. } => self.expr(body),
                ExprKind::Simulate { body } => self.expr(body),
            }

            if matches!(e.kind, ExprKind::RecordUpdate { .. }) {
                let taken = std::mem::replace(&mut e.kind, ExprKind::Record { fields: Vec::new() });
                let ExprKind::RecordUpdate { base, fields } = taken else {
                    unreachable!("just matched")
                };
                e.kind = self.expand_update(*base, fields);
            }
        })
    }
}

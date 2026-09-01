//! `?` expansion, which happens inside the parser.

use crate::ast::*;
use crate::effect_set::grow;
use indexmap::IndexMap;
use ply_span::{Diagnostic, Span, Symbol, codes};

/// How deep an alias chain may be followed before the pass gives up.
const MAX_ALIAS_DEPTH: u32 = 16;

/// Which pair of constructors the expansion names, read off the enclosing function's written return
/// type.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Result,
    Option,
}

impl Mode {
    /// The failure constructor, and whether it carries a payload.
    fn failure(self) -> (&'static str, bool) {
        match self {
            Mode::Result => ("Err", true),
            Mode::Option => ("None", false),
        }
    }

    fn success(self) -> &'static str {
        match self {
            Mode::Result => "Ok",
            Mode::Option => "Some",
        }
    }
}

/// Why a `?` has no meaning where it is written.
#[derive(Clone)]
enum Why {
    /// The enclosing `fn` wrote no `->`.
    NoReturnType,
    /// It wrote one, and its head is neither `Result` nor `Option`.
    NotResultOrOption(Symbol),
    /// A type parameter, a generic alias, or an inline type with no head constructor to read.
    Unreadable,
    /// A cross-module type or alias: `m::T`.
    CrossModule,
    /// An alias chain longer than [`MAX_ALIAS_DEPTH`].
    TooDeep,
    /// Not inside a `fn` at all: a `test`, a `law`, a `requires`/`ensures`, or a bare expression
    /// handed to [`crate::parse_expr`].
    NotInAFunction(&'static str),
    /// This module declares its own `Ok`/`Err`/`Some`/`None`, which would capture the expansion.
    Shadowed(Symbol),
    /// The same capture, reached the other way: `import m (Err)` binds `Err` unqualified, in
    /// [`crate::resolve::Namespace::Value`] alongside every other constructor, so the expansion
    /// would name *that* one.
    ShadowedByImport(Symbol),
}

impl Why {
    fn note(&self) -> String {
        match self {
            Why::NoReturnType => "this function writes no `->`, and `?` reads which constructors \
                                  to name off the written return type; write one"
                .to_string(),
            Why::NotResultOrOption(head) => format!(
                "this function returns `{head}`; `?` binds a `Result` inside a function \
                 returning `Result`, or an `Option` inside one returning `Option`, because \
                 those are the constructors it expands to"
            ),
            Why::Unreadable => "the return type has no head constructor this file can read: a \
                                type parameter and a generic alias have none until they are \
                                applied"
                .to_string(),
            Why::CrossModule => "the return type is named through another module, and `?` reads \
                                 only this file — a meaning read across a module boundary would \
                                 go stale in a file that never changed"
                .to_string(),
            Why::TooDeep => format!("more than {MAX_ALIAS_DEPTH} type aliases deep"),
            Why::NotInAFunction(what) => {
                format!("{what} has no written return type, and `?` needs one")
            }
            Why::Shadowed(name) => format!(
                "this module declares its own `{name}`, which would capture the `match` `?` \
                 expands to; rename it, or write the `match` out"
            ),
            Why::ShadowedByImport(name) => format!(
                "this module imports `{name}` unqualified, which would capture the `match` `?` \
                 expands to; import the module instead and qualify the name, or write the \
                 `match` out"
            ),
        }
    }
}

/// Rewrites every `?` in the module.
pub(crate) fn expand(module: &mut Module, diags: &mut Vec<Diagnostic>) {
    let types = collect_types(module);
    let shadowed = shadowing_ctor(module);
    let mut items = std::mem::take(&mut module.items);
    let mut cx = Cx {
        types: &types,
        shadowed,
        diags,
        mode: None,
        fresh: 0,
    };
    for item in &mut items {
        cx.item(item);
    }
    module.items = items;
}

/// The same pass for an expression parsed with no module around it ([`crate::parse_expr`]).
pub(crate) fn expand_bare(e: &mut Expr, diags: &mut Vec<Diagnostic>) {
    let types = IndexMap::new();
    let mut cx = Cx {
        types: &types,
        shadowed: None,
        diags,
        mode: None,
        fresh: 0,
    };
    cx.sweep(e, Some(&Why::NotInAFunction("a bare expression")), None);
}

/// This module's own `type` items, by simple name.
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

/// A binding in this module whose name the expansion would emit.
fn shadowing_ctor(module: &Module) -> Option<Why> {
    for import in &module.imports {
        if let ImportKind::Names(names) = &import.kind {
            for n in names {
                if is_expansion_name(&n.name) {
                    return Some(Why::ShadowedByImport(n.name.clone()));
                }
            }
        }
    }
    for item in &module.items {
        if let Item::Type(d) = item
            && let TypeDefBody::Sum(variants) = &d.body
        {
            for v in variants {
                if is_expansion_name(&v.name.name) {
                    return Some(Why::Shadowed(v.name.name.clone()));
                }
            }
        }
    }
    None
}

/// The four names [`Cx::wrap`] can emit.
fn is_expansion_name(name: &Symbol) -> bool {
    matches!(name.as_str(), "Ok" | "Err" | "Some" | "None")
}

struct Cx<'a> {
    types: &'a IndexMap<Symbol, TypeDef>,
    shadowed: Option<Why>,
    diags: &'a mut Vec<Diagnostic>,
    /// The mode of the `fn` being walked.
    mode: Option<Mode>,
    /// Counts the binders this pass has synthesized in the current item.
    fresh: u32,
}

/// A `?` the scan reached, lifted out of the expression it was written in.
struct Lift {
    operand: Expr,
    binder: Ident,
    at: Span,
}

/// What one step of the evaluation-order scan found.
enum Scan {
    /// A `?` was reached unconditionally with a pure prefix, and has been replaced in place by a
    /// reference to its binder.
    Found(Box<Lift>),
    /// Nothing found, and everything scanned is pure — the scan may continue to the right.
    Pure,
    /// Nothing found, and something impure was passed.
    Impure,
}

impl Cx<'_> {
    fn item(&mut self, item: &mut Item) {
        self.fresh = 0;
        self.mode = None;
        match item {
            Item::Fn(d) => {
                // A spec is a claim *about* the definition rather than part of it — normalization
                // erases it — and it has no return type of its own.
                for s in &mut d.spec {
                    let what = match s.kind {
                        SpecKind::Requires => "a `requires` clause",
                        SpecKind::Ensures => "an `ensures` clause",
                    };
                    self.sweep(&mut s.expr, Some(&Why::NotInAFunction(what)), None);
                }
                let mode = match (&self.shadowed, &d.ret) {
                    (Some(why), _) => Err(why.clone()),
                    (None, None) => Err(Why::NoReturnType),
                    (None, Some(ty)) => self.mode_of(ty, 0),
                };
                match mode {
                    Err(why) => self.sweep(&mut d.body, Some(&why), None),
                    Ok(m) => {
                        self.mode = Some(m);
                        self.ret(&mut d.body);
                        // Stage 3.
                        self.sweep(&mut d.body, None, None);
                        self.mode = None;
                    }
                }
            }
            Item::Test(d) => self.sweep(&mut d.body, Some(&Why::NotInAFunction("a `test`")), None),
            Item::Law(d) => {
                let why = Why::NotInAFunction("a `law`");
                if let Some(g) = &mut d.guard {
                    self.sweep(g, Some(&why), None);
                }
                self.sweep(&mut d.body, Some(&why), None);
            }
            Item::Type(_) | Item::Effect(_) | Item::Derive(_) | Item::EffectSet(_) => {}
        }
    }

    /// The head constructor of a written return type, following this module's own aliases.
    fn mode_of(&self, ty: &TypeExpr, depth: u32) -> Result<Mode, Why> {
        if depth > MAX_ALIAS_DEPTH {
            return Err(Why::TooDeep);
        }
        match ty {
            TypeExpr::Con { name, .. } if !name.is_bare() => Err(Why::CrossModule),
            TypeExpr::Con { name, .. } => {
                let sym = &name.name.name;
                match sym.as_str() {
                    // The prelude's — and a module that declared its own was refused before this
                    // ran.
                    "Result" => return Ok(Mode::Result),
                    "Option" => return Ok(Mode::Option),
                    _ => {}
                }
                match self.types.get(sym) {
                    // Not declared here and not the prelude's: a builtin, or an imported type.
                    None => Err(Why::NotResultOrOption(sym.clone())),
                    Some(d) if !d.params.is_empty() => Err(Why::Unreadable),
                    Some(d) => match &d.body {
                        TypeDefBody::Sum(_) => Err(Why::NotResultOrOption(sym.clone())),
                        TypeDefBody::Alias(t) => self.mode_of(t, depth + 1),
                    },
                }
            }
            // A lowercase bare name is a type parameter bound by an enclosing `<..>`, never a
            // declared type.
            TypeExpr::Var(_) => Err(Why::Unreadable),
            TypeExpr::Record { .. } | TypeExpr::Fn { .. } | TypeExpr::Unit { .. } => {
                Err(Why::Unreadable)
            }
        }
    }

    /// `e` is in **return position**: its value is the enclosing function's.
    fn ret(&mut self, e: &mut Expr) {
        grow(|| match &e.kind {
            ExprKind::Block { .. } => self.block(e),
            // A branch is a return position of its own, so it is walked first; the condition is
            // what the `if` node itself is the region root for.
            ExprKind::If { .. } => {
                let ExprKind::If {
                    then_branch,
                    else_branch,
                    ..
                } = &mut e.kind
                else {
                    unreachable!("just matched")
                };
                self.ret(then_branch);
                self.ret(else_branch);
                self.region(e, &mut |cx, e| match &mut e.kind {
                    ExprKind::If { cond, .. } => cx.scan(cond),
                    _ => unreachable!("the lift puts the `if` back in the success arm"),
                });
            }
            ExprKind::Match { .. } => {
                let ExprKind::Match { arms, .. } = &mut e.kind else {
                    unreachable!("just matched")
                };
                // A guard runs only when the arms above it did not match, so it is conditional and
                // can never be a region root.
                for arm in arms {
                    self.ret(&mut arm.body);
                }
                self.region(e, &mut |cx, e| match &mut e.kind {
                    ExprKind::Match { scrutinee, .. } => cx.scan(scrutinee),
                    _ => unreachable!("the lift puts the `match` back in the success arm"),
                });
            }
            _ => self.region(e, &mut |cx, e| cx.scan(e)),
        })
    }

    /// Lifts every `?` that `find` can reach out of `e`, wrapping `e` in one `match` per lift and
    /// leaving the rewritten `e` as the success arm's body.
    fn region(&mut self, e: &mut Expr, find: &mut dyn FnMut(&mut Cx, &mut Expr) -> Scan) {
        grow(|| {
            let Scan::Found(lift) = find(self, e) else {
                return;
            };
            let Lift {
                operand,
                binder,
                at,
            } = *lift;
            let span = e.span;
            let taken = std::mem::replace(
                e,
                Expr {
                    kind: ExprKind::Lit(Lit::Unit),
                    span,
                },
            );
            let pat = Pattern {
                span: binder.span,
                kind: PatternKind::Var(binder),
            };
            *e = self.wrap(operand, at, pat, taken);
            let ExprKind::Match { arms, .. } = &mut e.kind else {
                unreachable!("`wrap` builds a match")
            };
            self.region(&mut arms[1].body, find);
        })
    }

    /// A block in return position.
    fn block(&mut self, e: &mut Expr) {
        let ExprKind::Block { stmts, tail } = &mut e.kind else {
            unreachable!("only called on a block")
        };
        for i in 0..stmts.len() {
            if let Some(split) = self.split_at(stmts, tail, i) {
                *tail = Some(Box::new(split));
                return;
            }
        }
        if let Some(t) = tail {
            self.ret(t);
        }
    }

    /// Tries to split the block at statement `i`.
    fn split_at(
        &mut self,
        stmts: &mut Vec<Stmt>,
        tail: &mut Option<Box<Expr>>,
        i: usize,
    ) -> Option<Expr> {
        // `consumes` says whether the statement itself is swallowed by the split, which is the case
        // exactly when the `?` was the whole of it and its binder moved onto the success arm.
        let (operand, at, pat, consumes) = match &mut stmts[i] {
            // `let x: T = e?;` has no `let` left to carry `T` on after the split, and a written
            // annotation must not evaporate.
            Stmt::Let {
                ty: Some(_),
                value,
                span,
                ..
            } if matches!(value.kind, ExprKind::Try { .. }) => {
                let span = *span;
                self.annotated_let(value, span);
                return None;
            }
            // The shape every conversion in the corpus takes.
            Stmt::Let {
                pat,
                ty: None,
                value,
                ..
            } if matches!(value.kind, ExprKind::Try { .. }) => {
                let pat = pat.clone();
                self.take_try_stmt(value, pat)?
            }
            Stmt::Expr(v) if matches!(v.kind, ExprKind::Try { .. }) => {
                let pat = Pattern {
                    span: v.span,
                    kind: PatternKind::Wildcard,
                };
                self.take_try_stmt(v, pat)?
            }
            Stmt::Let { value, .. } => self.take_scanned(value)?,
            Stmt::Expr(v) => self.take_scanned(v)?,
        };

        let rest: Vec<Stmt> = stmts.drain(i + usize::from(consumes)..).collect();
        stmts.truncate(i);
        let mut body = Expr {
            span: at,
            kind: ExprKind::Block {
                stmts: rest,
                tail: tail.take(),
            },
        };
        // The success arm's body is a block in return position, so it is walked as one and the next
        // `?` in the run splits it again.
        self.block(&mut body);
        Some(self.wrap(operand, at, pat, body))
    }

    /// A statement whose whole value is a `?`
    fn take_try_stmt(
        &mut self,
        value: &mut Expr,
        pat: Pattern,
    ) -> Option<(Expr, Span, Pattern, bool)> {
        let ExprKind::Try { operand } = &mut value.kind else {
            unreachable!("only called on a `?`")
        };
        if let Scan::Found(lift) = self.scan(operand) {
            let Lift {
                operand,
                binder,
                at,
            } = *lift;
            return Some((operand, at, var_pattern(binder), false));
        }
        let span = value.span;
        let taken = std::mem::replace(
            operand.as_mut(),
            Expr {
                kind: ExprKind::Lit(Lit::Unit),
                span,
            },
        );
        Some((taken, span, pat, true))
    }

    fn take_scanned(&mut self, value: &mut Expr) -> Option<(Expr, Span, Pattern, bool)> {
        match self.scan(value) {
            Scan::Found(lift) => {
                let Lift {
                    operand,
                    binder,
                    at,
                } = *lift;
                Some((operand, at, var_pattern(binder), false))
            }
            _ => None,
        }
    }

    fn annotated_let(&mut self, value: &mut Expr, stmt: Span) {
        self.diags.push(
            Diagnostic::error(codes::TRY_POSITION, "a `?` on a `let` with a written type")
                .primary(value.span, "this `?` would discard the annotation")
                .secondary(stmt, "the type is written here")
                .note(
                    "`?` splits the block at this statement and the `let` is gone, so there is \
                     nothing left to carry the annotation. Write `let x = e?;`, or annotate the \
                     value being unwrapped instead",
                ),
        );
        unwrap_try(value);
    }

    /// Walks `e` in evaluation order looking for the first `?` that may be lifted to the head of
    /// the region.
    fn scan(&mut self, e: &mut Expr) -> Scan {
        grow(|| self.scan_inner(e))
    }

    fn scan_inner(&mut self, e: &mut Expr) -> Scan {
        match &mut e.kind {
            ExprKind::Lit(_) | ExprKind::Var(_) => Scan::Pure,

            ExprKind::Try { operand } => {
                // The operand is evaluated before the `?` acts, so an inner `?` is lifted first.
                if let found @ Scan::Found(_) = self.scan(operand) {
                    return found;
                }
                // The operand's *own* impurity is no reason to refuse: it is the thing being
                // unwrapped, not something evaluated before it.
                let binder = self.fresh_binder(e.span);
                let span = e.span;
                let operand = std::mem::replace(operand.as_mut(), Expr {
                    kind: ExprKind::Lit(Lit::Unit),
                    span,
                });
                e.kind = ExprKind::Var(QName::bare(binder.clone()));
                Scan::Found(Box::new(Lift {
                    operand,
                    binder,
                    at: span,
                }))
            }

            ExprKind::Binary { op, lhs, rhs } => {
                let short_circuits = matches!(op, BinOp::And | BinOp::Or);
                match self.scan(lhs) {
                    found @ Scan::Found(_) => found,
                    Scan::Impure => Scan::Impure,
                    Scan::Pure if short_circuits => purity(is_pure(rhs)),
                    Scan::Pure => self.scan(rhs),
                }
            }
            ExprKind::Unary { operand, .. } => self.scan(operand),

            ExprKind::App { func, args, named } => {
                // Named arguments scan after the positional ones because that is the order they
                // were written in; `defaults::expand` has not run yet and cannot, so this is the
                // only order there is.
                match self.sequence(
                    std::iter::once(func.as_mut())
                        .chain(args.iter_mut())
                        .chain(named.iter_mut().map(|n| &mut n.value)),
                ) {
                    found @ Scan::Found(_) => found,
                    // The call itself is impure, whatever its parts were.
                    _ => Scan::Impure,
                }
            }
            ExprKind::Perform { args, .. } => match self.sequence(args.iter_mut()) {
                found @ Scan::Found(_) => found,
                _ => Scan::Impure,
            },
            ExprKind::Record { fields } => self.sequence(fields.iter_mut().map(|(_, v)| v)),
            ExprKind::List { items } => self.sequence(items.iter_mut()),
            ExprKind::Field { base, .. } => self.scan(base),

            // Unreachable inside `parse_module`, where `record_update::expand` has already run —
            // walked rather than `unreachable!`ed because a conservative walk cannot panic and
            // cannot be wrong: the base is a path, and the written values are evaluated left to
            // right.
            ExprKind::RecordUpdate { base, fields } => self.sequence(
                std::iter::once(base.as_mut()).chain(fields.iter_mut().map(|(_, v)| v)),
            ),

            // Conditional from here down.
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => match self.scan(cond) {
                found @ Scan::Found(_) => found,
                Scan::Impure => Scan::Impure,
                Scan::Pure => purity(is_pure(then_branch) && is_pure(else_branch)),
            },
            ExprKind::Match { scrutinee, arms } => match self.scan(scrutinee) {
                found @ Scan::Found(_) => found,
                Scan::Impure => Scan::Impure,
                Scan::Pure => purity(
                    arms.iter()
                        .all(|a| is_pure(&a.body) && a.guard.as_ref().is_none_or(is_pure)),
                ),
            },

            // A nested block is not entered: lifting a `?` out of one would take it out of the
            // scope of the block's own binders, so `{ let a = f(); a? }` would stop meaning what it
            // says.
            ExprKind::Block { .. }
            // A lambda body is not evaluated here at all, and a handler, a cell, a region and a
            // `simulate` are barriers a `?` may not cross.
            | ExprKind::Lambda { .. }
            | ExprKind::Handle { .. }
            | ExprKind::WithCell { .. }
            | ExprKind::WithRegion { .. }
            | ExprKind::Simulate { .. } => purity(is_pure(e)),
        }
    }

    /// Sub-expressions left to right, stopping at the first `?` and at the first impure one.
    fn sequence<'e>(&mut self, parts: impl Iterator<Item = &'e mut Expr>) -> Scan {
        let mut acc = Scan::Pure;
        for p in parts {
            match acc {
                Scan::Pure => acc = self.scan(p),
                _ => break,
            }
        }
        acc
    }

    /// Refuses every `?` left in `e` and unwraps it, so that no [`ExprKind::Try`] escapes the
    /// parser.
    fn sweep(&mut self, e: &mut Expr, scope: Option<&Why>, barrier: Option<&'static str>) {
        grow(|| {
            if matches!(e.kind, ExprKind::Try { .. }) {
                match (barrier, scope) {
                    (Some(what), _) => self.barrier_refusal(e.span, what),
                    (None, Some(why)) => self.scope_refusal(e.span, why),
                    (None, None) => self.position_refusal(e.span),
                }
                unwrap_try(e);
            }
            let under = |b: &'static str| barrier.or(Some(b));
            match &mut e.kind {
                ExprKind::Lit(_) | ExprKind::Var(_) => {}
                ExprKind::Try { operand } => self.sweep(operand, scope, barrier),
                ExprKind::Binary { lhs, rhs, .. } => {
                    self.sweep(lhs, scope, barrier);
                    self.sweep(rhs, scope, barrier);
                }
                ExprKind::Unary { operand, .. } => self.sweep(operand, scope, barrier),
                ExprKind::Lambda { body, .. } => self.sweep(body, scope, under("a lambda")),
                ExprKind::App { func, args, named } => {
                    self.sweep(func, scope, barrier);
                    for a in args {
                        self.sweep(a, scope, barrier);
                    }
                    for n in named {
                        self.sweep(&mut n.value, scope, barrier);
                    }
                }
                ExprKind::If {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    self.sweep(cond, scope, barrier);
                    self.sweep(then_branch, scope, barrier);
                    self.sweep(else_branch, scope, barrier);
                }
                ExprKind::Match { scrutinee, arms } => {
                    self.sweep(scrutinee, scope, barrier);
                    for arm in arms {
                        if let Some(g) = &mut arm.guard {
                            self.sweep(g, scope, barrier);
                        }
                        self.sweep(&mut arm.body, scope, barrier);
                    }
                }
                ExprKind::Block { stmts, tail } => {
                    for s in stmts {
                        match s {
                            Stmt::Let { value, .. } => self.sweep(value, scope, barrier),
                            Stmt::Expr(e) => self.sweep(e, scope, barrier),
                        }
                    }
                    if let Some(t) = tail {
                        self.sweep(t, scope, barrier);
                    }
                }
                ExprKind::Record { fields } => {
                    for (_, v) in fields {
                        self.sweep(v, scope, barrier);
                    }
                }
                ExprKind::RecordUpdate { base, fields } => {
                    self.sweep(base, scope, barrier);
                    for (_, v) in fields {
                        self.sweep(v, scope, barrier);
                    }
                }
                ExprKind::Field { base, .. } => self.sweep(base, scope, barrier),
                ExprKind::List { items } => {
                    for i in items {
                        self.sweep(i, scope, barrier);
                    }
                }
                ExprKind::Perform { args, .. } => {
                    for a in args {
                        self.sweep(a, scope, barrier);
                    }
                }
                ExprKind::Handle {
                    body,
                    clauses,
                    return_clause,
                } => {
                    self.sweep(body, scope, under("a `handle` body"));
                    for c in clauses {
                        self.sweep(&mut c.body, scope, under("a `handle` clause"));
                    }
                    if let Some(r) = return_clause {
                        self.sweep(&mut r.body, scope, under("a `handle` return clause"));
                    }
                }
                ExprKind::WithCell { init, body, .. } => {
                    self.sweep(init, scope, barrier);
                    self.sweep(body, scope, under("a `with_cell` body"));
                }
                ExprKind::WithRegion { body, .. } => {
                    self.sweep(body, scope, under("a `with_region` body"))
                }
                ExprKind::Simulate { body } => self.sweep(body, scope, under("a `simulate` body")),
            }
        })
    }

    fn scope_refusal(&mut self, span: Span, why: &Why) {
        self.diags.push(
            Diagnostic::error(codes::TRY_SCOPE, "this file gives `?` no meaning here")
                .primary(span, "no `Result` or `Option` to expand this into")
                .note(why.note()),
        );
    }

    fn barrier_refusal(&mut self, span: Span, what: &'static str) {
        self.diags.push(
            Diagnostic::error(codes::TRY_SCOPE, "this file gives `?` no meaning here")
                .primary(span, "no `Result` or `Option` to expand this into")
                .note(format!(
                    "this `?` is inside {what}, which has no written return type of its own. `?` \
                     exits the expression it is written in and never a lambda, a handler or a \
                     region: name a function with a written `->` and call it"
                )),
        );
    }

    fn position_refusal(&mut self, span: Span) {
        self.diags.push(
            Diagnostic::error(
                codes::TRY_POSITION,
                "this `?` is not in a position it can be expanded from",
            )
            .primary(span, "`?` cannot exit from here")
            .note(
                "`?` lifts what it unwraps to the head of the statement, or of the return \
                 position, it is written in — so nothing conditional may sit between the two, \
                 and everything evaluated before it must be pure. Bind it first: `let a = e?;`",
            ),
        );
    }

    /// `match operand { <failure>, <success>(pat) -> body }`, **failure arm first** — the order the
    /// corpus writes 129 times to 3, and the reason a converted site keeps its hash.
    fn wrap(&mut self, operand: Expr, at: Span, pat: Pattern, body: Expr) -> Expr {
        let mode = self.mode.expect("a mode is established before any lift");
        let span = at;
        let (fail, carries) = mode.failure();
        let (fail_pat, fail_body) = match carries {
            true => {
                let binder = self.fresh_binder(span);
                (
                    ctor_pattern(fail, vec![var_pattern(binder.clone())], span),
                    Expr {
                        span,
                        kind: ExprKind::App {
                            func: Box::new(var(Ident::new(fail, span))),
                            args: vec![var(binder)],
                            named: Vec::new(),
                        },
                    },
                )
            }
            false => (
                ctor_pattern(fail, Vec::new(), span),
                var(Ident::new(fail, span)),
            ),
        };
        Expr {
            span,
            kind: ExprKind::Match {
                scrutinee: Box::new(operand),
                arms: vec![
                    MatchArm {
                        pat: fail_pat,
                        guard: None,
                        body: fail_body,
                        span,
                    },
                    MatchArm {
                        pat: ctor_pattern(mode.success(), vec![pat], span),
                        guard: None,
                        body,
                        span,
                    },
                ],
            },
        }
    }

    /// A binder no source can collide with.
    fn fresh_binder(&mut self, span: Span) -> Ident {
        let name = Symbol::new(format!("?{}", self.fresh));
        self.fresh += 1;
        Ident::new(name, span)
    }
}

fn purity(pure: bool) -> Scan {
    match pure {
        true => Scan::Pure,
        false => Scan::Impure,
    }
}

fn var(name: Ident) -> Expr {
    let span = name.span;
    Expr {
        span,
        kind: ExprKind::Var(QName::bare(name)),
    }
}

fn var_pattern(name: Ident) -> Pattern {
    Pattern {
        span: name.span,
        kind: PatternKind::Var(name),
    }
}

fn ctor_pattern(name: &'static str, args: Vec<Pattern>, span: Span) -> Pattern {
    Pattern {
        span,
        kind: PatternKind::Ctor {
            name: QName::bare(Ident::new(name, span)),
            args,
        },
    }
}

/// `e?` becomes `e`.
fn unwrap_try(e: &mut Expr) {
    let ExprKind::Try { operand } = &mut e.kind else {
        return;
    };
    let span = e.span;
    let inner = std::mem::replace(
        operand.as_mut(),
        Expr {
            kind: ExprKind::Lit(Lit::Unit),
            span,
        },
    );
    *e = inner;
}

//! Filling a call's unwritten arguments from the callee's signature.
//!
//! `f(x)` where `f`'s second parameter has a default becomes `f(x, <default>)`,
//! and `f(x, m: 1)` becomes `f(x, 1)`. After this pass every [`ExprKind::App`]
//! is fully positional and fully applied, which is what lets `ply-hash`,
//! `ply-core`, `ply-eval` and `ply-prove` carry no notion of a default at all —
//! ADR 0023's rule, that a construct with four implementations has four chances
//! to disagree.
//!
//! **Why this runs in `resolve` and not in the parser**, where
//! [`crate::record_update`] and [`crate::try_op`] run. Those read a shape out of
//! the module in front of them. A default lives in the *callee's* module, so
//! matching a call against a signature needs the whole program — which the
//! parser, running one file at a time, does not have. `resolve` is the first
//! point that does, and it is still before the driver hashes (ADR 0002: parse,
//! resolve, hash, gate 2, infer), which is the deadline that matters: `f(x)` and
//! `f(x, 1)` must reach normalization as the same bytes.
//!
//! It runs *inside* [`crate::resolve`] rather than beside it so that no entry
//! point can forget it, which is the guarantee `parse_module` gives the other
//! two passes. `no_named_argument_survives_resolve_anywhere_in_the_tree` in
//! [`crate::tests`] is the check.
//!
//! **Why crossing a module boundary is safe here**, where ADR 0023 §"Decision 4"
//! refused it for record update. That refusal rested on gate 1 skipping a file
//! whose bytes are unchanged, leaving a stale expansion behind. Gate 1's second
//! condition walks the file's *free names* and refuses any whose referent's hash
//! moved (`ply_cli::driver`). A default is part of the callee's `DefHash` and a
//! spliced default is a reference the caller now makes, so both halves are in
//! `hashes.deps` and an edited default re-parses every caller. Record update had
//! no such edge to ride: a record's field list is not a reference.
//!
//! A **builtin** is the one case with no hash to move — it normalizes as its own
//! text — so changing a builtin default is a `RUNTIME_VERSION` bump, exactly as
//! adding a builtin already is.

use crate::ast::{
    Expr, ExprKind, Ident, Item, NamedArg, Pattern, PatternKind, Program, Stmt, is_ctor_name,
};
use crate::effect_set::grow;
use crate::resolve::{Namespace, Resolved};
use indexmap::IndexMap;
use ply_span::{Diagnostic, Span, Symbol, codes};

/// One parameter of a callee this pass can match a call against.
#[derive(Clone)]
struct ParamInfo {
    name: Symbol,
    /// Already qualified against the module that wrote it, so splicing it into
    /// any caller means what it meant where it was written.
    default: Option<Expr>,
}

/// A callee's parameters, plus the module that wrote them — needed because the
/// implicit imports a spliced default requires are the *writer's* modules.
#[derive(Clone)]
struct Signature {
    params: Vec<ParamInfo>,
}

/// The builtins that carry a default.
///
/// The third table over the builtins, after the arities in
/// `ply_eval::builtins` and the schemes in `ply_core::infer`. It is here rather
/// than beside either because this crate depends on neither, and
/// `ply_eval::tests::every_builtin_agrees_on_its_arity_everywhere` is what keeps
/// the three from drifting.
///
/// `range` is deliberately absent. Its natural default is on the *leading*
/// parameter, so `range(5)` would fill `lo` and leave `hi` empty; the spelling
/// that works, `range(hi: 5)`, is longer than `range(0, 5)`. A default that
/// makes every call site worse is not one worth having.
fn builtin_signature(name: &Symbol) -> Option<Signature> {
    let span = Span::DUMMY;
    match name.as_str() {
        "assert" => Some(Signature {
            params: vec![
                ParamInfo {
                    name: Symbol::new("cond"),
                    default: None,
                },
                ParamInfo {
                    name: Symbol::new("message"),
                    // A prelude constructor, which is in every module's reach
                    // without an import — so this one needs no qualifying.
                    default: Some(Expr {
                        kind: ExprKind::Var(crate::ast::QName::bare(Ident::new("None", span))),
                        span,
                    }),
                },
            ],
        }),
        _ => None,
    }
}

pub(crate) fn expand(program: &mut Program, resolved: &mut Resolved, diags: &mut Vec<Diagnostic>) {
    let signatures = collect(program, resolved, diags);
    let mut items = std::mem::take(&mut program.modules)
        .into_iter()
        .enumerate()
        .collect::<Vec<_>>();
    for (m, module) in &mut items {
        let mut cx = Cx {
            signatures: &signatures,
            resolved,
            module: *m,
            scope: Vec::new(),
            implicit: Vec::new(),
            diags,
        };
        for item in &mut module.items {
            cx.item(item);
        }
        let implicit = std::mem::take(&mut cx.implicit);
        for (binder, owner) in implicit {
            if let Some(scope) = resolved.scopes.get_mut(*m) {
                scope.modules.entry(binder).or_insert((owner, Span::DUMMY));
            }
        }
    }
    program.modules = items.into_iter().map(|(_, module)| module).collect();
}

/// Every `fn` in the program by its program-wide name, with each default already
/// qualified against the module that wrote it.
fn collect(
    program: &Program,
    resolved: &Resolved,
    diags: &mut Vec<Diagnostic>,
) -> IndexMap<Symbol, Signature> {
    let mut out = IndexMap::new();
    for (m, module) in program.modules.iter().enumerate() {
        for item in &module.items {
            let Item::Fn(def) = item else { continue };
            let params = def
                .params
                .iter()
                .map(|p| ParamInfo {
                    name: p.name.name.clone(),
                    default: p
                        .default
                        .as_ref()
                        .map(|d| qualify(d, m, resolved, def.vis.is_public(), diags)),
                })
                .collect();
            out.insert(module.name.qualify(&def.name.name), Signature { params });
        }
    }
    out
}

/// Rewrites a default's free names to the form they keep in any caller.
///
/// A name bound inside the default stays bare. A name the writing module can see
/// becomes `<that module>::<name>`, with the module's own dotted name as the
/// binder — a binder no user can write, because a written one is a single
/// identifier and contains no `.`, so this can never capture or be captured.
///
/// A name neither of those is left alone: it is a builtin or a prelude
/// constructor, and those are in every module's reach already.
fn qualify(
    e: &Expr,
    owner: usize,
    resolved: &Resolved,
    public: bool,
    diags: &mut Vec<Diagnostic>,
) -> Expr {
    let mut q = Qualify {
        owner,
        resolved,
        public,
        bound: Vec::new(),
        diags,
    };
    let mut out = e.clone();
    q.expr(&mut out);
    out
}

struct Qualify<'a> {
    owner: usize,
    resolved: &'a Resolved,
    public: bool,
    bound: Vec<Symbol>,
    diags: &'a mut Vec<Diagnostic>,
}

impl Qualify<'_> {
    fn expr(&mut self, e: &mut Expr) {
        grow(|| match &mut e.kind {
            ExprKind::Var(q) => {
                if !q.is_bare() || self.bound.contains(q.symbol()) {
                    return;
                }
                let Ok(binding) = self
                    .resolved
                    .lookup(self.owner, Namespace::Value, &q.clone())
                else {
                    // A builtin or a prelude constructor: in reach everywhere,
                    // so it needs no qualifying and gets none.
                    return;
                };
                let owner = binding.owner;
                if owner == self.owner && self.public && !self.is_public(owner, q.symbol()) {
                    self.diags.push(
                        Diagnostic::error(
                            codes::DEFAULT_PRIVATE_NAME,
                            format!("a default on a `pub fn` cannot mention `{}`", q.symbol()),
                        )
                        .primary(q.span, "not exported by this module")
                        .note(
                            "the default is copied into every call site that omits the \
                             argument, and a caller in another module may not name this",
                        ),
                    );
                    return;
                }
                let Some(module) = self
                    .resolved
                    .scopes
                    .get(owner)
                    .map(|s| s.module.as_symbol())
                else {
                    return;
                };
                q.module = Some(Ident::new(module.clone(), q.span));
            }
            ExprKind::Lambda { params, body } => {
                let mark = self.bound.len();
                for p in params.iter() {
                    self.bound.push(p.name.name.clone());
                }
                self.expr(body);
                self.bound.truncate(mark);
            }
            ExprKind::App { func, args, named } => {
                self.expr(func);
                args.iter_mut().for_each(|a| self.expr(a));
                named.iter_mut().for_each(|n| self.expr(&mut n.value));
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    let mark = self.bound.len();
                    self.bind_pattern(&arm.pat);
                    if let Some(g) = &mut arm.guard {
                        self.expr(g);
                    }
                    self.expr(&mut arm.body);
                    self.bound.truncate(mark);
                }
            }
            ExprKind::Block { stmts, tail } => {
                let mark = self.bound.len();
                for stmt in stmts {
                    match stmt {
                        Stmt::Let { pat, value, .. } => {
                            self.expr(value);
                            self.bind_pattern(pat);
                        }
                        Stmt::Expr(e) => self.expr(e),
                    }
                }
                if let Some(t) = tail {
                    self.expr(t);
                }
                self.bound.truncate(mark);
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Unary { operand, .. } => self.expr(operand),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(cond);
                self.expr(then_branch);
                self.expr(else_branch);
            }
            ExprKind::Record { fields } => fields.iter_mut().for_each(|(_, v)| self.expr(v)),
            ExprKind::RecordUpdate { base, fields } => {
                self.expr(base);
                fields.iter_mut().for_each(|(_, v)| self.expr(v));
            }
            ExprKind::Field { base, .. } => self.expr(base),
            ExprKind::List { items } => items.iter_mut().for_each(|i| self.expr(i)),
            ExprKind::Try { operand } => self.expr(operand),
            // Refused by `is_default_expr` before this runs, and reached only
            // when the checker has already reported that. Walking them anyway
            // keeps this from depending on the order of two diagnostics.
            ExprKind::Perform { args, .. } => args.iter_mut().for_each(|a| self.expr(a)),
            ExprKind::Handle { body, .. }
            | ExprKind::WithCell { body, .. }
            | ExprKind::WithRegion { body, .. }
            | ExprKind::Simulate { body } => self.expr(body),
            ExprKind::Lit(_) => {}
        })
    }

    fn is_public(&self, owner: usize, name: &Symbol) -> bool {
        self.resolved
            .declarations
            .get(owner)
            .and_then(|d| d.get(Namespace::Value, name))
            .is_some_and(|d| d.vis.is_public())
    }

    fn bind_pattern(&mut self, p: &Pattern) {
        grow(|| match &p.kind {
            PatternKind::Var(n) => self.bound.push(n.name.clone()),
            PatternKind::Ctor { args, .. } => args.iter().for_each(|a| self.bind_pattern(a)),
            PatternKind::Record { fields, .. } => {
                fields.iter().for_each(|(_, p)| self.bind_pattern(p))
            }
            PatternKind::List { items, rest } => {
                items.iter().for_each(|i| self.bind_pattern(i));
                if let Some(r) = rest {
                    self.bind_pattern(r);
                }
            }
            PatternKind::Lit(_) | PatternKind::Wildcard => {}
        })
    }
}

struct Cx<'a> {
    signatures: &'a IndexMap<Symbol, Signature>,
    resolved: &'a Resolved,
    module: usize,
    /// Local binders, innermost last. A name bound here is not the global one,
    /// so a call through it has no signature to match against.
    scope: Vec<Symbol>,
    /// Modules a spliced default made this one reference, to be added to its
    /// scope once the walk lets go of `resolved`.
    implicit: Vec<(Symbol, usize)>,
    diags: &'a mut Vec<Diagnostic>,
}

impl Cx<'_> {
    fn item(&mut self, item: &mut Item) {
        match item {
            Item::Fn(d) => {
                let mark = self.scope.len();
                for p in &d.params {
                    self.scope.push(p.name.name.clone());
                }
                // A default is not walked here: it was qualified against this
                // module in `collect`, and a call inside one is refused by
                // `is_default_expr` before it could need expanding.
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
                    self.scope.push(b.name.name.clone());
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

    /// Children first, then this node, so a call nested in another's argument is
    /// filled before the outer one copies it.
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
                        self.scope.push(p.name.name.clone());
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
                            Stmt::Let { pat, value, .. } => {
                                self.expr(value);
                                self.bind_pattern(pat);
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
                ExprKind::List { items } => items.iter_mut().for_each(|i| self.expr(i)),
                ExprKind::Try { operand } => self.expr(operand),
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
                            self.scope.push(p.name.clone());
                        }
                        if let Some(k) = c.resume.clone() {
                            self.scope.push(k.name.clone());
                        }
                        self.expr(&mut c.body);
                        self.scope.truncate(mark);
                    }
                    if let Some(r) = return_clause {
                        let mark = self.scope.len();
                        self.scope.push(r.binder.name.clone());
                        self.expr(&mut r.body);
                        self.scope.truncate(mark);
                    }
                }
                ExprKind::WithCell {
                    init, binder, body, ..
                } => {
                    self.expr(init);
                    let mark = self.scope.len();
                    self.scope.push(binder.name.clone());
                    self.expr(body);
                    self.scope.truncate(mark);
                }
                ExprKind::WithRegion { body, .. } => self.expr(body),
                ExprKind::Simulate { body } => self.expr(body),
            }

            if matches!(e.kind, ExprKind::App { .. }) {
                self.fill(e);
            }
        })
    }

    /// Matches one call against its callee's signature.
    fn fill(&mut self, e: &mut Expr) {
        let span = e.span;
        let ExprKind::App { func, args, named } = &mut e.kind else {
            unreachable!("just matched")
        };
        let Some(sig) = self.signature_of(func) else {
            // No signature in hand: a call through a value, a constructor, or a
            // name that does not resolve. The first two are exactly applied by
            // rule and the third is somebody else's diagnostic — but a name on
            // an argument had nothing to select, and that is this pass's to say.
            for n in named.iter() {
                self.diags.push(
                    Diagnostic::error(
                        codes::UNKNOWN_ARGUMENT_NAME,
                        format!("`{}` names no parameter here", n.name.name),
                    )
                    .primary(n.name.span, "no signature to match this against")
                    .note(
                        "a named argument needs a callee reached by name; a call through a \
                         value, a lambda or a constructor is positional",
                    ),
                );
            }
            named.clear();
            return;
        };

        // Over-application is `E0202`'s to report, with the callee's own type in
        // hand. Bail rather than report a second, worse version of it.
        if args.len() > sig.params.len() {
            named.clear();
            return;
        }

        let mut slots: Vec<Option<Expr>> = sig.params.iter().map(|_| None).collect();
        for (i, a) in args.drain(..).enumerate() {
            slots[i] = Some(a);
        }
        let filled_positionally = slots.iter().filter(|s| s.is_some()).count();

        for n in named.drain(..) {
            let NamedArg { name, value, .. } = n;
            let Some(i) = sig.params.iter().position(|p| p.name == name.name) else {
                self.diags.push(
                    Diagnostic::error(
                        codes::UNKNOWN_ARGUMENT_NAME,
                        format!("`{}` names no parameter of this function", name.name),
                    )
                    .primary(name.span, "not a parameter")
                    .note(format!(
                        "the parameters are {}",
                        list(sig.params.iter().map(|p| p.name.as_str()))
                    )),
                );
                continue;
            };
            if slots[i].is_some() {
                let already = if i < filled_positionally {
                    "already filled by a positional argument"
                } else {
                    "already given by name"
                };
                self.diags.push(
                    Diagnostic::error(
                        codes::UNKNOWN_ARGUMENT_NAME,
                        format!("`{}` is {already}", name.name),
                    )
                    .primary(name.span, "given twice"),
                );
                continue;
            }
            slots[i] = Some(value);
        }

        let mut missing: Vec<&str> = Vec::new();
        let mut out = Vec::with_capacity(slots.len());
        for (slot, p) in slots.into_iter().zip(&sig.params) {
            match slot.or_else(|| p.default.clone().map(|d| respan(d, span))) {
                Some(v) => out.push(v),
                None => missing.push(p.name.as_str()),
            }
        }

        if !missing.is_empty() {
            self.diags.push(
                Diagnostic::error(
                    codes::MISSING_ARGUMENT,
                    format!(
                        "this call leaves {} unfilled",
                        list(missing.iter().copied())
                    ),
                )
                .primary(span, "no argument and no default")
                .note("pass it positionally, or by name as `<parameter>: <value>`"),
            );
            // Leave what was written rather than a half-built call: a later pass
            // reading a call this one could not complete should see the source.
            let ExprKind::App { args, .. } = &mut e.kind else {
                unreachable!("just matched")
            };
            *args = out;
            return;
        }

        let ExprKind::App { args, .. } = &mut e.kind else {
            unreachable!("just matched")
        };
        *args = out;
    }

    /// The callee's signature, if this call reaches one by name.
    fn signature_of(&mut self, func: &Expr) -> Option<Signature> {
        let ExprKind::Var(q) = &func.kind else {
            return None;
        };
        // A constructor is exactly applied and carries no defaults.
        if q.is_bare() && is_ctor_name(q.symbol()) {
            return None;
        }
        // A local binder wins over everything, so a call through one is a call
        // through a value.
        if q.is_bare() && self.scope.contains(q.symbol()) {
            return None;
        }
        match self.resolved.lookup(self.module, Namespace::Value, q) {
            Ok(binding) => self.signatures.get(&binding.qualified).cloned(),
            // Unresolved: a builtin, or a name whose own diagnostic is
            // somebody else's. `builtin_signature` answers `None` for the
            // second, which leaves the call exactly as written.
            Err(_) => q.is_bare().then(|| builtin_signature(q.symbol())).flatten(),
        }
    }

    fn bind_pattern(&mut self, p: &Pattern) {
        grow(|| match &p.kind {
            PatternKind::Var(n) => self.scope.push(n.name.clone()),
            PatternKind::Ctor { args, .. } => args.iter().for_each(|a| self.bind_pattern(a)),
            PatternKind::Record { fields, .. } => {
                fields.iter().for_each(|(_, p)| self.bind_pattern(p))
            }
            PatternKind::List { items, rest } => {
                items.iter().for_each(|i| self.bind_pattern(i));
                if let Some(r) = rest {
                    self.bind_pattern(r);
                }
            }
            PatternKind::Lit(_) | PatternKind::Wildcard => {}
        })
    }
}

/// A spliced default reports at the call site that omitted it, not at the
/// signature that supplied it: the reader is looking at the call.
fn respan(mut e: Expr, span: Span) -> Expr {
    struct S(Span);
    impl S {
        fn go(&self, e: &mut Expr) {
            grow(|| {
                e.span = self.0;
                match &mut e.kind {
                    ExprKind::App { func, args, named } => {
                        self.go(func);
                        args.iter_mut().for_each(|a| self.go(a));
                        named.iter_mut().for_each(|n| self.go(&mut n.value));
                    }
                    ExprKind::Binary { lhs, rhs, .. } => {
                        self.go(lhs);
                        self.go(rhs);
                    }
                    ExprKind::Unary { operand, .. } => self.go(operand),
                    ExprKind::List { items } => items.iter_mut().for_each(|i| self.go(i)),
                    ExprKind::Record { fields } => fields.iter_mut().for_each(|(_, v)| self.go(v)),
                    _ => {}
                }
            })
        }
    }
    S(span).go(&mut e);
    e
}

fn list<'a>(mut names: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    let Some(first) = names.next() else {
        return "nothing".to_string();
    };
    out.push('`');
    out.push_str(first);
    out.push('`');
    let rest: Vec<&str> = names.collect();
    for (i, n) in rest.iter().enumerate() {
        out.push_str(if i + 1 == rest.len() { " and " } else { ", " });
        out.push('`');
        out.push_str(n);
        out.push('`');
    }
    out
}

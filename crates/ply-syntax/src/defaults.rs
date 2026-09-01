//! Filling a call's unwritten arguments from the callee's signature.

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
    /// Already qualified against the module that wrote it, so splicing it into any caller means
    /// what it meant where it was written.
    default: Option<Expr>,
    /// The module binders [`qualify`] introduced, which every module the default lands in has to
    /// carry — including the one that wrote it, which does not import itself.
    imports: Vec<(Symbol, usize)>,
}

/// A callee's parameters, plus the module that wrote them — needed because the implicit imports a
/// spliced default requires are the *writer's* modules.
#[derive(Clone)]
struct Signature {
    params: Vec<ParamInfo>,
}

/// The builtins that carry a default.
fn builtin_signature(name: &Symbol) -> Option<Signature> {
    let span = Span::DUMMY;
    match name.as_str() {
        "assert" => Some(Signature {
            params: vec![
                ParamInfo {
                    name: Symbol::new("cond"),
                    default: None,
                    imports: Vec::new(),
                },
                ParamInfo {
                    name: Symbol::new("message"),
                    // A prelude constructor, which is in every module's reach without an import —
                    // so this one needs no qualifying, and brings no module in with it.
                    default: Some(Expr {
                        kind: ExprKind::Var(crate::ast::QName::bare(Ident::new("None", span))),
                        span,
                    }),
                    imports: Vec::new(),
                },
            ],
        }),
        _ => None,
    }
}

/// The shape this pass believes a builtin has: how many parameters, and how many of them carry a
/// default.
pub fn builtin_shape(name: &str) -> Option<(usize, usize)> {
    let sig = builtin_signature(&Symbol::new(name))?;
    let defaults = sig.params.iter().filter(|p| p.default.is_some()).count();
    Some((sig.params.len(), defaults))
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
        for (binder, target) in implicit {
            bind_module(resolved, *m, binder, target);
        }
    }
    program.modules = items.into_iter().map(|(_, module)| module).collect();
}

/// Every `fn` in the program by its program-wide name, with each default already qualified against
/// the module that wrote it.
fn collect(
    program: &Program,
    resolved: &mut Resolved,
    diags: &mut Vec<Diagnostic>,
) -> IndexMap<Symbol, Signature> {
    let mut out = IndexMap::new();
    let mut owner_imports: Vec<(usize, Symbol, usize)> = Vec::new();
    for (m, module) in program.modules.iter().enumerate() {
        for item in &module.items {
            let Item::Fn(def) = item else { continue };
            let names: Vec<&Symbol> = def.params.iter().map(|p| &p.name.name).collect();
            let params = def
                .params
                .iter()
                .map(|p| {
                    let Some(d) = p.default.as_ref().filter(|d| admissible(d, &names, diags))
                    else {
                        return ParamInfo {
                            name: p.name.name.clone(),
                            default: None,
                            imports: Vec::new(),
                        };
                    };
                    let (default, imports) = qualify(d, m, resolved, def.vis.is_public(), diags);
                    // The writing module needs the binder as much as any caller does: the checker
                    // types this default where it was written, and a module does not import itself.
                    for (binder, target) in &imports {
                        owner_imports.push((m, binder.clone(), *target));
                    }
                    ParamInfo {
                        name: p.name.name.clone(),
                        default: Some(default),
                        imports,
                    }
                })
                .collect();
            out.insert(module.name.qualify(&def.name.name), Signature { params });
        }
    }
    for (module, binder, target) in owner_imports {
        bind_module(resolved, module, binder, target);
    }
    out
}

/// Records that `module` now reaches `target` under `binder`.
fn bind_module(resolved: &mut Resolved, module: usize, binder: Symbol, target: usize) {
    if let Some(scope) = resolved.scopes.get_mut(module) {
        scope.modules.entry(binder).or_insert((target, Span::DUMMY));
    }
}

/// Whether a default may be spliced at all, reporting why not.
fn admissible(e: &Expr, params: &[&Symbol], diags: &mut Vec<Diagnostic>) -> bool {
    if !crate::ast::is_default_expr(e) {
        diags.push(
            Diagnostic::error(
                codes::DEFAULT_NOT_PURE,
                "a parameter default must be a pure, closed expression",
            )
            .primary(e.span, "this runs, rather than being a value")
            .note(
                "the default is copied into every call that omits the argument, so a call                  or a `perform` in it would run at the caller and not here",
            )
            .note("a literal, a constructor applied to literals, a record or a list is fine"),
        );
        return false;
    }
    let mut mentioned = Vec::new();
    mentions(e, params, &mut mentioned);
    if let Some(name) = mentioned.first() {
        diags.push(
            Diagnostic::error(
                codes::DEFAULT_NOT_PURE,
                format!("a parameter default cannot mention `{name}`"),
            )
            .primary(e.span, "names a parameter of this same signature")
            .note("a call site has no such binding for the default to be copied into"),
        );
        return false;
    }
    true
}

/// Which of `params` the expression mentions free.
fn mentions(e: &Expr, params: &[&Symbol], out: &mut Vec<Symbol>) {
    struct M<'a> {
        params: &'a [&'a Symbol],
        bound: Vec<Symbol>,
    }
    impl M<'_> {
        fn go(&mut self, e: &Expr, out: &mut Vec<Symbol>) {
            grow(|| match &e.kind {
                ExprKind::Var(q) => {
                    if q.is_bare()
                        && !self.bound.contains(q.symbol())
                        && self.params.contains(&q.symbol())
                    {
                        out.push(q.symbol().clone());
                    }
                }
                ExprKind::Lambda { params, body } => {
                    let mark = self.bound.len();
                    for p in params {
                        self.bound.push(p.name.name.clone());
                    }
                    self.go(body, out);
                    self.bound.truncate(mark);
                }
                ExprKind::App { func, args, named } => {
                    self.go(func, out);
                    args.iter().for_each(|a| self.go(a, out));
                    named.iter().for_each(|n| self.go(&n.value, out));
                }
                ExprKind::Binary { lhs, rhs, .. } => {
                    self.go(lhs, out);
                    self.go(rhs, out);
                }
                ExprKind::Unary { operand, .. } => self.go(operand, out),
                ExprKind::If {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    self.go(cond, out);
                    self.go(then_branch, out);
                    self.go(else_branch, out);
                }
                ExprKind::Match { scrutinee, arms } => {
                    self.go(scrutinee, out);
                    for arm in arms {
                        let mark = self.bound.len();
                        binders(&arm.pat, &mut self.bound);
                        if let Some(g) = &arm.guard {
                            self.go(g, out);
                        }
                        self.go(&arm.body, out);
                        self.bound.truncate(mark);
                    }
                }
                ExprKind::Block { stmts, tail } => {
                    let mark = self.bound.len();
                    for s in stmts {
                        match s {
                            Stmt::Let { pat, value, .. } => {
                                self.go(value, out);
                                binders(pat, &mut self.bound);
                            }
                            Stmt::Expr(e) => self.go(e, out),
                        }
                    }
                    if let Some(t) = tail {
                        self.go(t, out);
                    }
                    self.bound.truncate(mark);
                }
                ExprKind::Record { fields } => fields.iter().for_each(|(_, v)| self.go(v, out)),
                ExprKind::RecordUpdate { base, fields } => {
                    self.go(base, out);
                    fields.iter().for_each(|(_, v)| self.go(v, out));
                }
                ExprKind::Field { base, .. } => self.go(base, out),
                ExprKind::List { items } => items.iter().for_each(|i| self.go(i, out)),
                ExprKind::Try { operand } => self.go(operand, out),
                ExprKind::Perform { args, .. } => args.iter().for_each(|a| self.go(a, out)),
                ExprKind::Handle { body, .. }
                | ExprKind::WithCell { body, .. }
                | ExprKind::WithRegion { body, .. }
                | ExprKind::Simulate { body } => self.go(body, out),
                ExprKind::Lit(_) => {}
            })
        }
    }
    M {
        params,
        bound: Vec::new(),
    }
    .go(e, out);
}

fn binders(p: &Pattern, out: &mut Vec<Symbol>) {
    grow(|| match &p.kind {
        PatternKind::Var(n) => out.push(n.name.clone()),
        PatternKind::Ctor { args, .. } => args.iter().for_each(|a| binders(a, out)),
        PatternKind::Record { fields, .. } => fields.iter().for_each(|(_, p)| binders(p, out)),
        PatternKind::List { items, rest } => {
            items.iter().for_each(|i| binders(i, out));
            if let Some(r) = rest {
                binders(r, out);
            }
        }
        PatternKind::Lit(_) | PatternKind::Wildcard => {}
    })
}

/// Rewrites a default's free names to the form they keep in any caller.
fn qualify(
    e: &Expr,
    owner: usize,
    resolved: &Resolved,
    public: bool,
    diags: &mut Vec<Diagnostic>,
) -> (Expr, Vec<(Symbol, usize)>) {
    let mut q = Qualify {
        owner,
        resolved,
        public,
        bound: Vec::new(),
        imports: Vec::new(),
        diags,
    };
    let mut out = e.clone();
    q.expr(&mut out);
    (out, q.imports)
}

struct Qualify<'a> {
    owner: usize,
    resolved: &'a Resolved,
    public: bool,
    bound: Vec<Symbol>,
    /// The module binders this default now needs, wherever it is spliced.
    imports: Vec<(Symbol, usize)>,
    diags: &'a mut Vec<Diagnostic>,
}

impl Qualify<'_> {
    fn expr(&mut self, e: &mut Expr) {
        grow(|| match &mut e.kind {
            ExprKind::Var(q) => {
                if !q.is_bare() || self.bound.contains(q.symbol()) {
                    return;
                }
                let Some(binding) = self.resolved.find(self.owner, Namespace::Value, q) else {
                    // A builtin or a prelude constructor: in reach everywhere, so it needs no
                    // qualifying and gets none.
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
                let binder = module.clone();
                if !self.imports.iter().any(|(b, _)| *b == binder) {
                    self.imports.push((binder.clone(), owner));
                }
                q.module = Some(Ident::new(binder, q.span));
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
            // Refused by `is_default_expr` before this runs, and reached only when the checker has
            // already reported that.
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
    /// Local binders, innermost last.
    scope: Vec<Symbol>,
    /// Modules a spliced default made this one reference, to be added to its scope once the walk
    /// lets go of `resolved`.
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
                // A default is not walked here: it was qualified against this module in `collect`,
                // and a call inside one is refused by `is_default_expr` before it could need
                // expanding.
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

    /// Children first, then this node, so a call nested in another's argument is filled before the
    /// outer one copies it.
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
            // No signature in hand: a call through a value, a constructor, or a name that does not
            // resolve.
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

        // Over-application is `E0202`'s to report, with the callee's own type in hand.
        if args.len() > sig.params.len() {
            named.clear();
            return;
        }

        // Kept so that a call this pass cannot complete is left exactly as it was written.
        let written: Vec<Expr> = args.clone();
        let was_named = !named.is_empty();
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
            if let Some(written) = slot {
                out.push(written);
                continue;
            }
            let Some(default) = &p.default else {
                missing.push(p.name.as_str());
                continue;
            };
            // The default may name something in the module that wrote it, and this one may never
            // have imported that module.
            for edge in &p.imports {
                if !self.implicit.contains(edge) {
                    self.implicit.push(edge.clone());
                }
            }
            // Keeping the default's own span, not the call's.
            out.push(default.clone());
        }

        if !missing.is_empty() {
            // A call with no names in it that leaves a hole is under-applied, which was `E0202`
            // from inference before defaults existed and stays `E0202` now.
            if was_named {
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
            }
            let ExprKind::App { args, .. } = &mut e.kind else {
                unreachable!("just matched")
            };
            *args = written;
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
        // A local binder wins over everything, so a call through one is a call through a value.
        if q.is_bare() && self.scope.contains(q.symbol()) {
            return None;
        }
        // `find` and not `lookup`: this runs on every call in the program and most of them are
        // builtins, which resolve to nothing here.
        match self.resolved.find(self.module, Namespace::Value, q) {
            Some(binding) => self.signatures.get(&binding.qualified).cloned(),
            // A builtin, or a name whose own diagnostic is somebody else's.
            None => q.is_bare().then(|| builtin_signature(q.symbol())).flatten(),
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

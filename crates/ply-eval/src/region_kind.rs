//! Infers each region's kind: `unique` unless a continuation capture is reachable from it.

use crate::arena::RegionKind;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Expr, ExprKind, Item, Pattern, PatternKind, Program, QName, Stmt};
use ply_syntax::resolve::{Namespace, Resolved};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, OnceLock};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Cause {
    /// A handler clause with a `resume` binder, which may resume any number of times.
    Clause {
        effect: Symbol,
        op: Symbol,
    },
    /// Tail-resumptive: the continuation reaches no binder, so it alone does not force `shared`.
    TailClause {
        effect: Symbol,
        op: Symbol,
    },
    Escapes {
        effect: Symbol,
        op: Symbol,
    },
    Task {
        op: Symbol,
    },
    Simulate,
    Indirect,
    Callback {
        builtin: &'static str,
    },
}

impl Cause {
    pub fn describe(&self) -> String {
        match self {
            Cause::Clause { effect, op } => {
                format!("`{effect}.{op}` binds its continuation with `resume`")
            }
            Cause::TailClause { effect, op } => format!(
                "`{effect}.{op}` is tail-resumptive: its continuation is spliced by the `Resume` \
                 frame pushed for it and reaches no binder, so it cannot outlive this region"
            ),
            Cause::Escapes { effect, op } => format!(
                "`{effect}.{op}` is answered outside this region, so the capture crosses its \
                 boundary"
            ),
            Cause::Task { op } => {
                format!("`task.{op}` parks this task, and the scheduler resumes it")
            }
            Cause::Simulate => "a simulated region parks and resumes its tasks".to_string(),
            Cause::Indirect => {
                "the callee is not known here, so it may be any function in the program".to_string()
            }
            Cause::Callback { builtin } => {
                format!("`{builtin}` calls a function this analysis cannot name")
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CaptureSite {
    pub span: Span,
    pub cause: Cause,
    /// The definitions between the region body and the site, outermost first.
    pub through: Vec<Symbol>,
}

impl CaptureSite {
    /// `reached through `a` → `b``, or `None` when the site is in the region.
    pub fn chain(&self) -> Option<String> {
        if self.through.is_empty() {
            return None;
        }
        let names: Vec<String> = self.through.iter().map(|n| format!("`{n}`")).collect();
        Some(format!("reached through {}", names.join(" → ")))
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Region {
    pub span: Span,
    /// The brand: `r` of `with_region[r]` or of a `with_cell[r]` that opens its own region.
    pub brand: Symbol,
    pub kind: RegionKind,
    /// `Some` exactly when `kind` is [`RegionKind::Shared`] by inference.
    pub capture: Option<CaptureSite>,
    pub declared: bool,
}

#[derive(Clone, Default, Debug)]
pub struct Regions {
    /// Source order: by module, then by position.
    regions: Vec<Region>,
}

impl Regions {
    /// `Shared` for a span that opens no known region.
    pub fn kind(&self, span: Span) -> RegionKind {
        self.at(span).map_or(RegionKind::Shared, |r| r.kind)
    }

    pub fn at(&self, span: Span) -> Option<&Region> {
        self.regions.iter().find(|r| r.span == span)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Region> {
        self.regions.iter()
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn unique(&self) -> usize {
        self.regions
            .iter()
            .filter(|r| r.kind == RegionKind::Unique)
            .count()
    }

    pub fn shared(&self) -> usize {
        self.len() - self.unique()
    }
}

pub type Kinds = Arc<OnceLock<Regions>>;

pub fn infer(program: &Program, resolved: &Resolved) -> Regions {
    let (regions, _) = decide(program, resolved, &[]);
    regions
}

pub fn check(
    program: &Program,
    resolved: &Resolved,
    declared: &[(Span, RegionKind)],
) -> Result<Regions, Vec<Diagnostic>> {
    let (regions, refusals) = decide(program, resolved, declared);
    if refusals.is_empty() {
        Ok(regions)
    } else {
        Err(refusals)
    }
}

fn decide(
    program: &Program,
    resolved: &Resolved,
    declared: &[(Span, RegionKind)],
) -> (Regions, Vec<Diagnostic>) {
    let mut analysis = Analysis::new(program, resolved);
    analysis.scan_program();
    analysis.promote_indirect();
    analysis.propagate();
    analysis.settle(declared)
}

#[derive(Clone, Default)]
struct Scan {
    /// The first capture written in the body itself, in source order.
    direct: Option<CaptureSite>,
    /// The first place the body reaches code this analysis cannot name.
    indirect: Option<CaptureSite>,
    /// The first tail-resumptive clause, which is not a capture that outlives a region.
    tail: Option<CaptureSite>,
    /// Definitions named in the body, in source order.
    refs: Vec<Symbol>,
}

impl Scan {
    fn direct_at(&mut self, span: Span, cause: Cause) {
        let site = CaptureSite {
            span,
            cause,
            through: Vec::new(),
        };
        let slot = if matches!(site.cause, Cause::TailClause { .. }) {
            &mut self.tail
        } else {
            &mut self.direct
        };
        if slot.is_none() {
            *slot = Some(site);
        }
    }

    fn indirect_at(&mut self, span: Span, cause: Cause) {
        if self.indirect.is_none() {
            self.indirect = Some(CaptureSite {
                span,
                cause,
                through: Vec::new(),
            });
        }
    }

    fn absorb(&mut self, other: Scan) {
        if self.direct.is_none() {
            self.direct = other.direct;
        }
        if self.indirect.is_none() {
            self.indirect = other.indirect;
        }
        if self.tail.is_none() {
            self.tail = other.tail;
        }
        self.refs.extend(other.refs);
    }

    /// Prefers the capture actually written over one merely reachable.
    fn site(&self) -> Option<&CaptureSite> {
        self.direct.as_ref().or(self.indirect.as_ref())
    }
}

struct Found {
    span: Span,
    brand: Symbol,
    scan: Scan,
}

#[derive(Clone)]
struct Ctx {
    module: usize,
    /// Operations answered by a `handle` written inside the region being walked.
    handled: Vec<(Symbol, Symbol)>,
    /// Brands open here; a `with_cell[r]` inside `with_region[r]` opens no region of its own.
    brands: Vec<Symbol>,
}

struct Analysis<'a> {
    program: &'a Program,
    resolved: &'a Resolved,
    /// Program-wide definition names, to tell a `Var` from a constructor or builtin.
    definitions: BTreeSet<Symbol>,
    /// Local binders in scope, innermost last.
    locals: Vec<Symbol>,
    prelude_ctors: BTreeSet<Symbol>,
    scans: BTreeMap<Symbol, Scan>,
    found: Vec<Found>,
    /// The first capture written anywhere, which an unknown callee may reach.
    anywhere: Option<CaptureSite>,
    reaches: BTreeMap<Symbol, CaptureSite>,
}

impl<'a> Analysis<'a> {
    fn new(program: &'a Program, resolved: &'a Resolved) -> Analysis<'a> {
        let mut definitions = BTreeSet::new();
        for module in &program.modules {
            for item in &module.items {
                if let Item::Fn(f) = item {
                    definitions.insert(module.name.qualify(&f.name.name));
                }
            }
        }
        Analysis {
            program,
            resolved,
            definitions,
            locals: Vec::new(),
            prelude_ctors: ply_ty::prelude::ctor_arities()
                .into_iter()
                .map(|(name, _)| name)
                .collect(),
            scans: BTreeMap::new(),
            found: Vec::new(),
            anywhere: None,
            reaches: BTreeMap::new(),
        }
    }

    fn scan_program(&mut self) {
        for (m, module) in self.program.modules.iter().enumerate() {
            for item in &module.items {
                let (name, body, binders) = match item {
                    Item::Fn(f) => (
                        Some(module.name.qualify(&f.name.name)),
                        &f.body,
                        f.params.iter().map(|p| p.name.name.clone()).collect(),
                    ),
                    Item::Test(t) => (None, &t.body, Vec::new()),
                    Item::Law(l) => (
                        None,
                        &l.body,
                        l.binders.iter().map(|b| b.name.name.clone()).collect(),
                    ),
                    _ => continue,
                };
                let ctx = Ctx {
                    module: m,
                    handled: Vec::new(),
                    brands: Vec::new(),
                };
                let mut scan = Scan::default();
                self.scoped(binders, |a| a.walk(body, &ctx, &mut scan));
                if self.anywhere.is_none() {
                    self.anywhere = scan.direct.clone();
                }
                if let Some(name) = name {
                    self.scans.insert(name, scan);
                }
            }
        }
    }

    /// An unknown callee reaches whatever the program can reach.
    fn promote_indirect(&mut self) {
        if self.anywhere.is_none() {
            for scan in self.scans.values_mut() {
                scan.indirect = None;
            }
            for found in &mut self.found {
                found.scan.indirect = None;
            }
        }
    }

    /// Breadth-first over reverse calls, so each definition gets its shortest chain to a site.
    fn propagate(&mut self) {
        let mut callers: BTreeMap<Symbol, BTreeSet<Symbol>> = BTreeMap::new();
        for (name, scan) in &self.scans {
            for callee in &scan.refs {
                callers
                    .entry(callee.clone())
                    .or_default()
                    .insert(name.clone());
            }
        }

        let mut queue: VecDeque<Symbol> = VecDeque::new();
        for (name, scan) in &self.scans {
            if let Some(site) = scan.site() {
                self.reaches.insert(name.clone(), site.clone());
                queue.push_back(name.clone());
            }
        }
        while let Some(callee) = queue.pop_front() {
            let site = self.reaches[&callee].clone();
            let Some(callers) = callers.get(&callee) else {
                continue;
            };
            for caller in callers {
                if self.reaches.contains_key(caller) {
                    continue;
                }
                let mut through = vec![callee.clone()];
                through.extend(site.through.iter().cloned());
                self.reaches.insert(
                    caller.clone(),
                    CaptureSite {
                        span: site.span,
                        cause: site.cause.clone(),
                        through,
                    },
                );
                queue.push_back(caller.clone());
            }
        }
    }

    fn settle(mut self, declared: &[(Span, RegionKind)]) -> (Regions, Vec<Diagnostic>) {
        let mut regions = Vec::with_capacity(self.found.len());
        let mut refusals = Vec::new();
        for found in std::mem::take(&mut self.found) {
            let capture = self.capture_of(&found.scan);
            let inferred = match capture {
                Some(_) => RegionKind::Shared,
                None => RegionKind::Unique,
            };
            let forced = declared
                .iter()
                .find(|(span, _)| *span == found.span)
                .map(|(_, kind)| *kind);
            let kind = forced.unwrap_or(inferred);
            if forced == Some(RegionKind::Unique)
                && let Some(site) = &capture
            {
                refusals.push(refuse_unique(&found, site));
            }
            regions.push(Region {
                span: found.span,
                brand: found.brand,
                kind,
                capture,
                declared: forced.is_some(),
            });
        }
        (Regions { regions }, refusals)
    }

    fn capture_of(&self, scan: &Scan) -> Option<CaptureSite> {
        if let Some(site) = scan.site() {
            return Some(site.clone());
        }
        for callee in &scan.refs {
            if let Some(site) = self.reaches.get(callee) {
                let mut through = vec![callee.clone()];
                through.extend(site.through.iter().cloned());
                return Some(CaptureSite {
                    span: site.span,
                    cause: site.cause.clone(),
                    through,
                });
            }
        }
        None
    }

    fn walk(&mut self, e: &Expr, ctx: &Ctx, out: &mut Scan) {
        crate::limit::grow(|| self.walk_at(e, ctx, out));
    }

    fn scoped(&mut self, names: Vec<Symbol>, f: impl FnOnce(&mut Self)) {
        let depth = self.locals.len();
        self.locals.extend(names);
        f(self);
        self.locals.truncate(depth);
    }

    fn walk_at(&mut self, e: &Expr, ctx: &Ctx, out: &mut Scan) {
        match &e.kind {
            ExprKind::Var(q) => {
                if let Some(name) = self.definition(ctx.module, q) {
                    out.refs.push(name);
                }
            }
            ExprKind::App { func, args, .. } => {
                self.walk_call(e.span, func, args, ctx, out);
            }
            ExprKind::Lambda { params, body, .. } => {
                let bound = params.iter().map(|p| p.name.name.clone()).collect();
                self.scoped(bound, |a| a.walk(body, ctx, out));
            }
            ExprKind::Block { stmts, tail } => {
                let depth = self.locals.len();
                // A `let`'s binders scope over later statements and the tail, not its own value.
                for stmt in stmts {
                    match stmt {
                        Stmt::Let { pat, value, .. } => {
                            self.walk(value, ctx, out);
                            pattern_binders(pat, &mut self.locals);
                        }
                        Stmt::Expr(e) => self.walk(e, ctx, out),
                    }
                }
                if let Some(tail) = tail {
                    self.walk(tail, ctx, out);
                }
                self.locals.truncate(depth);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.walk(scrutinee, ctx, out);
                for arm in arms {
                    let mut bound = Vec::new();
                    pattern_binders(&arm.pat, &mut bound);
                    self.scoped(bound, |a| {
                        if let Some(guard) = &arm.guard {
                            a.walk(guard, ctx, out);
                        }
                        a.walk(&arm.body, ctx, out);
                    });
                }
            }
            ExprKind::Perform {
                effect, op, args, ..
            } => {
                for arg in args {
                    self.walk(arg, ctx, out);
                }
                self.walk_perform(e.span, effect, &op.name, ctx, out);
            }
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => {
                let mut inner = ctx.clone();
                for clause in clauses {
                    let effect = self.effect_name(ctx.module, &clause.effect);
                    let op = clause.op.name.clone();
                    // Both forms capture.
                    let cause = match clause.resume {
                        Some(_) => Cause::Clause {
                            effect: effect.clone(),
                            op: op.clone(),
                        },
                        None => Cause::TailClause {
                            effect: effect.clone(),
                            op: op.clone(),
                        },
                    };
                    out.direct_at(clause.span, cause);
                    inner.handled.push((effect, op));
                }
                self.walk(body, &inner, out);
                // Clause and return bodies run below their handler, so use the outer context.
                for clause in clauses {
                    let mut bound: Vec<Symbol> =
                        clause.params.iter().map(|p| p.name.clone()).collect();
                    bound.extend(clause.resume.iter().map(|r| r.name.clone()));
                    self.scoped(bound, |a| a.walk(&clause.body, ctx, out));
                }
                if let Some(ret) = return_clause {
                    let bound = vec![ret.binder.name.clone()];
                    self.scoped(bound, |a| a.walk(&ret.body, ctx, out));
                }
            }
            ExprKind::Simulate { body } => {
                out.direct_at(e.span, Cause::Simulate);
                self.walk(body, ctx, out);
            }
            ExprKind::WithRegion { region, body } => {
                self.walk_region(e.span, region.name.clone(), body, ctx, out);
            }
            ExprKind::WithCell {
                resource,
                init,
                binder,
                body,
            } => {
                self.walk(init, ctx, out);
                let bound = vec![binder.name.clone()];
                if ctx.brands.contains(&resource.name) {
                    self.scoped(bound, |a| a.walk(body, ctx, out));
                } else {
                    let span = e.span;
                    let brand = resource.name.clone();
                    self.scoped(bound, |a| a.walk_region(span, brand, body, ctx, out));
                }
            }
            _ => children(e, &mut |child| self.walk(child, ctx, out)),
        }
    }

    fn walk_region(&mut self, span: Span, brand: Symbol, body: &Expr, ctx: &Ctx, out: &mut Scan) {
        let mut inner = ctx.clone();
        inner.brands.push(brand.clone());
        // An enclosing `handle` answers across the boundary (`Cause::Escapes`): not inherited.
        inner.handled.clear();
        let mut scan = Scan::default();
        self.walk(body, &inner, &mut scan);
        // Post-order: an inner region is recorded before the region enclosing it.
        self.found.push(Found {
            span,
            brand,
            scan: scan.clone(),
        });
        out.absorb(scan);
    }

    fn walk_call(&mut self, span: Span, func: &Expr, args: &[Expr], ctx: &Ctx, out: &mut Scan) {
        for arg in args {
            self.walk(arg, ctx, out);
        }
        match &func.kind {
            ExprKind::Var(q) if !self.is_local(q) => {
                if let Some(name) = self.definition(ctx.module, q) {
                    out.refs.push(name);
                    return;
                }
                if self.is_constructor(ctx.module, q) {
                    return;
                }
                if q.is_bare()
                    && let Some(builtin) = crate::builtins::Builtin::from_name(q.symbol())
                {
                    if builtin.higher_order() {
                        self.walk_callback(span, builtin.name(), args, ctx, out);
                    }
                    return;
                }
                out.indirect_at(span, Cause::Indirect);
            }
            // A local, even one shadowing a definition, constructor or builtin.
            ExprKind::Var(_) => out.indirect_at(span, Cause::Indirect),
            // Written at the call site, so the callee is known.
            ExprKind::Lambda { params, body, .. } => {
                let bound = params.iter().map(|p| p.name.name.clone()).collect();
                self.scoped(bound, |a| a.walk(body, ctx, out));
            }
            // A field, the result of another call: nothing here names what will run.
            _ => {
                self.walk(func, ctx, out);
                out.indirect_at(span, Cause::Indirect);
            }
        }
    }

    fn walk_callback(
        &mut self,
        span: Span,
        builtin: &'static str,
        args: &[Expr],
        ctx: &Ctx,
        out: &mut Scan,
    ) {
        let nameable = match args.last().map(|arg| &arg.kind) {
            // Written or named at the call site; `walk_call` already recorded the edge.
            Some(ExprKind::Lambda { .. }) => true,
            Some(ExprKind::Var(q)) => {
                !self.is_local(q)
                    && (self.definition(ctx.module, q).is_some()
                        || self.is_constructor(ctx.module, q)
                        || (q.is_bare()
                            && crate::builtins::Builtin::from_name(q.symbol()).is_some()))
            }
            _ => false,
        };
        if !nameable {
            out.indirect_at(span, Cause::Callback { builtin });
        }
    }

    fn walk_perform(&mut self, span: Span, effect: &QName, op: &Symbol, ctx: &Ctx, out: &mut Scan) {
        let declared = self.global(ctx.module, Namespace::Effect, effect);
        if declared.is_none() && effect.is_bare() && effect.symbol().as_str() == "task" {
            out.direct_at(span, Cause::Task { op: op.clone() });
            return;
        }
        let name = declared.unwrap_or_else(|| effect.symbol().clone());
        // Handled inside this region, whose clause already made it `shared`.
        if ctx.handled.contains(&(name.clone(), op.clone())) {
            return;
        }
        out.direct_at(
            span,
            Cause::Escapes {
                effect: name,
                op: op.clone(),
            },
        );
    }

    fn is_local(&self, q: &QName) -> bool {
        q.is_bare() && self.locals.contains(q.symbol())
    }

    /// The definition `q` denotes; `None` for a local, constructor or builtin.
    fn definition(&self, module: usize, q: &QName) -> Option<Symbol> {
        if self.is_local(q) {
            return None;
        }
        let name = self.global(module, Namespace::Value, q)?;
        self.definitions.contains(&name).then_some(name)
    }

    fn is_constructor(&self, module: usize, q: &QName) -> bool {
        if self.is_local(q) {
            return false;
        }
        match self.global(module, Namespace::Value, q) {
            Some(name) => !self.definitions.contains(&name),
            None => q.is_bare() && self.prelude_ctors.contains(q.symbol()),
        }
    }

    fn effect_name(&self, module: usize, effect: &QName) -> Symbol {
        self.global(module, Namespace::Effect, effect)
            .unwrap_or_else(|| effect.symbol().clone())
    }

    fn global(&self, module: usize, ns: Namespace, q: &QName) -> Option<Symbol> {
        if q.is_bare() {
            return self
                .resolved
                .scopes
                .get(module)
                .and_then(|scope| scope.get(ns, q.symbol()))
                .map(|b| b.qualified.clone());
        }
        self.resolved
            .lookup(module, ns, q)
            .ok()
            .map(|b| b.qualified.clone())
    }
}

fn refuse_unique(found: &Found, site: &CaptureSite) -> Diagnostic {
    let mut d = Diagnostic::error(
        codes::REGION_KIND_REFUSED,
        format!(
            "region `{}` is declared `unique`, but a continuation is captured across it",
            found.brand
        ),
    )
    .primary(
        found.span,
        format!("`{}` is declared `unique` here", found.brand),
    )
    .secondary(site.span, site.cause.describe());
    if let Some(chain) = site.chain() {
        d = d.note(chain);
    }
    d.note(
        "a `unique` region is a bump pointer with no snapshot, so one resumption would observe \
         another resumption's writes",
    )
    .note(
        "remove the annotation and the region is inferred `shared`, which snapshots at the capture",
    )
}

fn pattern_binders(p: &Pattern, out: &mut Vec<Symbol>) {
    crate::limit::grow(|| match &p.kind {
        PatternKind::Wildcard | PatternKind::Lit(_) => {}
        PatternKind::Var(id) => out.push(id.name.clone()),
        PatternKind::Ctor { args, .. } => {
            for arg in args {
                pattern_binders(arg, out);
            }
        }
        PatternKind::Record { fields, .. } => {
            for (_, pat) in fields {
                pattern_binders(pat, out);
            }
        }
        PatternKind::List { items, rest } => {
            for item in items {
                pattern_binders(item, out);
            }
            if let Some(rest) = rest {
                pattern_binders(rest, out);
            }
        }
    });
}

/// Every subexpression, in source order, for the forms that bind nothing and open nothing.
fn children(e: &Expr, f: &mut impl FnMut(&Expr)) {
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Var(_) => {}
        ExprKind::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        ExprKind::Unary { operand, .. } => f(operand),
        ExprKind::Lambda { body, .. } => f(body),
        ExprKind::App { func, args, .. } => {
            f(func);
            for arg in args {
                f(arg);
            }
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
                if let Some(guard) = &arm.guard {
                    f(guard);
                }
                f(&arm.body);
            }
        }
        ExprKind::Block { stmts, tail } => {
            for stmt in stmts {
                match stmt {
                    Stmt::Let { value, .. } => f(value),
                    Stmt::Expr(e) => f(e),
                }
            }
            if let Some(tail) = tail {
                f(tail);
            }
        }
        ExprKind::Record { fields } => {
            for (_, value) in fields {
                f(value);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            f(base);
            for (_, value) in fields {
                f(value);
            }
        }
        ExprKind::Field { base, .. } => f(base),
        ExprKind::Try { operand } => f(operand),
        ExprKind::List { items } => {
            for item in items {
                f(item);
            }
        }
        ExprKind::Perform { args, .. } => {
            for arg in args {
                f(arg);
            }
        }
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            f(body);
            for clause in clauses {
                f(&clause.body);
            }
            if let Some(ret) = return_clause {
                f(&ret.body);
            }
        }
        ExprKind::WithCell { init, body, .. } => {
            f(init);
            f(body);
        }
        ExprKind::WithRegion { body, .. } | ExprKind::Simulate { body } => f(body),
    }
}

use crate::arena::{Arena, RegionKind};
use crate::builtins::Builtin;
use crate::env::Env;
use crate::handler::{OpDecl, check_operation, performed_atom};
use crate::host::{HostBinding, err_hermetic, err_machine_only_host, operation_label};
use crate::limit::{self, DEFAULT_MAX_CALLS, NAMED_CALLS, NESTED_CALLS, grow};
use crate::memo::{Lookup, Memo};
use crate::task_regions::TaskRegions;
use crate::trace::Trace;
use crate::value::{Closure, ClosureKind, Decimal, Value, type_error, values_equal};
use ply_core::CheckOutput;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{
    BinOp, Expr, ExprKind, HandleClause, Ident, Item, Lit, MatchArm, Mode, Param, Pattern,
    PatternKind, Program, QName, ReturnClause, Stmt, TestDef, TypeDefBody, UnOp,
};
use ply_syntax::resolve::{Namespace, Resolved};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

/// An operation's declaration, by program-wide effect name and operation name.
pub(crate) type OpTable = FxHashMap<(Symbol, Symbol), (bool, Mode)>;

struct HandlerFrame {
    clauses: Arc<Vec<HandleClause>>,
    /// Each clause's effect under its program-wide name, resolved where the `handle` was written: a
    /// perform reached from another module spells the same effect differently, and the two only
    /// meet once both are qualified.
    effects: Arc<Vec<Symbol>>,
    env: Env,
    /// The module the clause bodies were written in, which is not the one the perform that triggers
    /// them is reached from.
    module: usize,
    /// The calls pending when this handler was installed.
    calls: usize,
}

/// Ordered exactly as [`CheckOutput::tests`] is — load order, then source order — because the index
/// into the two is the same index.
struct TestSlot<'a> {
    module: usize,
    def: &'a TestDef,
}

pub struct Interp<'a> {
    program: &'a Program,
    resolved: &'a Resolved,
    check: Option<&'a CheckOutput>,
    /// Keyed by program-wide name, so two modules may declare one simple name.
    globals: FxHashMap<Symbol, Value>,
    /// What a nullary pure definition evaluated to.
    memo: Memo,
    ctors: FxHashMap<Symbol, usize>,
    ops: OpTable,
    tests: Vec<TestSlot<'a>>,
    handlers: Vec<HandlerFrame>,
    /// Where every cell this engine allocates lives, and the fixture every entry point resets to —
    /// so one seeded fixture serves every test in a run without any of them observing another's
    /// writes.
    regions: TaskRegions,
    /// Which of ADR 0017 §3's two kinds each region in this program is.
    region_kinds: crate::region_kind::Kinds,
    /// What this entry point performed, which is not what its row said it could.
    trace: Trace,
    /// The module a bare name is resolved in: the one that wrote the expression being evaluated,
    /// not the one that called it.
    module: usize,
    /// The pending calls, innermost last, by name.
    calls: Vec<Option<Symbol>>,
    max_calls: usize,
    /// What the run's host boundary is, held only in order to *refuse* at it.
    binding: Option<Arc<HostBinding>>,
}

impl<'a> Interp<'a> {
    pub fn new(program: &'a Program, resolved: &'a Resolved, check: &'a CheckOutput) -> Self {
        Self::build(program, resolved, Some(check))
    }

    /// Everything the evaluator needs is derivable from the resolved AST alone, so evaluation can
    /// be exercised without a type-check pass.
    pub fn for_program(program: &'a Program, resolved: &'a Resolved) -> Self {
        Self::build(program, resolved, None)
    }

    fn build(program: &'a Program, resolved: &'a Resolved, check: Option<&'a CheckOutput>) -> Self {
        let mut globals = FxHashMap::default();
        // The prelude's first, so a module declaring its own `Some` overwrites it — the resolution
        // order every other prelude name follows.
        let mut ctors: FxHashMap<Symbol, usize> =
            ply_core::prelude::ctor_arities().into_iter().collect();
        let mut ops = FxHashMap::default();
        let mut tests = Vec::new();

        for (m, module) in program.modules.iter().enumerate() {
            let qualify = |name: &Symbol| module.name.qualify(name);
            for item in &module.items {
                match item {
                    Item::Fn(f) => {
                        let params = f.params.iter().map(|p| p.name.name.clone()).collect();
                        let closure = Closure {
                            name: Some(qualify(&f.name.name)),
                            kind: ClosureKind::Fn {
                                params,
                                body: Arc::new(f.body.clone()),
                                env: Env::empty(),
                                module: m,
                            },
                        };
                        globals.insert(qualify(&f.name.name), Value::Closure(Arc::new(closure)));
                    }
                    Item::Type(t) => {
                        if let TypeDefBody::Sum(variants) = &t.body {
                            for v in variants {
                                ctors.insert(qualify(&v.name.name), v.fields.len());
                            }
                        }
                    }
                    Item::Effect(e) => {
                        for op in &e.ops {
                            ops.insert(
                                (qualify(&e.name.name), op.name.name.clone()),
                                (op.resource_param, op.mode),
                            );
                        }
                    }
                    Item::Test(t) => tests.push(TestSlot { module: m, def: t }),
                    // A law is not a global and not a test: `ply-prove` evaluates its body through
                    // `eval_expr_for_test`, with its binders bound to generated values.
                    Item::Law(_) | Item::Derive(_) | Item::EffectSet(_) => {}
                }
            }
        }

        Interp {
            program,
            resolved,
            check,
            globals,
            memo: Memo::default(),
            ctors,
            ops,
            tests,
            handlers: Vec::new(),
            regions: TaskRegions::new(),
            region_kinds: crate::region_kind::Kinds::default(),
            trace: Trace::new(),
            module: 0,
            calls: Vec::new(),
            max_calls: DEFAULT_MAX_CALLS,
            binding: None,
        }
    }

    /// The run's host boundary.
    pub fn set_host_binding(&mut self, binding: Arc<HostBinding>) {
        self.binding = Some(binding);
    }

    pub fn with_max_calls(mut self, max_calls: usize) -> Self {
        self.set_max_calls(max_calls);
        self
    }

    /// The same bound, moved on an evaluator that already exists.
    pub fn set_max_calls(&mut self, max_calls: usize) {
        self.max_calls = max_calls.max(1);
    }

    /// The atoms this engine performed at the last entry point.
    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    pub fn program(&self) -> &'a Program {
        self.program
    }

    pub fn cells(&self) -> &Arena {
        self.regions.arena()
    }

    pub fn cells_mut(&mut self) -> &mut Arena {
        self.regions.arena_mut()
    }

    pub fn regions(&self) -> &TaskRegions {
        &self.regions
    }

    /// Every subsequent entry point resets to this stack's fixture rather than to an empty one.
    pub fn set_regions(&mut self, regions: TaskRegions) {
        self.regions = regions;
    }

    pub fn check(&self) -> Option<&'a CheckOutput> {
        self.check
    }

    pub fn test_count(&self) -> usize {
        self.tests.len()
    }

    pub fn test_name(&self, index: usize) -> Option<&'a str> {
        self.tests.get(index).map(|t| t.def.name.as_str())
    }

    pub fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic> {
        let (module, body) = {
            let slot = self.tests.get(index).ok_or_else(|| {
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!(
                        "no test at index {index}; the program defines {}",
                        self.tests.len()
                    ),
                )
                .primary(Span::DUMMY, "requested test does not exist")
            })?;
            (slot.module, &slot.def.body)
        };
        self.reset();
        self.module = module;
        let outcome = self.eval(body, &Env::empty()).map(|_| ());
        self.end_entry_point();
        outcome
    }

    /// Runs the `ordinal`-th test declared by `module`.
    pub fn eval_test_in(&mut self, module: &Symbol, ordinal: usize) -> Result<(), Diagnostic> {
        let program = self.program;
        let found = self
            .tests
            .iter()
            .filter(|t| program.modules[t.module].name.as_symbol() == module)
            .nth(ordinal)
            .map(|slot| (slot.module, &slot.def.body));
        let Some((owner, body)) = found else {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("module `{module}` has no test at position {ordinal}"),
            )
            .primary(Span::DUMMY, "this test's module was not parsed")
            .note("run `ply cache clear`, or pass `--no-incremental`"));
        };
        self.reset();
        self.module = owner;
        let outcome = self.eval(body, &Env::empty()).map(|_| ());
        self.end_entry_point();
        outcome
    }

    pub fn eval_expr_for_test(&mut self, e: &Expr) -> Result<Value, Diagnostic> {
        self.reset();
        let outcome = self.eval(e, &Env::empty());
        self.end_entry_point();
        outcome
    }

    /// Closes every region the run left open.
    fn end_entry_point(&mut self) {
        self.regions.close_program_regions();
    }

    /// `name` is the program-wide name — `app.main`, not `main`.
    pub fn call(&mut self, name: &str, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
        // Both engines, at the same point and with the same message, or `--engine both` reports the
        // refusal as a divergence.
        let boundary = crate::escape::Boundary::EntryPoint { name };
        for arg in &args {
            crate::escape::check(&boundary, arg, span)?;
        }
        self.call_within(name, args, span)
    }

    /// The same call **without** the entry-point boundary check, for a caller that is not an entry
    /// point.
    pub(crate) fn call_within(
        &mut self,
        name: &str,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        self.reset();
        let sym = Symbol::new(name);
        let f = self.globals.get(&sym).cloned().ok_or_else(|| {
            Diagnostic::error(codes::UNKNOWN_NAME, format!("no definition named `{name}`"))
                .primary(span, "not defined in this program")
                .note("this name is program-wide: `store.orders.place`, not `place`")
        })?;
        let outcome = self.apply(f, args, span);
        self.end_entry_point();
        outcome
    }

    /// A previous failure can leave frames installed; nothing survives from one entry point to the
    /// next.
    fn reset(&mut self) {
        self.handlers.clear();
        self.calls.clear();
        self.trace.clear();
        self.module = 0;
        self.regions.reset();
    }

    /// Resolution already decided what this denotes; nothing here re-derives it.
    fn global(&self, ns: Namespace, q: &QName) -> Option<Symbol> {
        if q.is_bare() {
            return self
                .resolved
                .scopes
                .get(self.module)
                .and_then(|scope| scope.get(ns, q.symbol()))
                .map(|b| b.qualified.clone());
        }
        self.resolved
            .lookup(self.module, ns, q)
            .ok()
            .map(|b| b.qualified.clone())
    }

    /// The program-wide name a constructor reference denotes, falling back to the prelude's — which
    /// no module declares, so nothing qualifies it.
    fn ctor_name(&self, q: &QName) -> Option<Symbol> {
        match self.global(Namespace::Value, q) {
            Some(name) => Some(name),
            None if q.is_bare() && self.ctors.contains_key(q.symbol()) => Some(q.symbol().clone()),
            None => None,
        }
    }

    fn enter(&mut self, name: Option<&Symbol>, span: Span) -> Result<(), Diagnostic> {
        if self.calls.len() >= self.max_calls {
            return Err(self.err_recursion_limit(span));
        }
        self.calls.push(name.cloned());
        Ok(())
    }

    fn leave(&mut self) {
        self.calls.pop();
    }

    fn err_recursion_limit(&self, span: Span) -> Diagnostic {
        let innermost: Vec<Option<Symbol>> =
            self.calls.iter().rev().take(NAMED_CALLS).cloned().collect();
        limit::err_recursion_limit(span, NESTED_CALLS, self.max_calls, &innermost)
    }

    /// Every arm that needs more than a handful of locals is out of line: an unoptimized frame is
    /// sized for the union of all arms, and this function sits on the recursion path.
    fn eval(&mut self, e: &Expr, env: &Env) -> Result<Value, Diagnostic> {
        grow(|| self.eval_node(e, env))
    }

    fn eval_node(&mut self, e: &Expr, env: &Env) -> Result<Value, Diagnostic> {
        match &e.kind {
            ExprKind::Lit(lit) => Ok(literal(lit)),
            ExprKind::Var(q) => self.lookup(q, env),
            ExprKind::Unary { op, operand } => self.eval_unary(*op, operand, env, e.span),
            ExprKind::Binary { op, lhs, rhs } => self.eval_binary(*op, lhs, rhs, env, e.span),
            ExprKind::Lambda { params, body } => Ok(eval_lambda(params, body, env, self.module)),
            ExprKind::App { func, args, .. } => self.eval_app(func, args, env, e.span),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let c = self.eval(cond, env)?;
                if c.as_bool(cond.span, "an `if` condition")? {
                    self.eval(then_branch, env)
                } else {
                    self.eval(else_branch, env)
                }
            }
            ExprKind::Match { scrutinee, arms } => self.eval_match(scrutinee, arms, env),
            ExprKind::Block { stmts, tail } => self.eval_block(stmts, tail.as_deref(), env),
            ExprKind::Record { fields } => self.eval_record(fields, env),
            // Both engines evaluate a record update through the ordinary record path, because
            // expansion makes it the same tree.
            ExprKind::RecordUpdate { .. } => unreachable!(
                "`{{..b, f: e}}` is expanded away by `ply_syntax::parse_module`; the guard is \
                 `no_record_update_survives_parse_module_anywhere_in_the_tree`"
            ),
            // Unreachable for the same reason: the tree-walker and the machine both evaluate the
            // `match` `?` became, so there is no second implementation of an early exit here to
            // disagree with the other engine's.
            ExprKind::Try { .. } => unreachable!(
                "`e?` is expanded away by `ply_syntax::parse_module`; the guard is \
                 `no_try_survives_parse_module_anywhere_in_the_tree`"
            ),
            ExprKind::Field { base, field } => self.eval_field(base, field, env),
            ExprKind::List { items } => self.eval_list(items, env),
            ExprKind::Perform {
                effect,
                op,
                resource,
                args,
            } => self.eval_perform(effect, op, resource.as_ref(), args, env, e.span),
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => self.eval_handle(body, clauses, return_clause.as_deref(), env),
            ExprKind::WithCell {
                init, binder, body, ..
            } => self.eval_with_cell(init, binder, body, env, e.span),
            ExprKind::WithRegion { body, .. } => self.in_region(e.span, |me| me.eval(body, env)),
            // Refused before the body runs, for the reason a general clause is: running one unnamed
            // interleaving would be a plausible wrong answer and the result cache would keep it.
            ExprKind::Simulate { .. } => Err(crate::differential::machine_only_region(e.span)),
        }
    }

    #[inline(never)]
    fn eval_unary(
        &mut self,
        op: UnOp,
        operand: &Expr,
        env: &Env,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let v = self.eval(operand, env)?;
        // One implementation for both engines, so `--engine both` cannot report a divergence that
        // is the two of them disagreeing about a `-0.0`.
        crate::machine::apply_unary(op, &v, operand.span, span)
    }

    #[inline(never)]
    fn eval_app(
        &mut self,
        func: &Expr,
        args: &[Expr],
        env: &Env,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let callee = self.eval(func, env)?;
        let mut argv = Vec::with_capacity(args.len());
        for a in args {
            argv.push(self.eval(a, env)?);
        }
        self.apply(callee, argv, span)
    }

    #[inline(never)]
    fn eval_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        env: &Env,
    ) -> Result<Value, Diagnostic> {
        let v = self.eval(scrutinee, env)?;
        for arm in arms {
            let mut arm_env = env.clone();
            if !self.match_pattern(&arm.pat, &v, &mut arm_env)? {
                continue;
            }
            if let Some(guard) = &arm.guard {
                let g = self.eval(guard, &arm_env)?;
                if !g.as_bool(guard.span, "a match guard")? {
                    continue;
                }
            }
            return self.eval(&arm.body, &arm_env);
        }
        Err(err_non_exhaustive(scrutinee.span, &v))
    }

    #[inline(never)]
    fn eval_block(
        &mut self,
        stmts: &[Stmt],
        tail: Option<&Expr>,
        env: &Env,
    ) -> Result<Value, Diagnostic> {
        let mut scope = env.clone();
        for stmt in stmts {
            match stmt {
                Stmt::Let {
                    pat, value, span, ..
                } => {
                    let v = self.eval(value, &scope)?;
                    let mut next = scope.clone();
                    if !self.match_pattern(pat, &v, &mut next)? {
                        return Err(err_let_mismatch(*span, &v));
                    }
                    scope = next;
                }
                Stmt::Expr(expr) => {
                    self.eval(expr, &scope)?;
                }
            }
        }
        match tail {
            Some(t) => self.eval(t, &scope),
            None => Ok(Value::Unit),
        }
    }

    #[inline(never)]
    fn eval_record(&mut self, fields: &[(Ident, Expr)], env: &Env) -> Result<Value, Diagnostic> {
        let mut map = BTreeMap::new();
        for (name, value) in fields {
            let v = self.eval(value, env)?;
            map.insert(name.name.clone(), v);
        }
        Ok(Value::Record(Arc::new(map)))
    }

    #[inline(never)]
    fn eval_field(&mut self, base: &Expr, field: &Ident, env: &Env) -> Result<Value, Diagnostic> {
        let v = self.eval(base, env)?;
        match &v {
            Value::Record(fields) => match fields.get(&field.name) {
                Some(v) => Ok(v.clone()),
                None => Err(err_no_such_field(field, fields)),
            },
            other => Err(type_error(base.span, "field access", "a record", other)),
        }
    }

    #[inline(never)]
    fn eval_list(&mut self, items: &[Expr], env: &Env) -> Result<Value, Diagnostic> {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(self.eval(item, env)?);
        }
        Ok(Value::list(out))
    }

    #[inline(never)]
    fn eval_perform(
        &mut self,
        effect: &QName,
        op: &Ident,
        resource: Option<&Ident>,
        args: &[Expr],
        env: &Env,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let name = self.effect_name(effect);
        let mut argv = Vec::with_capacity(args.len());
        for a in args {
            argv.push(self.eval(a, env)?);
        }
        self.perform(
            &name,
            &op.name,
            resource.map(|r| r.name.clone()),
            argv,
            span,
        )
    }

    /// An effect no module declares keeps the name as written.
    fn effect_name(&self, effect: &QName) -> Symbol {
        self.global(Namespace::Effect, effect)
            .unwrap_or_else(|| effect.symbol().clone())
    }

    #[inline(never)]
    fn eval_handle(
        &mut self,
        body: &Expr,
        clauses: &[HandleClause],
        return_clause: Option<&ReturnClause>,
        env: &Env,
    ) -> Result<Value, Diagnostic> {
        // Refused before the body runs, not when the clause is reached: a program that ran halfway
        // and then failed has already written to the world, and the refusal is about what this
        // engine can express at all.
        if let Some(c) = clauses.iter().find(|c| c.resume.is_some()) {
            return Err(crate::differential::machine_only_clause(
                c.span,
                &self.effect_name(&c.effect),
                c.op.name.as_str(),
            ));
        }
        let effects: Vec<Symbol> = clauses
            .iter()
            .map(|c| self.effect_name(&c.effect))
            .collect();
        let mark = self.handlers.len();
        self.handlers.push(HandlerFrame {
            clauses: Arc::new(clauses.to_vec()),
            effects: Arc::new(effects),
            env: env.clone(),
            module: self.module,
            calls: self.calls.len(),
        });
        let result = self.eval(body, env);
        self.handlers.truncate(mark);
        let value = result?;

        match return_clause {
            Some(rc) => {
                let scope = env.bind(rc.binder.name.clone(), value);
                grow(|| self.eval(&rc.body, &scope))
            }
            None => Ok(value),
        }
    }

    /// The kind of the region opened at `span`, and `None` when that span opens no region of its
    /// own.
    fn region_kind(&self, span: Span) -> Option<RegionKind> {
        self.region_kinds().at(span).map(|region| region.kind)
    }

    /// This program's region kinds, inferring them if nothing has yet.
    pub fn region_kinds(&self) -> &crate::region_kind::Regions {
        self.region_kinds
            .get_or_init(|| crate::region_kind::infer(self.program, self.resolved))
    }

    /// See [`crate::Machine::shared_region_kinds`].
    pub fn shared_region_kinds(&self) -> crate::region_kind::Kinds {
        crate::region_kind::Kinds::clone(&self.region_kinds)
    }

    /// See [`crate::Machine::share_region_kinds`], whose contract this shares: `kinds` must be an
    /// answer about the same program.
    pub fn share_region_kinds(&mut self, kinds: crate::region_kind::Kinds) {
        self.region_kinds = kinds;
    }

    /// Opens the region, runs `body` in it and closes it — on the error path too, because a
    /// diagnostic propagating out of a region body is that region's lexical end just as much as a
    /// value is.
    fn in_region(
        &mut self,
        span: Span,
        f: impl FnOnce(&mut Self) -> Result<Value, Diagnostic>,
    ) -> Result<Value, Diagnostic> {
        let Some(kind) = self.region_kind(span) else {
            return f(self);
        };
        let region = self.regions.open_region(kind, span);
        let out = f(self);
        self.regions.close_region(region);
        out
    }

    #[inline(never)]
    fn eval_with_cell(
        &mut self,
        init: &Expr,
        binder: &Ident,
        body: &Expr,
        env: &Env,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let initial = self.eval(init, env)?;
        self.in_region(span, |me| {
            let Some(cell) = me.regions.alloc(initial) else {
                return Err(crate::handler::err_cells_exhausted(body.span));
            };
            let scope = env.bind(binder.name.clone(), Value::Cell(cell));
            me.eval(body, &scope)
        })
    }

    /// Locals, then the module's own items and its selective imports, then the prelude — the
    /// resolution order the whole language is specified in.
    fn lookup(&self, q: &QName, env: &Env) -> Result<Value, Diagnostic> {
        // The tree-walker never releases a binding — it runs no reference counting — so a released
        // slot here would be one the machine put in a scope this engine then read, which cannot
        // happen and is reported rather than silently resolved to something else.
        if q.is_bare()
            && let Some(slot) = env.lookup(q.symbol())
        {
            return match slot {
                crate::env::Slot::Live(v) => Ok(v.clone()),
                crate::env::Slot::Released => Err(Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!("`{q}` was read after reference counting dropped it"),
                )
                .primary(q.span, "this binding was released before this read")),
            };
        }
        if let Some(name) = self.global(Namespace::Value, q)
            && let Some(v) = self.globals.get(&name)
        {
            return Ok(v.clone());
        }
        if let Some(name) = self.ctor_name(q)
            && let Some(&arity) = self.ctors.get(&name)
        {
            return Ok(ctor_value(&name, arity));
        }
        if q.is_bare()
            && let Some(b) = Builtin::from_name(q.symbol())
        {
            return Ok(Value::builtin(b));
        }
        Err(err_unknown_name(q))
    }

    pub(crate) fn apply(
        &mut self,
        callee: Value,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let Value::Closure(closure) = &callee else {
            return Err(err_not_a_function(span, &callee));
        };

        match &closure.kind {
            ClosureKind::Fn {
                params,
                body,
                env,
                module,
            } => {
                if params.len() != args.len() {
                    return Err(arity_error(
                        span,
                        &closure.describe(),
                        params.len(),
                        args.len(),
                    ));
                }
                let memo = match (params.is_empty(), &closure.name) {
                    (true, Some(name)) => match self.memo.lookup(self.check, name) {
                        Lookup::Known(value) => return Ok(value),
                        Lookup::Remember => Some(name.clone()),
                        Lookup::Ignore => None,
                    },
                    _ => None,
                };
                let mut scope = env.clone();
                for (p, v) in params.iter().zip(args) {
                    scope = scope.bind(p.clone(), v);
                }
                let body = body.clone();
                // The body's bare names mean what they meant where it was written, which is not
                // where it is being called from.
                let caller = std::mem::replace(&mut self.module, *module);
                let entered = self.enter(closure.name.as_ref(), span);
                let result = match entered {
                    Err(d) => Err(d),
                    Ok(()) => {
                        let r = grow(|| self.eval(&body, &scope));
                        self.leave();
                        r
                    }
                };
                self.module = caller;
                if let (Some(name), Ok(value)) = (&memo, &result) {
                    self.memo.remember(name, value);
                }
                result
            }
            // Only a caller mixing the two engines' values reaches this, and answering with the
            // wrong function would be worse than refusing.
            ClosureKind::Code { .. } => Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("{} was compiled for the machine engine", closure.describe()),
            )
            .primary(span, "this function cannot run on the tree-walker")
            .note("run this program with `--engine machine`")),
            ClosureKind::Ctor { name, arity } => {
                if *arity != args.len() {
                    return Err(arity_error(span, &format!("`{name}`"), *arity, args.len()));
                }
                Ok(Value::Ctor {
                    name: name.clone(),
                    args: Arc::new(args),
                })
            }
            ClosureKind::Builtin(b) => {
                let b = *b;
                self.call_builtin(b, args, span)
            }
        }
    }

    /// Walks the handler stack inward-out.
    fn perform(
        &mut self,
        effect: &Symbol,
        op: &Symbol,
        resource: Option<Symbol>,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let decl = op_decl(&self.ops, effect, op);
        check_operation(decl, effect, op, resource.is_some(), span)?;
        if let Some(atom) = performed_atom(effect, resource.as_ref(), decl) {
            self.trace.record(atom);
        }

        for i in (0..self.handlers.len()).rev() {
            let clauses = self.handlers[i].clauses.clone();
            let effects = self.handlers[i].effects.clone();
            let Some(index) = clauses
                .iter()
                .zip(effects.iter())
                .position(|(c, e)| clause_matches(c, e, effect, op, &resource))
            else {
                continue;
            };

            let clause = &clauses[index];
            if clause.params.len() != args.len() {
                let what = format!("the handler clause for `{effect}.{op}`");
                return Err(arity_error(span, &what, clause.params.len(), args.len()));
            }

            let mut scope = self.handlers[i].env.clone();
            for (p, v) in clause.params.iter().zip(args) {
                scope = scope.bind(p.name.clone(), v);
            }

            let handler_module = self.handlers[i].module;
            let installed_at = self.handlers[i].calls;
            let outer = self.handlers.split_off(i);
            let performer = std::mem::replace(&mut self.module, handler_module);
            // The clause runs below its own handler, so the calls the body made since the handler
            // was installed are not pending while it runs — they are held aside exactly as the
            // machine's `capture` holds them, and put back when the value returns to the perform
            // site.
            let pending = self.calls.split_off(installed_at);
            let result = grow(|| self.eval(&clause.body, &scope));
            self.calls.truncate(installed_at);
            self.calls.extend(pending);
            self.module = performer;
            self.handlers.truncate(i);
            self.handlers.extend(outer);
            return result;
        }

        if let Some(binding) = &self.binding
            && let Some(path) = binding.would_serve(effect, op, resource.as_ref())
        {
            let operation = operation_label(effect, op, resource.as_ref());
            return Err(if binding.is_hermetic() {
                err_hermetic(span, &operation, path)
            } else {
                err_machine_only_host(span, &operation, path)
            });
        }
        Err(err_unhandled(span, effect, op, resource.as_ref()))
    }

    fn eval_binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        env: &Env,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        if let BinOp::And | BinOp::Or = op {
            let l = self
                .eval(lhs, env)?
                .as_bool(lhs.span, "a logical operator")?;
            if l == matches!(op, BinOp::Or) {
                return Ok(Value::Bool(l));
            }
            let r = self
                .eval(rhs, env)?
                .as_bool(rhs.span, "a logical operator")?;
            return Ok(Value::Bool(r));
        }

        let l = self.eval(lhs, env)?;
        let r = self.eval(rhs, env)?;
        strict_binary(op, &l, &r, lhs.span, rhs.span, span)
    }

    fn match_pattern(
        &mut self,
        pat: &Pattern,
        value: &Value,
        env: &mut Env,
    ) -> Result<bool, Diagnostic> {
        Ok(match &pat.kind {
            PatternKind::Wildcard => true,
            PatternKind::Var(id) => {
                // A nullary constructor written bare is indistinguishable from a binder in the AST,
                // so the constructor table decides.
                let declared = self.ctor_name(&QName::bare(id.clone()));
                match declared.as_ref().and_then(|name| self.ctors.get(name)) {
                    Some(0) => {
                        let ctor = declared.expect("a hit came from a resolved name");
                        matches!(value, Value::Ctor { name, args }
                            if *name == ctor && args.is_empty())
                    }
                    _ => {
                        *env = env.bind(id.name.clone(), value.clone());
                        true
                    }
                }
            }
            PatternKind::Lit(lit) => lit_matches(lit, value),
            PatternKind::Ctor { name, args } => match value {
                Value::Ctor {
                    name: vname,
                    args: vargs,
                } => {
                    let expected = self.ctor_name(name);
                    if expected.as_ref() != Some(vname) || vargs.len() != args.len() {
                        return Ok(false);
                    }
                    for (p, v) in args.iter().zip(vargs.iter()) {
                        if !self.match_pattern(p, v, env)? {
                            return Ok(false);
                        }
                    }
                    true
                }
                _ => false,
            },
            PatternKind::Record { fields, rest } => match value {
                Value::Record(map) => {
                    if !*rest && map.len() != fields.len() {
                        return Ok(false);
                    }
                    for (name, p) in fields {
                        let Some(v) = map.get(&name.name).cloned() else {
                            return Ok(false);
                        };
                        if !self.match_pattern(p, &v, env)? {
                            return Ok(false);
                        }
                    }
                    true
                }
                _ => false,
            },
            PatternKind::List { items, rest } => match value {
                Value::List(xs) => {
                    let fits = match rest {
                        Some(_) => xs.len() >= items.len(),
                        None => xs.len() == items.len(),
                    };
                    if !fits {
                        return Ok(false);
                    }
                    for (p, v) in items.iter().zip(xs.iter()) {
                        if !self.match_pattern(p, v, env)? {
                            return Ok(false);
                        }
                    }
                    match rest {
                        Some(rest) => {
                            let tail = Value::list(xs[items.len()..].to_vec());
                            self.match_pattern(rest, &tail, env)?
                        }
                        None => true,
                    }
                }
                _ => false,
            },
        })
    }
}

/// What the two engines share so that a mis-declared operation reads the same whichever one ran it.
pub(crate) fn op_decl(ops: &OpTable, effect: &Symbol, op: &Symbol) -> OpDecl {
    match ops.get(&(effect.clone(), op.clone())) {
        Some(&(resource_param, mode)) => OpDecl::Declared {
            resource_param,
            mode,
        },
        None if ops.keys().any(|(e, _)| e == effect) => OpDecl::NoSuchOp,
        None => OpDecl::UnknownEffect,
    }
}

pub(crate) fn literal(lit: &Lit) -> Value {
    match lit {
        Lit::Int(i) => Value::Int(*i),
        Lit::Bool(b) => Value::Bool(*b),
        Lit::Str(s) => Value::str(s),
        Lit::Bytes(b) => Value::bytes(b),
        Lit::Float(f) => Value::Float(*f),
        Lit::Decimal { mantissa, scale } => Value::Decimal(decimal_lit(*mantissa, *scale)),
        Lit::Unit => Value::Unit,
    }
}

/// A `Decimal` literal's value.
pub(crate) fn decimal_lit(mantissa: i128, scale: u32) -> Decimal {
    Decimal::try_from_i128_with_scale(mantissa, scale).unwrap_or(Decimal::ZERO)
}

/// A literal pattern against a value, shared by both engines so a `--engine both` divergence cannot
/// be the two of them disagreeing about a NaN.
pub(crate) fn lit_matches(lit: &Lit, value: &Value) -> bool {
    match (lit, value) {
        (Lit::Int(a), Value::Int(b)) => a == b,
        (Lit::Bool(a), Value::Bool(b)) => a == b,
        (Lit::Str(a), Value::Str(b)) => a.as_str() == b.as_ref(),
        (Lit::Bytes(a), Value::Bytes(b)) => a.as_slice() == b.as_ref(),
        (Lit::Float(a), Value::Float(b)) => a == b,
        // By numeric value, matching `==`: a `1.50m` pattern matches `1.5m`.
        (Lit::Decimal { mantissa, scale }, Value::Decimal(b)) => {
            decimal_lit(*mantissa, *scale) == *b
        }
        (Lit::Unit, Value::Unit) => true,
        _ => false,
    }
}

#[inline(never)]
fn eval_lambda(params: &[Param], body: &Expr, env: &Env, module: usize) -> Value {
    Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Fn {
            params: params.iter().map(|p| p.name.name.clone()).collect(),
            body: Arc::new(body.clone()),
            env: env.clone(),
            module,
        },
    }))
}

/// Constructor values kept per thread.
const CTOR_CACHE_KEEP: usize = 4096;

thread_local! {
    /// Keyed by the constructor's program-wide name, holding the arity the value was built at: two
    /// programs run on one thread can spell one name with two arities, and the second must not read
    /// the first's value.
    static CTOR_VALUES: RefCell<FxHashMap<Symbol, (usize, Value)>> =
        RefCell::new(FxHashMap::default());
}

/// The value a mention of a constructor evaluates to: one per constructor per thread, built on
/// first mention.
pub(crate) fn ctor_value(name: &Symbol, arity: usize) -> Value {
    let fresh = || {
        if arity == 0 {
            Value::ctor(name.clone(), Vec::new())
        } else {
            Value::Closure(Arc::new(Closure {
                name: Some(name.clone()),
                kind: ClosureKind::Ctor {
                    name: name.clone(),
                    arity,
                },
            }))
        }
    };
    // `try_with`, because a `Value` dropped during thread-local teardown can reach here after the
    // cache is gone, and building a fresh one is the right answer there rather than an abort.
    CTOR_VALUES
        .try_with(|cache| {
            let mut cache = cache.borrow_mut();
            match cache.get(name) {
                Some((at, value)) if *at == arity => value.clone(),
                Some(_) => {
                    let value = fresh();
                    cache.insert(name.clone(), (arity, value.clone()));
                    value
                }
                None => {
                    let value = fresh();
                    if cache.len() < CTOR_CACHE_KEEP {
                        cache.insert(name.clone(), (arity, value.clone()));
                    }
                    value
                }
            }
        })
        .unwrap_or_else(|_| fresh())
}

/// A clause without a resource label handles every resource of its operation; an operation declared
/// without `[r]` has exactly one anyway.
fn clause_matches(
    c: &HandleClause,
    clause_effect: &Symbol,
    effect: &Symbol,
    op: &Symbol,
    resource: &Option<Symbol>,
) -> bool {
    clause_effect == effect
        && c.op.name == *op
        && match (&c.resource, resource) {
            (None, _) => true,
            (Some(cr), Some(r)) => cr.name == *r,
            (Some(_), None) => false,
        }
}

#[inline(never)]
pub(crate) fn strict_binary(
    op: BinOp,
    l: &Value,
    r: &Value,
    lspan: Span,
    rspan: Span,
    span: Span,
) -> Result<Value, Diagnostic> {
    match op {
        BinOp::Eq => Ok(Value::Bool(values_equal(l, r, span)?)),
        BinOp::Ne => Ok(Value::Bool(!values_equal(l, r, span)?)),
        BinOp::Concat => {
            let a = l.as_str(lspan, "`++`")?;
            let b = r.as_str(rspan, "`++`")?;
            Ok(Value::str(format!("{a}{b}")))
        }
        // `Float` answers by IEEE, where `NaN < x` and `NaN >= x` are both false — so a comparison
        // is not the negation of its converse.
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            if let (Value::Float(a), Value::Float(b)) = (l, r) {
                return Ok(Value::Bool(match op {
                    BinOp::Lt => a < b,
                    BinOp::Le => a <= b,
                    BinOp::Gt => a > b,
                    _ => a >= b,
                }));
            }
            let ordering = match (l, r) {
                (Value::Int(a), Value::Int(b)) => a.cmp(b),
                (Value::Str(a), Value::Str(b)) => a.as_ref().cmp(b.as_ref()),
                (Value::Decimal(a), Value::Decimal(b)) => a.cmp(b),
                (Value::Int(_) | Value::Str(_) | Value::Decimal(_) | Value::Float(_), other) => {
                    return Err(type_error(rspan, "a comparison", l.type_name(), other));
                }
                (other, _) => {
                    return Err(type_error(
                        lspan,
                        "a comparison",
                        "Int, String, Float or Decimal",
                        other,
                    ));
                }
            };
            Ok(Value::Bool(match op {
                BinOp::Lt => ordering.is_lt(),
                BinOp::Le => ordering.is_le(),
                BinOp::Gt => ordering.is_gt(),
                _ => ordering.is_ge(),
            }))
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            match (l, r) {
                (Value::Float(a), Value::Float(b)) => return float_arithmetic(op, *a, *b, span),
                (Value::Decimal(a), Value::Decimal(b)) => {
                    return decimal_arithmetic(op, *a, *b, rspan, span);
                }
                _ => {}
            }
            let a = l.as_int(lspan, "arithmetic")?;
            let b = r.as_int(rspan, "arithmetic")?;
            let (result, what) = match op {
                BinOp::Add => (a.checked_add(b), "addition"),
                BinOp::Sub => (a.checked_sub(b), "subtraction"),
                BinOp::Mul => (a.checked_mul(b), "multiplication"),
                BinOp::Div if b == 0 => return Err(err_zero_divisor(rspan, "division")),
                BinOp::Div => (a.checked_div(b), "division"),
                _ if b == 0 => return Err(err_zero_divisor(rspan, "remainder")),
                _ => (a.checked_rem(b), "remainder"),
            };
            match result {
                Some(n) => Ok(Value::Int(n)),
                None => Err(err_overflow(span, what, a, b)),
            }
        }
        BinOp::And | BinOp::Or => Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "internal error: a short-circuiting operator reached strict evaluation",
        )
        .primary(span, "please report this")),
    }
}

/// IEEE-754, unmodified.
fn float_arithmetic(op: BinOp, a: f64, b: f64, span: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Float(match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => a / b,
        BinOp::Rem => a % b,
        _ => {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "internal error: a non-arithmetic operator reached float arithmetic",
            )
            .primary(span, "please report this"));
        }
    }))
}

/// Exact, or a diagnostic.
fn decimal_arithmetic(
    op: BinOp,
    a: Decimal,
    b: Decimal,
    rspan: Span,
    span: Span,
) -> Result<Value, Diagnostic> {
    let (result, what) = match op {
        BinOp::Add => (a.checked_add(b), "addition"),
        BinOp::Sub => (a.checked_sub(b), "subtraction"),
        // Exact while the result's scale fits, and half-to-even at scale 28 otherwise.
        BinOp::Mul => (a.checked_mul(b), "multiplication"),
        BinOp::Rem => {
            if b.is_zero() {
                return Err(err_zero_divisor(rspan, "remainder"));
            }
            (a.checked_rem(b), "remainder")
        }
        BinOp::Div => {
            return Err(Diagnostic::error(
                codes::DECIMAL_DIVISION,
                "`/` is not defined on `Decimal`",
            )
            .primary(span, "the exact quotient of two decimals is not a decimal")
            .note("call `decimal_div(a, b, scale, HalfEven)` and say how to round"));
        }
        _ => {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "internal error: a non-arithmetic operator reached decimal arithmetic",
            )
            .primary(span, "please report this"));
        }
    };
    match result {
        Some(d) => Ok(Value::Decimal(d)),
        None => Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`Decimal` overflow in {what}"),
        )
        .primary(
            span,
            format!("{a} and {b} need more than 96 bits of mantissa"),
        )
        .note("`Decimal` is exact and bounded; it will not round to make room")),
    }
}

pub(crate) fn arity_error(span: Span, what: &str, expected: usize, got: usize) -> Diagnostic {
    Diagnostic::error(
        codes::ARITY_MISMATCH,
        format!(
            "{what} takes {expected} argument{}, but {got} were given",
            plural(expected)
        ),
    )
    .primary(span, format!("{got} argument{} here", plural(got)))
}

#[cold]
#[inline(never)]
pub(crate) fn err_unknown_name(q: &QName) -> Diagnostic {
    Diagnostic::error(
        codes::UNKNOWN_NAME,
        format!("cannot find `{q}` in this scope"),
    )
    .primary(q.span, "not bound here")
}

#[cold]
#[inline(never)]
pub(crate) fn err_not_a_function(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NOT_A_FUNCTION,
        format!("cannot call a value of type {}", v.type_name()),
    )
    .primary(span, format!("this is {}", v.render()))
}

#[cold]
#[inline(never)]
pub(crate) fn err_non_exhaustive(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NON_EXHAUSTIVE_MATCH,
        "no match arm applied to the scrutinee",
    )
    .primary(span, format!("this evaluated to {}", v.render()))
    .note("add an arm covering this value, or a `_` catch-all")
}

#[cold]
#[inline(never)]
pub(crate) fn err_let_mismatch(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NON_EXHAUSTIVE_MATCH,
        "`let` pattern did not match the bound value",
    )
    .primary(span, format!("value was {}", v.render()))
    .note("use `match` when the pattern can fail")
}

#[cold]
#[inline(never)]
pub(crate) fn err_no_such_field(field: &Ident, fields: &BTreeMap<Symbol, Value>) -> Diagnostic {
    let known: Vec<String> = fields.keys().map(|k| format!("`{k}`")).collect();
    Diagnostic::error(
        codes::UNKNOWN_NAME,
        format!("record has no field `{}`", field.name),
    )
    .primary(field.span, "no such field")
    .note(if known.is_empty() {
        "the record is empty".to_string()
    } else {
        format!("available fields: {}", known.join(", "))
    })
}

#[cold]
#[inline(never)]
pub(crate) fn err_zero_divisor(span: Span, what: &str) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, format!("{what} by zero"))
        .primary(span, "this divisor is 0")
}

#[cold]
#[inline(never)]
pub(crate) fn err_overflow(span: Span, what: &str, a: i64, b: i64) -> Diagnostic {
    let detail = if what == "negation" {
        format!("-{a} does not fit in Int")
    } else {
        format!("{a} and {b} overflow Int")
    };
    Diagnostic::error(codes::RUNTIME_ERROR, format!("integer overflow in {what}"))
        .primary(span, detail)
}

#[cold]
#[inline(never)]
pub(crate) fn err_unhandled(
    span: Span,
    effect: &Symbol,
    op: &Symbol,
    resource: Option<&Symbol>,
) -> Diagnostic {
    let label = match resource {
        Some(r) => format!("{effect}.{op}[{r}]"),
        None => format!("{effect}.{op}"),
    };
    Diagnostic::error(codes::UNHANDLED_EFFECT, format!("no handler for `{label}`"))
        .primary(span, "performed here with no enclosing handler")
        .note("wrap this in a `handle ... with { ... }` that names the operation")
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name no other test in this module uses, because the cache is thread-local and a test
    /// binary may run two tests on one thread.
    fn name(s: &str) -> Symbol {
        Symbol::new(format!("interp::tests::{s}"))
    }

    fn ctor_args(v: &Value) -> Arc<Vec<Value>> {
        match v {
            Value::Ctor { args, .. } => args.clone(),
            other => panic!("expected a `Ctor`, found a `{}`", other.type_name()),
        }
    }

    fn closure_of(v: &Value) -> Arc<Closure> {
        match v {
            Value::Closure(c) => c.clone(),
            other => panic!("expected a `Closure`, found a `{}`", other.type_name()),
        }
    }

    /// "Built once" as identity rather than as an allocation count: an equal value would mean it
    /// was rebuilt.
    #[test]
    fn a_nullary_constructors_value_is_built_once_per_thread() {
        let n = name("Red");
        let first = ctor_value(&n, 0);
        let second = ctor_value(&n, 0);
        assert!(
            Arc::ptr_eq(&ctor_args(&first), &ctor_args(&second)),
            "two mentions of one nullary constructor answered with two values, so \
             `ctor_value` built the second rather than sharing the first"
        );
    }

    #[test]
    fn a_constructor_closure_is_built_once_per_thread() {
        let n = name("Box");
        let first = closure_of(&ctor_value(&n, 1));
        let second = closure_of(&ctor_value(&n, 1));
        assert!(
            Arc::ptr_eq(&first, &second),
            "two mentions of one constructor answered with two closures"
        );
        assert_eq!(first.arity(), 1);
    }

    /// The hazard a cache keyed by name has and a fresh build does not: two programs run on one
    /// thread can spell one constructor with two arities, and the second must not be handed the
    /// first's value.
    #[test]
    fn a_name_met_at_another_arity_is_not_answered_from_the_cache() {
        let n = name("Same");
        assert!(matches!(ctor_value(&n, 0), Value::Ctor { .. }));
        assert_eq!(closure_of(&ctor_value(&n, 2)).arity(), 2);
        assert_eq!(closure_of(&ctor_value(&n, 2)).arity(), 2);
        assert!(
            matches!(ctor_value(&n, 0), Value::Ctor { .. }),
            "the name went back to arity 0 and the cache still answered with the closure it \
             had been rebuilt at"
        );
    }

    #[test]
    fn a_shared_constructor_value_is_the_value_a_fresh_one_was() {
        let n = name("Green");
        let shared = ctor_value(&n, 0);
        let fresh = Value::ctor(n.clone(), Vec::new());
        assert!(
            values_equal(&shared, &fresh, Span::DUMMY).expect("two `Ctor`s compare"),
            "the shared value is not the value a mention used to build"
        );
        // A closure has no equality a program can ask for, so this is the statement
        // [`Value::builtin`]'s note rests on instead: the ordering that decides a `Map`'s key order
        // cannot separate two of them.
        let f = ctor_value(&name("Pair"), 2);
        let g = Value::Closure(Arc::new(Closure {
            name: Some(name("Pair")),
            kind: ClosureKind::Ctor {
                name: name("Pair"),
                arity: 2,
            },
        }));
        assert_eq!(f.cmp(&g), std::cmp::Ordering::Equal);
    }

    /// ADR 0019 §0.1 at this seam.
    #[test]
    fn a_cached_constructor_value_holds_nothing() {
        let held = ctor_value(&name("Empty"), 0);
        assert!(
            ctor_args(&held).is_empty(),
            "a cached nullary constructor is holding {} value(s)",
            ctor_args(&held).len()
        );
        for arity in 0..4 {
            let v = ctor_value(&name(&format!("Arity{arity}")), arity);
            assert!(
                !matches!(v, Value::Secret(_)),
                "`ctor_value` answered with a `Secret`, which would give a credential the \
                 lifetime of the cache"
            );
        }
    }

    /// What the cache trades: a `malloc`/`free` pair for a hash of the name and a refcount bump.
    #[test]
    #[ignore = "timing; run with `cargo test -p ply-eval --release --lib interp::tests::a_cached_mention_against_the_allocation_it_replaces -- --ignored --nocapture`"]
    fn a_cached_mention_against_the_allocation_it_replaces() {
        const MENTIONS: usize = 200_000;
        // A real constructor's program-wide name rather than this module's prefixed one: the cache
        // hashes the name, so its length is a cost.
        let n = Symbol::new("m.Red");
        let b = Symbol::new("m.Box");
        let per = |s: f64| 1e9 * s / MENTIONS as f64;
        let time = |f: &dyn Fn()| {
            let t = std::time::Instant::now();
            for _ in 0..MENTIONS {
                f();
            }
            t.elapsed().as_secs_f64()
        };
        let rebuild_closure = || {
            std::hint::black_box(Value::Closure(Arc::new(Closure {
                name: Some(b.clone()),
                kind: ClosureKind::Ctor {
                    name: b.clone(),
                    arity: 1,
                },
            })));
        };
        let arms: [(&str, &dyn Fn()); 4] = [
            ("nullary, cached", &|| {
                std::hint::black_box(ctor_value(&n, 0));
            }),
            ("nullary, rebuilt", &|| {
                std::hint::black_box(Value::ctor(n.clone(), Vec::new()));
            }),
            ("arity 1, cached", &|| {
                std::hint::black_box(ctor_value(&b, 1));
            }),
            ("arity 1, rebuilt", &rebuild_closure),
        ];
        for (_, arm) in &arms {
            arm();
        }
        let mut best = [f64::MAX; 4];
        for _ in 0..7 {
            for (i, (_, arm)) in arms.iter().enumerate() {
                best[i] = best[i].min(time(*arm));
            }
        }
        for (i, (label, _)) in arms.iter().enumerate() {
            println!("  {label:<18} {:>6.1}ns a mention", per(best[i]));
        }
        for (cached, rebuilt, what) in [
            (0, 1, "a nullary constructor"),
            (2, 3, "a constructor closure"),
        ] {
            let ratio = best[cached] / best[rebuilt];
            println!("  {what}: {ratio:.2}x what rebuilding it costs");
            assert!(
                ratio < 1.5,
                "a cached mention of {what} cost {:.1}ns against {:.1}ns to rebuild it: the \
                 lookup is dearer than the allocation it replaced, and ADR 0019 §\"What is \
                 assumed\" item 1 is failing at this seam",
                per(best[cached]),
                per(best[rebuilt])
            );
        }
    }

    /// What the cache can hold, so its memory cost is a number rather than a hope, and what a
    /// program past the bound gets — which is what it got before the cache existed.
    #[test]
    fn past_the_bound_a_mention_is_built_as_it_was_before() {
        let entry = size_of::<Symbol>() + size_of::<usize>() + size_of::<Value>();
        println!(
            "one entry is at least {entry} bytes; the cache keeps at most {CTOR_CACHE_KEEP} of \
             them per thread"
        );
        for i in 0..CTOR_CACHE_KEEP {
            let filler = ctor_value(&name(&format!("Filler{i}")), 0);
            assert!(matches!(filler, Value::Ctor { .. }));
        }
        let held = CTOR_VALUES.with(|c| c.borrow().len());
        assert_eq!(
            held, CTOR_CACHE_KEEP,
            "the cache stopped short of its bound"
        );

        let overflow = name("Overflow");
        let first = ctor_value(&overflow, 0);
        let second = ctor_value(&overflow, 0);
        assert!(
            values_equal(&first, &second, Span::DUMMY).expect("two `Ctor`s compare"),
            "a constructor past the bound answered with two different values"
        );
        assert_eq!(
            CTOR_VALUES.with(|c| c.borrow().len()),
            CTOR_CACHE_KEEP,
            "the cache grew past its own bound"
        );
    }
}

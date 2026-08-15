use crate::builtins::Builtin;
use crate::env::Env;
use crate::handler::{OpDecl, check_operation, performed_atom};
use crate::host::{HostBinding, err_hermetic, err_machine_only_host, operation_label};
use crate::limit::{self, DEFAULT_MAX_CALLS, NAMED_CALLS, NESTED_CALLS, grow};
use crate::trace::Trace;
use crate::value::{Closure, ClosureKind, Decimal, Value, type_error, values_equal};
use crate::world::World;
use ply_core::CheckOutput;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{
    BinOp, Expr, ExprKind, HandleClause, Ident, Item, Lit, MatchArm, Mode, Param, Pattern,
    PatternKind, Program, QName, ReturnClause, Stmt, TestDef, TypeDefBody, UnOp,
};
use ply_syntax::resolve::{Namespace, Resolved};
use rustc_hash::FxHashMap;
use std::collections::BTreeMap;
use std::sync::Arc;

/// An operation's declaration, by program-wide effect name and operation name.
pub(crate) type OpTable = FxHashMap<(Symbol, Symbol), (bool, Mode)>;

struct HandlerFrame {
    clauses: Arc<Vec<HandleClause>>,
    /// Each clause's effect under its program-wide name, resolved where the
    /// `handle` was written: a perform reached from another module spells the
    /// same effect differently, and the two only meet once both are qualified.
    effects: Arc<Vec<Symbol>>,
    env: Env,
    /// The module the clause bodies were written in, which is not the one the
    /// perform that triggers them is reached from.
    module: usize,
    /// The calls pending when this handler was installed. A clause body runs on
    /// the stack *below* its own handler, so those are the calls pending while
    /// it runs — which is what the machine's `capture` does to its own stack,
    /// and the two engines have to charge one program the same way.
    calls: usize,
}

/// Ordered exactly as [`CheckOutput::tests`] is — load order, then source order
/// — because the index into the two is the same index.
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
    ctors: FxHashMap<Symbol, usize>,
    ops: OpTable,
    tests: Vec<TestSlot<'a>>,
    handlers: Vec<HandlerFrame>,
    /// The world every entry point forks from, so one seeded world serves every
    /// test in a run without any of them observing another's writes.
    base_world: World,
    world: World,
    /// What this entry point performed, which is not what its row said it could.
    trace: Trace,
    /// The module a bare name is resolved in: the one that wrote the expression
    /// being evaluated, not the one that called it.
    module: usize,
    /// The pending calls, innermost last, by name. Its length is the depth the
    /// budget bounds, so the two are one field rather than two that can drift.
    calls: Vec<Option<Symbol>>,
    max_calls: usize,
    /// What the run's host boundary is, held only in order to *refuse* at it.
    ///
    /// The tree-walker serves no host operation: a `Pending` answer needs a
    /// reactor it has no way to poll. It carries the binding so that reaching
    /// the boundary is the same diagnostic on both engines — `E0424` when
    /// nothing is bound, and a machine-only refusal when something is — rather
    /// than an `E0303` that tells the reader to file a bug.
    binding: Option<Arc<HostBinding>>,
}

impl<'a> Interp<'a> {
    pub fn new(program: &'a Program, resolved: &'a Resolved, check: &'a CheckOutput) -> Self {
        Self::build(program, resolved, Some(check))
    }

    /// Everything the evaluator needs is derivable from the resolved AST alone,
    /// so evaluation can be exercised without a type-check pass.
    pub fn for_program(program: &'a Program, resolved: &'a Resolved) -> Self {
        Self::build(program, resolved, None)
    }

    fn build(program: &'a Program, resolved: &'a Resolved, check: Option<&'a CheckOutput>) -> Self {
        let mut globals = FxHashMap::default();
        // The prelude's first, so a module declaring its own `Some` overwrites
        // it — the resolution order every other prelude name follows.
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
                    // A law is not a global and not a test: `ply-prove`
                    // evaluates its body through `eval_expr_for_test`, with
                    // its binders bound to generated values. A `derive` is not
                    // one either — expansion has already appended the globals
                    // it stands for.
                    Item::Law(_) | Item::Derive(_) => {}
                }
            }
        }

        Interp {
            program,
            resolved,
            check,
            globals,
            ctors,
            ops,
            tests,
            handlers: Vec::new(),
            base_world: World::new(),
            world: World::new(),
            trace: Trace::new(),
            module: 0,
            calls: Vec::new(),
            max_calls: DEFAULT_MAX_CALLS,
            binding: None,
        }
    }

    /// The run's host boundary. See [`Interp::binding`].
    pub fn set_host_binding(&mut self, binding: Arc<HostBinding>) {
        self.binding = Some(binding);
    }

    pub fn with_max_calls(mut self, max_calls: usize) -> Self {
        self.max_calls = max_calls.max(1);
        self
    }

    /// The atoms this engine performed at the last entry point.
    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    pub fn program(&self) -> &'a Program {
        self.program
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub(crate) fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Every subsequent entry point forks from `world` rather than from an
    /// empty one. A fixture built once is handed to every test this way.
    pub fn set_base_world(&mut self, world: World) {
        self.base_world = world;
        self.world = self.base_world.fork();
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
        self.eval(body, &Env::empty()).map(|_| ())
    }

    /// Runs the `ordinal`-th test declared by `module`.
    ///
    /// A position in this program is not a position in a [`CheckOutput`]: the
    /// incremental front end reports every module's tests while parsing only
    /// some of them, so the two lists agree on order but not on length. Naming
    /// the module is what survives that. Two tests in one module may share a
    /// label, so the ordinal — not the label — is the second half of the key.
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
        self.eval(body, &Env::empty()).map(|_| ())
    }

    pub fn eval_expr_for_test(&mut self, e: &Expr) -> Result<Value, Diagnostic> {
        self.reset();
        self.eval(e, &Env::empty())
    }

    /// `name` is the program-wide name — `app.main`, not `main`.
    pub fn call(&mut self, name: &str, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
        self.reset();
        let sym = Symbol::new(name);
        let f = self.globals.get(&sym).cloned().ok_or_else(|| {
            Diagnostic::error(codes::UNKNOWN_NAME, format!("no definition named `{name}`"))
                .primary(span, "not defined in this program")
                .note("this name is program-wide: `store.orders.place`, not `place`")
        })?;
        self.apply(f, args, span)
    }

    /// A previous failure can leave frames installed; nothing survives from one
    /// entry point to the next.
    fn reset(&mut self) {
        self.handlers.clear();
        self.calls.clear();
        self.trace.clear();
        self.module = 0;
        self.world = self.base_world.fork();
    }

    /// Resolution already decided what this denotes; nothing here re-derives it.
    ///
    /// A bare name goes straight to the module's scope rather than through
    /// [`Resolved::lookup`], because a miss there is the ordinary prelude case
    /// and building a diagnostic for every `len(..)` would not be free.
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

    /// The program-wide name a constructor reference denotes, falling back to
    /// the prelude's — which no module declares, so nothing qualifies it. A
    /// module that declares its own `Some` shadows the prelude's, exactly as one
    /// declaring its own `len` does.
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

    /// Every arm that needs more than a handful of locals is out of line: an
    /// unoptimized frame is sized for the union of all arms, and this function
    /// sits on the recursion path.
    /// Grows once per node, not once per Ply call: the call bound does not reach
    /// this recursion at all, because an expression can nest arbitrarily deep
    /// while calling nothing. The machine spends a heap frame where this spends
    /// a native one, so this is what keeps the two engines agreeing on a program
    /// the front end already accepted.
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
            ExprKind::App { func, args } => self.eval_app(func, args, env, e.span),
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
            } => self.eval_with_cell(init, binder, body, env),
            // Refused before the body runs, for the reason a general clause is:
            // running one unnamed interleaving would be a plausible wrong answer
            // and the result cache would keep it.
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
        // One implementation for both engines, so `--engine both` cannot report
        // a divergence that is the two of them disagreeing about a `-0.0`.
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

    /// An effect no module declares keeps the name as written. Inference has
    /// already rejected that, and falling back this way keeps a perform and the
    /// clause meant to handle it agreeing rather than mysteriously not.
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
        // Refused before the body runs, not when the clause is reached: a
        // program that ran halfway and then failed has already written to the
        // world, and the refusal is about what this engine can express at all.
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

    #[inline(never)]
    fn eval_with_cell(
        &mut self,
        init: &Expr,
        binder: &Ident,
        body: &Expr,
        env: &Env,
    ) -> Result<Value, Diagnostic> {
        let initial = self.eval(init, env)?;
        let cell = self.world.alloc(initial);
        let scope = env.bind(binder.name.clone(), Value::Cell(cell));
        self.eval(body, &scope)
    }

    /// Locals, then the module's own items and its selective imports, then the
    /// prelude — the resolution order the whole language is specified in.
    fn lookup(&self, q: &QName, env: &Env) -> Result<Value, Diagnostic> {
        if q.is_bare()
            && let Some(v) = env.lookup(q.symbol())
        {
            return Ok(v.clone());
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
                let mut scope = env.clone();
                for (p, v) in params.iter().zip(args) {
                    scope = scope.bind(p.clone(), v);
                }
                let body = body.clone();
                // The body's bare names mean what they meant where it was
                // written, which is not where it is being called from.
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
                result
            }
            // Only a caller mixing the two engines' values reaches this, and
            // answering with the wrong function would be worse than refusing.
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

    /// Walks the handler stack inward-out. The clause body runs with the stack
    /// truncated to what was installed *below* the matching handler, so a
    /// handler that performs the operation it handles reaches the next handler
    /// out instead of catching itself forever.
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
            // The clause runs below its own handler, so the calls the body made
            // since the handler was installed are not pending while it runs —
            // they are held aside exactly as the machine's `capture` holds them,
            // and put back when the value returns to the perform site.
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
                // A nullary constructor written bare is indistinguishable from a
                // binder in the AST, so the constructor table decides.
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

/// What the two engines share so that a mis-declared operation reads the same
/// whichever one ran it.
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
///
/// Total because both producers of a `Lit::Decimal` already enforce the type's
/// range — the lexer refuses a mantissa past 96 bits or a scale past 28, and the
/// body decoder refuses the same bytes — so the fallback is a shape no stream
/// this evaluator is handed can carry.
pub(crate) fn decimal_lit(mantissa: i128, scale: u32) -> Decimal {
    Decimal::try_from_i128_with_scale(mantissa, scale).unwrap_or(Decimal::ZERO)
}

/// A literal pattern against a value, shared by both engines so a `--engine
/// both` divergence cannot be the two of them disagreeing about a NaN.
///
/// `Float` matches by IEEE `==`, so a `NaN` pattern matches nothing at all —
/// including a NaN scrutinee. A pattern that answered otherwise would be a
/// second equality on the type, and nobody wrote that one down.
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

pub(crate) fn ctor_value(name: &Symbol, arity: usize) -> Value {
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
}

/// A clause without a resource label handles every resource of its operation;
/// an operation declared without `[r]` has exactly one anyway.
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
        // `Float` answers by IEEE, where `NaN < x` and `NaN >= x` are both
        // false — so a comparison is not the negation of its converse. That is
        // what the type says, and smoothing it over here would make the operator
        // disagree with `==` on the same two values.
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

/// IEEE-754, unmodified. There is no overflow error and no zero-divisor error:
/// `1.0 / 0.0` is `Infinity` and `0.0 / 0.0` is `NaN`, and those are values the
/// standard defines rather than failures. Refusing them would make `Float` a
/// worse `Decimal` instead of a different type — and the cost is stated in the
/// type, which is why nothing about a `Float` may be `proved`.
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

/// Exact, or a diagnostic. Never a silent wrap and never a silent rounding: a
/// total that quietly lost a cent is the failure this type exists to prevent.
///
/// `/` never arrives — inference refuses it with `E0209`, because the exact
/// quotient of two decimals is not in general a decimal and an operator would
/// have to round. `%` does, and is exact: the *remainder* of a decimal division
/// is a decimal even when the quotient is not.
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
        // Exact while the result's scale fits, and half-to-even at scale 28
        // otherwise. `checked_mul` is what applies that rule; a mantissa that
        // leaves 96 bits is `None` and is reported rather than rounded.
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

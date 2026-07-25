use crate::builtins::Builtin;
use crate::env::Env;
use crate::value::{Closure, ClosureKind, Value, first_difference, type_error, values_equal};
use ply_core::CheckOutput;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{
    BinOp, Expr, ExprKind, HandleClause, Ident, Item, Lit, MatchArm, Module, Param, Pattern,
    PatternKind, ReturnClause, Stmt, TestDef, TypeDefBody, UnOp,
};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

/// A semantic limit on runaway recursion, not a stack-safety one: [`grow`]
/// keeps the native stack from being the binding constraint.
pub const DEFAULT_MAX_DEPTH: usize = 10_000;

/// One Ply call costs several native frames and an unoptimized build makes each
/// of them large, so a worker's default 2 MiB runs out long before
/// `max_depth` does. Growing on demand means the depth limit is what a user
/// hits, and it is reported as a diagnostic instead of aborting the process
/// (and with it every unrelated test sharing it).
fn grow<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, f)
}

struct HandlerFrame {
    clauses: Arc<Vec<HandleClause>>,
    env: Env,
}

pub struct Interp<'a> {
    module: &'a Module,
    check: Option<&'a CheckOutput>,
    globals: FxHashMap<Symbol, Value>,
    ctors: FxHashMap<Symbol, usize>,
    ops: FxHashMap<(Symbol, Symbol), bool>,
    tests: Vec<&'a TestDef>,
    handlers: Vec<HandlerFrame>,
    depth: usize,
    max_depth: usize,
}

impl<'a> Interp<'a> {
    pub fn new(module: &'a Module, check: &'a CheckOutput) -> Self {
        Self::build(module, Some(check))
    }

    /// Everything the evaluator needs is derivable from the AST alone, so
    /// evaluation can be exercised without a type-check pass.
    pub fn for_module(module: &'a Module) -> Self {
        Self::build(module, None)
    }

    fn build(module: &'a Module, check: Option<&'a CheckOutput>) -> Self {
        let mut globals = FxHashMap::default();
        let mut ctors: FxHashMap<Symbol, usize> = FxHashMap::default();
        let mut ops = FxHashMap::default();
        let mut tests = Vec::new();

        for item in &module.items {
            match item {
                Item::Fn(f) => {
                    let params = f.params.iter().map(|p| p.name.name.clone()).collect();
                    let closure = Closure {
                        name: Some(f.name.name.clone()),
                        kind: ClosureKind::Fn {
                            params,
                            body: Arc::new(f.body.clone()),
                            env: Env::empty(),
                        },
                    };
                    globals.insert(f.name.name.clone(), Value::Closure(Arc::new(closure)));
                }
                Item::Type(t) => {
                    if let TypeDefBody::Sum(variants) = &t.body {
                        for v in variants {
                            ctors.insert(v.name.name.clone(), v.fields.len());
                        }
                    }
                }
                Item::Effect(e) => {
                    for op in &e.ops {
                        ops.insert(
                            (e.name.name.clone(), op.name.name.clone()),
                            op.resource_param,
                        );
                    }
                }
                Item::Test(t) => tests.push(t),
            }
        }

        ctors.entry(Symbol::new("Some")).or_insert(1);
        ctors.entry(Symbol::new("None")).or_insert(0);

        Interp {
            module,
            check,
            globals,
            ctors,
            ops,
            tests,
            handlers: Vec::new(),
            depth: 0,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }

    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth.max(1);
        self
    }

    pub fn module(&self) -> &'a Module {
        self.module
    }

    pub fn check(&self) -> Option<&'a CheckOutput> {
        self.check
    }

    pub fn test_count(&self) -> usize {
        self.tests.len()
    }

    pub fn test_name(&self, index: usize) -> Option<&'a str> {
        self.tests.get(index).map(|t| t.name.as_str())
    }

    pub fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic> {
        let test = *self.tests.get(index).ok_or_else(|| {
            Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!(
                    "no test at index {index}; the module defines {}",
                    self.tests.len()
                ),
            )
            .primary(Span::DUMMY, "requested test does not exist")
        })?;
        self.reset();
        self.eval(&test.body, &Env::empty()).map(|_| ())
    }

    pub fn eval_expr_for_test(&mut self, e: &Expr) -> Result<Value, Diagnostic> {
        self.reset();
        self.eval(e, &Env::empty())
    }

    pub fn eval_main(&mut self) -> Result<Value, Diagnostic> {
        self.call("main", Vec::new(), Span::DUMMY)
    }

    pub fn call(&mut self, name: &str, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
        self.reset();
        let sym = Symbol::new(name);
        let f = self.globals.get(&sym).cloned().ok_or_else(|| {
            Diagnostic::error(codes::UNKNOWN_NAME, format!("no definition named `{name}`"))
                .primary(span, "not defined in this module")
        })?;
        self.apply(f, args, span)
    }

    /// A previous failure can leave frames installed; nothing survives from one
    /// entry point to the next.
    fn reset(&mut self) {
        self.handlers.clear();
        self.depth = 0;
    }

    fn enter(&mut self, span: Span) -> Result<(), Diagnostic> {
        if self.depth >= self.max_depth {
            return Err(err_recursion_limit(span, self.max_depth));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Every arm that needs more than a handful of locals is out of line: an
    /// unoptimized frame is sized for the union of all arms, and this function
    /// sits on the recursion path.
    fn eval(&mut self, e: &Expr, env: &Env) -> Result<Value, Diagnostic> {
        match &e.kind {
            ExprKind::Lit(lit) => Ok(literal(lit)),
            ExprKind::Var(id) => self.lookup(&id.name, id.span, env),
            ExprKind::Unary { op, operand } => self.eval_unary(*op, operand, env, e.span),
            ExprKind::Binary { op, lhs, rhs } => self.eval_binary(*op, lhs, rhs, env, e.span),
            ExprKind::Lambda { params, body } => Ok(eval_lambda(params, body, env)),
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
        match op {
            UnOp::Neg => {
                let i = v.as_int(operand.span, "negation")?;
                match i.checked_neg() {
                    Some(n) => Ok(Value::Int(n)),
                    None => Err(err_overflow(span, "negation", i, 0)),
                }
            }
            UnOp::Not => Ok(Value::Bool(!v.as_bool(operand.span, "`!`")?)),
        }
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
        effect: &Ident,
        op: &Ident,
        resource: Option<&Ident>,
        args: &[Expr],
        env: &Env,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let mut argv = Vec::with_capacity(args.len());
        for a in args {
            argv.push(self.eval(a, env)?);
        }
        self.perform(
            &effect.name,
            &op.name,
            resource.map(|r| r.name.clone()),
            argv,
            span,
        )
    }

    #[inline(never)]
    fn eval_handle(
        &mut self,
        body: &Expr,
        clauses: &[HandleClause],
        return_clause: Option<&ReturnClause>,
        env: &Env,
    ) -> Result<Value, Diagnostic> {
        let mark = self.handlers.len();
        self.handlers.push(HandlerFrame {
            clauses: Arc::new(clauses.to_vec()),
            env: env.clone(),
        });
        let result = self.eval(body, env);
        self.handlers.truncate(mark);
        let value = result?;

        match return_clause {
            Some(rc) => {
                let scope = env.bind(rc.binder.name.clone(), value);
                self.enter(rc.span)?;
                let out = grow(|| self.eval(&rc.body, &scope));
                self.leave();
                out
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
        let cell = Rc::new(RefCell::new(initial));
        let scope = env.bind(binder.name.clone(), Value::Cell(cell));
        self.eval(body, &scope)
    }

    fn lookup(&self, name: &Symbol, span: Span, env: &Env) -> Result<Value, Diagnostic> {
        if let Some(v) = env.lookup(name) {
            return Ok(v.clone());
        }
        if let Some(v) = self.globals.get(name) {
            return Ok(v.clone());
        }
        if let Some(&arity) = self.ctors.get(name) {
            return Ok(ctor_value(name, arity));
        }
        if let Some(b) = Builtin::from_name(name) {
            return Ok(Value::builtin(b));
        }
        Err(err_unknown_name(name, span))
    }

    pub(crate) fn apply(
        &mut self,
        callee: Value,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let closure = match callee {
            Value::Closure(c) => c,
            other => return Err(err_not_a_function(span, &other)),
        };

        match &closure.kind {
            ClosureKind::Fn { params, body, env } => {
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
                self.enter(span)?;
                let result = grow(|| self.eval(&body, &scope));
                self.leave();
                result
            }
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
        self.check_operation_exists(effect, op, resource.is_some(), span)?;

        for i in (0..self.handlers.len()).rev() {
            let clauses = self.handlers[i].clauses.clone();
            let Some(index) = clauses
                .iter()
                .position(|c| clause_matches(c, effect, op, &resource))
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

            let outer = self.handlers.split_off(i);
            let result = match self.enter(span) {
                Err(d) => Err(d),
                Ok(()) => {
                    let r = grow(|| self.eval(&clause.body, &scope));
                    self.leave();
                    r
                }
            };
            self.handlers.truncate(i);
            self.handlers.extend(outer);
            return result;
        }

        Err(err_unhandled(span, effect, op, resource.as_ref()))
    }

    /// Inference rules these out; reaching one means the evaluator was handed a
    /// module that was never checked, so name the mistake rather than guess.
    #[cold]
    #[inline(never)]
    fn check_operation_exists(
        &self,
        effect: &Symbol,
        op: &Symbol,
        has_resource: bool,
        span: Span,
    ) -> Result<(), Diagnostic> {
        match self.ops.get(&(effect.clone(), op.clone())) {
            Some(&resource_param) => {
                if resource_param && !has_resource {
                    return Err(Diagnostic::error(
                        codes::RESOURCE_REQUIRED,
                        format!(
                            "`{effect}.{op}` is resource-parameterized and needs a `[resource]`"
                        ),
                    )
                    .primary(span, "missing resource label"));
                }
                Ok(())
            }
            None if self.ops.keys().any(|(e, _)| e == effect) => Err(Diagnostic::error(
                codes::UNKNOWN_OPERATION,
                format!("effect `{effect}` has no operation `{op}`"),
            )
            .primary(span, "unknown operation")),
            None => Ok(()),
        }
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
                match self.ctors.get(&id.name) {
                    Some(0) => matches!(value, Value::Ctor { name, args }
                        if *name == id.name && args.is_empty()),
                    _ => {
                        *env = env.bind(id.name.clone(), value.clone());
                        true
                    }
                }
            }
            PatternKind::Lit(lit) => match (lit, value) {
                (Lit::Int(a), Value::Int(b)) => a == b,
                (Lit::Bool(a), Value::Bool(b)) => a == b,
                (Lit::Str(a), Value::Str(b)) => a.as_str() == b.as_ref(),
                (Lit::Unit, Value::Unit) => true,
                _ => false,
            },
            PatternKind::Ctor { name, args } => match value {
                Value::Ctor {
                    name: vname,
                    args: vargs,
                } => {
                    if *vname != name.name || vargs.len() != args.len() {
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

    pub(crate) fn assert_eq_failure(
        &self,
        actual: &Value,
        expected: &Value,
        span: Span,
    ) -> Diagnostic {
        let mut diag = Diagnostic::error(
            codes::ASSERTION_FAILED,
            format!(
                "assertion failed: expected {}, found {}",
                expected.render(),
                actual.render()
            ),
        )
        .primary(span, "these values are not equal")
        .note(format!("expected: {}", expected.render()))
        .note(format!("actual:   {}", actual.render()));

        if let Some((path, exp, act)) = first_difference(actual, expected) {
            diag = diag.note(format!(
                "first difference at `{path}`: expected {exp}, found {act}"
            ));
        }
        diag
    }
}

fn literal(lit: &Lit) -> Value {
    match lit {
        Lit::Int(i) => Value::Int(*i),
        Lit::Bool(b) => Value::Bool(*b),
        Lit::Str(s) => Value::str(s),
        Lit::Unit => Value::Unit,
    }
}

#[inline(never)]
fn eval_lambda(params: &[Param], body: &Expr, env: &Env) -> Value {
    Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Fn {
            params: params.iter().map(|p| p.name.name.clone()).collect(),
            body: Arc::new(body.clone()),
            env: env.clone(),
        },
    }))
}

fn ctor_value(name: &Symbol, arity: usize) -> Value {
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
    effect: &Symbol,
    op: &Symbol,
    resource: &Option<Symbol>,
) -> bool {
    c.effect.name == *effect
        && c.op.name == *op
        && match (&c.resource, resource) {
            (None, _) => true,
            (Some(cr), Some(r)) => cr.name == *r,
            (Some(_), None) => false,
        }
}

#[inline(never)]
fn strict_binary(
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
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ordering = match (l, r) {
                (Value::Int(a), Value::Int(b)) => a.cmp(b),
                (Value::Str(a), Value::Str(b)) => a.as_ref().cmp(b.as_ref()),
                (Value::Int(_) | Value::Str(_), other) => {
                    return Err(type_error(rspan, "a comparison", l.type_name(), other));
                }
                (other, _) => {
                    return Err(type_error(lspan, "a comparison", "Int or String", other));
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
            codes::RUNTIME_ERROR,
            "internal error: a short-circuiting operator reached strict evaluation",
        )
        .primary(span, "please report this")),
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
fn err_recursion_limit(span: Span, max: usize) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("recursion limit of {max} nested calls exceeded"),
    )
    .primary(span, "this call is too deeply nested")
    .note("check for a recursive call that never reaches its base case")
}

#[cold]
#[inline(never)]
fn err_unknown_name(name: &Symbol, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::UNKNOWN_NAME,
        format!("cannot find `{name}` in this scope"),
    )
    .primary(span, "not bound here")
}

#[cold]
#[inline(never)]
fn err_not_a_function(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NOT_A_FUNCTION,
        format!("cannot call a value of type {}", v.type_name()),
    )
    .primary(span, format!("this is {}", v.render()))
}

#[cold]
#[inline(never)]
fn err_non_exhaustive(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NON_EXHAUSTIVE_MATCH,
        "no match arm applied to the scrutinee",
    )
    .primary(span, format!("this evaluated to {}", v.render()))
    .note("add an arm covering this value, or a `_` catch-all")
}

#[cold]
#[inline(never)]
fn err_let_mismatch(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NON_EXHAUSTIVE_MATCH,
        "`let` pattern did not match the bound value",
    )
    .primary(span, format!("value was {}", v.render()))
    .note("use `match` when the pattern can fail")
}

#[cold]
#[inline(never)]
fn err_no_such_field(field: &Ident, fields: &BTreeMap<Symbol, Value>) -> Diagnostic {
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
fn err_zero_divisor(span: Span, what: &str) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, format!("{what} by zero"))
        .primary(span, "this divisor is 0")
}

#[cold]
#[inline(never)]
fn err_overflow(span: Span, what: &str, a: i64, b: i64) -> Diagnostic {
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
fn err_unhandled(
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

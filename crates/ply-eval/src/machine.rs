//! The control-stack machine.
//!
//! A configuration is `⟨S, K, W⟩` — state, [`Stack`], [`World`] — and [`step`]
//! is one transition of ADR 0005 §1.3. Nothing about a Ply computation lives on
//! the native stack: a call costs one [`Frame::Call`] on the heap, which is what
//! makes capturing a continuation O(one segment per enclosing handler) and what
//! turns the old depth guard into an exact, O(1) bound on pending frames.
//!
//! [`step`]: Machine::step

use crate::builtins::{self, Builtin, Step};
use crate::code::{self, Code, NodeKind, Stmt as CodeStmt, lower};
use crate::cont::{Frame, Next, Stack};
use crate::env::Env;
use crate::handler::{self, Request, Transition};
use crate::interp::{
    OpTable, arity_error, ctor_value, err_non_exhaustive, err_not_a_function, err_overflow,
    err_unknown_name, literal, op_decl,
};
use crate::limit::{self, DEFAULT_MAX_CALLS, NAMED_CALLS, NESTED_CALLS, PENDING_FRAMES};
use crate::trace::Trace;
use crate::value::{Closure, ClosureKind, Value};
use crate::world::World;
use ply_core::CheckOutput;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{
    BinOp, Expr, FnDef, Item, Lit, Pattern, PatternKind, Program, QName, TypeDefBody, UnOp,
};
use ply_syntax::resolve::{Namespace, Resolved};
use rustc_hash::FxHashMap;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

/// A bound on pending frames: a resource limit, not a native-stack workaround.
/// The frames are heap cells and this is how many of them a program may hold at
/// once.
///
/// It is not the bound a runaway recursion hits. That is [`DEFAULT_MAX_CALLS`],
/// which both engines share and which every recursion reaches first, since a
/// call costs at least one frame. This one catches a program that pends a
/// million frames without nesting ten thousand calls.
pub const DEFAULT_MAX_FRAMES: usize = 1_000_000;

const CALL_SCAN_LIMIT: usize = 4096;

pub enum Progress {
    Running,
    Halted(Value),
}

/// `S` of `⟨S, K, W⟩`. It is [`handler::State`] plus the one shape no handler
/// transition can produce.
enum State {
    Eval { code: Code, env: Env, module: usize },
    Return(Value),
    Perform(Request),
    Halt(Value),
}

impl From<handler::State> for State {
    fn from(s: handler::State) -> State {
        match s {
            handler::State::Eval { code, env, module } => State::Eval { code, env, module },
            handler::State::Return(value) => State::Return(value),
            handler::State::Perform(request) => State::Perform(request),
        }
    }
}

/// Ordered exactly as [`CheckOutput::tests`] is — load order, then source order
/// — because the index into the two is the same index.
struct TestSlot<'a> {
    module: usize,
    name: &'a str,
    body: &'a Expr,
}

/// A definition as written, and the module its bare names mean something in.
struct FnSlot<'a> {
    def: &'a FnDef,
    module: usize,
}

pub struct Machine<'a> {
    program: &'a Program,
    resolved: &'a Resolved,
    check: Option<&'a CheckOutput>,
    /// Keyed by program-wide name, so two modules may declare one simple name.
    fns: FxHashMap<Symbol, FnSlot<'a>>,
    /// The lowered form of the definitions this machine has actually called.
    ///
    /// Lowering is a traversal per definition, and a test reaches a handful of
    /// a project's ten thousand. Doing it on construction cost a whole-program
    /// traversal per worker per concurrency group — the single largest item in
    /// the machine's profile — for code no test in the group would run.
    lowered: FxHashMap<Symbol, Value>,
    ctors: FxHashMap<Symbol, usize>,
    ops: OpTable,
    tests: Vec<TestSlot<'a>>,
    /// The world every entry point forks from, so one seeded world serves every
    /// test in a run without any of them observing another's writes.
    base_world: World,
    world: World,
    /// What this entry point performed, which is not what its row said it could.
    trace: Trace,
    stack: Stack,
    state: State,
    current: Span,
    max_frames: usize,
    max_calls: usize,
}

impl<'a> Machine<'a> {
    pub fn new(
        program: &'a Program,
        resolved: &'a Resolved,
        check: &'a CheckOutput,
    ) -> Machine<'a> {
        Machine::build(program, resolved, Some(check))
    }

    /// Everything the machine needs is derivable from the resolved AST alone,
    /// so evaluation can be exercised without a type-check pass.
    pub fn for_program(program: &'a Program, resolved: &'a Resolved) -> Machine<'a> {
        Machine::build(program, resolved, None)
    }

    fn build(
        program: &'a Program,
        resolved: &'a Resolved,
        check: Option<&'a CheckOutput>,
    ) -> Machine<'a> {
        let mut fns = FxHashMap::default();
        let mut ctors: FxHashMap<Symbol, usize> = FxHashMap::default();
        let mut ops = FxHashMap::default();
        let mut tests = Vec::new();

        for (m, module) in program.modules.iter().enumerate() {
            let qualify = |name: &Symbol| module.name.qualify(name);
            for item in &module.items {
                match item {
                    Item::Fn(f) => {
                        fns.insert(qualify(&f.name.name), FnSlot { def: f, module: m });
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
                    Item::Test(t) => tests.push(TestSlot {
                        module: m,
                        name: t.name.as_str(),
                        body: &t.body,
                    }),
                }
            }
        }

        Machine {
            program,
            resolved,
            check,
            fns,
            lowered: FxHashMap::default(),
            ctors,
            ops,
            tests,
            base_world: World::new(),
            world: World::new(),
            trace: Trace::new(),
            stack: Stack::new(),
            state: State::Halt(Value::Unit),
            current: Span::DUMMY,
            max_frames: DEFAULT_MAX_FRAMES,
            max_calls: DEFAULT_MAX_CALLS,
        }
    }

    pub fn with_max_frames(mut self, max: usize) -> Machine<'a> {
        self.max_frames = max.max(1);
        self
    }

    pub fn with_max_calls(mut self, max: usize) -> Machine<'a> {
        self.max_calls = max.max(1);
        self
    }

    /// The atoms this engine performed at the last entry point.
    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    pub fn program(&self) -> &'a Program {
        self.program
    }

    pub fn check(&self) -> Option<&'a CheckOutput> {
        self.check
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// Every subsequent entry point forks from `world` rather than from an
    /// empty one. A fixture built once is handed to every test this way.
    pub fn set_base_world(&mut self, world: World) {
        self.base_world = world;
        self.world = self.base_world.fork();
    }

    pub fn test_count(&self) -> usize {
        self.tests.len()
    }

    pub fn test_name(&self, index: usize) -> Option<&'a str> {
        self.tests.get(index).map(|t| t.name)
    }

    pub fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic> {
        let Some(slot) = self.tests.get(index) else {
            return Err(Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!(
                    "no test at index {index}; the program defines {}",
                    self.tests.len()
                ),
            )
            .primary(Span::DUMMY, "requested test does not exist"));
        };
        let (module, body) = (slot.module, lower(slot.body));
        self.drive(body, Env::empty(), module).map(|_| ())
    }

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
            .map(|slot| (slot.module, lower(slot.body)));
        let Some((owner, body)) = found else {
            return Err(Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("module `{module}` has no test at position {ordinal}"),
            )
            .primary(Span::DUMMY, "this test's module was not parsed")
            .note("run `ply cache clear`, or pass `--no-incremental`"));
        };
        self.drive(body, Env::empty(), owner).map(|_| ())
    }

    pub fn eval_expr_for_test(&mut self, e: &Expr) -> Result<Value, Diagnostic> {
        self.drive(lower(e), Env::empty(), 0)
    }

    /// `name` is the program-wide name — `app.main`, not `main`.
    pub fn call(&mut self, name: &str, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
        let sym = Symbol::new(name);
        let f = self.definition(&sym).ok_or_else(|| {
            Diagnostic::error(codes::UNKNOWN_NAME, format!("no definition named `{name}`"))
                .primary(span, "not defined in this program")
                .note("this name is program-wide: `store.orders.place`, not `place`")
        })?;
        self.reset();
        self.apply(f, args, span)?;
        self.run()
    }

    /// One transition. Public so a stepper, a tracer and a fuel budget can each
    /// be written outside the machine.
    pub fn step(&mut self) -> Result<Progress, Diagnostic> {
        match std::mem::replace(&mut self.state, State::Return(Value::Unit)) {
            State::Eval { code, env, module } => {
                self.eval(&code, env, module)?;
                Ok(Progress::Running)
            }
            State::Return(value) => {
                self.ret(value)?;
                Ok(Progress::Running)
            }
            State::Perform(request) => {
                let decl = op_decl(&self.ops, &request.effect, &request.op);
                // Charged after the declaration check and before the handler
                // search, which is where the tree-walker charges it: an
                // unhandled `perform` was still performed, and two engines that
                // record it at different moments disagree on a failing program.
                handler::check_operation(
                    decl,
                    &request.effect,
                    &request.op,
                    request.resource.is_some(),
                    request.span,
                )?;
                if let Some(atom) =
                    handler::performed_atom(&request.effect, request.resource.as_ref(), decl)
                {
                    self.trace.record(atom);
                }
                let transition = handler::perform(&self.stack, request, decl)?;
                self.take(transition)?;
                Ok(Progress::Running)
            }
            State::Halt(value) => {
                self.state = State::Halt(value.clone());
                Ok(Progress::Halted(value))
            }
        }
    }

    pub fn stack(&self) -> &Stack {
        &self.stack
    }

    /// A stack that is a value cannot leak from one entry point to the next, so
    /// this restores the world rather than unwinding anything.
    fn reset(&mut self) {
        self.stack = Stack::new();
        self.world = self.base_world.fork();
        self.trace.clear();
        self.state = State::Halt(Value::Unit);
    }

    fn drive(&mut self, code: Code, env: Env, module: usize) -> Result<Value, Diagnostic> {
        self.reset();
        self.state = State::Eval { code, env, module };
        self.run()
    }

    fn run(&mut self) -> Result<Value, Diagnostic> {
        loop {
            if let Progress::Halted(value) = self.step()? {
                return Ok(value);
            }
        }
    }

    pub(crate) fn go_eval(&mut self, code: Code, env: Env, module: usize) {
        self.state = State::Eval { code, env, module };
    }

    pub(crate) fn go_return(&mut self, value: Value) {
        self.state = State::Return(value);
    }

    /// Adopts what a [`handler`] transition decided. Every stack it hands back
    /// is checked against both bounds here, so a splice that would make the
    /// machine unbounded is refused at the one place splices land.
    pub(crate) fn take(&mut self, transition: Transition) -> Result<(), Diagnostic> {
        if transition.stack.calls() > self.max_calls {
            return Err(self.err_call_limit(self.current, &transition.stack));
        }
        if transition.stack.frames() > self.max_frames {
            return Err(self.err_frame_limit(self.current));
        }
        self.stack = transition.stack;
        self.state = transition.state.into();
        Ok(())
    }

    fn eval(&mut self, code: &Code, env: Env, module: usize) -> Result<(), Diagnostic> {
        let span = code.span;
        self.current = span;
        match &code.kind {
            NodeKind::Lit(lit) => self.go_return(literal(lit)),

            NodeKind::Var(q) => {
                let value = self.lookup(q, &env, module)?;
                self.go_return(value);
            }

            NodeKind::Unary { op, operand } => {
                self.push(
                    Frame::Unary {
                        op: *op,
                        operand_span: operand.span,
                        span,
                    },
                    span,
                )?;
                self.go_eval(operand.clone(), env, module);
            }

            NodeKind::Binary { op, lhs, rhs } => {
                self.push(
                    Frame::BinaryRhs {
                        op: *op,
                        rhs: rhs.clone(),
                        env: env.clone(),
                        module,
                        lhs_span: lhs.span,
                        span,
                    },
                    span,
                )?;
                self.go_eval(lhs.clone(), env, module);
            }

            NodeKind::Lambda { params, body } => {
                self.go_return(Value::Closure(Arc::new(Closure {
                    name: None,
                    kind: ClosureKind::Code {
                        params: params.clone(),
                        body: body.clone(),
                        env,
                        module,
                    },
                })));
            }

            NodeKind::App { func, args } => {
                self.push(
                    Frame::AppCallee {
                        args: args.clone(),
                        env: env.clone(),
                        module,
                        span,
                    },
                    span,
                )?;
                self.go_eval(func.clone(), env, module);
            }

            NodeKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.push(
                    Frame::If {
                        then_branch: then_branch.clone(),
                        else_branch: else_branch.clone(),
                        env: env.clone(),
                        module,
                        cond_span: cond.span,
                    },
                    span,
                )?;
                self.go_eval(cond.clone(), env, module);
            }

            NodeKind::Match { scrutinee, arms } => {
                self.push(
                    Frame::MatchArms {
                        // Unread while `next` is 0: the value this frame is
                        // waiting for *is* the scrutinee. A retry after a failed
                        // guard is what carries a real one.
                        scrutinee: Value::Unit,
                        arms: arms.clone(),
                        next: 0,
                        env: env.clone(),
                        module,
                        scrutinee_span: scrutinee.span,
                    },
                    span,
                )?;
                self.go_eval(scrutinee.clone(), env, module);
            }

            NodeKind::Block { stmts, tail } => {
                self.enter_block(stmts.clone(), 0, tail.clone(), env, module)?;
            }

            NodeKind::Record { fields } => {
                if fields.is_empty() {
                    self.go_return(Value::Record(Arc::new(BTreeMap::new())));
                } else {
                    self.push(
                        Frame::RecordField {
                            done: Vec::with_capacity(fields.len()),
                            fields: fields.clone(),
                            next: 1,
                            env: env.clone(),
                            module,
                        },
                        span,
                    )?;
                    self.go_eval(fields[0].1.clone(), env, module);
                }
            }

            NodeKind::Field { base, field } => {
                self.push(
                    Frame::FieldAccess {
                        field: field.clone(),
                        base_span: base.span,
                    },
                    span,
                )?;
                self.go_eval(base.clone(), env, module);
            }

            NodeKind::List { items } => {
                if items.is_empty() {
                    self.go_return(Value::list(Vec::new()));
                } else {
                    self.push(
                        Frame::ListItem {
                            done: Vec::with_capacity(items.len()),
                            items: items.clone(),
                            next: 1,
                            env: env.clone(),
                            module,
                        },
                        span,
                    )?;
                    self.go_eval(items[0].clone(), env, module);
                }
            }

            NodeKind::Perform {
                effect,
                op,
                resource,
                args,
            } => {
                let effect = self.effect_name(module, effect);
                let transition = handler::perform_args(
                    &self.stack,
                    &effect,
                    op,
                    resource,
                    Vec::with_capacity(args.len()),
                    args,
                    0,
                    &env,
                    module,
                    span,
                );
                self.take(transition)?;
            }

            NodeKind::Handle { body, clauses, ret } => {
                let effects = Rc::new(
                    clauses
                        .iter()
                        .map(|c| self.effect_name(module, &c.effect))
                        .collect(),
                );
                let transition = handler::enter_handle(
                    &self.stack,
                    body,
                    clauses,
                    effects,
                    ret.as_ref(),
                    &env,
                    module,
                    span,
                );
                self.take(transition)?;
            }

            NodeKind::WithCell {
                resource,
                init,
                binder,
                body,
            } => {
                let transition = handler::enter_with_cell(
                    &self.stack,
                    resource,
                    binder,
                    init,
                    body,
                    &env,
                    module,
                );
                self.take(transition)?;
            }
        }
        Ok(())
    }

    fn ret(&mut self, value: Value) -> Result<(), Diagnostic> {
        match self.stack.next() {
            Next::Frame(frame, rest) => {
                self.stack = rest;
                self.dispatch(frame, value)
            }
            Next::Leave(prompt, rest) => {
                let transition = handler::leave_handle(&prompt, value, rest);
                self.take(transition)
            }
            Next::Done => {
                self.state = State::Halt(value);
                Ok(())
            }
        }
    }

    pub(crate) fn apply(
        &mut self,
        callee: Value,
        args: Vec<Value>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        match callee {
            Value::Closure(closure) => self.enter_closure(&closure, args, span),
            Value::Continuation(k) => {
                let transition = handler::apply_continuation(&self.stack, &k, args, span)?;
                self.take(transition)
            }
            other => Err(err_not_a_function(span, &other)),
        }
    }

    fn enter_closure(
        &mut self,
        closure: &Arc<Closure>,
        args: Vec<Value>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        match &closure.kind {
            ClosureKind::Code {
                params,
                body,
                env,
                module,
            } => {
                let (body, env, module) = (body.clone(), env.clone(), *module);
                self.enter_code(closure, params, body, env, module, args, span)
            }
            // A closure the tree-walker made, handed in through `call`. Lowering
            // it here costs a traversal per call and keeps the two engines'
            // values interchangeable, which is what `--engine both` needs.
            ClosureKind::Fn {
                params,
                body,
                env,
                module,
            } => {
                let (body, env, module) = (lower(body), env.clone(), *module);
                let params: Vec<Symbol> = params.clone();
                self.enter_code(closure, &params, body, env, module, args, span)
            }
            ClosureKind::Ctor { name, arity } => {
                if *arity != args.len() {
                    return Err(arity_error(span, &format!("`{name}`"), *arity, args.len()));
                }
                self.go_return(Value::Ctor {
                    name: name.clone(),
                    args: Arc::new(args),
                });
                Ok(())
            }
            ClosureKind::Builtin(b) => {
                let b = *b;
                self.call_builtin(b, args, span)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn enter_code(
        &mut self,
        closure: &Closure,
        params: &[Symbol],
        body: Code,
        env: Env,
        module: usize,
        args: Vec<Value>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        if params.len() != args.len() {
            return Err(arity_error(
                span,
                &closure.describe(),
                params.len(),
                args.len(),
            ));
        }
        let mut scope = env;
        for (p, v) in params.iter().zip(args) {
            scope = scope.bind(p.clone(), v);
        }
        if self.stack.calls() >= self.max_calls {
            return Err(self.err_call_limit(span, &self.stack));
        }
        self.push(
            Frame::Call {
                name: closure.name.clone(),
                call_site: span,
            },
            span,
        )?;
        self.go_eval(body, scope, module);
        Ok(())
    }

    pub(crate) fn call_builtin(
        &mut self,
        b: Builtin,
        args: Vec<Value>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let step = builtins::call(b, args, &mut self.world, span)?;
        self.run_builtin_step(step, span)
    }

    /// The machine's half of the builtin step protocol: a suspension becomes a
    /// frame on the heap, so a continuation captured inside `map`'s callback can
    /// be resumed as many times as it likes.
    pub(crate) fn run_builtin_step(&mut self, step: Step, span: Span) -> Result<(), Diagnostic> {
        match step {
            Step::Done(value) => {
                self.go_return(value);
                Ok(())
            }
            Step::Apply {
                callee,
                args,
                frame,
            } => {
                self.push(frame, span)?;
                self.apply(callee, args, span)
            }
        }
    }

    pub(crate) fn enter_block(
        &mut self,
        stmts: Rc<Vec<CodeStmt>>,
        next: usize,
        tail: Option<Code>,
        scope: Env,
        module: usize,
    ) -> Result<(), Diagnostic> {
        if let Some(stmt) = stmts.get(next) {
            let code = match stmt {
                CodeStmt::Let { value, .. } => value.clone(),
                CodeStmt::Expr(e) => e.clone(),
            };
            let span = code.span;
            self.push(
                Frame::BlockStep {
                    stmts,
                    next: next + 1,
                    tail,
                    scope: scope.clone(),
                    module,
                },
                span,
            )?;
            self.go_eval(code, scope, module);
            return Ok(());
        }
        match tail {
            Some(t) => self.go_eval(t, scope, module),
            None => self.go_return(Value::Unit),
        }
        Ok(())
    }

    pub(crate) fn try_arms(
        &mut self,
        scrutinee: Value,
        arms: Rc<Vec<code::Arm>>,
        from: usize,
        env: Env,
        module: usize,
        scrutinee_span: Span,
    ) -> Result<(), Diagnostic> {
        let mut hit = None;
        for (i, arm) in arms.iter().enumerate().skip(from) {
            let mut arm_env = env.clone();
            if self.match_pattern(&arm.pat, &scrutinee, &mut arm_env, module)? {
                hit = Some((i, arm_env, arm.guard.clone(), arm.body.clone()));
                break;
            }
        }
        match hit {
            None => Err(err_non_exhaustive(scrutinee_span, &scrutinee)),
            Some((_, arm_env, None, body)) => {
                self.go_eval(body, arm_env, module);
                Ok(())
            }
            Some((at, arm_env, Some(guard), _)) => {
                let guard_span = guard.span;
                self.push(
                    Frame::MatchGuard {
                        scrutinee,
                        arms,
                        at,
                        arm_env: arm_env.clone(),
                        env,
                        module,
                        scrutinee_span,
                    },
                    guard_span,
                )?;
                self.go_eval(guard, arm_env, module);
                Ok(())
            }
        }
    }

    pub(crate) fn match_pattern(
        &self,
        pat: &Pattern,
        value: &Value,
        env: &mut Env,
        module: usize,
    ) -> Result<bool, Diagnostic> {
        Ok(match &pat.kind {
            PatternKind::Wildcard => true,
            PatternKind::Var(id) => {
                // A nullary constructor written bare is indistinguishable from a
                // binder in the AST, so the constructor table decides.
                let declared = self.global(module, Namespace::Value, &QName::bare(id.clone()));
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
                    let expected = self.global(module, Namespace::Value, name);
                    if expected.as_ref() != Some(vname) || vargs.len() != args.len() {
                        return Ok(false);
                    }
                    for (p, v) in args.iter().zip(vargs.iter()) {
                        if !self.match_pattern(p, v, env, module)? {
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
                        if !self.match_pattern(p, &v, env, module)? {
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
                        if !self.match_pattern(p, v, env, module)? {
                            return Ok(false);
                        }
                    }
                    match rest {
                        Some(rest) => {
                            let tail = Value::list(xs[items.len()..].to_vec());
                            self.match_pattern(rest, &tail, env, module)?
                        }
                        None => true,
                    }
                }
                _ => false,
            },
        })
    }

    /// Locals, then the module's own items and its selective imports, then the
    /// prelude — the resolution order the whole language is specified in.
    fn lookup(&mut self, q: &QName, env: &Env, module: usize) -> Result<Value, Diagnostic> {
        if q.is_bare()
            && let Some(v) = env.lookup(q.symbol())
        {
            return Ok(v.clone());
        }
        if let Some(name) = self.global(module, Namespace::Value, q) {
            if let Some(v) = self.definition(&name) {
                return Ok(v);
            }
            if let Some(&arity) = self.ctors.get(&name) {
                return Ok(ctor_value(&name, arity));
            }
        }
        if q.is_bare()
            && let Some(b) = Builtin::from_name(q.symbol())
        {
            return Ok(Value::builtin(b));
        }
        Err(err_unknown_name(q))
    }

    /// The closure for a program-wide name, lowering its body the first time.
    fn definition(&mut self, name: &Symbol) -> Option<Value> {
        if let Some(v) = self.lowered.get(name) {
            return Some(v.clone());
        }
        let slot = self.fns.get(name)?;
        let closure = Closure {
            name: Some(name.clone()),
            kind: ClosureKind::Code {
                params: Rc::new(
                    slot.def
                        .params
                        .iter()
                        .map(|p| p.name.name.clone())
                        .collect(),
                ),
                body: lower(&slot.def.body),
                env: Env::empty(),
                module: slot.module,
            },
        };
        let value = Value::Closure(Arc::new(closure));
        self.lowered.insert(name.clone(), value.clone());
        Some(value)
    }

    /// Resolution already decided what this denotes; nothing here re-derives it.
    ///
    /// A bare name goes straight to the module's scope rather than through
    /// [`Resolved::lookup`], because a miss there is the ordinary prelude case
    /// and building a diagnostic for every `len(..)` would not be free.
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

    /// An effect no module declares keeps the name as written. Inference has
    /// already rejected that, and falling back this way keeps a perform and the
    /// clause meant to handle it agreeing rather than mysteriously not.
    fn effect_name(&self, module: usize, effect: &QName) -> Symbol {
        self.global(module, Namespace::Effect, effect)
            .unwrap_or_else(|| effect.symbol().clone())
    }

    pub(crate) fn push(&mut self, frame: Frame, span: Span) -> Result<(), Diagnostic> {
        if self.stack.frames() >= self.max_frames {
            return Err(self.err_frame_limit(span));
        }
        self.stack = self.stack.push(frame);
        Ok(())
    }

    pub(crate) fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// The bound both engines share. The stack is a parameter because a splice
    /// is refused before it is adopted, so the calls to name are the ones on the
    /// stack that would have been installed, not the ones on the current one.
    fn err_call_limit(&self, span: Span, stack: &Stack) -> Diagnostic {
        limit::err_recursion_limit(span, NESTED_CALLS, self.max_calls, &innermost_calls(stack))
    }

    fn err_frame_limit(&self, span: Span) -> Diagnostic {
        limit::err_recursion_limit(
            span,
            PENDING_FRAMES,
            self.max_frames,
            &innermost_calls(&self.stack),
        )
    }
}

/// The innermost pending calls, innermost first — the same list the tree-walker
/// reads off its own nesting, so the two engines' notes are one string.
///
/// The scan is bounded: at the frame limit the stack holds a million frames and
/// six names are wanted, so walking all of them to build a note about a program
/// that is already failing would be the slowest thing in the run.
fn innermost_calls(stack: &Stack) -> Vec<Option<Symbol>> {
    let mut out = Vec::new();
    let mut stack = stack.clone();
    for _ in 0..CALL_SCAN_LIMIT {
        match stack.next() {
            Next::Frame(Frame::Call { name, .. }, rest) => {
                out.push(name);
                if out.len() == NAMED_CALLS {
                    break;
                }
                stack = rest;
            }
            Next::Frame(_, rest) | Next::Leave(_, rest) => stack = rest,
            Next::Done => break,
        }
    }
    out
}

pub(crate) fn apply_unary(
    op: UnOp,
    value: &Value,
    operand_span: Span,
    span: Span,
) -> Result<Value, Diagnostic> {
    match op {
        UnOp::Neg => {
            let i = value.as_int(operand_span, "negation")?;
            match i.checked_neg() {
                Some(n) => Ok(Value::Int(n)),
                None => Err(err_overflow(span, "negation", i, 0)),
            }
        }
        UnOp::Not => Ok(Value::Bool(!value.as_bool(operand_span, "`!`")?)),
    }
}

/// `||` is decided by a `true` left operand and `&&` by a `false` one; anything
/// else has to evaluate the right.
pub(crate) fn short_circuits(op: BinOp, lhs: bool) -> bool {
    lhs == matches!(op, BinOp::Or)
}

#[cfg(test)]
mod tests;

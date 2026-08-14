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
use crate::cont::{Continuation, Delimiter, Frame, Next, SimId, Stack};
use crate::env::Env;
use crate::handler::{self, Answered, Request, Scheduled, Transition};
use crate::interp::{
    OpTable, arity_error, ctor_value, err_non_exhaustive, err_not_a_function, err_overflow,
    err_unknown_name, literal, op_decl,
};
use crate::limit::{self, DEFAULT_MAX_CALLS, NAMED_CALLS, NESTED_CALLS, PENDING_FRAMES};
use crate::region::{self, Region, StepSite, Trail};
use crate::sched::{Resumption, Turn};
use crate::sim::{Access, Answer, DEFAULT_STEPS, Seed};
use crate::trace::Trace;
use crate::value::{Closure, ClosureKind, Value};
use crate::world::World;
use ply_core::CheckOutput;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{
    BinOp, Expr, FnDef, Item, Lit, Mode, Pattern, PatternKind, Program, QName, TypeDefBody, UnOp,
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
    /// The seed the next entry point's `simulate` region runs at, and the
    /// scheduling-step budget one interleaving may spend. A run is a pure
    /// function of its definitions and this.
    seed: Seed,
    sim_steps: u32,
    /// The region currently live. Nesting is `E0416` and a region whose control
    /// was discarded is `E0413`, so at most one is ever live; a `Task` handle
    /// naming any other region is `E0413` rather than an index into the wrong
    /// scheduler.
    sims: Vec<Region>,
    /// How many regions this entry point has entered. A region's [`SimId`] is
    /// its ordinal, not its depth: two regions in sequence are two ids, so a
    /// continuation captured in the first is recognized as foreign by the second
    /// rather than steering it.
    entered_sims: u32,
    /// The one choice sequence this entry point makes, across every region it
    /// enters. Reset at every entry point, so a test that reached no region
    /// reports nothing rather than the previous test's search.
    trail: Trail,
    /// What this entry point's regions did, built once the last of them ended.
    record: Option<region::Record>,
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
                    // A law is not a global and not a test: `ply-prove`
                    // evaluates its body through `eval_expr_for_test`, with
                    // its binders bound to generated values.
                    Item::Law(_) => {}
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
            seed: Seed::default(),
            sim_steps: DEFAULT_STEPS,
            sims: Vec::new(),
            entered_sims: 0,
            trail: Trail::new(Seed::default()),
            record: None,
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

    /// Fix the interleaving the next entry point takes.
    ///
    /// A `simulate` region's value is the one its seed names; exploring the
    /// others is a search, and driving that search is the test runner's job
    /// rather than the machine's. `steps` bounds one interleaving's scheduling
    /// points, which is what turns a livelock into [`codes::DEADLOCK`].
    pub fn set_seed(&mut self, seed: Seed, steps: u32) {
        self.seed = seed;
        self.sim_steps = steps.max(1);
    }

    /// What the last entry point's `simulate` regions did, or `None` when it
    /// reached none. Never a zeroed record: a search that observed nothing must
    /// say so rather than report an interleaving nobody ran.
    ///
    /// Every region of the entry point, over one choice sequence. A record
    /// covering one of several regions would be a search input describing one
    /// region and an `exhaustive` claim made about all of them.
    pub fn simulated(&self) -> Option<&region::Record> {
        self.record.as_ref()
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
                codes::INTERNAL_ERROR,
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
                codes::INTERNAL_ERROR,
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

    /// An expression from `module`, with `bindings` already in scope.
    ///
    /// This is what a spec clause and a law body need and what
    /// [`Machine::eval_expr_for_test`] cannot give them: a clause is written in
    /// its own module, so its bare names resolve there and nowhere else, and its
    /// binders are values the caller drew rather than anything the program
    /// bound.
    pub fn eval_expr_in(
        &mut self,
        e: &Expr,
        module: usize,
        bindings: &[(Symbol, Value)],
    ) -> Result<Value, Diagnostic> {
        let mut env = Env::empty();
        for (name, value) in bindings {
            env = env.bind(name.clone(), value.clone());
        }
        self.drive(lower(e), env, module)
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
                let atom =
                    handler::performed_atom(&request.effect, request.resource.as_ref(), decl);
                if let Some(atom) = atom.clone() {
                    self.trace.record(atom);
                }
                if !self.sims.is_empty() {
                    self.note_step_site(request.span);
                    // A step's accesses are every atom the tracer recorded as
                    // well as every cell the world did. The scheduler's own
                    // bookkeeping is dropped inside `record_access`, and a
                    // prelude operation contributes no atom here at all — the
                    // seeded handlers' table is the only thing that speaks for
                    // those, so `random.write` cannot be counted twice.
                    if let Some(atom) = atom {
                        self.trail.record_access(Access::Atom(atom));
                    }
                }
                match handler::perform(&self.stack, request, decl)? {
                    Answered::Handler(transition) => self.take(transition)?,
                    Answered::Scheduler(scheduled) => self.run_scheduled(scheduled)?,
                }
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
        self.sims.clear();
        self.entered_sims = 0;
        self.trail = Trail::new(self.seed.clone());
        self.record = None;
    }

    fn drive(&mut self, code: Code, env: Env, module: usize) -> Result<Value, Diagnostic> {
        self.reset();
        self.state = State::Eval { code, env, module };
        let outcome = self.run();
        self.close_regions(outcome)
    }

    /// A failure abandons the region where it stands. The diagnostic gains the
    /// task it failed in and the seed that replays it, and the steps taken so
    /// far are still the interleaving the search observed — a run that failed
    /// halfway is exactly the run the search is looking for.
    ///
    /// A run that *succeeded* with a region still live did not finish that
    /// region: some handler outside it discarded the continuation it was handed,
    /// so the region's tasks were destroyed unfinished. The search's whole input
    /// would be a trace that stops there, and it would report `exhaustive` over
    /// the schedules it cut short — so the program is refused instead.
    fn close_regions(&mut self, outcome: Result<Value, Diagnostic>) -> Result<Value, Diagnostic> {
        let outcome = match self.sims.pop() {
            None => outcome,
            Some(mut region) => {
                self.sims.clear();
                let failure = match outcome {
                    Err(diagnostic) => diagnostic,
                    Ok(_) => err_region_abandoned(region.span, &region.sched.unfinished()),
                };
                let failure = region.sched.fail(failure, self.trail.seed());
                self.trail.leave(
                    region.handlers.clock().now(),
                    region.handlers.rand().drawn(),
                );
                Err(failure)
            }
        };
        if self.trail.entered() {
            self.record = Some(self.trail.record());
        }
        outcome
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
            return Err(self.err_frame_limit(self.current, &transition.stack));
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

            NodeKind::Simulate { body } => {
                self.enter_simulate(body.clone(), env, module, span)?;
            }
        }
        Ok(())
    }

    /// `simulate { body }` — install the seeded scheduler and give its root task
    /// the first step.
    ///
    /// The region's own stack is captured here rather than pushed onto: every
    /// task runs on it plus a delimiter of its own, so a resumption splices onto
    /// the region's control and not onto whichever task ran last.
    fn enter_simulate(
        &mut self,
        body: Code,
        env: Env,
        module: usize,
        span: Span,
    ) -> Result<(), Diagnostic> {
        if let Some(outer) = self.sims.last() {
            // Two mistakes with one symptom. If the outer region's delimiter is
            // still on the stack, control is genuinely inside it and this is
            // nesting; if it is not, a handler discarded the region's
            // continuation and the tasks it left behind will never run.
            return Err(if self.stack.holds_sim(outer.id) {
                err_nested_simulation(span, outer.span)
            } else {
                err_region_abandoned(outer.span, &outer.sched.unfinished())
                    .secondary(span, "reached from here")
            });
        }
        let id = SimId(self.entered_sims);
        self.entered_sims += 1;
        self.trail.enter(span);
        self.sims.push(Region::new(
            id,
            self.seed.root,
            self.trail.drawn(),
            self.sim_steps,
            self.stack.clone(),
            body,
            env,
            module,
            span,
        ));
        self.schedule()
    }

    /// A `task.*`, `clock.*` or `random.*` perform that reached its region's
    /// delimiter. This ends the performing task's step, so it is also the one
    /// place a step's site is recorded and the last chance the scheduler has to
    /// hear about the step it is closing.
    fn run_scheduled(&mut self, scheduled: Scheduled) -> Result<(), Diagnostic> {
        let Scheduled {
            region,
            effect,
            op,
            args,
            span,
            k,
        } = scheduled;
        if region_mut(&mut self.sims, region).is_none() {
            return Err(err_task_escapes(span, &effect, &op));
        }
        self.trail.end_step(span);
        let live = region_mut(&mut self.sims, region).expect("the region was just found");
        let task = match live.sched.current() {
            Some(task) => task,
            None => {
                return Err(Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!("`{effect}.{op}` was performed while no task was running"),
                )
                .primary(span, "performed here"));
            }
        };

        match (effect.as_str(), op.as_str()) {
            ("task", "spawn") => {
                let [body] = take_args(&effect, &op, args, span)?;
                // The delimiters the spawn site sat under, which is what the
                // spawned body has to run under: a `handle` written inside the
                // region encloses the tasks it lexically contains, and the row
                // `spawn` publishes already says the enclosing handler
                // discharges them.
                let over = k.delimiters();
                let id = live.sched.spawn(body, over, span);
                live.sched.suspend(k, Value::Task(id))?;
            }
            ("task", "join") => {
                let [handle] = take_args(&effect, &op, args, span)?;
                let target = handle.as_task(span, "`task.join`")?;
                live.sched.join(k, target, span)?;
            }
            ("task", "yield") => {
                let [] = take_args(&effect, &op, args, span)?;
                live.sched.suspend(k, Value::Unit)?;
            }
            _ => {
                let Some(sig) = crate::sim::signature(&effect, &op) else {
                    return Err(Diagnostic::error(
                        codes::UNKNOWN_OPERATION,
                        format!("`{effect}` has no operation `{op}`"),
                    )
                    .primary(span, "unknown operation")
                    .note("a `simulate` region answers `task`, `clock` and `random`"));
                };
                let answer = live.handlers.dispatch(sig, task, &args, span)?;
                match answer {
                    Answer::Value(value) => live.sched.suspend(k, value)?,
                    Answer::Sleeping { deadline } => live.sched.sleep_until(k, deadline, span)?,
                }
                if let Some(access) = sig.step_access() {
                    self.trail.record_access(access);
                }
            }
        }
        self.schedule()
    }

    /// A task's body returned to its own delimiter.
    fn finish_task(&mut self, region: SimId, value: Value) -> Result<(), Diagnostic> {
        let span = self.current;
        if region_mut(&mut self.sims, region).is_none() {
            return Err(err_task_escapes(span, "task", "return"));
        }
        self.trail.end_step(span);
        let live = region_mut(&mut self.sims, region).expect("the region was just found");
        live.sched.finish(value)?;
        self.schedule()
    }

    /// Puts a cell access into the current step's access set.
    ///
    /// Two tests hold two worlds, so `ply_test::shared_footprint` may drop
    /// `cell` atoms; two tasks in one simulated run hold **one** world, and a
    /// dependence relation that drops them prunes away every shared-memory race
    /// in the corpus while reporting a *larger* reduction for having done it.
    fn record_cell_access(&mut self, b: Builtin, args: &[Value]) {
        let mode = match b {
            Builtin::CellGet => Mode::Read,
            Builtin::CellSet => Mode::Write,
            _ => return,
        };
        let Some(Value::Cell(id)) = args.first() else {
            return;
        };
        if self.sims.is_empty() {
            return;
        }
        self.trail.record_access(Access::Cell { id: *id, mode });
        self.note_step_site(self.current);
    }

    /// Puts a cell *allocation* into the current step's access set.
    ///
    /// Allocation has no location to name — that is the point of it — so it is
    /// its own kind of access, dependent with every other allocation. Without it
    /// two tasks that each open a private `with_cell` look like tasks that touch
    /// nothing, and §6.1's soundness condition is false of them: run in the
    /// other order they reach a *different world*, because the two ids are
    /// swapped.
    pub(crate) fn record_alloc_access(&mut self) {
        if self.sims.is_empty() {
            return;
        }
        self.trail.record_access(Access::Alloc);
        self.note_step_site(self.current);
    }

    /// Names where the running step is standing, once per step.
    ///
    /// Guarded on both sides because finding it walks the stack for the
    /// innermost pending call: outside a region there is no step to name, and
    /// within one only the step's first access is recorded, so the cost is per
    /// step rather than per access.
    fn note_step_site(&mut self, span: Span) {
        if self.sims.is_empty() || self.trail.has_site() {
            return;
        }
        let site = StepSite {
            definition: innermost_calls(&self.stack).into_iter().flatten().next(),
            span,
        };
        self.trail.note_site(site);
    }

    /// Give the next step to whichever task the seed names, or deliver the
    /// region's value when every task has finished.
    ///
    /// Virtual time advances inside [`Scheduler::next`] and only there, and only
    /// with nothing enabled — which is why a simulated timeout can never fire
    /// ahead of work that could still run.
    ///
    /// [`Scheduler::next`]: crate::sched::Scheduler::next
    fn schedule(&mut self) -> Result<(), Diagnostic> {
        let Some(region) = self.sims.last_mut() else {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "the machine asked for a scheduling decision outside a simulated region",
            )
            .primary(self.current, "no `simulate` region is live here"));
        };
        let turn = region
            .sched
            .next(region.handlers.clock_mut(), &mut self.trail)?;
        let region = self.sims.last_mut().expect("the region is still live");
        match turn {
            Turn::Complete(value) => {
                let region = self.sims.pop().expect("the region was just borrowed");
                self.trail.leave(
                    region.handlers.clock().now(),
                    region.handlers.rand().drawn(),
                );
                self.stack = region.below;
                self.go_return(value);
                Ok(())
            }
            Turn::Run { resumption, .. } => {
                let (below, id) = (region.below.clone(), region.id);
                match resumption {
                    Resumption::Enter => {
                        let (body, env, module) =
                            (region.body.clone(), region.env.clone(), region.module);
                        self.stack = below.push_sim(id);
                        self.go_eval(body, env, module);
                        Ok(())
                    }
                    Resumption::Start { body, over, span } => {
                        self.stack = install(below, &over);
                        self.apply(body, Vec::new(), span)
                    }
                    Resumption::Resume { k, value } => {
                        let transition = handler::resume(&below, &k, value);
                        self.take(transition)
                    }
                }
            }
        }
    }

    /// Splices a continuation the program itself named — a `resume k` clause's
    /// `k`, or the implicit one a tail-resumptive clause gets.
    ///
    /// A continuation that crosses a `simulate` region's delimiter re-installs
    /// that region wherever it is spliced, so the region's own anchor moves with
    /// it: the value it eventually delivers has to land on the stack the
    /// resumption put it over, not on the stack the `simulate` was entered on.
    /// Without that, a clause resuming twice loses everything the first
    /// resumption still had pending — silently, and with a wrong world.
    ///
    /// A continuation naming a region that has already ended is
    /// [`codes::TASK_ESCAPES_SCOPE`]: the scheduler it belongs to is gone, and
    /// re-running its tasks against a fresh one would be a different program.
    pub(crate) fn resume_continuation(
        &mut self,
        k: &Continuation,
        value: Value,
    ) -> Result<(), Diagnostic> {
        if let Some(id) = k.sim() {
            let anchor = k.under_sim(&self.stack);
            match (region_mut(&mut self.sims, id), anchor) {
                (Some(region), Some(anchor)) => region.below = anchor,
                _ => return Err(err_region_ended(self.current)),
            }
        }
        let transition = handler::resume(&self.stack, k, value);
        self.take(transition)
    }

    /// The stack is moved out rather than borrowed: a pop that owns its stack
    /// takes the frame out of its link instead of cloning it. `Next::Done`
    /// leaves `Stack::default()` behind, which is the stack a `Done` was
    /// reporting anyway — an empty base segment.
    fn ret(&mut self, value: Value) -> Result<(), Diagnostic> {
        match std::mem::take(&mut self.stack).into_next() {
            Next::Frame(frame, rest) => {
                self.stack = rest;
                self.dispatch(frame, value)
            }
            Next::Leave(Delimiter::Ply(prompt), rest) => {
                let transition = handler::leave_handle(&prompt, value, rest);
                self.take(transition)
            }
            // A task's body returned. The region does not: it delivers its own
            // value only once every task has finished, which is what makes spawn
            // structured without a second construct.
            Next::Leave(Delimiter::Sim(region), _) => self.finish_task(region, value),
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
        match &callee {
            Value::Closure(closure) => self.enter_closure(closure, args, span),
            Value::Continuation(k) => {
                let value = handler::continuation_argument(args, span)?;
                self.resume_continuation(k, value)
            }
            other => Err(err_not_a_function(span, other)),
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
        self.record_cell_access(b, &args);
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
            return Err(self.err_frame_limit(span, &self.stack));
        }
        self.stack = std::mem::take(&mut self.stack).pushed(frame);
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

    fn err_frame_limit(&self, span: Span, stack: &Stack) -> Diagnostic {
        limit::err_recursion_limit(
            span,
            PENDING_FRAMES,
            self.max_frames,
            &innermost_calls(stack),
        )
    }
}

/// The live region with this id, if it is the one still running.
///
/// A [`SimId`] is an ordinal among the regions an entry point has entered, not
/// an index into the live ones: a continuation captured in a region that has
/// ended names an id nothing here holds, and that is the point of it.
fn region_mut(sims: &mut [Region], id: SimId) -> Option<&mut Region> {
    sims.iter_mut().find(|region| region.id == id)
}

/// Puts a spawned task under the delimiters its `spawn` site sat under. `over`
/// is innermost first, so it is installed from the outside in.
fn install(below: Stack, over: &[Delimiter]) -> Stack {
    over.iter().rev().fold(below, |stack, delimiter| {
        stack.push_delimiter(delimiter.clone())
    })
}

/// A scheduled operation's arguments, at the arity its prelude declaration has.
fn take_args<const N: usize>(
    effect: &Symbol,
    op: &Symbol,
    args: Vec<Value>,
    span: Span,
) -> Result<[Value; N], Diagnostic> {
    let len = args.len();
    <[Value; N]>::try_from(args).map_err(|_| arity_error(span, &format!("`{effect}.{op}`"), N, len))
}

#[cold]
#[inline(never)]
fn err_nested_simulation(span: Span, outer: Span) -> Diagnostic {
    Diagnostic::error(
        codes::NESTED_SIMULATION,
        "a `simulate` region may not run inside another one",
    )
    .primary(span, "this region is entered while one is already running")
    .secondary(outer, "the region already running")
    .note(
        "two schedulers mean two notions of `runnable` and a state space that is a product of both",
    )
    .note("hoist the inner region out, or handle its effects with an ordinary `handle`")
}

/// A region whose control was discarded while it still had tasks.
///
/// A handler outside the region may capture across its delimiter and never
/// resume, which destroys the scheduler with tasks still runnable. ADR 0006 §1.3
/// makes the handler the scope precisely so that cannot happen quietly: an
/// abandoned task is an unexplored interleaving, and a search over the truncated
/// trace would report `exhaustive` over the schedules it cut short.
#[cold]
#[inline(never)]
fn err_region_abandoned(span: Span, unfinished: &[crate::sim::TaskId]) -> Diagnostic {
    let waiting = unfinished
        .iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::error(
        codes::TASK_ESCAPES_SCOPE,
        format!(
            "this `simulate` region was abandoned with {} still unfinished",
            plural(unfinished.len(), "task", "tasks")
        ),
    )
    .primary(span, "control left this region and never came back")
    .note(format!("unfinished when the region was discarded: {waiting}"))
    .note("a handler outside the region captured across it and did not resume; spawn is structured, so the region's tasks have nowhere to run")
    .note("resume the continuation, or move the handler inside the `simulate` region")
}

/// A continuation carrying a region that has already delivered its value.
#[cold]
#[inline(never)]
fn err_region_ended(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::TASK_ESCAPES_SCOPE,
        "this continuation re-enters a `simulate` region that has already ended",
    )
    .primary(span, "resumed here, after the region delivered its value")
    .note("a task is a suspended machine state and its scheduler ends with the region, so a second resumption has no scheduler to run against")
    .note("resume such a continuation at most once, or install the handler inside the `simulate` region so the capture does not cross it")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// A scheduled operation whose region is gone: a `Task` or a continuation
/// smuggled past the `}` that ended the scheduler it names.
#[cold]
#[inline(never)]
fn err_task_escapes(span: Span, effect: &str, op: &str) -> Diagnostic {
    Diagnostic::error(
        codes::TASK_ESCAPES_SCOPE,
        format!("`{effect}.{op}` reached a `simulate` region that has already ended"),
    )
    .primary(span, "this control outlived the scheduler it belongs to")
    .note("a task is a key into its region's scheduler, and the scheduler ends with the region")
    .note("join the task inside the `simulate` region that spawned it")
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
        match stack.into_next() {
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

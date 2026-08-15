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
use crate::host::{
    HostAnswer, HostBinding, HostRequest, HostRuntime, HostUse, Pending, attribute,
    err_blocking_answered_inline, err_hermetic, err_host_in_search, operation_label,
};
use crate::interp::{
    OpTable, arity_error, ctor_value, err_non_exhaustive, err_not_a_function, err_overflow,
    err_unknown_name, literal, op_decl,
};
use crate::limit::{self, DEFAULT_MAX_CALLS, NAMED_CALLS, NESTED_CALLS, PENDING_FRAMES};
use crate::region::{self, Region, StepSite, Trail};
use crate::sched::{HostPolicy, Policy, Resumption, Scheduler, Turn};
use crate::sim::{Access, Answer, DEFAULT_STEPS, Seed};
use crate::trace::Trace;
use crate::value::{Closure, ClosureKind, Value};
use crate::world::World;
use ply_core::CheckOutput;
use ply_core::ty::{EffectAtom, Footprint};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{
    BinOp, Expr, FnDef, Item, Mode, Pattern, PatternKind, Program, QName, TypeDefBody, UnOp,
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
    /// The handler of last resort. Hermetic by default and everywhere, because
    /// the default is the whole of the guarantee: a suite that acquires a live
    /// dependency without anyone deciding to is the failure mode the language
    /// exists to prevent.
    binding: Arc<HostBinding>,
    /// What answers a [`HostAnswer::Pending`]. Separate from the binding because
    /// the binding is `Arc`-shared across the runner's worker threads while a
    /// runtime is one thread's reactor handle, and because a value-shaped
    /// handler — a clock read, a byte operation — never touches it.
    runtime: Option<Rc<dyn HostRuntime>>,
    /// At-most-once host operations answered in this entry point.
    host_ops: u64,
    /// What this entry point reached across the boundary, and the authority on
    /// whether its green verdict may be cached.
    host_use: HostUse,
    /// The declared footprint of the entry point about to run. `None` means no
    /// claim was made and nothing is checked against it — an evaluator driven
    /// without a type-check pass has no row to check.
    declared: Option<Footprint>,
    /// Whether this entry point is one of several runs of the same test.
    ///
    /// A search re-runs a test **whole** per interleaving, so a host operation
    /// anywhere in it — not only inside the region — is performed once per
    /// schedule explored. Set by the caller driving the search, because only the
    /// caller knows whether the plan it is about to run has more than one run in
    /// it.
    re_executed: bool,
    /// The last at-most-once host operation answered, so `E0426` can name the
    /// packet it is refusing to send twice.
    last_linear: Option<HostMark>,
}

/// An at-most-once host operation that already happened, as `E0426` prints it.
struct HostMark {
    operation: String,
    path: &'static str,
    span: Span,
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
                    // its binders bound to generated values. A `derive` is not
                    // one either — expansion has already appended the globals
                    // it stands for.
                    Item::Law(_) | Item::Derive(_) => {}
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
            binding: Arc::new(HostBinding::hermetic()),
            runtime: None,
            host_ops: 0,
            host_use: HostUse::default(),
            declared: None,
            re_executed: false,
            last_linear: None,
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

    /// Bind the host boundary. Absent this call a machine is hermetic, which is
    /// what `ply test` and `ply run` want and what every test in this crate
    /// gets.
    pub fn set_host_binding(&mut self, binding: Arc<HostBinding>) {
        self.binding = binding;
    }

    /// The reactor a [`HostAnswer::Pending`] is polled on.
    ///
    /// Separate from [`set_host_binding`] rather than folded into it: a
    /// `HostBinding` is shared across the runner's workers by `Arc` and so must
    /// stay `Send + Sync`, while a runtime handle belongs to the one thread its
    /// machine runs on. A handler that answers `Pending` with no runtime set is
    /// a diagnostic rather than a hang.
    ///
    /// [`set_host_binding`]: Machine::set_host_binding
    pub fn set_host_runtime(&mut self, runtime: Rc<dyn HostRuntime>) {
        self.runtime = Some(runtime);
    }

    pub fn host_binding(&self) -> &HostBinding {
        &self.binding
    }

    /// At-most-once host operations answered in this entry point. Zero for the
    /// life of a hermetic run, which is why no existing multi-shot program
    /// changes behaviour under W1.
    pub fn host_ops(&self) -> u64 {
        self.host_ops
    }

    /// The declared footprint of the entry point about to run. A host answer
    /// whose atom is outside it is [`codes::HOST_FOOTPRINT_ESCAPE`] — the one
    /// mechanical defence in the system against a footprint that under-reports.
    ///
    /// Set before the entry point and **not** cleared by the reset each entry
    /// point performs: the claim is the caller's and the caller states it once
    /// per test.
    pub fn set_declared_footprint(&mut self, footprint: Footprint) {
        self.declared = Some(footprint);
    }

    /// Declare that this entry point is one of several runs of the same test, so
    /// that reaching the host boundary is [`codes::HOST_IN_SIMULATION`] rather
    /// than a packet sent once per interleaving.
    ///
    /// The refusal a `simulate` region already carries, stated over the whole
    /// test rather than over the region: the search re-runs a test whole, so an
    /// operation in the prefix or the suffix around a region is re-performed
    /// exactly as one inside it is. `Machine::innermost_simulation` cannot see
    /// that — it is empty before the region is entered and after it closes — so
    /// the fact has to come from the caller driving the search.
    ///
    /// Set per entry point by the caller, like [`set_declared_footprint`]; a
    /// machine nobody tells is not re-executed.
    ///
    /// [`set_declared_footprint`]: Machine::set_declared_footprint
    pub fn set_re_executed(&mut self, re_executed: bool) {
        self.re_executed = re_executed;
    }

    /// What the last entry point reached across the boundary, or `None` when it
    /// reached none. Never a zeroed record: "reached no host handler" and
    /// "reached one that did nothing" are different claims about a cache entry.
    pub fn host_use(&self) -> Option<&HostUse> {
        (!self.host_use.is_empty()).then_some(&self.host_use)
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
                match handler::perform(&self.stack, request, decl, self.host_ops)? {
                    Answered::Handler(transition) => self.take(transition)?,
                    Answered::Scheduler(scheduled) => self.run_scheduled(scheduled)?,
                    Answered::Unhandled(request) => self.perform_host(request)?,
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
        self.host_ops = 0;
        self.host_use = HostUse::default();
        self.last_linear = None;
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

    /// A `perform` that walked the whole stack without finding a handler.
    ///
    /// This is the boundary, and it is reached only here: every `handle` and
    /// every `simulate` delimiter was consulted first, innermost out, so a test
    /// double in scope shadows a real socket by the ordinary rule rather than by
    /// a special case. The binding is not a `Delimiter` and no `Continuation`
    /// contains it, which is what keeps capture, splice and `Next::Leave`
    /// untouched by this milestone.
    fn perform_host(&mut self, request: Request) -> Result<(), Diagnostic> {
        let Request {
            effect,
            op,
            resource,
            args,
            span,
        } = request;
        let operation = operation_label(&effect, &op, resource.as_ref());
        let would = self.binding.would_serve(&effect, &op, resource.as_ref());

        // Before resolution and before the hermetic check, because this is the
        // terminal answer: a region that reaches a socket cannot be repaired by
        // `--host`, and a diagnostic that suggests a flag which will then refuse
        // the program has cost the reader a round trip to learn nothing.
        if would.is_some()
            && let Some(region) = self.innermost_simulation()
        {
            return Err(err_host_in_simulation(span, &operation, region));
        }

        let Some(bound) = self.binding.resolve(&effect, &op, resource.as_ref()) else {
            return Err(match would {
                None => handler::err_unhandled(span, &effect, &op, resource.as_ref()),
                Some(path) if self.binding.is_hermetic() => {
                    let hermetic = err_hermetic(span, &operation, path);
                    // E0424's second remedy is `--host`, and for a test the
                    // search re-runs that flag leads straight to `E0425`. Saying
                    // so here costs a line and saves the reader a round trip.
                    match self.re_executed {
                        true => hermetic.note(
                            "`--host` would then refuse this: the search runs a seeded test whole once per interleaving, so a handler would answer it once per schedule",
                        ),
                        false => hermetic,
                    }
                }
                Some(path) => err_unenumerated_atom(span, &operation, path),
            });
        };

        // `task.*` is answered by opening a region rather than by calling a
        // handler: a task is a suspended machine state, and a handler is handed
        // argument values and a span. Lazily, per ADR 0011 §9 — a region opened
        // around every entry point would make every existing `simulate` nested
        // and `E0416` under `--host`.
        if crate::sim::TASK_OPS.contains(&op.as_str()) && effect.as_str() == "task" {
            return self.open_production_region(effect, op, args, span);
        }

        let atom = bound.atom.clone();
        let declaration = bound.op.clone();
        let handler = Arc::clone(bound.handler);

        if let Some(declared) = &self.declared
            && !declared.contains(&atom)
        {
            return Err(err_footprint_escape(
                span,
                &operation,
                &atom,
                declared,
                declaration.path,
            ));
        }

        // Outside every region and still re-executed. The search re-runs the
        // whole test, so an operation in the prefix or the suffix around a
        // region is performed once per interleaving exactly as one inside it is,
        // and `innermost_simulation` cannot see that: it is empty before the
        // region is entered and after it closes.
        //
        // Below the `task.*` branch on purpose. Opening a production region
        // performs nothing outside the program, and the seeded and production
        // schedulers already exclude each other in three independent ways
        // (ADR 0011 §9) — `E0416` is the specific answer there, and refusing
        // first would replace it with a vaguer one.
        if self.re_executed {
            return Err(err_host_in_search(span, &operation, declaration.path));
        }

        let runtime = self.runtime.clone();
        let answered = {
            let request = HostRequest {
                atom: atom.clone(),
                op: &declaration,
                args: &args,
                span,
            };
            match &runtime {
                Some(rt) => handler.call(rt.as_ref(), &request),
                None => handler.call(&Unbound, &request),
            }
        };

        // The operation happened — a refusal included, because a handler that
        // failed may have acted before it failed and nothing here can know. What
        // is undecided is only when its answer arrives. Charged before the `?` so
        // that a task parked on a token, and a run a handler refused, have both
        // already spent their linearity: that is the direction which refuses a
        // replay rather than allowing one, and it is what makes a failure a
        // handler produced a *host-backed* failure that M5 must not re-run.
        self.host_use.record(&atom);
        if declaration.linearity.is_linear() {
            self.host_ops = self.host_ops.saturating_add(1);
            self.last_linear = Some(HostMark {
                operation: operation.clone(),
                path: declaration.path,
                span,
            });
        }

        // Every refusal is stamped with where it came from. A handler picks its
        // own code, and several of the codes decide whether `ply test` reports a
        // failure as the program's fault or as Ply's — so the classification is
        // taken back here rather than left to each handler's good manners.
        let answer = answered.map_err(|d| attribute(d, declaration.path, &operation, span))?;

        let pending = match answer {
            HostAnswer::Value(value) => {
                // The declaration's structural half, and the only half of it
                // anything can check: `blocking: true` says the work left this
                // thread and a token is coming back, so a value is this thread
                // having done the work while every task sharing it waited.
                if declaration.blocking {
                    return Err(err_blocking_answered_inline(
                        span,
                        &operation,
                        declaration.path,
                    ));
                }
                self.go_return(value);
                return Ok(());
            }
            HostAnswer::Pending(pending) => pending,
        };
        let Some(rt) = runtime else {
            return Err(err_no_runtime(span, &operation, pending, declaration.path));
        };

        // Inside a production region the performing task leaves the enabled set
        // and the others keep running — which is the whole of ADR 0008 §8. Two
        // tasks where one waits on bytes the other must send would otherwise
        // deadlock on the thread, silently and with no diagnostic, because
        // `block_on` holds the only thread either of them can run on.
        if let Some(region) = self.host_region() {
            let Some(segments) = self.stack.sim_depth() else {
                return Err(err_task_lost_its_region(span, &operation));
            };
            let (k, _) = self.stack.capture(segments, self.host_ops);
            let live = region_mut(&mut self.sims, region).expect("the region was just found");
            live.sched.park_on_host(k, pending, span)?;
            return self.schedule();
        }

        // Outside one a `Pending` has nowhere to park, so the machine drives the
        // runtime until the token resolves. This is the one place in the
        // language where a Ply computation blocks a real thread.
        let value = rt.block_on(pending)?;
        self.go_return(value);
        Ok(())
    }

    /// The innermost live production region, if control is actually inside it.
    ///
    /// `holds_sim` and not merely "a region exists": a handler outside the region
    /// may have discarded the continuation that carried its delimiter, and
    /// parking a task on a scheduler control has left is a token nothing will
    /// ever resume.
    fn host_region(&self) -> Option<SimId> {
        self.sims
            .iter()
            .rev()
            .find(|region| region.sched.policy() == Policy::Host && self.stack.holds_sim(region.id))
            .map(|region| region.id)
    }

    /// Opens the production region a `task.*` perform reached the binding at, or
    /// answers into one already open.
    ///
    /// The region's root task is the computation already in progress — this
    /// entire stack — so the region's value is the entry point's and there is
    /// nothing under it to deliver onto. The perform that opened it is then
    /// answered by [`Machine::run_scheduled`], the same path every later
    /// `task.*` takes, rather than by a second implementation of what `spawn`
    /// means.
    fn open_production_region(
        &mut self,
        effect: Symbol,
        op: Symbol,
        args: Vec<Value>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        // A production region already live means control left its delimiter —
        // `find_handler` would have answered otherwise — so the tasks it still
        // holds will never run. Opening a second one would strand them.
        if let Some(open) = self.sims.iter().find(|r| r.sched.policy() == Policy::Host) {
            return Err(err_region_abandoned(open.span, &open.sched.unfinished()));
        }
        let Some(permit) = HostPolicy::of(&self.binding) else {
            return Err(err_hermetic(
                span,
                &operation_label(&effect, &op, None),
                self.binding
                    .would_serve(&effect, &op, None)
                    .unwrap_or("ply_host::sched::spawn"),
            ));
        };
        let id = SimId(self.entered_sims);
        self.entered_sims += 1;
        let k = std::mem::take(&mut self.stack).into_task(id, self.host_ops);
        let sched = Scheduler::production(id, span, permit).rooted_running()?;
        self.sims.push(Region::production(id, sched, span));
        self.run_scheduled(Scheduled {
            region: id,
            effect,
            op,
            args,
            span,
            k,
        })
    }

    /// The span of the innermost live **seeded** region.
    ///
    /// A `Policy::Host` region is opened *by* the host binding at the first
    /// `task.*` that reached it, so a host operation inside one is ordinary
    /// rather than `E0425`. Only a `simulate` re-runs its body once per
    /// interleaving, and only that is what makes reaching a socket from inside a
    /// region a proof about packets it sent along the way.
    fn innermost_simulation(&self) -> Option<Span> {
        self.sims
            .iter()
            .rev()
            .find(|region| region.sched.policy() == Policy::Seeded)
            .map(|region| region.span)
    }

    /// `E0426`, built from what [`handler::resume`] refused and the operation
    /// this machine is protecting.
    fn err_replayed(&self, refused: handler::Replayed) -> Diagnostic {
        err_continuation_resumed(self.current, refused.resumes, self.last_linear.as_ref())
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
        let Some(live) = region_mut(&mut self.sims, region) else {
            return Err(err_task_escapes(span, &effect, &op));
        };
        // A `Sim` delimiter is found by operation name alone, which is right for
        // a seeded region and too wide for a production one: it answers `task`
        // and nothing else, so a `clock.now` inside it must go to the host
        // binding rather than be given virtual time that starts at zero and only
        // moves when every task is asleep. `k` is dropped rather than spliced —
        // capture does not touch the stack it read — so the perform is exactly
        // where it was.
        if !live.sched.answers(effect.as_str(), op.as_str()) {
            return self.perform_host(Request {
                effect,
                op,
                resource: None,
                args,
                span,
            });
        }
        if live.sched.records_steps() {
            self.trail.end_step(span);
        }
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
        let Some(live) = region_mut(&mut self.sims, region) else {
            return Err(err_task_escapes(span, "task", "return"));
        };
        if live.sched.records_steps() {
            self.trail.end_step(span);
        }
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

    /// Give the next step to whichever task the seed names — or, in a production
    /// region, to whichever is ready — or deliver the region's value when every
    /// task has finished.
    ///
    /// Virtual time advances inside [`Scheduler::next`] and only there, and only
    /// with nothing enabled — which is why a simulated timeout can never fire
    /// ahead of work that could still run. A production region has no virtual
    /// clock and takes neither the [`Trail`] nor the [`Clock`]: it records no
    /// step, so it cannot fabricate an `Exploration` and cannot spend a seed's
    /// choice sequence.
    ///
    /// [`Scheduler::next`]: crate::sched::Scheduler::next
    /// [`Clock`]: crate::sim::Clock
    fn schedule(&mut self) -> Result<(), Diagnostic> {
        let Some(region) = self.sims.last_mut() else {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "the machine asked for a scheduling decision outside a simulated region",
            )
            .primary(self.current, "no `simulate` region is live here"));
        };
        let turn = match region.sched.policy() {
            Policy::Seeded => region
                .sched
                .next(region.handlers.clock_mut(), &mut self.trail)?,
            // The runtime is reached only with nothing enabled: a region whose
            // tasks are all value-shaped never polls and never parks, so a
            // machine bound without a reactor still schedules. One that does
            // reach for a reactor gets `Unbound`'s diagnostic naming the
            // omission, rather than a refusal to start over a reactor it would
            // not have used.
            Policy::Host => {
                let runtime = self.runtime.clone();
                let region = self.sims.last_mut().expect("the region is still live");
                match &runtime {
                    Some(rt) => region.sched.next_host(rt.as_ref())?,
                    None => region.sched.next_host(&Unbound)?,
                }
            }
        };
        let region = self.sims.last_mut().expect("the region is still live");
        match turn {
            Turn::Complete(value) => {
                let region = self.sims.pop().expect("the region was just borrowed");
                // A production region's clock and stream were never drawn from,
                // so handing them to the trail would report zeroes over whatever
                // a seeded region earlier in the entry point actually reached.
                if region.sched.records_steps() {
                    self.trail.leave(
                        region.handlers.clock().now(),
                        region.handlers.rand().drawn(),
                    );
                }
                self.stack = region.below;
                self.go_return(value);
                Ok(())
            }
            Turn::Run { resumption, .. } => {
                let (below, id) = (region.below.clone(), region.id);
                match resumption {
                    Resumption::Enter => {
                        let Some(body) = region.body.clone() else {
                            return Err(err_lazy_region_entered(region.span));
                        };
                        let (env, module) = (region.env.clone(), region.module);
                        self.stack = below.push_sim(id);
                        self.go_eval(body, env, module);
                        Ok(())
                    }
                    Resumption::Start { body, over, span } => {
                        self.stack = install(below, &over);
                        self.apply(body, Vec::new(), span)
                    }
                    Resumption::Resume { k, value } => {
                        let transition = handler::resume(&below, &k, value, self.host_ops)
                            .map_err(|refused| self.err_replayed(refused))?;
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
        let transition = handler::resume(&self.stack, k, value, self.host_ops)
            .map_err(|refused| self.err_replayed(refused))?;
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
                let declared = self.ctor_name(module, &QName::bare(id.clone()));
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
            PatternKind::Lit(lit) => crate::interp::lit_matches(lit, value),
            PatternKind::Ctor { name, args } => match value {
                Value::Ctor {
                    name: vname,
                    args: vargs,
                } => {
                    let expected = self.ctor_name(module, name);
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
        if let Some(name) = self.global(module, Namespace::Value, q)
            && let Some(v) = self.definition(&name)
        {
            return Ok(v);
        }
        if let Some(name) = self.ctor_name(module, q)
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
    /// The program-wide name a constructor reference denotes, falling back to
    /// the prelude's — which no module declares, so nothing qualifies it.
    fn ctor_name(&self, module: usize, q: &QName) -> Option<Symbol> {
        match self.global(module, Namespace::Value, q) {
            Some(name) => Some(name),
            None if q.is_bare() && self.ctors.contains_key(q.symbol()) => Some(q.symbol().clone()),
            None => None,
        }
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

/// A lazily opened region asked to evaluate a body it does not have.
#[cold]
#[inline(never)]
fn err_lazy_region_entered(region: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a region opened at the host boundary was asked to enter a body",
    )
    .primary(
        region,
        "this region's root task is the computation that opened it, so it has no body",
    )
    .note("this is Ply's fault: report it with the program that produced it")
}

/// A task about to park on a host token whose region delimiter is not on its
/// stack. Parking anyway is a token nothing will ever resume — a hang, which is
/// the one shape a defect at this boundary must never take.
#[cold]
#[inline(never)]
fn err_task_lost_its_region(span: Span, operation: &str) -> Diagnostic {
    Diagnostic::error(
        codes::TASK_ESCAPES_SCOPE,
        format!("`{operation}` would park a task whose scheduler its control has already left"),
    )
    .primary(
        span,
        "performed here, outside the region that owns this task",
    )
    .note("a handler outside the region discarded the continuation that carried its delimiter")
    .note("keep the `handle` inside the region, or resume the continuation it was given")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The runtime a machine has when nobody gave it one.
///
/// A value-shaped handler — a clock read, a byte operation — never touches the
/// runtime at all, so a machine with a binding and no reactor is a legitimate
/// configuration and must not be a panic. One that *does* reach for a reactor
/// gets a diagnostic naming the omission.
struct Unbound;

impl HostRuntime for Unbound {
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        Err(err_unbound_runtime(&format!("poll `{pending}`")))
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Err(err_unbound_runtime("park"))
    }

    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        Err(err_unbound_runtime(&format!("block on `{pending}`")))
    }
}

#[cold]
#[inline(never)]
fn err_unbound_runtime(what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a host handler asked the runtime to {what}, and this run has no host runtime"),
    )
    .primary(Span::DUMMY, "no reactor is bound to this machine")
    .note("`Machine::set_host_runtime` was never called, so only handlers that answer a value outright can run here")
}

/// A `HostAnswer::Pending` with no reactor to resolve it.
#[cold]
#[inline(never)]
fn err_no_runtime(span: Span, operation: &str, pending: Pending, path: &'static str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{operation}` did not complete and this run has no host runtime to wait on it"),
    )
    .primary(span, "performed here")
    .note(format!("`{path}` answered a pending `{pending}`"))
    .note("`Machine::set_host_runtime` was never called; a handler that can answer `Pending` needs one")
}

/// `E0427` — a registration claims this operation, the run is bound, and the
/// binding enumerated no atom for it.
///
/// The static picture and the dynamic one disagree: binding resolves an `Any`
/// registration against the atoms the program's declared footprints contain, so
/// reaching one they do not contain means a footprint under-reported. That is
/// the failure mode the whole boundary is built around, so it is loud and it is
/// Ply's fault rather than a quiet unbound pass.
#[cold]
#[inline(never)]
fn err_unenumerated_atom(span: Span, operation: &str, path: &'static str) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_FOOTPRINT_ESCAPE,
        format!("`{operation}` reached a bound host handler that this run never enumerated"),
    )
    .primary(span, "performed here")
    .note(format!("`{path}` is registered for this operation, but binding resolved no atom for it against the program's declared footprints"))
    .note("a footprint that does not contain an atom the program performs is a footprint that under-reports, and scheduling and isolation are decided from it")
    .note("this is Ply's fault: report it with the program that produced it")
}

/// `E0427` — a host handler answered an atom outside the entry point's row.
#[cold]
#[inline(never)]
fn err_footprint_escape(
    span: Span,
    operation: &str,
    atom: &EffectAtom,
    declared: &Footprint,
    path: &'static str,
) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_FOOTPRINT_ESCAPE,
        format!("`{path}` answered `{atom}`, which is outside this entry point's declared footprint"),
    )
    .primary(span, format!("`{operation}` performed here"))
    .note(format!("the entry point's declared footprint is {declared}"))
    .note("scheduling and world isolation are decided from that footprint, so an operation outside it may have run beside work it conflicts with")
    .note("this is Ply's fault: the run knows two of its own answers disagree and nothing in the definition graph decides which was meant")
}

/// `E0425` — a host operation reached from inside a `simulate` region.
#[cold]
#[inline(never)]
pub(crate) fn err_host_in_simulation(span: Span, operation: &str, region: Span) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_IN_SIMULATION,
        format!("`{operation}` reached the host boundary from inside a `simulate` region"),
    )
    .primary(span, "performed here, against a real resource")
    .secondary(region, "this region re-runs its body once per interleaving")
    .note("the search runs the region whole for every schedule it explores, so this operation would be performed once per interleaving")
    .note("and the result would then be reported as a proof over every interleaving")
    .note("handle the operation with a test double inside the region, or hoist it out of the region entirely")
}

/// `E0426` — a second resumption across an at-most-once host operation.
#[cold]
#[inline(never)]
fn err_continuation_resumed(span: Span, resumes: u32, last: Option<&HostMark>) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::HOST_CONTINUATION_RESUMED,
        "this continuation is resumed again across a host operation",
    )
    .primary(span, format!("resumption {resumes} of one continuation"));
    if let Some(mark) = last {
        if !mark.span.is_dummy() {
            diagnostic = diagnostic.secondary(
                mark.span,
                format!("`{}` was performed here, after the capture", mark.operation),
            );
        }
        diagnostic = diagnostic.note(format!(
            "`{}` is registered `at-most-once`, so replaying this control would perform it again",
            mark.path
        ));
    }
    diagnostic
        .note("multi-shot resumption stays available for pure and in-memory handlers; the restriction is on the boundary, not on the feature")
        .note("resume at most once across a host operation, or — only if replaying it really changes nothing outside the program — register the operation `Linearity::Repeatable`")
        .note("the rule is conservative: it refuses when any at-most-once host operation happened after the capture, including in another task")
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
        // `-0.0` is a `Float` distinct from `0.0` and negation is how a program
        // reaches it, so this arm is not decoration. Negating a `Decimal` is
        // exact at every value the type holds.
        UnOp::Neg => match value {
            Value::Float(f) => Ok(Value::Float(-f)),
            Value::Decimal(d) => Ok(Value::Decimal(-*d)),
            _ => {
                let i = value.as_int(operand_span, "negation")?;
                match i.checked_neg() {
                    Some(n) => Ok(Value::Int(n)),
                    None => Err(err_overflow(span, "negation", i, 0)),
                }
            }
        },
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
